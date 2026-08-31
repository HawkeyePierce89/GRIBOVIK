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

pub use edges::build_edges;
pub use error::AnalysisError;
pub use lang::{analyzer_for_extension, LanguageAnalyzer, Symbol};
pub use nodes::{build_nodes, FileInput, FILE_KIND};
pub use snapshot::{ChangeKind, Confidence, DiffLine, DiffTag, Edge, GraphSnapshot, Meta, Node};
