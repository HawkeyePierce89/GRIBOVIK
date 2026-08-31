//! Swift symbol and call extraction.

use tree_sitter::{Language, Node};

use crate::core::diff::LineRange;
use crate::core::error::AnalysisError;
use crate::core::lang::{self, LanguageAnalyzer, Symbol};

/// Analyzer for `.swift` sources.
pub struct SwiftAnalyzer;

/// How Swift spells qualification, both for nested types and for members.
const SEPARATOR: &str = ".";

/// Sibling kinds that belong to the declaration below them. Swift attributes
/// (`@available(..)`) parse *inside* the declaration, so only comments matter
/// here.
const PRELUDE_KINDS: &[&str] = &["comment", "multiline_comment"];

fn language() -> Language {
    tree_sitter_swift::LANGUAGE.into()
}

impl LanguageAnalyzer for SwiftAnalyzer {
    fn symbols(&self, src: &str) -> Result<Vec<Symbol>, AnalysisError> {
        let tree = lang::parse(&language(), src, "swift")?;
        let mut out = Vec::new();
        collect(tree.root_node(), src, "", false, &mut out);
        Ok(out)
    }

    fn calls_in_range(&self, src: &str, range: LineRange) -> Vec<String> {
        let Ok(tree) = lang::parse(&language(), src, "swift") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        lang::for_each_descendant(tree.root_node(), &mut |node| {
            if node.kind() != "call_expression" || !range.contains(lang::start_line(node)) {
                return;
            }
            // The callee is the first named child; `call_suffix` (arguments and
            // any trailing closure) follows it. Neither carries a field name.
            if let Some(name) = node
                .named_child(0)
                .and_then(|callee| callee_name(callee, src))
            {
                lang::push_unique(&mut out, name);
            }
        });
        out
    }
}

/// Walk the members of one container, emitting a symbol per declaration.
///
/// Function bodies are never descended into: a nested `func` or a closure
/// belongs to the symbol that encloses it, and that symbol's line span already
/// covers it. Type bodies *are* descended into, with `in_type` set — that is
/// the only thing separating a method from a free function in the grammar.
///
/// Because Swift declares members inside their type, the spans emitted here
/// nest: `Counter` covers `Counter.bump`. See [`Symbol::start_line`].
fn collect(container: Node, src: &str, prefix: &str, in_type: bool, out: &mut Vec<Symbol>) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        match child.kind() {
            // `protocol_function_declaration` is a bodyless requirement; it is
            // named exactly like a real method and worth reviewing as one.
            "function_declaration" | "protocol_function_declaration" => {
                let kind = if in_type { "method" } else { "function" };
                let name = lang::field_text(child, "name", src);
                push_symbol(child, prefix, kind, name, out);
            }
            // `init` is spelled as a keyword rather than a name field.
            "init_declaration" => push_symbol(child, prefix, "method", "init", out),
            // One node covers `class`, `struct`, `enum`, `actor` and
            // `extension`; the `declaration_kind` field says which, and doubles
            // as the symbol kind.
            "class_declaration" => {
                let declaration_kind = lang::field_text(child, "declaration_kind", src);
                let type_name = child
                    .child_by_field_name("name")
                    .map(|node| base_type_name(node, src))
                    .unwrap_or_default();
                // An extension is not a declaration of its own — emitting one
                // would collide with the type it extends. Its members still
                // qualify on that type.
                if declaration_kind != "extension" {
                    push_symbol(child, prefix, declaration_kind, &type_name, out);
                }
                descend(child, src, &join(prefix, &type_name), out);
            }
            "protocol_declaration" => {
                let name = lang::field_text(child, "name", src).to_string();
                push_symbol(child, prefix, "protocol", &name, out);
                descend(child, src, &join(prefix, &name), out);
            }
            "typealias_declaration" => {
                let name = lang::field_text(child, "name", src);
                push_symbol(child, prefix, "typealias", name, out);
            }
            _ => {}
        }
    }
}

/// Collect the members of a type or protocol body, if it has one.
fn descend(declaration: Node, src: &str, prefix: &str, out: &mut Vec<Symbol>) {
    if let Some(body) = declaration.child_by_field_name("body") {
        collect(body, src, prefix, true, out);
    }
}

