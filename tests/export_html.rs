use std::fs;
use std::path::Path;
use std::process::Command;

use clap::Parser;
use gribovik::cli::{self, Args, Session};
use gribovik::git::Repo;
use tempfile::TempDir;

/// Run git in `dir`, asserting success and returning trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/master"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn write_file(dir: &Path, path: &str, contents: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, contents).unwrap();
}

fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn parse(args: &[&str]) -> Args {
    Args::try_parse_from(std::iter::once("gribovik").chain(args.iter().copied())).unwrap()
}

#[test]
fn export_html_writes_a_self_contained_page() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write_file(dir, "src/counter.rs", "pub fn bump() {}\n");
    let base = commit(dir, "baseline");
    write_file(
        dir,
        "src/counter.rs",
        "pub fn bump() { record(); }\npub fn record() {}\n",
    );
    let head = commit(dir, "feature");

    let repo = Repo::discover(dir).unwrap();
    let out_dir = TempDir::new().unwrap();
    let out_file = out_dir.path().join("out.html");

    let args = parse(&[&base, &head, "--export", out_file.to_str().unwrap()]);

    let session = cli::prepare(&repo, &args).unwrap();

    match session {
        Session::Export {
            snapshot,
            assets,
            path,
        } => {
            gribovik::export::write(&assets, &snapshot, &path).unwrap();
        }
        _ => panic!("expected Session::Export"),
    }

    // Assert exactly one file exists in the output directory
    let entries: Vec<_> = fs::read_dir(out_dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);

    let html = fs::read_to_string(&out_file).unwrap();
    assert!(
        html.contains("__GRIBOVIK_SNAPSHOT__"),
        "missing snapshot payload"
    );
    assert!(html.contains("<script"), "missing script tags");
    assert!(html.contains("<style"), "missing style tags");
    assert!(html.contains("record"), "missing changed symbol name");

    assert!(
        !html.contains("/assets/"),
        "contains un-inlined /assets/ path"
    );
    assert!(!html.contains("/api/"), "contains un-inlined /api/ path");
}

#[test]
fn export_with_no_changes_writes_no_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write_file(dir, "src/counter.rs", "pub fn bump() {}\n");
    let base = commit(dir, "baseline");

    let repo = Repo::discover(dir).unwrap();
    let out_dir = TempDir::new().unwrap();
    let out_file = out_dir.path().join("out.html");

    let args = parse(&[
        &base,
        &base, // no changes
        "--export",
        out_file.to_str().unwrap(),
    ]);

    let session = cli::prepare(&repo, &args).unwrap();

    match session {
        Session::NoChanges(msg) => {
            assert!(msg.contains("no reviewable changes"));
        }
        _ => panic!("expected Session::NoChanges"),
    }

    // Assert directory is empty
    let entries: Vec<_> = fs::read_dir(out_dir.path()).unwrap().collect();
    assert!(entries.is_empty(), "expected no files written");
}

#[test]
fn export_creates_missing_parent_directories() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write_file(dir, "src/counter.rs", "pub fn bump() {}\n");
    let base = commit(dir, "baseline");
    write_file(
        dir,
        "src/counter.rs",
        "pub fn bump() { record(); }\npub fn record() {}\n",
    );
    let head = commit(dir, "feature");

    let repo = Repo::discover(dir).unwrap();
    let out_dir = TempDir::new().unwrap();
    // Neither `out` nor `out/nested` exists yet.
    let out_file = out_dir
        .path()
        .join("out")
        .join("nested")
        .join("review.html");

    let args = parse(&[&base, &head, "--export", out_file.to_str().unwrap()]);

    let session = cli::prepare(&repo, &args).unwrap();

    match session {
        Session::Export {
            snapshot,
            assets,
            path,
        } => {
            gribovik::export::write(&assets, &snapshot, &path).unwrap();
        }
        _ => panic!("expected Session::Export"),
    }

    assert!(out_file.exists(), "export did not create {out_file:?}");
    let html = fs::read_to_string(&out_file).unwrap();
    assert!(
        html.contains("__GRIBOVIK_SNAPSHOT__"),
        "missing snapshot payload"
    );
}
