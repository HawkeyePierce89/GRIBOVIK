//! Errors raised by the pure analysis core.

use thiserror::Error;

/// Anything that can go wrong while analyzing sources. The core never touches
/// git, HTTP or the filesystem, so these are the only failure modes.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AnalysisError {
    /// A tree-sitter parse produced no usable tree for `path`.
    #[error("failed to parse {path}: {reason}")]
    Parse { path: String, reason: String },

    /// No `LanguageAnalyzer` is registered for this file extension.
    #[error("unsupported file extension: {0}")]
    UnsupportedExtension(String),
}

impl AnalysisError {
    /// Re-label a parse failure with the file it came from.
    ///
    /// Analyzers only ever see source text, so they report the language they
    /// were parsing as; the caller that owns the path swaps it in before the
    /// message reaches a reviewer. Other variants are returned untouched.
    pub fn with_path(self, path: impl Into<String>) -> Self {
        match self {
            Self::Parse { reason, .. } => Self::Parse {
                path: path.into(),
                reason,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_offending_input() {
        let parse = AnalysisError::Parse {
            path: "src/a.rs".to_string(),
            reason: "tree-sitter returned no tree".to_string(),
        };
        assert_eq!(
            parse.to_string(),
            "failed to parse src/a.rs: tree-sitter returned no tree"
        );

        assert_eq!(
            AnalysisError::UnsupportedExtension("md".to_string()).to_string(),
            "unsupported file extension: md"
        );
    }

    #[test]
    fn with_path_relabels_only_parse_failures() {
        let relabeled = AnalysisError::Parse {
            path: "rust".to_string(),
            reason: "source contains syntax errors".to_string(),
        }
        .with_path("src/a.rs");
        assert_eq!(
            relabeled.to_string(),
            "failed to parse src/a.rs: source contains syntax errors"
        );

        let untouched = AnalysisError::UnsupportedExtension("md".to_string());
        assert_eq!(untouched.clone().with_path("src/a.rs"), untouched);
    }
}
