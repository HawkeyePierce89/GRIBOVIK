//! Exercises `gribovik::git` against real repositories built in temp dirs.
//!
//! These shell out to the same `git` binary the tool uses in anger, which is
//! the only way to be sure the flags and output parsing agree with reality.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use gribovik::git::{Blob, ChangedFile, FileStatus, Repo};
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

/// A repository with a deterministic identity and a `master` branch, so the
/// host's `init.defaultBranch` and user config cannot leak into the tests.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/master"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn write(dir: &Path, path: &str, contents: &str) {
    let full = dir.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(full, contents).unwrap();
}

/// Stage everything and commit, returning the new commit's sha.
fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// macOS puts temp dirs behind a `/var` -> `/private/var` symlink; git reports
/// the resolved path, so comparisons have to resolve too.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// The current commit of `dir`.
fn head_sha(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

/// A one-commit repository: the starting point for most cases.
fn seeded_repo() -> (TempDir, Repo) {
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    write(dir.path(), "src/lib.rs", "fn one() {}\n");
    commit(dir.path(), "initial");
    let repo = Repo::discover(dir.path()).unwrap();
    (dir, repo)
}

/// Publish `branch` to a bare remote so `origin/<branch>` exists locally.
fn push_to_new_origin(dir: &Path, branch: &str) -> TempDir {
    let remote = TempDir::new().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        dir,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(dir, &["push", "-q", "origin", &format!("master:{branch}")]);
    git(dir, &["fetch", "-q", "origin"]);
    remote
}

#[test]
fn discovers_the_worktree_root_from_a_subdirectory() {
    let (dir, _) = seeded_repo();
    let nested = dir.path().join("src");

    let repo = Repo::discover(&nested).unwrap();

    assert_eq!(canonical(repo.root()), canonical(dir.path()));
}

#[test]
fn discovery_outside_a_repository_says_so() {
    let dir = TempDir::new().unwrap();

    let err = Repo::discover(dir.path()).unwrap_err();

    assert!(
        err.to_string().contains("not a git repository"),
        "unexpected error: {err}"
    );
}

#[test]
fn discovery_of_a_missing_directory_says_so() {
    let dir = TempDir::new().unwrap();

    let err = Repo::discover(dir.path().join("nope")).unwrap_err();

    assert!(
        err.to_string().contains("no such directory"),
        "unexpected error: {err}"
    );
}

#[test]
fn base_falls_back_to_origin_master() {
    let (dir, repo) = seeded_repo();
    let base = head_sha(dir.path());
    let _remote = push_to_new_origin(dir.path(), "master");
    write(dir.path(), "src/lib.rs", "fn one() { two(); }\n");
    commit(dir.path(), "work");

    assert!(repo.rev_exists("origin/master"));
    assert_eq!(repo.resolve_base(None, "HEAD").unwrap(), base);
}

#[test]
fn base_falls_back_to_origin_main_when_master_is_absent() {
    let (dir, repo) = seeded_repo();
    let base = head_sha(dir.path());
    let _remote = push_to_new_origin(dir.path(), "main");
    write(dir.path(), "src/lib.rs", "fn one() { two(); }\n");
    commit(dir.path(), "work");

    assert!(!repo.rev_exists("origin/master"));
    assert_eq!(repo.resolve_base(None, "HEAD").unwrap(), base);
}

#[test]
fn base_prefers_origin_master_over_origin_main() {
    let (dir, repo) = seeded_repo();
    let master_tip = head_sha(dir.path());
    let _remote = push_to_new_origin(dir.path(), "master");
    // origin/main is ahead: picking it would move the base forward.
    write(dir.path(), "src/lib.rs", "fn one() { two(); }\n");
    commit(dir.path(), "extra");
    git(dir.path(), &["push", "-q", "origin", "master:main"]);
    git(dir.path(), &["fetch", "-q", "origin"]);

    assert_eq!(repo.resolve_base(None, "HEAD").unwrap(), master_tip);
}

#[test]
fn base_without_any_origin_ref_explains_the_problem() {
    let (_dir, repo) = seeded_repo();

    let err = repo.resolve_base(None, "HEAD").unwrap_err();

    let message = err.to_string();
    assert!(message.contains("origin/master"), "unexpected: {message}");
    assert!(message.contains("origin/main"), "unexpected: {message}");
    assert!(
        message.contains("pass a base explicitly"),
        "unexpected: {message}"
    );
}

#[test]
fn an_explicit_base_resolves_to_the_merge_base() {
    let (dir, repo) = seeded_repo();
    let fork_point = head_sha(dir.path());
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "src/feature.rs", "fn feature() {}\n");
    commit(dir.path(), "feature work");
    git(dir.path(), &["checkout", "-q", "master"]);
    write(dir.path(), "src/lib.rs", "fn one() { moved_on(); }\n");
    commit(dir.path(), "master moved on");

    assert_eq!(
        repo.resolve_base(Some("master"), "feature").unwrap(),
        fork_point
    );
}

