//! The command line: arguments, and the decision they lead to.
//!
//! `gribovik [BASE] [HEAD]` reviews a revision range. Both positionals are
//! optional because the common case is "review my branch": with no arguments
//! the base is the merge base with `origin/master` (or `origin/main`) and the
//! head is `HEAD`.
//!
//! Everything up to the point of binding a port lives in [`prepare`], so the
//! whole decision — including "there is nothing to review" — is testable
//! without a server.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use crate::git::Repo;
use crate::pipeline::{self, Analysis};
use crate::review;
use crate::server::assets::Assets;
use crate::server::AppState;

/// An interactive diff graph for manual code review.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "gribovik", version, about, long_about = None)]
pub struct Args {
    /// Revision to diff from. Defaults to the merge base with origin/master,
    /// falling back to origin/main.
    pub base: Option<String>,

    /// Revision to diff to. Defaults to HEAD.
    pub head: Option<String>,

    /// Port to serve on. 0 asks the OS for a free one.
    #[arg(long, default_value_t = 0, value_name = "PORT")]
    pub port: u16,

    /// Print the URL instead of opening a browser.
    #[arg(long)]
    pub no_open: bool,

    /// Serve the frontend from this directory instead of the embedded build.
    #[arg(long, value_name = "DIR")]
    pub assets: Option<PathBuf>,
}

/// What the CLI does once the diff has been analyzed.
#[derive(Debug)]
pub enum Session {
    /// Nothing changed in the range: a message for the reviewer, and no server.
    NoChanges(String),
    /// A graph worth looking at, and everything the server needs to show it.
    Serve {
        state: Arc<AppState>,
        port: u16,
        open: bool,
    },
}

/// Analyze the range named by `args` and decide what to do about it.
///
/// This does read and write the filesystem — it loads blobs through git and
/// reads any previously saved review state — but it never binds a port, so an
/// empty diff costs nothing.
pub fn prepare(repo: &Repo, args: &Args) -> Result<Session> {
    let analysis = pipeline::analyze(repo, args.base.as_deref(), args.head.as_deref())?;
    let snapshot = match analysis {
        Analysis::NoChanges { base, head } => {
            return Ok(Session::NoChanges(format!(
                "no reviewable changes between {base} and {head}"
            )));
        }
        Analysis::Graph(snapshot) => *snapshot,
    };

    let state_path = review::state_path(repo.git_dir()?, &snapshot.meta.base, &snapshot.meta.head);
    let state = review::load(&state_path);
    let assets = Assets::new(args.assets.clone());

    Ok(Session::Serve {
        state: Arc::new(AppState::new(snapshot, state, state_path, assets)),
        port: args.port,
        open: !args.no_open,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("gribovik").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn no_positionals_leaves_both_revisions_to_the_defaults() {
        let args = parse(&[]);
        assert_eq!(args.base, None);
        assert_eq!(args.head, None);
    }

    #[test]
    fn one_positional_is_the_base() {
        let args = parse(&["origin/main"]);
        assert_eq!(args.base.as_deref(), Some("origin/main"));
        assert_eq!(args.head, None);
    }

    #[test]
    fn two_positionals_are_base_then_head() {
        let args = parse(&["main", "feature"]);
        assert_eq!(args.base.as_deref(), Some("main"));
        assert_eq!(args.head.as_deref(), Some("feature"));
    }

    #[test]
    fn a_third_positional_is_rejected() {
        assert!(Args::try_parse_from(["gribovik", "a", "b", "c"]).is_err());
    }

    #[test]
    fn flags_default_to_an_ephemeral_port_an_open_browser_and_embedded_assets() {
        let args = parse(&[]);
        assert_eq!(args.port, 0);
        assert!(!args.no_open);
        assert_eq!(args.assets, None);
    }

    #[test]
    fn flags_are_read_alongside_positionals() {
        let args = parse(&[
            "main",
            "--port",
            "8080",
            "--no-open",
            "--assets",
            "web/dist",
        ]);
        assert_eq!(args.base.as_deref(), Some("main"));
        assert_eq!(args.port, 8080);
        assert!(args.no_open);
        assert_eq!(args.assets, Some(PathBuf::from("web/dist")));
    }

    #[test]
    fn a_non_numeric_port_is_rejected() {
        assert!(Args::try_parse_from(["gribovik", "--port", "http"]).is_err());
    }

    /// Run git in `dir`, asserting success.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git is installed");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository whose `master` and `HEAD` hold the same tree.
    fn repo_without_changes() -> (TempDir, Repo) {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["symbolic-ref", "HEAD", "refs/heads/master"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(dir.path().join("src.rs"), "fn one() {}\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);

        let repo = Repo::discover(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn an_empty_diff_prepares_a_message_instead_of_a_server() {
        let (_dir, repo) = repo_without_changes();
        let args = parse(&["master", "HEAD"]);

        match prepare(&repo, &args).unwrap() {
            Session::NoChanges(message) => {
                assert!(
                    message.contains("no reviewable changes"),
                    "unexpected message: {message}"
                );
            }
            Session::Serve { .. } => panic!("an unchanged range should not start a server"),
        }
    }

    #[test]
    fn an_unknown_revision_is_an_error_rather_than_an_empty_graph() {
        let (_dir, repo) = repo_without_changes();
        let args = parse(&["no-such-branch"]);

        let err = prepare(&repo, &args).unwrap_err().to_string();

        assert!(err.contains("unknown revision"), "unexpected error: {err}");
    }
}
