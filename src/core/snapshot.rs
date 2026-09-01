//! The GraphSnapshot wire contract.
//!
//! This module is one of exactly two places the contract is defined; the other
//! is `web/src/types/snapshot.ts`. Field names are snake_case on the wire and
//! must stay identical on both sides.

use serde::{Deserialize, Serialize};

/// A complete analysis result: what changed, and how the changed symbols call
/// each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub meta: Meta,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Context about the analyzed revision range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Absolute path to the repository root.
    pub repo: String,
    pub base: String,
    pub head: String,
    /// Number of files that contributed to the graph.
    pub files_changed: usize,
    /// Non-fatal problems worth surfacing to the reviewer.
    pub warnings: Vec<String>,
}

/// One reviewable card: a changed symbol, or a file-level catch-all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// `"<file>::<qualified_name>"`, unique within a snapshot.
    pub id: String,
    pub file: String,
    pub name: String,
    /// Language-specific symbol kind (`"function"`, `"method"`, `"struct"`, …)
    /// or `"file"` for the synthetic file-level node.
    pub kind: String,
    pub change: ChangeKind,
    /// The lines of the overall file diff that belong to this node.
    pub diff: Vec<DiffLine>,
}

/// A single line of a unified diff, carrying its position on both sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub tag: DiffTag,
    /// 1-based line number in the old revision; `None` for added lines.
    pub old_line: Option<u32>,
    /// 1-based line number in the new revision; `None` for deleted lines.
    pub new_line: Option<u32>,
    /// Line content without its trailing newline.
    pub text: String,
}

/// A resolved call from one changed symbol to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Node id of the caller.
    pub from: String,
    /// Node id of the callee.
    pub to: String,
    pub confidence: Confidence,
}

/// How a symbol changed between base and head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// How sure the resolver is that an edge points at the right callee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Certain,
    Ambiguous,
}