#[test]
fn an_unknown_base_revision_is_reported_by_name() {
    let (_dir, repo) = seeded_repo();

    let err = repo.resolve_base(Some("no-such-ref"), "HEAD").unwrap_err();

    assert_eq!(err.to_string(), "unknown revision: no-such-ref");
}

#[test]
fn an_unknown_head_revision_is_reported_by_name() {
    let (_dir, repo) = seeded_repo();

    let err = repo.resolve_base(None, "no-such-head").unwrap_err();

    assert_eq!(err.to_string(), "unknown revision: no-such-head");
}

#[test]
fn changed_files_maps_add_modify_and_delete() {
    let (dir, repo) = seeded_repo();
    write(dir.path(), "src/gone.rs", "fn gone() {}\n");
    let base = commit(dir.path(), "base");
    write(dir.path(), "src/lib.rs", "fn one() { two(); }\n");
    write(dir.path(), "src/new.rs", "fn added() {}\n");
    fs::remove_file(dir.path().join("src/gone.rs")).unwrap();
    let head = commit(dir.path(), "work");

    let mut changed = repo.changed_files(&base, &head).unwrap();
    changed.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(
        changed,
        vec![
            ChangedFile {
                path: "src/gone.rs".to_string(),
                status: FileStatus::Deleted
            },
            ChangedFile {
                path: "src/lib.rs".to_string(),
                status: FileStatus::Modified
            },
            ChangedFile {
                path: "src/new.rs".to_string(),
                status: FileStatus::Added
            },
        ]
    );
}

#[test]
fn changed_files_reports_a_rename_as_a_delete_and_an_add() {
    let (dir, repo) = seeded_repo();
    let body = "fn renamed() {\n    let a = 1;\n    let b = 2;\n    println!(\"{a}{b}\");\n}\n";
    write(dir.path(), "src/from.rs", body);
    let base = commit(dir.path(), "base");
    fs::rename(dir.path().join("src/from.rs"), dir.path().join("src/to.rs")).unwrap();
    let head = commit(dir.path(), "rename");

    let mut changed = repo.changed_files(&base, &head).unwrap();
    changed.sort_by(|a, b| a.path.cmp(&b.path));

    assert_eq!(
        changed,
        vec![
            ChangedFile {
                path: "src/from.rs".to_string(),
                status: FileStatus::Deleted
            },
            ChangedFile {
                path: "src/to.rs".to_string(),
                status: FileStatus::Added
            },
        ]
    );
}

#[test]
fn changed_files_is_empty_for_identical_revisions() {
    let (_dir, repo) = seeded_repo();

    assert!(repo.changed_files("HEAD", "HEAD").unwrap().is_empty());
}

#[test]
fn blob_reads_a_path_at_a_revision() {
    let (dir, repo) = seeded_repo();
    let base = head_sha(dir.path());
    write(dir.path(), "src/lib.rs", "fn one() { two(); }\n");
    let head = commit(dir.path(), "work");

    assert_eq!(
        repo.blob(&base, "src/lib.rs").unwrap(),
        Blob::Text("fn one() {}\n".to_string())
    );
    assert_eq!(
        repo.blob(&head, "src/lib.rs").unwrap(),
        Blob::Text("fn one() { two(); }\n".to_string())
    );
}

#[test]
fn blob_of_a_path_absent_at_that_revision_is_missing() {
    let (dir, repo) = seeded_repo();
    let base = head_sha(dir.path());
    write(dir.path(), "src/later.rs", "fn later() {}\n");
    let head = commit(dir.path(), "add later");

    assert_eq!(repo.blob(&base, "src/later.rs").unwrap(), Blob::Missing);
    assert!(matches!(
        repo.blob(&head, "src/later.rs").unwrap(),
        Blob::Text(_)
    ));
}

