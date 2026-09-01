//! Single-file HTML export.
//!
//! The frontend build (`npm run build`) uses Vite to produce a self-contained
//! `export.html` (inlined JS, CSS, no external assets). This module reads that
//! shell and injects the snapshot as a JSON literal in an inline `<script>` tag.
//! The shell itself expects `window.__GRIBOVIK_SNAPSHOT__` to be populated.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

use crate::core::snapshot::GraphSnapshot;
use crate::server::assets::Assets;

/// Serializes the snapshot into JSON, escapes it for inclusion in an inline
/// script tag, and injects it into the HTML page immediately before `</head>`.
pub fn embed_snapshot(page: &str, snapshot: &GraphSnapshot) -> Result<String> {
    let json = serde_json::to_string(snapshot).context("failed to serialize snapshot")?;
    // Escape '<' to prevent XSS and </script> breakout.
    let escaped_json = json.replace("<", "\\u003c");

    let script = format!(
        "<script id=\"gribovik-snapshot\">window.__GRIBOVIK_SNAPSHOT__ = {};</script>",
        escaped_json
    );

    let head_close_idx = page
        .find("</head>")
        .ok_or_else(|| anyhow!("export shell is missing </head> anchor"))?;

    let mut result = String::with_capacity(page.len() + script.len());
    result.push_str(&page[..head_close_idx]);
    result.push_str(&script);
    result.push_str(&page[head_close_idx..]);

    Ok(result)
}

/// Reads the `export.html` shell, injects the snapshot, and writes to `path`.
pub fn write(assets: &Assets, snapshot: &GraphSnapshot, path: &Path) -> Result<()> {
    let page_bytes = assets
        .read("export.html")
        .ok_or_else(|| anyhow!("export.html is missing from assets"))?;
    let page = String::from_utf8(page_bytes).context("export.html is not valid UTF-8")?;

    let embedded = embed_snapshot(&page, snapshot)?;
    std::fs::write(path, embedded)
        .with_context(|| format!("failed to write export to {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::snapshot::{ChangeKind, DiffLine, DiffTag, Meta, Node};

    #[test]
    fn embed_snapshot_roundtrips_escaped_json() {
        let mut snapshot = GraphSnapshot {
            meta: Meta {
                repo: "/tmp/repo".to_string(),
                base: "abc123".to_string(),
                head: "def456".to_string(),
                files_changed: 1,
                warnings: vec![],
            },
            nodes: vec![],
            edges: vec![],
        };
        snapshot.nodes.push(Node {
            id: "src/test.rs::foo".to_string(),
            file: "test.rs".to_string(),
            name: "foo".to_string(),
            kind: "function".to_string(),
            change: ChangeKind::Modified,
            diff: vec![DiffLine {
                tag: DiffTag::Add,
                old_line: None,
                new_line: Some(1),
                text: "let x = \"</script>\";".to_string(),
            }],
        });

        let page = "<html><head></head><body></body></html>";
        let output = embed_snapshot(page, &snapshot).unwrap();

        // Ensure no literal </script> exists before the tag we emit,
        // specifically in our injected data.
        let script_start = output.find("<script id=\"gribovik-snapshot\">").unwrap();
        let payload_start =
            script_start + "<script id=\"gribovik-snapshot\">window.__GRIBOVIK_SNAPSHOT__ = ".len();
        let script_end = output[payload_start..].find(";</script>").unwrap() + payload_start;
        let payload = &output[payload_start..script_end];

        assert!(!payload.contains("</script>"));

        let roundtripped: GraphSnapshot = serde_json::from_str(payload).unwrap();
        assert_eq!(roundtripped.nodes[0].diff[0].text, "let x = \"</script>\";");
    }

    #[test]
    fn embed_snapshot_requires_head_anchor() {
        let snapshot = GraphSnapshot {
            meta: Meta {
                repo: "/tmp/repo".to_string(),
                base: "abc123".to_string(),
                head: "def456".to_string(),
                files_changed: 0,
                warnings: vec![],
            },
            nodes: vec![],
            edges: vec![],
        };
        let page = "<html><body></body></html>";
        assert!(embed_snapshot(page, &snapshot).is_err());
    }

    #[test]
    fn embedded_asset_contains_head_anchor() {
        let assets = Assets::new(None);
        let page_bytes = assets.read("export.html").unwrap();
        let page = String::from_utf8(page_bytes).unwrap();
        assert!(page.contains("</head>"));
    }
}
