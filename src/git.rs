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

impl Blob {
    /// The source text, if there is any; `Missing` and `NonUtf8` both yield
    /// `None`.
    pub fn into_text(self) -> Option<String> {
        match self {
            Blob::Text(text) => Some(text),
            Blob::Missing | Blob::NonUtf8 => None,
        }
    }
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

    /// Whether `rev` names a commit in this repository.
    pub fn rev_exists(&self, rev: &str) -> bool {
        let spec = format!("{rev}^{{commit}}");
        self.git(&["rev-parse", "--verify", "--quiet", &spec])
            .map(|out| out.status.success())
            .unwrap_or(false)
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
        let base = match explicit {
            Some(rev) => {
                if !self.rev_exists(rev) {
                    bail!("unknown revision: {rev}");
                }
                rev.to_string()
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
                })?,
        };
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
        let out = self.git(&["diff", "--name-status", "-z", base, head])?;
        if !out.status.success() {
            bail!("git diff {base}..{head} failed: {}", stderr_string(&out));
        }
        parse_name_status(&stdout_string(&out)?)
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

    /// Read `path` at `rev` as source text, recording a warning when a blob is
    /// skipped because it is not UTF-8.
    pub fn blob_text(
        &self,
        rev: &str,
        path: &str,
        warnings: &mut Vec<String>,
    ) -> Result<Option<String>> {
        let blob = self.blob(rev, path)?;
        if blob == Blob::NonUtf8 {
            warnings.push(format!("skipped non-UTF-8 file {path} at {rev}"));
        }
        Ok(blob.into_text())
    }

    fn git(&self, args: &[&str]) -> Result<Output> {
        run_git(&self.root, args)
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
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

    #[test]
    fn blob_into_text_keeps_only_readable_sources() {
        assert_eq!(
            Blob::Text("fn main() {}".to_string()).into_text(),
            Some("fn main() {}".to_string())
        );
        assert_eq!(Blob::Missing.into_text(), None);
        assert_eq!(Blob::NonUtf8.into_text(), None);
    }
}