#[test]
fn blob_of_an_unknown_revision_is_an_error() {
    let (_dir, repo) = seeded_repo();

    let err = repo.blob("no-such-ref", "src/lib.rs").unwrap_err();

    assert!(
        err.to_string().contains("no-such-ref"),
        "unexpected error: {err}"
    );
}

#[test]
fn a_non_utf8_blob_is_classified_rather_than_erroring() {
    let (dir, repo) = seeded_repo();
    fs::write(dir.path().join("logo.bin"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
    let head = commit(dir.path(), "binary");

    // The caller decides what to do about it; reading is never a failure.
    assert_eq!(repo.blob(&head, "logo.bin").unwrap(), Blob::NonUtf8);
    assert_eq!(
        repo.blob(&head, "src/lib.rs").unwrap(),
        Blob::Text("fn one() {}\n".to_string())
    );
}

/// The bare `HEAD` says nothing in the header the browser shows, so it is
/// expanded to the branch it is on.
#[test]
fn the_default_head_is_labelled_with_the_branch_it_is_on() {
    let (dir, repo) = seeded_repo();
    git(dir.path(), &["checkout", "-q", "-b", "feature-a"]);

    assert_eq!(repo.head_label("HEAD"), "feature-a");

    git(dir.path(), &["checkout", "-q", "-b", "feature-b"]);

    assert_eq!(repo.head_label("HEAD"), "feature-b");
}

#[test]
fn a_detached_head_is_labelled_with_its_commit() {
    let (dir, repo) = seeded_repo();
    let sha = head_sha(dir.path());
    git(dir.path(), &["checkout", "-q", "--detach", &sha]);

    let label = repo.head_label("HEAD");

    assert_ne!(label, "HEAD", "a detached HEAD names no branch");
    assert!(
        sha.starts_with(&label) && !label.is_empty(),
        "expected a prefix of {sha}, got {label}"
    );
}

#[test]
fn an_explicit_revision_is_left_as_the_reviewer_wrote_it() {
    let (dir, repo) = seeded_repo();
    git(dir.path(), &["checkout", "-q", "-b", "feature-a"]);

    assert_eq!(repo.head_label("master"), "master");
}

/// Run git in `dir` with `stdin`, asserting success and returning raw stdout.
fn git_stdin(dir: &Path, args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("git is installed");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin)
        .expect("git took its input");
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Paths are bytes to git, so a repository can hold a file whose name is not
/// UTF-8 — and one of those anywhere in the range used to abort the whole
/// review with "git printed non-UTF-8 output", even though nothing else in it
/// was affected. The tree is built with `mktree` rather than on disk because
/// APFS refuses to create such a name at all.
#[test]
fn a_path_that_is_not_utf8_does_not_abort_the_diff() {
    let (dir, repo) = seeded_repo();
    let base = head_sha(dir.path());

    let blob = git(dir.path(), &["hash-object", "-w", "--stdin"]);
    // `mktree` builds one level, so both entries are top-level and in sorted
    // order: 0xe9 sorts after `k`.
    let mut spec = format!("100644 blob {blob}\tkeep.rs\n100644 blob {blob}\t").into_bytes();
    spec.extend_from_slice(&[0xe9]);
    spec.extend_from_slice(b".rs\n");
    let tree = String::from_utf8(git_stdin(dir.path(), &["mktree"], &spec))
        .unwrap()
        .trim()
        .to_string();
    let head = git(
        dir.path(),
        &["commit-tree", &tree, "-p", &base, "-m", "weird"],
    );

    let changed = repo.changed_files(&base, &head).unwrap();

    // The undecodable name survives as a lossy placeholder rather than taking
    // the run down with it; reading that path reports it missing by name.
    assert!(
        changed
            .iter()
            .any(|file| file.path == "keep.rs" && file.status == FileStatus::Added),
        "got {changed:?}"
    );
    assert!(
        changed
            .iter()
            .any(|file| file.path.ends_with(".rs")
                && file.path.contains(char::REPLACEMENT_CHARACTER)),
        "got {changed:?}"
    );
}