fn push_symbol(node: Node, prefix: &str, kind: &str, name: &str, out: &mut Vec<Symbol>) {
    if name.is_empty() {
        return;
    }
    out.push(Symbol {
        name: name.to_string(),
        qualified_name: join(prefix, name),
        kind: kind.to_string(),
        start_line: lang::leading_line(node, PRELUDE_KINDS),
        end_line: lang::end_line(node),
    });
}

/// The bare callee name of a call, or `None` for shapes we cannot attribute
/// (calling a closure held in a variable, say).
fn callee_name<'a>(callee: Node, src: &'a str) -> Option<&'a str> {
    match callee.kind() {
        // `foo()`, and constructors such as `Point(x: 1)`.
        "simple_identifier" => Some(lang::text(callee, src)),
        // `value.method()`, `Foo.bar()`, `a.b.c()` — resolve on the last
        // navigation suffix.
        "navigation_expression" => callee
            .child_by_field_name("suffix")
            .and_then(|suffix| suffix.child_by_field_name("suffix"))
            .map(|node| lang::text(node, src)),
        // Leading-dot shorthand: `.make()`, and what `Box<Int>.make()` degrades
        // to, since the grammar reads the angle brackets as comparisons.
        "prefix_expression" => callee
            .child_by_field_name("target")
            .and_then(|node| callee_name(node, src)),
        _ => None,
    }
}

/// Reduce a type reference to the name we qualify on: `Box<T>` → `Box`,
/// `Outer.Inner` → `Outer.Inner`.
fn base_type_name(node: Node, src: &str) -> String {
    lang::text(node, src)
        .split('<')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
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

    const ADDED_BEFORE: &str =
        include_str!("../../../tests/fixtures/swift/added_func/before.swift");
    const ADDED_AFTER: &str = include_str!("../../../tests/fixtures/swift/added_func/after.swift");
    const MODIFIED_BEFORE: &str =
        include_str!("../../../tests/fixtures/swift/modified_method/before.swift");
    const MODIFIED_AFTER: &str =
        include_str!("../../../tests/fixtures/swift/modified_method/after.swift");
    const EXTENSION_BEFORE: &str =
        include_str!("../../../tests/fixtures/swift/extension_method/before.swift");
    const EXTENSION_AFTER: &str =
        include_str!("../../../tests/fixtures/swift/extension_method/after.swift");
    const DELETED_BEFORE: &str =
        include_str!("../../../tests/fixtures/swift/deleted_struct/before.swift");
    const DELETED_AFTER: &str =
        include_str!("../../../tests/fixtures/swift/deleted_struct/after.swift");
    const NESTED_BEFORE: &str =
        include_str!("../../../tests/fixtures/swift/nested_closure/before.swift");
    const NESTED_AFTER: &str =
        include_str!("../../../tests/fixtures/swift/nested_closure/after.swift");

    /// Every symbol rendered as `name | qualified_name | kind | start-end`, so
    /// expectations read as a table and mismatches diff line by line.
    fn outline(src: &str) -> Vec<String> {
        SwiftAnalyzer
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
        SwiftAnalyzer.calls_in_range(src, LineRange::inclusive(start, end))
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
                // Starts at the doc comment on line 5, not at `public func` on 7.
                row("greetAll", "greetAll", "function", 5, 9),
            ]
        );
    }

    #[test]
    fn methods_are_qualified_by_their_enclosing_type() {
        assert_eq!(
            outline(MODIFIED_BEFORE),
            vec![
                row("Counter", "Counter", "class", 1, 11),
                row("init", "Counter.init", "method", 4, 6),
                row("bump", "Counter.bump", "method", 8, 10),
            ]
        );
        assert_eq!(
            outline(MODIFIED_AFTER),
            vec![
                row("Counter", "Counter", "class", 1, 14),
                row("init", "Counter.init", "method", 4, 6),
                row("bump", "Counter.bump", "method", 8, 11),
                row("log", "Counter.log", "method", 13, 13),
                row("step", "step", "function", 16, 18),
            ]
        );
    }

    #[test]
    fn extension_members_qualify_on_the_extended_type() {
        assert_eq!(
            outline(EXTENSION_BEFORE),
            vec![
                row("Point", "Point", "struct", 1, 4),
                // The `extension Point` block itself is not a symbol.
                row("magnitude", "Point.magnitude", "method", 7, 9),
            ]
        );
        assert_eq!(
            outline(EXTENSION_AFTER),
            vec![
                row("Point", "Point", "struct", 1, 4),
                row("magnitude", "Point.magnitude", "method", 7, 9),
                row("moved", "Point.moved", "method", 11, 13),
            ]
        );
    }

    #[test]
    fn deleted_fixture_loses_its_type_and_extension_method() {
        assert_eq!(
            outline(DELETED_BEFORE),
            vec![
                row("Legacy", "Legacy", "struct", 1, 3),
                row("describe", "Legacy.describe", "method", 6, 8),
                row("keep", "keep", "function", 11, 13),
            ]
        );
        assert_eq!(
            outline(DELETED_AFTER),
            vec![row("keep", "keep", "function", 1, 3)]
        );
    }

    #[test]
    fn nested_functions_and_closures_fold_into_their_parent() {
        for src in [NESTED_BEFORE, NESTED_AFTER] {
            assert_eq!(outline(src), vec![row("total", "total", "function", 1, 7)]);
        }
    }

    #[test]
    fn enums_actors_protocols_and_typealiases_are_symbols() {
        let src = "\
enum Color {
    case red

    func hex() -> String { \"#f00\" }
}

actor Worker {
    func run() {}
}

protocol Draw {
    func draw()
}

typealias Name = String

extension Array {
    func second() -> Element? { nil }
}
";
        assert_eq!(
            outline(src),
            vec![
                row("Color", "Color", "enum", 1, 5),
                row("hex", "Color.hex", "method", 4, 4),
                row("Worker", "Worker", "actor", 7, 9),
                row("run", "Worker.run", "method", 8, 8),
                row("Draw", "Draw", "protocol", 11, 13),
                // A bodyless protocol requirement is still a reviewable member.
                row("draw", "Draw.draw", "method", 12, 12),
                row("Name", "Name", "typealias", 15, 15),
                row("second", "Array.second", "method", 18, 18),
            ]
        );
    }

    #[test]
    fn generic_types_qualify_on_the_bare_name() {
        let src = "\
struct Box<T> {
    func get() -> T? { nil }
}

