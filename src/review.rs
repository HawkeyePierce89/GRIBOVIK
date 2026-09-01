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

use crate::core::snapshot::{DiffTag, GraphSnapshot, Node};

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
    /// [`fingerprint`] of the node's diff when the status was recorded.
    ///
    /// The state file is keyed by branch rather than by commit, so it outlives
    /// the commits it describes on purpose — a new commit must not orphan a
    /// review of four hundred cards. What it must not do is carry an approval
    /// across a change to the very code that was approved, and the node id
    /// alone cannot tell those apart: `src/a.rs::foo` names the same card
    /// before and after `foo` is rewritten. Stamping the diff the reviewer
    /// actually looked at is what lets [`reconcile`] send exactly the changed
    /// cards back to pending and leave the rest alone.
    ///
    /// `None` on an entry written before this field existed, and on one the
    /// server has never stamped; both are treated as "does not match".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
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
/// Revision names are percent-encoded on the way in, both so that a `/` cannot
/// turn `origin/master..HEAD` into nested directories and so that no two
/// revision names can land on one file; see [`sanitize`].
pub fn state_path(git_dir: impl AsRef<Path>, base: &str, head: &str) -> PathBuf {
    git_dir
        .as_ref()
        .join("gribovik")
        .join(format!("{}..{}.json", sanitize(base), sanitize(head)))
}

/// Encode a revision name so that it can be a file name and still names only
/// itself.
///
/// Anything outside `[A-Za-z0-9._-]` becomes `%XX`, `%` included, which makes
/// the mapping reversible and therefore collision-free. Replacing `/` with `-`
/// was not: `feature/foo` and `feature-foo` are different branches that share
/// a merge base, so they were filed under one name, and the first click on one
/// of them overwrote the other's verdicts with its own.
///
/// `.` is left alone even though the parts are joined with `..`, because the
/// only way that could be ambiguous is a revision name containing `..`, and
/// git forbids that in a ref and rejects it as a commit-ish before the range
/// ever reaches this function.
fn sanitize(rev: &str) -> String {
    let mut out = String::with_capacity(rev.len());
    for byte in rev.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => {
                out.push(char::from(byte))
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// FNV-1a, 64-bit: the offset basis and the prime.
///
/// Hand-rolled rather than pulled from a crate because the fingerprint never
/// leaves this machine and defends against nothing but accident — what it
/// needs is to be identical across runs and versions of the compiler, which
/// `DefaultHasher` explicitly does not promise.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn feed(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A short digest of the diff a node is showing.
///
/// The tag and the text of every line go in, and deliberately *not* the line
/// numbers. What the reviewer approved is the content of the card; where it
/// sits in the file is not part of that judgement, and hashing the positions
/// would send every card below an added import back to pending on a branch
/// that is still being written — which is most of the review, most of the time.
/// A card whose fingerprint is unchanged is a card whose text is unchanged, and
/// a verdict on it is still a verdict on what is there now.
pub fn fingerprint(node: &Node) -> String {
    let mut hash = FNV_OFFSET;
    for line in &node.diff {
        hash = feed(
            hash,
            &[match line.tag {
                DiffTag::Add => b'+',
                DiffTag::Del => b'-',
                DiffTag::Context => b' ',
            }],
        );
        hash = feed(hash, line.text.as_bytes());
        hash = feed(hash, b"\n");
    }
    format!("{hash:016x}")
}

/// Drop the statuses that were recorded against a different version of the
/// code, keeping everything else.
///
/// Run once, on the state loaded at startup, against the snapshot the session
/// will serve. A node whose diff still fingerprints the same keeps its verdict;
/// one whose diff moved on goes back to pending, because "approved" there would
/// mean the reviewer approved lines they have never seen. Comments survive
/// either way — they are the reviewer's own words, and a stale note is worth
/// more than no note.
///
/// Entries for nodes that are not in this snapshot at all are left untouched:
/// there is nothing to compare them against, and a range that stops showing a
/// file should not silently erase what was decided about it.
pub fn reconcile(state: ReviewState, snapshot: &GraphSnapshot) -> ReviewState {
    let current: BTreeMap<&str, String> = snapshot
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), fingerprint(node)))
        .collect();

    state
        .into_iter()
        .filter_map(|(id, mut review)| {
            match current.get(id.as_str()) {
                Some(now) if review.fingerprint.as_deref() == Some(now.as_str()) => {}
                Some(_) => {
                    review.status = Status::Pending;
                    review.fingerprint = None;
                }
                None => {}
            }
            // An entry that is now pending with nothing else in it says exactly
            // what an absent entry says, and keeping it would grow the file by
            // one dead record per rewritten symbol.
            if review.status == Status::Pending && review.comments.is_empty() {
                return None;
            }
            Some((id, review))
        })
        .collect()
}

