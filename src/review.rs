//! Review state: what the reviewer decided about each node, on disk.
//!
//! State lives beside the repository it describes, under
//! `.git/gribovik/<base>..<head>.json`, so it survives restarts of the CLI and
//! disappears with the clone. This module is part of the I/O shell.
//!
//! Reading is deliberately forgiving: a missing or unreadable file yields an
//! empty state rather than an error, because losing a few approvals is a much
//! smaller problem than refusing to start a review at all.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

/// The state file for one revision range inside `repo_root`.
///
/// Revision names go into the file name verbatim except for `/`, which would
/// otherwise turn `origin/master..HEAD` into nested directories.
pub fn state_path(repo_root: impl AsRef<Path>, base: &str, head: &str) -> PathBuf {
    repo_root
        .as_ref()
        .join(".git")
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

    let json = serde_json::to_string_pretty(state).context("could not serialize review state")?;
    let temp = temp_path(path);
    fs::write(&temp, json).with_context(|| format!("could not write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

/// A sibling of `path` to stage the write in. It has to share a directory with
/// the target for the rename to stay on one filesystem, and hence be atomic.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state.json".to_string());
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(".{name}.{}.tmp", std::process::id()))
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
            state_path("/tmp/repo", "abc123", "def456"),
            PathBuf::from("/tmp/repo/.git/gribovik/abc123..def456.json")
        );
    }

    #[test]
    fn state_path_sanitizes_slashes_in_revision_names() {
        assert_eq!(
            state_path("/tmp/repo", "origin/master", "feature/thing"),
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
