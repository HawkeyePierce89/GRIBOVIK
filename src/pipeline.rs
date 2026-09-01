//! The bridge between git and the analysis core.
//!
//! Everything git-shaped stops here: this module resolves the revision range,
//! loads the two sides of every changed source file, and hands the pure core a
//! plain list of [`FileInput`]s. It is part of the I/O shell, so it speaks
//! `anyhow` — the messages end up on a reviewer's terminal.

use anyhow::Result;

use crate::core::nodes::extension;
use crate::core::{build_snapshot, supports_extension, FileInput, GraphSnapshot, MetaInput};
use crate::git::{Blob, FileStatus, Repo};

/// The revision analyzed when the caller does not name one.
pub const DEFAULT_HEAD: &str = "HEAD";

/// What an analysis run found.
///
/// The empty case is its own variant rather than an empty snapshot so the CLI
/// can say "nothing to review" and exit instead of opening a browser onto a
/// blank canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Analysis {
    /// The range touches no reviewable source: no supported file changed, or
    /// the ones that did turned out to hold no changed lines.
    ///
    /// The warnings ride along because they are the whole explanation on this
    /// path: a range whose only `.rs` file is unreadable produces "no
    /// reviewable changes" and exit 0, which reads as "nothing changed"
    /// unless the reason the file was skipped comes with it.
    NoChanges {
        base: String,
        head: String,
        warnings: Vec<String>,
    },
    /// A graph with at least one card.
    Graph(Box<GraphSnapshot>),
}

impl Analysis {
    /// The snapshot, if there is anything to review.
    pub fn snapshot(&self) -> Option<&GraphSnapshot> {
        match self {
            Analysis::Graph(snapshot) => Some(snapshot),
            Analysis::NoChanges { .. } => None,
        }
    }

    /// The revision range that was analyzed, whatever the outcome.
    pub fn range(&self) -> (&str, &str) {
        match self {
            Analysis::Graph(snapshot) => (&snapshot.meta.base, &snapshot.meta.head),
            Analysis::NoChanges { base, head, .. } => (base, head),
        }
    }
}

/// Analyze `base..head` in `repo`.
///
/// `base` is resolved the way [`Repo::resolve_base`] describes — an explicit
/// revision or the `origin/master`/`origin/main` fallback, in both cases
/// reduced to the merge base with `head`, so the graph shows what `head` added
/// rather than what the base branch moved on to. `head` defaults to `HEAD` and
/// is kept as written: review state is filed under this name, and a branch
/// name stays stable across new commits where a sha would not.
pub fn analyze(repo: &Repo, base: Option<&str>, head: Option<&str>) -> Result<Analysis> {
    let head = head.unwrap_or(DEFAULT_HEAD);
    let base = repo.resolve_base(base, head)?;

    let mut warnings = Vec::new();
    let mut files = Vec::new();
    for changed in repo.changed_files(&base, head)? {
        if !supports_extension(extension(&changed.path)) {
            continue;
        }
        // An added file has no base side and a deleted one no head side;
        // asking git for them would only produce noise.
        let wants_old = changed.status != FileStatus::Added;
        let wants_new = changed.status != FileStatus::Deleted;
        let old = match wants_old {
            true => read_side(repo, &base, &changed.path, &mut warnings)?,
            false => None,
        };
        let new = match wants_new {
            true => read_side(repo, head, &changed.path, &mut warnings)?,
            false => None,
        };
        // A side we expected and could not read leaves nothing trustworthy to
        // diff against; `read_side` already said why.
        if (wants_old && old.is_none()) || (wants_new && new.is_none()) {
            continue;
        }
        files.push(FileInput {
            path: changed.path,
            old,
            new,
        });
    }

    if files.is_empty() {
        return Ok(Analysis::NoChanges {
            base,
            head: head.to_string(),
            warnings,
        });
    }

    let meta = MetaInput {
        repo: repo.root().display().to_string(),
        base: base.clone(),
        head: head.to_string(),
        warnings,
    };
    let snapshot = build_snapshot(meta, &files);
    if snapshot.nodes.is_empty() {
        return Ok(Analysis::NoChanges {
            base,
            head: head.to_string(),
            warnings: snapshot.meta.warnings,
        });
    }
    Ok(Analysis::Graph(Box::new(snapshot)))
}

/// Read one side of a file, warning about anything that keeps it out of the
/// graph. Only called for sides the file's status says should exist.
fn read_side(
    repo: &Repo,
    rev: &str,
    path: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    match repo.blob(rev, path)? {
        Blob::Text(text) => Ok(Some(text)),
        Blob::NonUtf8 => {
            warnings.push(format!("skipped {path}: not UTF-8 text at {rev}"));
            Ok(None)
        }
        Blob::Missing => {
            warnings.push(format!("skipped {path}: missing at {rev}"));
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::snapshot::{ChangeKind, Meta};
    use crate::core::Node;

    fn snapshot_with(nodes: Vec<Node>) -> GraphSnapshot {
        GraphSnapshot {
            meta: Meta {
                repo: "/tmp/repo".to_string(),
                base: "aaa".to_string(),
                head: "feature".to_string(),
                files_changed: nodes.len(),
                warnings: Vec::new(),
            },
            nodes,
            edges: Vec::new(),
        }
    }

    #[test]
    fn a_graph_exposes_its_snapshot_and_range() {
        let analysis = Analysis::Graph(Box::new(snapshot_with(vec![Node {
            id: "src/a.rs::alpha".to_string(),
            file: "src/a.rs".to_string(),
            name: "alpha".to_string(),
            kind: "function".to_string(),
            change: ChangeKind::Added,
            diff: Vec::new(),
        }])));

        assert_eq!(analysis.range(), ("aaa", "feature"));
        assert_eq!(analysis.snapshot().unwrap().nodes.len(), 1);
        assert_eq!(analysis.snapshot().unwrap().nodes.len(), 1);
    }

    #[test]
    fn no_changes_carries_the_range_but_no_snapshot() {
        let analysis = Analysis::NoChanges {
            base: "aaa".to_string(),
            head: "feature".to_string(),
            warnings: Vec::new(),
        };
        assert_eq!(analysis.range(), ("aaa", "feature"));
        assert!(analysis.snapshot().is_none());
        assert!(analysis.snapshot().is_none());
    }
}