/// Stamp every entry with the fingerprint of the node it describes, right
/// before the state is written.
///
/// The server does this rather than the browser: the fingerprint is a claim
/// about what the analysis produced, and the analysis lives on this side. The
/// client round-trips the field without reading it, so the two sides never have
/// to agree on a hash function.
pub fn stamp(state: &mut ReviewState, snapshot: &GraphSnapshot) {
    for node in &snapshot.nodes {
        if let Some(review) = state.get_mut(&node.id) {
            review.fingerprint = Some(fingerprint(node));
        }
    }
}

/// Read the state at `path`, falling back to an empty state.
///
/// A missing file is the normal first-run case and says nothing. Every other
/// failure — no permission, not valid UTF-8, corrupt JSON — is reported on
/// stderr and then ignored, so a hand-edited or truncated file costs the
/// reviewer their marks but not the session. The report is the whole point:
/// the first click of the new session writes the empty state back over the
/// file, so a silent fallback would turn "this file is unreadable today" into
/// "this review is gone", with nothing on the terminal to say so.
pub fn load(path: impl AsRef<Path>) -> ReviewState {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return ReviewState::new(),
        Err(err) => {
            eprintln!(
                "warning: ignoring unreadable review state {}: {err}",
                path.display()
            );
            return ReviewState::new();
        }
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
    use crate::core::snapshot::{ChangeKind, DiffLine, Meta};
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
                fingerprint: None,
            },
        );
        state.insert(
            "src/b.rs::B::beta".to_string(),
            NodeReview {
                status: Status::Rejected,
                comments: vec![],
                fingerprint: None,
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
    fn state_path_encodes_slashes_in_revision_names() {
        assert_eq!(
            state_path("/tmp/repo/.git", "origin/master", "feature/thing"),
            PathBuf::from("/tmp/repo/.git/gribovik/origin%2Fmaster..feature%2Fthing.json")
        );
    }

    #[test]
    fn branches_that_differ_only_in_a_separator_get_different_files() {
        assert_ne!(
            state_path("/tmp/repo/.git", "abc123", "feature/foo"),
            state_path("/tmp/repo/.git", "abc123", "feature-foo")
        );
    }

    #[test]
    fn an_encoded_name_is_encoded_again_rather_than_colliding() {
        assert_ne!(
            state_path("/tmp/repo/.git", "abc123", "feature/foo"),
            state_path("/tmp/repo/.git", "abc123", "feature%2Ffoo")
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

    /// A file that is not UTF-8 fails in `read_to_string`, not in serde, so it
    /// used to take the silent path — an empty state with nothing on stderr,
    /// and the reviewer's first click wrote that emptiness back over the file.
    #[test]
    fn load_of_a_file_that_is_not_utf8_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = state_path(dir.path(), "base", "head");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, [0x7b, 0xff, 0xfe, 0x7d]).unwrap();

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

    #[test]
    fn a_fingerprint_is_written_and_read_under_its_own_name() {
        let review = NodeReview {
            status: Status::Approved,
            comments: vec![],
            fingerprint: Some("28438e3e5b981cc8".to_string()),
        };
        let value = serde_json::to_value(&review).unwrap();
        assert_eq!(
            value,
            json!({
                "status": "approved",
                "comments": [],
                "fingerprint": "28438e3e5b981cc8"
            })
        );
        assert_eq!(serde_json::from_value::<NodeReview>(value).unwrap(), review);
    }

    fn node(id: &str, text: &str) -> Node {
        Node {
            id: id.to_string(),
            file: "src/a.rs".to_string(),
            name: "foo".to_string(),
            kind: "function".to_string(),
            change: ChangeKind::Modified,
            diff: vec![DiffLine {
                tag: DiffTag::Add,
                old_line: None,
                new_line: Some(1),
                text: text.to_string(),
            }],
        }
    }

    fn snapshot(nodes: Vec<Node>) -> GraphSnapshot {
        GraphSnapshot {
            meta: Meta {
                repo: "repo".to_string(),
                base: "abc123".to_string(),
                head: "feature".to_string(),
                files_changed: 1,
                warnings: vec![],
            },
            nodes,
            edges: vec![],
        }
    }

    #[test]
    fn a_fingerprint_follows_the_diff_and_nothing_else() {
        assert_eq!(
            fingerprint(&node("src/a.rs::foo", "same")),
            fingerprint(&node("src/a.rs::other", "same"))
        );
        assert_ne!(
            fingerprint(&node("src/a.rs::foo", "one")),
            fingerprint(&node("src/a.rs::foo", "two"))
        );
    }

    /// An edit higher up the file renumbers every line below it without
    /// touching a word of them. The card is the same card, so the verdict on it
    /// has to survive; hashing the line numbers is what used to send it back to
    /// pending.
    #[test]
    fn a_fingerprint_ignores_where_in_the_file_the_card_sits() {
        let here = node("src/a.rs::foo", "same");
        let mut moved = node("src/a.rs::foo", "same");
        for line in &mut moved.diff {
            line.old_line = line.old_line.map(|n| n + 40);
            line.new_line = line.new_line.map(|n| n + 40);
        }
        assert_eq!(fingerprint(&here), fingerprint(&moved));

        let reconciled = reconcile(
            ReviewState::from([(
                "src/a.rs::foo".to_string(),
                NodeReview {
                    status: Status::Approved,
                    comments: vec![],
                    fingerprint: Some(fingerprint(&here)),
                },
            )]),
            &snapshot(vec![moved]),
        );
        assert_eq!(reconciled["src/a.rs::foo"].status, Status::Approved);
    }

    #[test]
    fn stamping_records_the_fingerprint_of_each_reviewed_node() {
        let snapshot = snapshot(vec![node("src/a.rs::foo", "one")]);
        let mut state = ReviewState::new();
        state.insert("src/a.rs::foo".to_string(), NodeReview::default());
        state.insert("src/gone.rs::old".to_string(), NodeReview::default());

        stamp(&mut state, &snapshot);

        assert_eq!(
            state["src/a.rs::foo"].fingerprint.as_deref(),
            Some(fingerprint(&node("src/a.rs::foo", "one")).as_str())
        );
        assert_eq!(state["src/gone.rs::old"].fingerprint, None);
    }

    #[test]
    fn reconcile_keeps_a_verdict_on_code_that_did_not_move() {
        let snapshot = snapshot(vec![node("src/a.rs::foo", "one")]);
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::foo".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![],
                fingerprint: Some(fingerprint(&node("src/a.rs::foo", "one"))),
            },
        );

        let reconciled = reconcile(state.clone(), &snapshot);

        assert_eq!(reconciled, state);
    }

    #[test]
    fn reconcile_sends_a_rewritten_symbol_back_to_pending() {
        // The same node id, approved against the diff it used to carry.
        let snapshot = snapshot(vec![node("src/a.rs::foo", "two")]);
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::foo".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![Comment {
                    text: "looked at the old one".to_string(),
                    created_at: "2026-09-01T10:00:00.000Z".to_string(),
                }],
                fingerprint: Some(fingerprint(&node("src/a.rs::foo", "one"))),
            },
        );

        let reconciled = reconcile(state, &snapshot);

        let entry = &reconciled["src/a.rs::foo"];
        assert_eq!(entry.status, Status::Pending);
        assert_eq!(entry.fingerprint, None);
        // The reviewer's own words outlive the verdict they came with.
        assert_eq!(entry.comments.len(), 1);
    }

    #[test]
    fn reconcile_drops_a_verdict_that_has_nothing_left_to_say() {
        let snapshot = snapshot(vec![node("src/a.rs::foo", "two")]);
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::foo".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![],
                fingerprint: Some(fingerprint(&node("src/a.rs::foo", "one"))),
            },
        );

        assert_eq!(reconcile(state, &snapshot), ReviewState::new());
    }

    /// A state file written before fingerprints existed carries none, and the
    /// safe reading of "no record of what was approved" is "not approved".
    #[test]
    fn reconcile_does_not_trust_an_unstamped_verdict() {
        let snapshot = snapshot(vec![node("src/a.rs::foo", "one")]);
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::foo".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![Comment {
                    text: "from an older run".to_string(),
                    created_at: "2026-09-01T10:00:00.000Z".to_string(),
                }],
                fingerprint: None,
            },
        );

        let reconciled = reconcile(state, &snapshot);

        assert_eq!(reconciled["src/a.rs::foo"].status, Status::Pending);
    }

    /// A node the current range does not show cannot be compared against
    /// anything, and erasing it would lose a verdict the reviewer may still
    /// want when the range comes back.
    #[test]
    fn reconcile_leaves_a_node_outside_the_snapshot_alone() {
        let snapshot = snapshot(vec![node("src/a.rs::foo", "one")]);
        let mut state = ReviewState::new();
        state.insert(
            "src/elsewhere.rs::bar".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![],
                fingerprint: Some("stale".to_string()),
            },
        );

        let reconciled = reconcile(state.clone(), &snapshot);

        assert_eq!(reconciled, state);
    }
}
