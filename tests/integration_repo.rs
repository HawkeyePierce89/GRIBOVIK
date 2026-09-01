//! End-to-end analysis: a real temp repository in, a complete snapshot out.
//!
//! This is the only test that exercises the whole stack — git, the diff, all
//! three language analyzers, node construction and edge resolution — against
//! commits made by the actual `git` binary, so it is where a regression in the
//! seams between those layers shows up.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gribovik::core::{ChangeKind, Confidence, DiffTag, GraphSnapshot, Node};
use gribovik::git::Repo;
use gribovik::pipeline::{analyze, Analysis};
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
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, contents).unwrap();
}

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

/// The baseline revision of a repository touching all three languages, plus a
/// file no analyzer handles.
fn write_baseline(dir: &Path) {
    write(
        dir,
        "src/counter.rs",
        r#"pub struct Counter {
    value: u32,
}

impl Counter {
    pub fn bump(&mut self) {
        self.value += 1;
    }
}

pub struct Legacy {
    pub id: u32,
}
"#,
    );
    write(
        dir,
        "src/imports.rs",
        r#"use std::fmt;

pub fn describe() -> String {
    String::from("x")
}
"#,
    );
    write(
        dir,
        "app/model.swift",
        r#"class Session {
    func refresh() {
        reset()
    }
}

struct Legacy {
    let id: Int
}
"#,
    );
    write(
        dir,
        "web/app.ts",
        r#"export interface Config {
  name: string;
}

export const render = (config: Config): string => {
  return config.name;
};
"#,
    );
    write(dir, "README.md", "# fixture\n");
}

/// The head revision: an added function, a modified method, a deleted type, an
/// import-only edit, two same-named functions in different directories, and a
/// change to a file no analyzer handles.
fn write_head(dir: &Path) {
    write(
        dir,
        "src/counter.rs",
        r#"pub struct Counter {
    value: u32,
}

impl Counter {
    pub fn bump(&mut self) {
        self.value += 1;
        record();
    }
}

pub fn record() {
    format_value();
}
"#,
    );
    write(
        dir,
        "src/fmt/short.rs",
        r#"pub fn format_value() -> String {
    String::from("short")
}
"#,
    );
    write(
        dir,
        "src/text/long.rs",
        r#"pub fn format_value() -> String {
    String::from("long")
}
"#,
    );
    write(
        dir,
        "src/imports.rs",
        r#"use std::fmt;
use std::io;

pub fn describe() -> String {
    String::from("x")
}
"#,
    );
    write(
        dir,
        "app/model.swift",
        r#"class Session {
    func refresh() {
        reset()
        touch()
    }
}

func touch() {
}
"#,
    );
    write(
        dir,
        "web/app.ts",
        r#"export interface Config {
  name: string;
  theme: string;
}

export const render = (config: Config): string => {
  return decorate(config.name);
};

export const decorate = (label: string): string => {
  return label.toUpperCase();
};
"#,
    );
    write(dir, "README.md", "# fixture\n\nnow with prose\n");
}

/// Baseline commit, head commit, and the sha of the baseline.
fn fixture_repo(dir: &Path) -> String {
    init_repo(dir);
    write_baseline(dir);
    let base = commit(dir, "baseline");
    write_head(dir);
    commit(dir, "feature");
    base
}

fn expect_graph(analysis: Analysis) -> GraphSnapshot {
    match analysis {
        Analysis::Graph(snapshot) => *snapshot,
        Analysis::NoChanges { base, head, .. } => {
            panic!("expected a graph for {base}..{head}")
        }
    }
}

/// `(id, kind, change)` for every card, in snapshot order.
fn outline(snapshot: &GraphSnapshot) -> Vec<(&str, &str, ChangeKind)> {
    snapshot
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind.as_str(), node.change))
        .collect()
}

/// Node ids key the edges and the review state on disk, so a snapshot with two
/// nodes under one id silently ties two cards' verdicts together. Asserted on
/// every end-to-end snapshot rather than in one place.
fn assert_ids_are_unique(snapshot: &GraphSnapshot) {
    let mut seen = std::collections::HashSet::new();
    for node in &snapshot.nodes {
        assert!(
            seen.insert(node.id.as_str()),
            "duplicate node id {}: {:?}",
            node.id,
            outline(snapshot)
        );
    }
}

fn node<'a>(snapshot: &'a GraphSnapshot, id: &str) -> &'a Node {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

/// `(tag, old_line, new_line, text)` for every line of a card's diff.
fn lines(node: &Node) -> Vec<(DiffTag, Option<u32>, Option<u32>, &str)> {
    node.diff
        .iter()
        .map(|line| (line.tag, line.old_line, line.new_line, line.text.as_str()))
        .collect()
}

fn edges(snapshot: &GraphSnapshot) -> Vec<(&str, &str, Confidence)> {
    snapshot
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str(), edge.confidence))
        .collect()
}

