//! Review state: what the reviewer decided about each node, on disk.
//!
//! State lives beside the repository it describes, under
//! `<git-dir>/gribovik/<base>..<head>.json`, so it survives restarts of the CLI
//! and disappears with the clone. This module is part of the I/O shell.
//!
//! Reading is deliberately forgiving: a missing or unreadable file yields an
//! empty state rather than an error, because losing a few approvals is a much
//! smaller problem than refusing to start a review at all.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Node id → the reviewer's verdict on that node.
///
/// A `BTreeMap` keeps the JSON stable across saves, which makes the file
/// readable and diffable if anyone ever looks at it by hand.
pub type ReviewState = BTreeMap<String, NodeReview>;

/// Everything the reviewer recorded about one node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeReview {
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// Where a node stands in the review.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Approved,
    Rejected,
    /// Also the state of every node with no entry in the map at all.
    #[default]
    Pending,
}

/// One free-text note attached to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub text: String,
    /// Stamped by the client and treated as opaque here — the backend never
    /// parses or orders by it, so the crate needs no clock dependency.
    pub created_at: String,
}

/// The state file for one revision range, inside the repository's git
/// directory.
///
/// `git_dir` comes from `Repo::git_dir` rather than being built as
/// `<root>/.git`: in a linked worktree or a submodule that path is a file, and
/// writing under it fails.
///
/// Revision names go into the file name verbatim except for `/`, which would
/// otherwise turn `origin/master..HEAD` into nested directories.
pub fn state_path(git_dir: impl AsRef<Path>, base: &str, head: &str) -> PathBuf {
    git_dir
        .as_ref()
        .join("gribovik")
        .join(format!("{}..{}.json", sanitize(base), sanitize(head)))
}

/// Replace path separators in a revision name so it can be a file name.
fn sanitize(rev: &str) -> String {
    rev.replace('/', "-")
}

/// Read the state at `path`, falling back to an empty state.
///
/// A missing file is the normal first-run case. A corrupt one is reported on
/// stderr and then ignored, so a hand-edited or truncated file costs the
/// reviewer their marks but not the session.
pub fn load(path: impl AsRef<Path>) -> ReviewState {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return ReviewState::new(),
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(err) => {
            eprintln!(
                "warning: ignoring unreadable review state {}: {err}",
                path.display()
            );
            ReviewState::new()
        }
    }
}

/// Write `state` to `path`, creating the directory if needed.
///
/// The write goes to a sibling temp file and is then renamed, so a crash
/// mid-write leaves the previous state intact instead of a half-written file.
pub fn save(path: impl AsRef<Path>, state: &ReviewState) -> Result<()> {
    let path = path.as_ref();
    let dir = path
        .parent()
        .with_context(|| format!("review state path has no parent: {}", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;

    let name = path
        .file_name()
        .with_context(|| format!("review state path has no file name: {}", path.display()))?;

    let json = serde_json::to_string_pretty(state).context("could not serialize review state")?;
    let temp = temp_path(dir, name);
    fs::write(&temp, json).with_context(|| format!("could not write {}", temp.display()))?;
    // A failing rename — a read-only directory, a target that turned into one
    // — would otherwise leave the staging file behind, and `POST /api/state`
    // fires on every click, so one persistent failure litters `.git/gribovik`
    // with a file per verdict.
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err).with_context(|| format!("could not write {}", path.display()));
    }
    Ok(())
}

/// A sibling of `path` to stage the write in. It has to share a directory with
/// the target for the rename to stay on one filesystem, and hence be atomic.
///
/// The counter matters as much as the pid: two saves racing inside one process
/// would otherwise stage into the same file, and the first rename would pull it
/// out from under the second.
fn temp_path(dir: &Path, name: &OsStr) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        ".{}.{}.{nonce}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn sample() -> ReviewState {
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::alpha".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![Comment {
                    text: "reads fine".to_string(),
                    created_at: "2026-09-01T10:00:00.000Z".to_string(),
                }],
            },
        );
        state.insert(
            "src/b.rs::B::beta".to_string(),
            NodeReview {
                status: Status::Rejected,
                comments: vec![],
            },
        );
        state
    }

    #[test]
    fn state_path_nests_under_git_dir() {
        assert_eq!(
            state_path("/tmp/repo/.git", "abc123", "def456"),
            PathBuf::from("/tmp/repo/.git/gribovik/abc123..def456.json")
        );
    }

    #[test]
    fn state_path_sanitizes_slashes_in_revision_names() {
        assert_eq!(
            state_path("/tmp/repo/.git", "origin/master", "feature/thing"),
            PathBuf::from("/tmp/repo/.git/gribovik/origin-master..feature-thing.json")
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "origin/main", "HEAD");

        save(&path, &sample()).unwrap();

        assert!(path.exists(), "save should create the state file");
        assert_eq!(load(&path), sample());
    }

    #[test]
    fn save_creates_the_gribovik_directory() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        assert!(!path.parent().unwrap().exists());

        save(&path, &ReviewState::new()).unwrap();

        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn save_leaves_no_temp_files_behind() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        save(&path, &sample()).unwrap();

        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "unexpected temp files: {leftovers:?}");
    }

    /// The staging file has to go even when the rename does not happen: a
    /// directory standing where the state file belongs fails every save, and
    /// the browser posts on every click.
    #[test]
    fn a_failed_rename_leaves_no_temp_file_behind() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        fs::create_dir_all(&path).unwrap();

        assert!(save(&path, &sample()).is_err());

        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "unexpected temp files: {leftovers:?}");
    }

    #[test]
    fn save_overwrites_a_previous_state() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        save(&path, &sample()).unwrap();

        let mut second = ReviewState::new();
        second.insert("src/c.rs::gamma".to_string(), NodeReview::default());
        save(&path, &second).unwrap();

        assert_eq!(load(&path), second);
    }

    #[test]
    fn load_of_a_missing_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");

        assert_eq!(load(&path), ReviewState::new());
    }

    #[test]
    fn load_of_corrupt_json_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is not json").unwrap();

        assert_eq!(load(&path), ReviewState::new());
    }

    #[test]
    fn load_of_a_directory_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        fs::create_dir_all(&path).unwrap();

        assert_eq!(load(&path), ReviewState::new());
    }

    /// Two saves racing inside one process used to stage into the same temp
    /// file, so one thread's rename pulled the file out from under the other.
    #[test]
    fn concurrent_saves_all_succeed() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let path = path.clone();
                scope.spawn(move || {
                    for _ in 0..20 {
                        save(&path, &sample()).expect("concurrent save");
                    }
                });
            }
        });

        // Whoever wrote last, the file is a whole state and not a fragment.
        assert_eq!(load(&path), sample());
        // And no staging file survived the race.
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn status_and_comments_use_the_wire_spelling() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(
            value,
            json!({
                "src/a.rs::alpha": {
                    "status": "approved",
                    "comments": [
                        { "text": "reads fine", "created_at": "2026-09-01T10:00:00.000Z" }
                    ]
                },
                "src/b.rs::B::beta": {
                    "status": "rejected",
                    "comments": []
                }
            })
        );
    }

    #[test]
    fn pending_is_the_default_status() {
        assert_eq!(NodeReview::default().status, Status::Pending);
        assert_eq!(
            serde_json::to_value(Status::Pending).unwrap(),
            json!("pending")
        );
        let filled_in: NodeReview = serde_json::from_str("{}").unwrap();
        assert_eq!(filled_in, NodeReview::default());
    }
}
