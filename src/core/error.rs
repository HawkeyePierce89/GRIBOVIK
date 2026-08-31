//! Errors raised by the pure analysis core.

use thiserror::Error;

/// Anything that can go wrong while analyzing sources. The core never touches
/// git, HTTP or the filesystem, so these are the only failure modes.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnalysisError {
    /// A tree-sitter parse produced no usable tree for `path`.
    #[error("failed to parse {path}: {reason}")]
    Parse { path: String, reason: String },

    /// No `LanguageAnalyzer` is registered for this file extension.
    #[error("unsupported file extension: {0}")]
    UnsupportedExtension(String),

    /// A line range is empty-in-the-wrong-direction or points past the source.
    #[error("invalid line range {start}..{end}")]
    InvalidRange { start: u32, end: u32 },
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

        assert_eq!(
            AnalysisError::InvalidRange { start: 12, end: 4 }.to_string(),
            "invalid line range 12..4"
        );
    }
}
