//! Rust symbol and call extraction.

use tree_sitter::{Language, Node};

use crate::core::diff::LineRange;
use crate::core::error::AnalysisError;
use crate::core::lang::{self, LanguageAnalyzer, Symbol};

/// Analyzer for `.rs` sources.
pub struct RustAnalyzer;

/// How Rust spells qualification, both in `impl` blocks and modules.
const SEPARATOR: &str = "::";

/// Sibling kinds that belong to the declaration below them: attributes and the
/// doc comments above an item.
const PRELUDE_KINDS: &[&str] = &["attribute_item", "line_comment", "block_comment"];

fn language() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}

impl LanguageAnalyzer for RustAnalyzer {
    fn symbols(&self, src: &str) -> Result<Vec<Symbol>, AnalysisError> {
        let tree = lang::parse(&language(), src, "rust")?;
        let mut out = Vec::new();
        collect(tree.root_node(), src, "", false, &mut out);
        Ok(out)
    }

    fn calls_in_range(&self, src: &str, range: LineRange) -> Vec<String> {
        let Ok(tree) = lang::parse(&language(), src, "rust") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        lang::for_each_descendant(tree.root_node(), &mut |node| {
            if !range.contains(lang::start_line(node)) {
                return;
            }
            match node.kind() {
                // `foo()`, `Type::assoc()`, `value.method()`.
                "call_expression" => {
                    if let Some(callee) = node.child_by_field_name("function") {
                        if let Some(name) = callee_name(callee, src) {
                            lang::push_unique(&mut out, name);
                        }
                    }
                }
                // `Type { .. }` counts as a use of `Type`, which is how a
                // constructor-free struct ends up connected to its users.
                "struct_expression" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        lang::push_unique(&mut out, &base_type_name(name, src));
                    }
                }
                // Macro arguments are an opaque `token_tree`, not expressions,
                // so `println!("{}", extra())` holds no `call_expression` at
                // all. Rust code calls through `format!`, `assert_eq!`, `vec!`
                // and friends constantly; skipping them leaves a large share of
                // real call sites out of the graph.
                "token_tree" => push_token_tree_calls(node, src, &mut out),
                _ => {}
            }
        });
        out
    }
}

/// Walk the items of one container, emitting a symbol per declaration.
///
/// Bodies of functions are never descended into: a nested `fn` belongs to the
/// symbol that encloses it, and the enclosing symbol's line span already covers
/// it. `impl` blocks are descended into with `in_impl` set, which is the only
/// thing that distinguishes a method from a free function in the grammar.
fn collect(container: Node, src: &str, prefix: &str, in_impl: bool, out: &mut Vec<Symbol>) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                let kind = if in_impl { "method" } else { "function" };
                push_symbol(child, src, prefix, kind, out);
            }
            "struct_item" => push_symbol(child, src, prefix, "struct", out),
            "enum_item" => push_symbol(child, src, prefix, "enum", out),
            "trait_item" => push_symbol(child, src, prefix, "trait", out),
            "impl_item" => {
                if let Some(body) = child.child_by_field_name("body") {
                    let type_name = child
                        .child_by_field_name("type")
                        .map(|node| base_type_name(node, src))
                        .unwrap_or_default();
                    collect(body, src, &join(prefix, &type_name), true, out);
                }
            }
            "mod_item" => {
                if let Some(body) = child.child_by_field_name("body") {
                    let name = lang::field_text(child, "name", src);
                    collect(body, src, &join(prefix, name), false, out);
                }
            }
            _ => {}
        }
    }
}

fn push_symbol(node: Node, src: &str, prefix: &str, kind: &str, out: &mut Vec<Symbol>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = lang::text(name_node, src);
    out.push(Symbol {
        name: name.to_string(),
        qualified_name: join(prefix, name),
        kind: kind.to_string(),
        start_line: lang::leading_line(node, PRELUDE_KINDS),
        end_line: lang::end_line(node),
    });
}

/// Emit the calls spelled out in the raw tokens of a macro argument list.
///
/// Inside a `token_tree` nothing is an expression — a call is just an
/// `identifier` sitting next to a parenthesized `token_tree`. Requiring the
/// parentheses is what keeps `matches!(x, Foo { .. })` from reading as a call
/// to `Foo`, and taking the identifier adjacent to the parentheses resolves
/// `Type::assoc()` on its last segment exactly as `callee_name` does.
///
/// Nested token trees are reached by the caller's descendant walk, so this only
/// looks at direct children.
fn push_token_tree_calls(tree: Node, src: &str, out: &mut Vec<String>) {
    let mut cursor = tree.walk();
    let children: Vec<Node> = tree.children(&mut cursor).collect();
    for pair in children.windows(2) {
        let (name, args) = (pair[0], pair[1]);
        if name.kind() != "identifier" || args.kind() != "token_tree" {
            continue;
        }
        if !src[args.byte_range()].starts_with('(') {
            continue;
        }
        lang::push_unique(out, lang::text(name, src));
    }
}

