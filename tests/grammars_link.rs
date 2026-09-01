//! Proves all three tree-sitter grammars are compiled in and accepted by the
//! `tree-sitter` runtime version we pinned. A grammar/runtime ABI mismatch
//! shows up here as a `set_language` error rather than deep inside an analyzer.

use tree_sitter::Parser;

fn parses(language: &tree_sitter::Language, source: &str) -> String {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .expect("grammar ABI is compatible with the tree-sitter runtime");
    let tree = parser.parse(source, None).expect("parser produced a tree");
    assert!(!tree.root_node().has_error(), "unexpected parse error");
    tree.root_node().kind().to_string()
}

#[test]
fn rust_grammar_links() {
    assert_eq!(
        parses(&tree_sitter_rust::LANGUAGE.into(), "fn main() {}\n"),
        "source_file"
    );
}

#[test]
fn swift_grammar_links() {
    assert_eq!(
        parses(&tree_sitter_swift::LANGUAGE.into(), "func main() {}\n"),
        "source_file"
    );
}

#[test]
fn typescript_grammars_link() {
    assert_eq!(
        parses(
            &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "export const a: number = 1;\n"
        ),
        "program"
    );
    assert_eq!(
        parses(
            &tree_sitter_typescript::LANGUAGE_TSX.into(),
            "const A = () => <div />;\n"
        ),
        "program"
    );
}