/// Which side of the diff a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffTag {
    Add,
    Del,
    Context,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> GraphSnapshot {
        GraphSnapshot {
            meta: Meta {
                repo: "/tmp/repo".to_string(),
                base: "abc123".to_string(),
                head: "def456".to_string(),
                files_changed: 2,
                warnings: vec!["skipped binary blob src/logo.png".to_string()],
            },
            nodes: vec![
                Node {
                    id: "src/a.rs::alpha".to_string(),
                    file: "src/a.rs".to_string(),
                    name: "alpha".to_string(),
                    kind: "function".to_string(),
                    change: ChangeKind::Added,
                    diff: vec![DiffLine {
                        tag: DiffTag::Add,
                        old_line: None,
                        new_line: Some(1),
                        text: "fn alpha() {}".to_string(),
                    }],
                },
                Node {
                    id: "src/b.rs::B::beta".to_string(),
                    file: "src/b.rs".to_string(),
                    name: "B::beta".to_string(),
                    kind: "method".to_string(),
                    change: ChangeKind::Modified,
                    diff: vec![
                        DiffLine {
                            tag: DiffTag::Context,
                            old_line: Some(4),
                            new_line: Some(4),
                            text: "    fn beta(&self) {".to_string(),
                        },
                        DiffLine {
                            tag: DiffTag::Del,
                            old_line: Some(5),
                            new_line: None,
                            text: "        alpha();".to_string(),
                        },
                    ],
                },
                Node {
                    id: "src/c.rs::Gone".to_string(),
                    file: "src/c.rs".to_string(),
                    name: "Gone".to_string(),
                    kind: "struct".to_string(),
                    change: ChangeKind::Deleted,
                    diff: vec![],
                },
            ],
            edges: vec![
                Edge {
                    from: "src/b.rs::B::beta".to_string(),
                    to: "src/a.rs::alpha".to_string(),
                    confidence: Confidence::Certain,
                },
                Edge {
                    from: "src/a.rs::alpha".to_string(),
                    to: "src/c.rs::Gone".to_string(),
                    confidence: Confidence::Ambiguous,
                },
            ],
        }
    }

    /// The JSON shape is a contract shared with `web/src/types/snapshot.ts`.
    /// Compare against a literal so any rename fails loudly here.
    #[test]
    fn serializes_with_the_exact_wire_field_names() {
        let expected = json!({
            "meta": {
                "repo": "/tmp/repo",
                "base": "abc123",
                "head": "def456",
                "files_changed": 2,
                "warnings": ["skipped binary blob src/logo.png"]
            },
            "nodes": [
                {
                    "id": "src/a.rs::alpha",
                    "file": "src/a.rs",
                    "name": "alpha",
                    "kind": "function",
                    "change": "added",
                    "diff": [
                        { "tag": "add", "old_line": null, "new_line": 1, "text": "fn alpha() {}" }
                    ]
                },
                {
                    "id": "src/b.rs::B::beta",
                    "file": "src/b.rs",
                    "name": "B::beta",
                    "kind": "method",
                    "change": "modified",
                    "diff": [
                        { "tag": "context", "old_line": 4, "new_line": 4, "text": "    fn beta(&self) {" },
                        { "tag": "del", "old_line": 5, "new_line": null, "text": "        alpha();" }
                    ]
                },
                {
                    "id": "src/c.rs::Gone",
                    "file": "src/c.rs",
                    "name": "Gone",
                    "kind": "struct",
                    "change": "deleted",
                    "diff": []
                }
            ],
            "edges": [
                { "from": "src/b.rs::B::beta", "to": "src/a.rs::alpha", "confidence": "certain" },
                { "from": "src/a.rs::alpha", "to": "src/c.rs::Gone", "confidence": "ambiguous" }
            ]
        });

        assert_eq!(serde_json::to_value(sample()).unwrap(), expected);
    }

    #[test]
    fn round_trips_through_json() {
        let snapshot = sample();
        let text = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<GraphSnapshot>(&text).unwrap(),
            snapshot
        );
    }

    /// `old_line`/`new_line` stay present as `null` rather than being omitted,
    /// so the TypeScript side can type them as `number | null`.
    #[test]
    fn absent_line_numbers_serialize_as_null() {
        let line = DiffLine {
            tag: DiffTag::Add,
            old_line: None,
            new_line: Some(7),
            text: String::new(),
        };
        let value = serde_json::to_value(&line).unwrap();
        assert!(value.get("old_line").is_some());
        assert!(value["old_line"].is_null());
    }

    #[test]
    fn enums_spell_themselves_in_lowercase() {
        for (change, spelled) in [
            (ChangeKind::Added, "added"),
            (ChangeKind::Modified, "modified"),
            (ChangeKind::Deleted, "deleted"),
        ] {
            assert_eq!(serde_json::to_value(change).unwrap(), json!(spelled));
            assert_eq!(
                serde_json::from_value::<ChangeKind>(json!(spelled)).unwrap(),
                change
            );
        }
        for (tag, spelled) in [
            (DiffTag::Add, "add"),
            (DiffTag::Del, "del"),
            (DiffTag::Context, "context"),
        ] {
            assert_eq!(serde_json::to_value(tag).unwrap(), json!(spelled));
            assert_eq!(
                serde_json::from_value::<DiffTag>(json!(spelled)).unwrap(),
                tag
            );
        }
        for (confidence, spelled) in [
            (Confidence::Certain, "certain"),
            (Confidence::Ambiguous, "ambiguous"),
        ] {
            assert_eq!(serde_json::to_value(confidence).unwrap(), json!(spelled));
            assert_eq!(
                serde_json::from_value::<Confidence>(json!(spelled)).unwrap(),
                confidence
            );
        }
    }

    #[test]
    fn rejects_capitalized_enum_spellings() {
        assert!(serde_json::from_value::<ChangeKind>(json!("Added")).is_err());
        assert!(serde_json::from_value::<DiffTag>(json!("Add")).is_err());
        assert!(serde_json::from_value::<Confidence>(json!("Certain")).is_err());
    }
}
