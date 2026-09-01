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

/// A two-commit repo in `dir` whose head adds one symbol and a call to it,
/// returning the two revisions. The shape every export test wants.
fn repo_with_a_changed_symbol(dir: &Path) -> (String, String) {
    init_repo(dir);
    write_file(dir, "src/counter.rs", "pub fn bump() {}\n");
    let base = commit(dir, "baseline");
    write_file(
        dir,
        "src/counter.rs",
        "pub fn bump() { record(); }\npub fn record() {}\n",
    );
    let head = commit(dir, "feature");
    (base, head)
}

/// Take `base..head` through `cli::prepare` and write the export to `out`.
fn export(repo: &Repo, base: &str, head: &str, out: &Path) {
    let args = parse(&[base, head, "--export", out.to_str().unwrap()]);

    match cli::prepare(repo, &args).unwrap() {
        Session::Export {
            snapshot,
            assets,
            path,
        } => {
            gribovik::export::write(&assets, &snapshot, &path).unwrap();
        }
        _ => panic!("expected Session::Export"),
    }
}

#[test]
fn export_html_writes_a_self_contained_page() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let (base, head) = repo_with_a_changed_symbol(dir);

    let repo = Repo::discover(dir).unwrap();
    let out_dir = TempDir::new().unwrap();
    let out_file = out_dir.path().join("out.html");

    export(&repo, &base, &head, &out_file);

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
    let (base, head) = repo_with_a_changed_symbol(dir);

    let repo = Repo::discover(dir).unwrap();
    let out_dir = TempDir::new().unwrap();
    // Neither `out` nor `out/nested` exists yet.
    let out_file = out_dir
        .path()
        .join("out")
        .join("nested")
        .join("review.html");

    export(&repo, &base, &head, &out_file);

    assert!(out_file.exists(), "export did not create {out_file:?}");
}

#[test]
fn export_to_a_bare_filename_writes_into_the_working_directory() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let (base, head) = repo_with_a_changed_symbol(dir);

    // The PR workflow runs exactly this form, whose parent path is empty
    // rather than absent. Going through the binary keeps the working
    // directory to this process, out of reach of the other tests.
    let out = Command::new(env!("CARGO_BIN_EXE_gribovik"))
        .current_dir(dir)
        .args(["--export", "review.html", &base, &head])
        .output()
        .expect("the gribovik binary runs");

    assert!(
        out.status.success(),
        "gribovik failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let html = fs::read_to_string(dir.join("review.html")).unwrap();
    assert!(
        html.contains("__GRIBOVIK_SNAPSHOT__"),
        "missing snapshot payload"
    );
}