#[test]
fn analyzes_a_multi_language_change_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let base = fixture_repo(dir);

    let repo = Repo::discover(dir).unwrap();
    let snapshot = expect_graph(analyze(&repo, Some(&base), None).unwrap());

    assert_ids_are_unique(&snapshot);
    assert_eq!(snapshot.meta.repo, canonical(dir).display().to_string());
    assert_eq!(snapshot.meta.base, base);
    // `HEAD` is expanded to the branch it names, so that two branches off the
    // same base do not file their reviews under one name.
    assert_eq!(snapshot.meta.head, "master");
    // The six source files; `README.md` never reaches the core.
    assert_eq!(snapshot.meta.files_changed, 6);
    assert!(
        snapshot.meta.warnings.is_empty(),
        "unexpected warnings: {:?}",
        snapshot.meta.warnings
    );

    assert_eq!(
        outline(&snapshot),
        vec![
            // Swift: the class's span contains its method, but every changed
            // line goes to the innermost symbol holding it — so only the
            // method is carded, and `Session` is not asked about a second
            // time. A class-level edit would still give `Session` a card.
            ("app/model.swift::Legacy", "struct", ChangeKind::Deleted),
            (
                "app/model.swift::Session.refresh",
                "method",
                ChangeKind::Modified
            ),
            ("app/model.swift::touch", "function", ChangeKind::Added),
            ("src/counter.rs::Legacy", "struct", ChangeKind::Deleted),
            (
                "src/counter.rs::Counter::bump",
                "method",
                ChangeKind::Modified
            ),
            ("src/counter.rs::record", "function", ChangeKind::Added),
            (
                "src/fmt/short.rs::format_value",
                "function",
                ChangeKind::Added
            ),
            // The import-only edit belongs to no symbol.
            ("src/imports.rs::<file>", "file", ChangeKind::Modified),
            (
                "src/text/long.rs::format_value",
                "function",
                ChangeKind::Added
            ),
            ("web/app.ts::Config", "interface", ChangeKind::Modified),
            ("web/app.ts::render", "function", ChangeKind::Modified),
            ("web/app.ts::decorate", "function", ChangeKind::Added),
        ]
    );
    // `describe` moved down a line without changing, so it is not a card.
    assert!(!snapshot
        .nodes
        .iter()
        .any(|node| node.id == "src/imports.rs::describe"));

    assert_eq!(
        edges(&snapshot),
        vec![
            (
                "app/model.swift::Session.refresh",
                "app/model.swift::touch",
                Confidence::Certain
            ),
            (
                "src/counter.rs::Counter::bump",
                "src/counter.rs::record",
                Confidence::Certain
            ),
            (
                "web/app.ts::render",
                "web/app.ts::decorate",
                Confidence::Certain
            ),
        ]
    );
    // `format_value` is declared in two changed files, neither of them the
    // caller's own directory. The name alone cannot pick one, and the
    // whole-graph tier answers unique names only, so `record` gets no arrow
    // rather than one to each.
    assert!(!snapshot
        .edges
        .iter()
        .any(|edge| edge.to.ends_with("::format_value")));
}

#[test]
fn every_card_carries_its_own_slice_of_the_diff() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let base = fixture_repo(dir);

    let repo = Repo::discover(dir).unwrap();
    let snapshot = expect_graph(analyze(&repo, Some(&base), None).unwrap());

    // A modified method: context on both sides, one added line numbered only
    // on the new side, and the trailing brace shifted by one.
    assert_eq!(
        lines(node(&snapshot, "src/counter.rs::Counter::bump")),
        vec![
            (
                DiffTag::Context,
                Some(6),
                Some(6),
                "    pub fn bump(&mut self) {"
            ),
            (
                DiffTag::Context,
                Some(7),
                Some(7),
                "        self.value += 1;"
            ),
            (DiffTag::Add, None, Some(8), "        record();"),
            (DiffTag::Context, Some(8), Some(9), "    }"),
        ]
    );

    // A deleted type keeps its old-side line numbers and has no new ones.
    assert_eq!(
        lines(node(&snapshot, "src/counter.rs::Legacy"))
            .into_iter()
            .filter(|(tag, ..)| *tag == DiffTag::Del)
            .collect::<Vec<_>>(),
        vec![
            (DiffTag::Del, Some(11), None, "pub struct Legacy {"),
            (DiffTag::Del, Some(12), None, "    pub id: u32,"),
        ]
    );

    // The file-level card holds exactly the hunk no symbol claimed.
    assert_eq!(
        lines(node(&snapshot, "src/imports.rs::<file>")),
        vec![(DiffTag::Add, None, Some(2), "use std::io;")]
    );

    // An added symbol is all additions, on new-side lines only.
    let added = node(&snapshot, "src/fmt/short.rs::format_value");
    assert!(added
        .diff
        .iter()
        .all(|line| line.tag == DiffTag::Add && line.old_line.is_none()));
    assert_eq!(
        added
            .diff
            .iter()
            .filter_map(|line| line.new_line)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[test]
fn without_an_explicit_base_the_origin_fallback_decides_the_range() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let remote_dir = TempDir::new().unwrap();
    let remote = remote_dir.path().join("origin.git");

    init_repo(dir);
    write_baseline(dir);
    let base = commit(dir, "baseline");
    git(dir, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(dir, &["push", "-q", "origin", "master"]);
    write_head(dir);
    commit(dir, "feature");

    let repo = Repo::discover(dir).unwrap();
    let snapshot = expect_graph(analyze(&repo, None, None).unwrap());

    // origin/master is where the branch left the baseline behind.
    assert_eq!(snapshot.meta.base, base);
    assert!(!snapshot.nodes.is_empty());
}

#[test]
fn an_explicit_head_revision_is_reported_as_written() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let base = fixture_repo(dir);
    git(dir, &["branch", "feature"]);

    let repo = Repo::discover(dir).unwrap();
    let snapshot = expect_graph(analyze(&repo, Some(&base), Some("feature")).unwrap());

    // Review state is filed under this name, so it must survive verbatim.
    assert_eq!(snapshot.meta.head, "feature");
    assert!(!snapshot.nodes.is_empty());
}