extension Box<Int> {
    func zero() -> Int { 0 }
}
";
        assert_eq!(
            outline(src),
            vec![
                row("Box", "Box", "struct", 1, 3),
                row("get", "Box.get", "method", 2, 2),
                row("zero", "Box.zero", "method", 6, 6),
            ]
        );
    }

    #[test]
    fn call_shapes_all_reduce_to_a_bare_name() {
        let src = "\
func caller() {
    plain()
    Foo.bar()
    value.method()
    a.b.c()
    _ = Widget(n: 1)
    items.map { double($0) }
}
";
        assert_eq!(
            calls(src, 1, 8),
            vec!["plain", "bar", "method", "c", "Widget", "map", "double"]
        );
    }

    #[test]
    fn leading_dot_shorthand_resolves_to_the_member_name() {
        let src = "func pick() {\n    _ = .make()\n}\n";
        assert_eq!(calls(src, 1, 3), vec!["make"]);
    }

    #[test]
    fn calls_are_limited_to_the_requested_span() {
        assert_eq!(calls(MODIFIED_AFTER, 8, 11), vec!["step", "log"]);
        assert_eq!(calls(MODIFIED_AFTER, 13, 13), Vec::<String>::new());
        assert_eq!(calls(MODIFIED_AFTER, 16, 18), Vec::<String>::new());
    }

    #[test]
    fn a_constructor_call_counts_as_a_use_of_the_type() {
        assert_eq!(calls(DELETED_BEFORE, 11, 13), vec!["Legacy"]);
        assert_eq!(calls(EXTENSION_AFTER, 11, 13), vec!["Point"]);
    }

    #[test]
    fn a_nested_call_belongs_to_the_enclosing_symbol() {
        assert_eq!(
            calls(NESTED_BEFORE, 1, 7),
            vec!["map", "double", "reduce"],
            "the closure's call is attributed to `total`"
        );
    }

    #[test]
    fn repeated_callees_are_reported_once() {
        let src = "func f() {\n    g()\n    g()\n}\n";
        assert_eq!(calls(src, 1, 4), vec!["g"]);
    }

    #[test]
    fn syntax_errors_surface_as_a_parse_error() {
        let broken = "func broken( {\n";
        let error = SwiftAnalyzer.symbols(broken).unwrap_err();
        assert!(matches!(error, AnalysisError::Parse { .. }), "{error:?}");
        // Calls degrade to nothing rather than propagating the failure.
        assert_eq!(calls(broken, 1, 2), Vec::<String>::new());
    }
}
