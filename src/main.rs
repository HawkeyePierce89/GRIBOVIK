//! Binary entry point.
//!
//! This is the `anyhow` boundary: everything below returns errors, and here
//! they collapse into one line on stderr and exit code 1. There is no backtrace
//! and no panic message for the reviewer to decode — "not a git repository" is
//! the whole story.

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use gribovik::cli::{self, Args, Session};
use gribovik::git::Repo;
use gribovik::server;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("gribovik: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    let repo = Repo::discover(cwd)?;

    let session = cli::prepare(&repo, &args)?;
    let (state, port, open) = match session {
        // Nothing to review is a success, not a failure: the reviewer asked a
        // question and got an answer.
        Session::NoChanges(message) => {
            println!("{message}");
            return Ok(());
        }
        Session::Export {
            snapshot,
            assets,
            path,
        } => {
            gribovik::export::write(&assets, &snapshot, &path)?;
            println!("exported to {}", path.display());
            return Ok(());
        }
        Session::Serve { state, port, open } => (state, port, open),
    };

    // The URL is only known once the OS has handed out a port, so both the
    // announcement and the browser launch happen from the bind callback.
    server::serve(state, port, |addr| {
        let url = format!("http://{addr}");
        println!("gribovik is reviewing at {url}");
        println!("press Ctrl+C to stop");
        if open && webbrowser::open(&url).is_err() {
            eprintln!("gribovik: could not open a browser; visit {url}");
        }
    })
    .await
}
