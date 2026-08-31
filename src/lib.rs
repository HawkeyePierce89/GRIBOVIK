//! GRIBOVIK — an interactive diff graph for manual code review.
//!
//! The crate is layered: [`core`] is pure analysis, and the modules around it
//! (git, server, CLI) form a thin I/O shell.

pub mod core;
pub mod git;
pub mod pipeline;
pub mod review;