/// The bare `HEAD` says nothing in the header the browser shows, so the head
/// is reported as the branch it is on.
#[test]
fn the_default_head_is_reported_as_the_branch_it_is_on() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let base = fixture_repo(dir);
    let repo = Repo::discover(dir).unwrap();

    git(dir, &["checkout", "-q", "-b", "feature-a"]);
    let a = expect_graph(analyze(&repo, Some(&base), None).unwrap());
    git(dir, &["checkout", "-q", "-b", "feature-b"]);
    let b = expect_graph(analyze(&repo, Some(&base), None).unwrap());

    assert_eq!(a.meta.head, "feature-a");
    assert_eq!(b.meta.head, "feature-b");
    assert_eq!(a.meta.base, b.meta.base, "both branches share the base");
}

#[test]
fn a_range_with_no_source_changes_reports_no_changes() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write(dir, "README.md", "# fixture\n");
    let base = commit(dir, "baseline");
    write(dir, "README.md", "# fixture\n\nnow with prose\n");
    write(dir, "docs/notes.txt", "nothing to analyze\n");
    commit(dir, "prose only");

    let repo = Repo::discover(dir).unwrap();
    let analysis = analyze(&repo, Some(&base), None).unwrap();

    assert_eq!(
        analysis,
        Analysis::NoChanges {
            base,
            head: "master".to_string(),
            warnings: Vec::new(),
        }
    );
    assert!(analysis.snapshot().is_none());
}

#[test]
fn an_empty_range_reports_no_changes() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let head = {
        init_repo(dir);
        write_baseline(dir);
        commit(dir, "baseline")
    };

    let repo = Repo::discover(dir).unwrap();
    let analysis = analyze(&repo, Some(&head), None).unwrap();

    assert!(matches!(analysis, Analysis::NoChanges { .. }));
    assert_eq!(analysis.range().0, head);
}

#[test]
fn a_source_file_that_is_not_utf8_is_skipped_with_a_warning() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write(dir, "src/counter.rs", "pub fn bump() {}\n");
    fs::write(dir.join("src/blob.rs"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    let base = commit(dir, "baseline");
    write(dir, "src/counter.rs", "pub fn bump() {\n    log();\n}\n");
    fs::write(dir.join("src/blob.rs"), [0xff, 0xfe, 0x00, 0x9f, 0x9f]).unwrap();
    commit(dir, "feature");

    let repo = Repo::discover(dir).unwrap();
    let snapshot = expect_graph(analyze(&repo, Some(&base), None).unwrap());

    // The unreadable file contributes a warning and nothing else; the readable
    // one is analyzed as usual.
    assert_eq!(
        outline(&snapshot),
        vec![("src/counter.rs::bump", "function", ChangeKind::Modified)]
    );
    assert_eq!(snapshot.meta.files_changed, 1);
    assert!(
        snapshot
            .meta
            .warnings
            .iter()
            .any(|warning| warning.contains("src/blob.rs") && warning.contains("not UTF-8")),
        "expected a non-UTF-8 warning, got {:?}",
        snapshot.meta.warnings
    );
}

#[test]
fn an_unknown_revision_is_reported_by_name() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    write(dir, "src/counter.rs", "pub fn bump() {}\n");
    commit(dir, "baseline");

    let repo = Repo::discover(dir).unwrap();
    let error = analyze(&repo, Some("nope"), None).unwrap_err();
    assert_eq!(error.to_string(), "unknown revision: nope");
}

#[test]
fn a_range_whose_only_source_is_unreadable_reports_why() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init_repo(dir);
    fs::write(dir.join("blob.rs"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    let base = commit(dir, "baseline");
    fs::write(dir.join("blob.rs"), [0xff, 0xfe, 0x00, 0x9f, 0x9f]).unwrap();
    commit(dir, "feature");

    let repo = Repo::discover(dir).unwrap();
    let analysis = analyze(&repo, Some(&base), None).unwrap();

    // "no reviewable changes" on its own would tell the reviewer the opposite
    // of the truth: the `.rs` file did change, and was skipped.
    match analysis {
        Analysis::NoChanges { warnings, .. } => assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("blob.rs") && warning.contains("not UTF-8")),
            "expected a non-UTF-8 warning, got {warnings:?}"
        ),
        Analysis::Graph(_) => panic!("an unreadable-only range should not produce a graph"),
    }
}