/// The bare callee name of a call, or `None` for shapes we cannot attribute
/// (calling a closure held in a variable, say).
fn callee_name<'a>(function: Node, src: &'a str) -> Option<&'a str> {
    match function.kind() {
        "identifier" => Some(lang::text(function, src)),
        // `Type::assoc()` resolves on the last segment.
        "scoped_identifier" => function
            .child_by_field_name("name")
            .map(|node| lang::text(node, src)),
        // Method calls are `field_expression`s; tree-sitter-rust has no
        // dedicated method-call node.
        "field_expression" => function
            .child_by_field_name("field")
            .map(|node| lang::text(node, src)),
        // `foo::<T>()`.
        "generic_function" => function
            .child_by_field_name("function")
            .and_then(|node| callee_name(node, src)),
        _ => None,
    }
}

/// Strip generics and module paths off a type reference: `a::B<C>` → `B`.
fn base_type_name(node: Node, src: &str) -> String {
    match node.kind() {
        "generic_type" => node
            .child_by_field_name("type")
            .map(|inner| base_type_name(inner, src))
            .unwrap_or_default(),
        "scoped_type_identifier" => lang::field_text(node, "name", src).to_string(),
        // The grammar aliases scoped names to `type_identifier`, so the text
        // may still carry a path.
        _ => lang::text(node, src)
            .rsplit(SEPARATOR)
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}{SEPARATOR}{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDED_BEFORE: &str = include_str!("../../../tests/fixtures/rust/added_fn/before.rs");
    const ADDED_AFTER: &str = include_str!("../../../tests/fixtures/rust/added_fn/after.rs");
    const MODIFIED_BEFORE: &str =
        include_str!("../../../tests/fixtures/rust/modified_method/before.rs");
    const MODIFIED_AFTER: &str =
        include_str!("../../../tests/fixtures/rust/modified_method/after.rs");
    const DELETED_BEFORE: &str =
        include_str!("../../../tests/fixtures/rust/deleted_struct/before.rs");
    const DELETED_AFTER: &str =
        include_str!("../../../tests/fixtures/rust/deleted_struct/after.rs");
    const NESTED_BEFORE: &str = include_str!("../../../tests/fixtures/rust/nested_fn/before.rs");
    const NESTED_AFTER: &str = include_str!("../../../tests/fixtures/rust/nested_fn/after.rs");

    /// Every symbol rendered as `name | qualified_name | kind | start-end`, so
    /// expectations read as a table and mismatches diff line by line.
    fn outline(src: &str) -> Vec<String> {
        RustAnalyzer
            .symbols(src)
            .expect("fixture parses")
            .iter()
            .map(|symbol| {
                row(
                    &symbol.name,
                    &symbol.qualified_name,
                    &symbol.kind,
                    symbol.start_line,
                    symbol.end_line,
                )
            })
            .collect()
    }

    fn row(name: &str, qualified_name: &str, kind: &str, start: u32, end: u32) -> String {
        format!("{name} | {qualified_name} | {kind} | {start}-{end}")
    }

    fn calls(src: &str, start: u32, end: u32) -> Vec<String> {
        RustAnalyzer.calls_in_range(src, LineRange::inclusive(start, end))
    }

    #[test]
    fn added_fixture_gains_a_symbol() {
        assert_eq!(
            outline(ADDED_BEFORE),
            vec![row("greet", "greet", "function", 1, 3)]
        );
        assert_eq!(
            outline(ADDED_AFTER),
            vec![
                row("greet", "greet", "function", 1, 3),
                // Starts at the doc comment on line 5, not at `pub fn` on 7.
                row("greet_all", "greet_all", "function", 5, 9),
            ]
        );
    }

    #[test]
    fn methods_are_qualified_by_their_impl_type() {
        assert_eq!(
            outline(MODIFIED_BEFORE),
            vec![
                row("Counter", "Counter", "struct", 1, 3),
                row("new", "Counter::new", "method", 6, 8),
                row("bump", "Counter::bump", "method", 10, 12),
            ]
        );
        assert_eq!(
            outline(MODIFIED_AFTER),
            vec![
                row("Counter", "Counter", "struct", 1, 3),
                row("new", "Counter::new", "method", 6, 8),
                row("bump", "Counter::bump", "method", 10, 13),
                row("log", "Counter::log", "method", 15, 15),
                row("step", "step", "function", 18, 20),
            ]
        );
    }

    #[test]
    fn deleted_fixture_loses_its_type_and_method() {
        assert_eq!(
            outline(DELETED_BEFORE),
            vec![
                row("Legacy", "Legacy", "struct", 1, 3),
                row("id", "Legacy::id", "method", 6, 8),
                row("keep", "keep", "function", 11, 13),
            ]
        );
        assert_eq!(
            outline(DELETED_AFTER),
            vec![row("keep", "keep", "function", 1, 3)]
        );
    }

    #[test]
    fn nested_functions_fold_into_their_parent() {
        for src in [NESTED_BEFORE, NESTED_AFTER] {
            assert_eq!(outline(src), vec![row("total", "total", "function", 1, 7)]);
        }
    }

    #[test]
    fn types_traits_and_modules_are_symbols() {
        let src = "\
enum Color {
    Red,
}

trait Draw {
    fn draw(&self);
}

impl Draw for Color {
    fn draw(&self) {}
}

impl<T> Holder<T> {
    fn get(&self) {}
}

mod util {
    pub fn helper() {}

    impl Inner {
        fn deep(&self) {}
    }
}
";
        assert_eq!(
            outline(src),
            vec![
                row("Color", "Color", "enum", 1, 3),
                row("Draw", "Draw", "trait", 5, 7),
                // Trait impls qualify on the implementing type, not the trait.
                row("draw", "Color::draw", "method", 10, 10),
                // Generics are stripped from the qualifier.
                row("get", "Holder::get", "method", 14, 14),
                row("helper", "util::helper", "function", 18, 18),
                row("deep", "util::Inner::deep", "method", 21, 21),
            ]
        );
    }

    #[test]
    fn call_shapes_all_reduce_to_a_bare_name() {
        let src = "\
fn caller() {
    plain();
    Type::assoc();
    value.method();
    generic::<u32>();
    crate::util::log();
    Widget { n: 1 };
    println!(\"skipped\");
}
";
        assert_eq!(
            calls(src, 1, 9),
            vec!["plain", "assoc", "method", "generic", "log", "Widget"]
        );
    }

    #[test]
    fn calls_are_limited_to_the_requested_span() {
        assert_eq!(calls(MODIFIED_AFTER, 10, 13), vec!["step", "log"]);
        assert_eq!(calls(MODIFIED_AFTER, 6, 8), vec!["Counter"]);
        assert_eq!(calls(MODIFIED_AFTER, 15, 15), Vec::<String>::new());
    }

    #[test]
    fn a_nested_function_call_belongs_to_the_enclosing_symbol() {
        assert_eq!(
            calls(NESTED_BEFORE, 1, 7),
            vec!["sum", "map", "iter", "double"]
        );
    }

    #[test]
    fn a_struct_literal_counts_as_a_use_of_the_type() {
        assert_eq!(calls(DELETED_BEFORE, 11, 13), vec!["id", "Legacy"]);
    }

    #[test]
    fn calls_inside_macro_arguments_are_found() {
        let src = "fn caller() {\n    println!(\"{}\", extra());\n    let v = vec![make()];\n    \
                   assert_eq!(Counter::compute(), 1);\n    direct();\n}\n";
        assert_eq!(
            calls(src, 1, 6),
            vec!["extra", "make", "compute", "direct"],
            "macro arguments are raw tokens, not expressions, so the calls in \
             them have to be read off the token tree"
        );
    }

    #[test]
    fn a_braced_type_in_a_macro_is_not_a_call() {
        let src = "fn caller() {\n    matches!(x, Foo { .. });\n}\n";
        assert_eq!(calls(src, 1, 3), Vec::<String>::new());
    }

    #[test]
    fn repeated_callees_are_reported_once() {
        let src = "fn f() {\n    g();\n    g();\n}\n";
        assert_eq!(calls(src, 1, 4), vec!["g"]);
    }

    #[test]
    fn syntax_errors_surface_as_a_parse_error() {
        let error = RustAnalyzer.symbols("fn broken( {\n").unwrap_err();
        assert!(matches!(error, AnalysisError::Parse { .. }), "{error:?}");
        // Calls degrade to nothing rather than propagating the failure.
        assert_eq!(calls("fn broken( {\n", 1, 2), Vec::<String>::new());
    }
}
