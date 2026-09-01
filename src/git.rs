//! Thin wrapper around the `git` command line.
//!
//! Shelling out keeps the dependency surface small and matches what a reviewer
//! would type by hand. This module is part of the I/O shell: it uses `anyhow`
//! and produces errors phrased for a human reading them on stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{anyhow, bail, Context, Result};

/// Remote-tracking refs probed, in order, when no base revision is given.
const BASE_CANDIDATES: [&str; 2] = ["origin/master", "origin/main"];

/// A discovered git repository, identified by its worktree root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    root: PathBuf,
}

/// How a file changed between two revisions. Renames are decomposed into a
/// delete of the old path plus an add of the new one, so the analyzer never
/// has to reason about rename pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
}

/// One entry of `git diff --name-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

/// The result of reading one path at one revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blob {
    /// The blob exists and decodes as UTF-8.
    Text(String),
    /// The path does not exist at that revision (expected for adds/deletes).
    Missing,
    /// The blob exists but is not UTF-8, so there is nothing to analyze.
    NonUtf8,
}

impl Repo {
    /// Find the worktree root containing `cwd`.
    pub fn discover(cwd: impl AsRef<Path>) -> Result<Self> {
        let cwd = cwd.as_ref();
        if !cwd.is_dir() {
            bail!("no such directory: {}", cwd.display());
        }
        let out = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            bail!("not a git repository: {}", cwd.display());
        }
        let root = String::from_utf8(out.stdout)
            .context("git printed a non-UTF-8 repository path")?
            .trim()
            .to_string();
        Ok(Self {
            root: PathBuf::from(root),
        })
    }

    /// The absolute worktree root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory git keeps its own data in — where review state belongs.
    ///
    /// This is *not* always `<root>/.git`: in a linked worktree or a submodule
    /// that name is a file pointing elsewhere. Asking git for the common dir
    /// also means every worktree of a repository shares one review, which is
    /// what a reviewer expects from a state keyed on a revision range.
    pub fn git_dir(&self) -> Result<PathBuf> {
        let out = self.git(&["rev-parse", "--git-common-dir"])?;
        if !out.status.success() {
            bail!(
                "could not locate the git directory: {}",
                stderr_string(&out)
            );
        }
        let dir = PathBuf::from(stdout_string(&out)?.trim());
        // `--git-common-dir` answers relative to the worktree root when it can.
        Ok(if dir.is_absolute() {
            dir
        } else {
            self.root.join(dir)
        })
    }

    /// Whether `rev` names a commit in this repository.
    pub fn rev_exists(&self, rev: &str) -> bool {
        let spec = format!("{rev}^{{commit}}");
        self.git(&["rev-parse", "--verify", "--quiet", &spec])
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// A name for `rev` that tells two branches apart.
    ///
    /// Review state is filed under `<base>..<head>`, and `base` is a merge
    /// base: two branches cut from the same commit share it. Left as written,
    /// the default `HEAD` makes them share the whole file name too, so one
    /// branch's verdicts show up pre-applied to the other's code and the first
    /// click there overwrites them. Expanding `HEAD` to the branch it is on
    /// discriminates without giving up the stability a branch name has over a
    /// sha — a new commit does not orphan the review. A detached `HEAD` has no
    /// branch to name, so it falls back to the commit it points at, which is
    /// exactly as stable as the checkout itself.
    ///
    /// Any other revision is already a name the reviewer chose and is returned
    /// untouched.
    pub fn head_label(&self, rev: &str) -> String {
        if rev != "HEAD" {
            return rev.to_string();
        }
        let branch = self
            .git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| stdout_string(&out).ok())
            .map(|name| name.trim().to_string());
        match branch {
            Some(name) if name != "HEAD" && !name.is_empty() => name,
            _ => self
                .git(&["rev-parse", "--short", "HEAD"])
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| stdout_string(&out).ok())
                .map(|sha| sha.trim().to_string())
                .filter(|sha| !sha.is_empty())
                .unwrap_or_else(|| rev.to_string()),
        }
    }

    /// The revision the base was *named* by: the explicit argument, or
    /// whichever of `origin/master` / `origin/main` exists.
    ///
    /// This is the branch-side half of [`resolve_base`](Self::resolve_base),
    /// split out because review state is filed under it. The merge base is a
    /// commit id, and it moves the moment the branch is rebased or master is
    /// merged in — filing under it would orphan a review of four hundred cards
    /// on a routine `git rebase`, which is exactly what the per-node
    /// fingerprints exist to avoid. The name does not move, so the saved state
    /// is found again and `review::reconcile` re-opens only the cards whose
    /// own text actually changed.
    pub fn base_label(&self, explicit: Option<&str>) -> Result<String> {
        match explicit {
            Some(rev) => {
                if !self.rev_exists(rev) {
                    bail!("unknown revision: {rev}");
                }
                Ok(rev.to_string())
            }
            None => BASE_CANDIDATES
                .iter()
                .find(|candidate| self.rev_exists(candidate))
                .map(|candidate| candidate.to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "no base revision given and neither origin/master nor origin/main exists; \
                         pass a base explicitly"
                    )
                }),
        }
    }

    /// Resolve the revision the diff is taken from.
    ///
    /// With an explicit base, that revision is verified; without one,
    /// `origin/master` then `origin/main` are probed. Either way the answer is
    /// the merge base with `head`, so the diff shows what `head` adds rather
    /// than what the base branch moved on to.
    pub fn resolve_base(&self, explicit: Option<&str>, head: &str) -> Result<String> {
        if !self.rev_exists(head) {
            bail!("unknown revision: {head}");
        }
        let base = self.base_label(explicit)?;
        self.merge_base(&base, head)
    }

    /// The best common ancestor of two revisions.
    pub fn merge_base(&self, base: &str, head: &str) -> Result<String> {
        let out = self.git(&["merge-base", base, head])?;
        if !out.status.success() {
            bail!("no common ancestor between {base} and {head}");
        }
        Ok(stdout_string(&out)?.trim().to_string())
    }

    /// Every path that differs between `base` and `head`.
    pub fn changed_files(&self, base: &str, head: &str) -> Result<Vec<ChangedFile>> {
        // `--` keeps a revision that begins with a dash from reaching git as an
        // option, however it got as far as here.
        let out = self.git(&["diff", "--name-status", "-z", base, head, "--"])?;
        if !out.status.success() {
            bail!("git diff {base}..{head} failed: {}", stderr_string(&out));
        }
        // Lossy on purpose. Paths are bytes to git, and one file with a
        // non-UTF-8 name — a stray latin-1 asset that the extension filter
        // would drop on the next line anyway — must not abort the review of
        // everything else. A mangled path that does reach `blob` reads as
        // missing, which `read_side` already reports by name.
        parse_name_status(&String::from_utf8_lossy(&out.stdout))
    }

    /// Read `path` as it exists at `rev`.
    pub fn blob(&self, rev: &str, path: &str) -> Result<Blob> {
        let spec = format!("{rev}:{path}");
        let out = self.git(&["show", &spec])?;
        if !out.status.success() {
            let stderr = stderr_string(&out);
            // git distinguishes "this path isn't in that tree" from "that
            // revision doesn't exist"; only the former is routine.
            if stderr.contains("does not exist") || stderr.contains("exists on disk, but not in") {
                return Ok(Blob::Missing);
            }
            bail!("failed to read {spec}: {stderr}");
        }
        match String::from_utf8(out.stdout) {
            Ok(text) => Ok(Blob::Text(text)),
            Err(_) => Ok(Blob::NonUtf8),
        }
    }

    fn git(&self, args: &[&str]) -> Result<Output> {
        run_git(&self.root, args)
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        // `blob` reads git's diagnostics to tell "not in that tree" from a real
        // failure, and a git built with NLS translates them. Pin the locale so
        // the messages we match are the ones git's own source spells out.
        .env("LC_ALL", "C")
        .env("LANGUAGE", "")
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}` (is git installed?)", args.join(" ")))
}

fn stdout_string(out: &Output) -> Result<String> {
    String::from_utf8(out.stdout.clone()).context("git printed non-UTF-8 output")
}

fn stderr_string(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// Parse the NUL-separated form of `git diff --name-status`.
///
/// Records are `<status>\0<path>\0`, except renames and copies which carry two
/// paths. NUL separation is what makes paths with spaces or newlines safe.
fn parse_name_status(raw: &str) -> Result<Vec<ChangedFile>> {
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    let mut changed = Vec::new();
    while let Some(status) = fields.next() {
        let code = status
            .chars()
            .next()
            .ok_or_else(|| anyhow!("git diff produced an empty status field"))?;
        let mut next_path = || {
            fields
                .next()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("git diff produced status {code} without a path"))
        };
        match code {
            'A' => changed.push(ChangedFile {
                path: next_path()?,
                status: FileStatus::Added,
            }),
            // A type change (symlink <-> file) still means "same path, new
            // contents" as far as the analyzer is concerned.
            'M' | 'T' => changed.push(ChangedFile {
                path: next_path()?,
                status: FileStatus::Modified,
            }),
            'D' => changed.push(ChangedFile {
                path: next_path()?,
                status: FileStatus::Deleted,
            }),
            'R' => {
                let old = next_path()?;
                let new = next_path()?;
                changed.push(ChangedFile {
                    path: old,
                    status: FileStatus::Deleted,
                });
                changed.push(ChangedFile {
                    path: new,
                    status: FileStatus::Added,
                });
            }
            // Only emitted when the reviewer's git config sets
            // `diff.renames = copies`; the source is already reviewed under its
            // own path, so only the new copy is a change.
            'C' => {
                let _source = next_path()?;
                changed.push(ChangedFile {
                    path: next_path()?,
                    status: FileStatus::Added,
                });
            }
            // Unmerged, unknown and broken-pairing entries cannot appear for a
            // two-revision diff; skip their path rather than guessing.
            _ => {
                let _ = next_path()?;
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adds_modifications_and_deletions() {
        let raw = "A\0src/new.rs\0M\0src/old.rs\0D\0src/gone.rs\0";
        assert_eq!(
            parse_name_status(raw).unwrap(),
            vec![
                ChangedFile {
                    path: "src/new.rs".to_string(),
                    status: FileStatus::Added
                },
                ChangedFile {
                    path: "src/old.rs".to_string(),
                    status: FileStatus::Modified
                },
                ChangedFile {
                    path: "src/gone.rs".to_string(),
                    status: FileStatus::Deleted
                },
            ]
        );
    }

    #[test]
    fn splits_a_rename_into_a_delete_and_an_add() {
        let raw = "R100\0src/from.rs\0src/to.rs\0";
        assert_eq!(
            parse_name_status(raw).unwrap(),
            vec![
                ChangedFile {
                    path: "src/from.rs".to_string(),
                    status: FileStatus::Deleted
                },
                ChangedFile {
                    path: "src/to.rs".to_string(),
                    status: FileStatus::Added
                },
            ]
        );
    }

    #[test]
    fn keeps_only_the_destination_of_a_copy() {
        let raw = "C75\0src/from.rs\0src/to.rs\0";
        assert_eq!(
            parse_name_status(raw).unwrap(),
            vec![ChangedFile {
                path: "src/to.rs".to_string(),
                status: FileStatus::Added
            }]
        );
    }

    #[test]
    fn handles_paths_containing_spaces() {
        let raw = "M\0src/a file.rs\0";
        assert_eq!(
            parse_name_status(raw).unwrap(),
            vec![ChangedFile {
                path: "src/a file.rs".to_string(),
                status: FileStatus::Modified
            }]
        );
    }

    #[test]
    fn empty_diff_yields_no_entries() {
        assert!(parse_name_status("").unwrap().is_empty());
    }

    #[test]
    fn a_status_without_a_path_is_an_error() {
        assert!(parse_name_status("M\0").is_err());
    }
}
