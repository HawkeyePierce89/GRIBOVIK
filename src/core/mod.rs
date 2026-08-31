//! Pure analysis core.
//!
//! Everything under `core` is git-free, HTTP-free and filesystem-free: it takes
//! source text in and returns a [`snapshot::GraphSnapshot`]. Errors are
//! `thiserror`-based ([`error::AnalysisError`]); `anyhow` starts at the shell.

pub mod diff;
pub mod edges;
pub mod error;
pub mod lang;
pub mod nodes;
pub mod snapshot;

use std::collections::BTreeSet;

pub use edges::build_edges;
pub use error::AnalysisError;
pub use lang::{analyzer_for_extension, supports_extension, LanguageAnalyzer, Symbol};
pub use nodes::{build_nodes, FileInput, FILE_KIND};
pub use snapshot::{ChangeKind, Confidence, DiffLine, DiffTag, Edge, GraphSnapshot, Meta, Node};

/// What the I/O shell knows about a run and the core cannot discover on its
/// own: where the repository is, which revisions were compared, and anything
/// that already went wrong while loading the files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaInput {
    /// Absolute path to the repository root.
    pub repo: String,
    pub base: String,
    pub head: String,
    /// Warnings raised before analysis started (unreadable blobs and the
    /// like). Warnings from the analysis itself are appended to these.
    pub warnings: Vec<String>,
}

impl MetaInput {
    /// A meta input with no pre-existing warnings.
    pub fn new(repo: impl Into<String>, base: impl Into<String>, head: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            base: base.into(),
            head: head.into(),
            warnings: Vec::new(),
        }
    }
}

/// The core's single entry point: changed files in, a complete graph out.
///
/// `files_changed` counts the files that actually contributed a card, not the
/// files handed in — a listed file whose content did not change produces no
/// nodes and does not count.
pub fn build_snapshot(meta: MetaInput, files: &[FileInput]) -> GraphSnapshot {
    let MetaInput {
        repo,
        base,
        head,
        mut warnings,
    } = meta;
    let (nodes, analysis_warnings) = build_nodes(files);
    let edges = build_edges(files, &nodes);
    warnings.extend(analysis_warnings);
    let files_changed = nodes
        .iter()
        .map(|node| node.file.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    GraphSnapshot {
        meta: Meta {
            repo,
            base,
            head,
            files_changed,
            warnings,
        },
        nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(snapshot: &GraphSnapshot) -> Vec<&str> {
        snapshot.nodes.iter().map(|node| node.id.as_str()).collect()
    }

    #[test]
    fn wires_nodes_edges_and_meta_together() {
        let files = [
            FileInput::modified(
                "src/a.rs",
                "fn caller() {\n}\n",
                "fn caller() {\n    callee();\n}\n",
            ),
            FileInput::added("src/b.rs", "fn callee() {\n}\n"),
        ];
        let snapshot = build_snapshot(MetaInput::new("/tmp/repo", "aaa", "bbb"), &files);

        assert_eq!(snapshot.meta.repo, "/tmp/repo");
        assert_eq!(snapshot.meta.base, "aaa");
        assert_eq!(snapshot.meta.head, "bbb");
        assert_eq!(snapshot.meta.files_changed, 2);
        assert!(snapshot.meta.warnings.is_empty());
        assert_eq!(ids(&snapshot), ["src/a.rs::caller", "src/b.rs::callee"]);
        assert_eq!(
            snapshot.edges,
            vec![Edge {
                from: "src/a.rs::caller".to_string(),
                to: "src/b.rs::callee".to_string(),
                confidence: Confidence::Certain,
            }]
        );
    }

    /// Shell warnings come first, then whatever the analysis had to say, so a
    /// reviewer reads them in the order they happened.
    #[test]
    fn keeps_shell_warnings_and_appends_analysis_warnings() {
        let meta = MetaInput {
            repo: "/tmp/repo".to_string(),
            base: "aaa".to_string(),
            head: "bbb".to_string(),
            warnings: vec!["skipped logo.png: not UTF-8 text".to_string()],
        };
        let files = [FileInput::added("notes.md", "# hi\n")];
        let snapshot = build_snapshot(meta, &files);

        assert_eq!(snapshot.meta.warnings.len(), 2);
        assert_eq!(
            snapshot.meta.warnings[0],
            "skipped logo.png: not UTF-8 text"
        );
        assert!(snapshot.meta.warnings[1].contains("unsupported file extension"));
        assert_eq!(ids(&snapshot), ["notes.md::<file>"]);
    }

    /// A file listed as changed whose text is identical yields no cards, so it
    /// must not inflate the counter either.
    #[test]
    fn counts_only_files_that_produced_cards() {
        let files = [
            FileInput::modified("src/a.rs", "fn same() {}\n", "fn same() {}\n"),
            FileInput::added("src/b.rs", "fn fresh() {}\n"),
        ];
        let snapshot = build_snapshot(MetaInput::new("/tmp/repo", "aaa", "bbb"), &files);

        assert_eq!(ids(&snapshot), ["src/b.rs::fresh"]);
        assert_eq!(snapshot.meta.files_changed, 1);
    }

    #[test]
    fn an_empty_file_list_yields_an_empty_graph() {
        let snapshot = build_snapshot(MetaInput::new("/tmp/repo", "aaa", "bbb"), &[]);
        assert!(snapshot.nodes.is_empty());
        assert!(snapshot.edges.is_empty());
        assert_eq!(snapshot.meta.files_changed, 0);
    }
}
