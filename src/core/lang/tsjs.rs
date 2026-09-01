//! TypeScript and JavaScript symbol and call extraction.
//!
//! One analyzer serves all four extensions; only the grammar changes. See
//! [`Dialect`].

use tree_sitter::{Language, Node};

use crate::core::diff::Span;
use crate::core::error::AnalysisError;
use crate::core::lang::{self, LanguageAnalyzer, Symbol};

/// How TypeScript spells qualification: `Counter.bump`.
const SEPARATOR: &str = ".";

/// Sibling kinds that belong to the declaration below them. Decorators parse
/// *inside* the declaration they annotate, so only comments matter here.
const PRELUDE_KINDS: &[&str] = &["comment"];

/// Which of the two grammars to parse with.
///
/// They disagree on a single character: [`Dialect::Tsx`] reads `<` as the start
/// of JSX, [`Dialect::TypeScript`] as a type argument list. A file has to be
/// parsed with the one its extension implies — `.ts` may say `foo<T>()`, `.tsx`
/// may say `<Foo />`, and neither grammar accepts the other's spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// The `typescript` grammar: `.ts`.
    TypeScript,
    /// The `tsx` grammar: `.tsx`, `.jsx`, and `.js` — plain JavaScript is a
    /// subset of what it accepts.
    Tsx,
}

impl Dialect {
    fn language(self) -> Language {
        match self {
            Dialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Dialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    /// The name that identifies this dialect in parse errors.
    fn label(self) -> &'static str {
        match self {
            Dialect::TypeScript => "typescript",
            Dialect::Tsx => "tsx",
        }
    }
}

/// Analyzer for `.ts`, `.tsx`, `.js` and `.jsx` sources.
pub struct TsJsAnalyzer {
    dialect: Dialect,
}

impl TsJsAnalyzer {
    pub fn new(dialect: Dialect) -> Self {
        Self { dialect }
    }
}

impl LanguageAnalyzer for TsJsAnalyzer {
    fn symbols(&self, src: &str) -> Result<Vec<Symbol>, AnalysisError> {
        let tree = lang::parse(&self.dialect.language(), src, self.dialect.label())?;
        let mut out = Vec::new();
        collect(tree.root_node(), src, "", &mut out);
        Ok(out)
    }

    fn calls_in_span(&self, src: &str, span: &Span) -> Vec<String> {
        let Ok(tree) = lang::parse(&self.dialect.language(), src, self.dialect.label()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        lang::for_each_descendant(tree.root_node(), &mut |node| {
            if !span.claims(lang::start_line(node)) {
                return;
            }
            let callee = match node.kind() {
                // `foo()`, `obj.foo()`, `foo?.()`.
                "call_expression" => node.child_by_field_name("function"),
                // `new Counter(0)` counts as a use of `Counter`, which is how a
                // class ends up connected to the code that instantiates it.
                "new_expression" => node.child_by_field_name("constructor"),
                _ => None,
            };
            if let Some(name) = callee.and_then(|callee| callee_name(callee, src)) {
                lang::push_unique(&mut out, name);
            }
        });
        out
    }
}

/// Walk the statements of one container, emitting a symbol per declaration.
///
/// Function bodies are never descended into: a nested function or callback
/// belongs to the symbol that encloses it, and that symbol's line span already
/// covers it. Class bodies *are* descended into, by [`collect_members`].
///
/// Because TypeScript declares methods inside their class, the spans emitted
/// here nest: `Counter` covers `Counter.bump`. See [`Symbol::start_line`].
fn collect(container: Node, src: &str, prefix: &str, out: &mut Vec<Symbol>) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        let Some((statement, declaration)) = unwrap(child) else {
            continue;
        };
        match declaration.kind() {
            // `function_signature` is the bodyless `declare function f(): void`.
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                push_named(statement, declaration, src, prefix, "function", out);
            }
            "class_declaration" | "abstract_class_declaration" => {
                let name = lang::field_text(declaration, "name", src).to_string();
                push_symbol(statement, prefix, "class", &name, out);
                if let Some(body) = declaration.child_by_field_name("body") {
                    collect_members(body, src, &join(prefix, &name), out);
                }
            }
            // Interface members are types rather than code, so the interface is
            // reviewed as one unit and its body is not descended into.
            "interface_declaration" => {
                push_named(statement, declaration, src, prefix, "interface", out);
            }
            "type_alias_declaration" => {
                push_named(statement, declaration, src, prefix, "type_alias", out);
            }
            "enum_declaration" => push_named(statement, declaration, src, prefix, "enum", out),
            "lexical_declaration" | "variable_declaration" => {
                collect_bindings(statement, declaration, src, prefix, out);
            }
            _ => {}
        }
    }
}

/// Emit a symbol per member of a class body.
fn collect_members(body: Node, src: &str, prefix: &str, out: &mut Vec<Symbol>) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            // `abstract_method_signature` is a bodyless requirement; it is named
            // exactly like a real method and worth reviewing as one.
            "method_definition" | "abstract_method_signature" => {
                push_named(child, child, src, prefix, "method", out);
            }
            // A field holding a function — `handle = (e) => {…}` — is a method
            // in everything but grammar. A field holding data is not a symbol.
            "public_field_definition" if holds_a_function(child) => {
                push_named(child, child, src, prefix, "method", out);
            }
            _ => {}
        }
    }
}

/// Emit a symbol for each `const`/`let`/`var` binding whose value is a function.
fn collect_bindings(
    statement: Node,
    declaration: Node,
    src: &str,
    prefix: &str,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = declaration.walk();
    let declarators: Vec<Node> = declaration
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "variable_declarator")
        .collect();
    // A binding declared on its own owns the whole statement: its leading
    // comment, the `export` keyword and the trailing `;`. Bindings sharing a
    // statement (`const a = 1, b = () => 2`) can only own themselves.
    let sole = declarators.len() == 1;
    for declarator in declarators {
        if !holds_a_function(declarator) {
            continue;
        }
        let node = if sole { statement } else { declarator };
        let name = lang::field_text(declarator, "name", src).to_string();
        push_symbol(node, prefix, "function", &name, out);
    }
}

/// Peel the `export …` / `declare …` wrappers off a statement, returning the
/// statement (whose span the symbol takes, so that `export` and any leading
/// comment count as part of it) and the declaration that names it.
fn unwrap(node: Node) -> Option<(Node, Node)> {
    match node.kind() {
        "export_statement" | "ambient_declaration" => node
            .child_by_field_name("declaration")
            .or_else(|| node.named_child(0))
            .map(|declaration| (node, declaration)),
        _ => Some((node, node)),
    }
}

/// Whether a binding or field is initialized with a function expression.
fn holds_a_function(node: Node) -> bool {
    node.child_by_field_name("value").is_some_and(|value| {
        matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "generator_function"
        )
    })
}

/// Push a symbol named by `declaration`'s `name` field, spanning `node`.
fn push_named(
    node: Node,
    declaration: Node,
    src: &str,
    prefix: &str,
    kind: &str,
    out: &mut Vec<Symbol>,
) {
    let name = lang::field_text(declaration, "name", src).to_string();
    push_symbol(node, prefix, kind, &name, out);
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
/// (calling a function held in an array element, say).
fn callee_name<'a>(callee: Node, src: &'a str) -> Option<&'a str> {
    match callee.kind() {
        // `foo()`, and `new Foo()`.
        "identifier" => Some(lang::text(callee, src)),
        // `obj.foo()`, `this.foo()`, `a.b.c()` — resolve on the last property.
        "member_expression" => callee
            .child_by_field_name("property")
            .map(|property| lang::text(property, src)),
        // Wrappers that do not change which function is called: `(foo)()`,
        // `foo!()`, `(foo as Fn)()`.
        "parenthesized_expression" | "non_null_expression" => callee
            .named_child(0)
            .and_then(|inner| callee_name(inner, src)),
        "as_expression" | "satisfies_expression" => callee
            .child_by_field_name("left")
            .or_else(|| callee.named_child(0))
            .and_then(|inner| callee_name(inner, src)),
        _ => None,
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
    use crate::core::diff::LineRange;

    const ADDED_BEFORE: &str = include_str!("../../../tests/fixtures/ts/added_arrow/before.ts");
    const ADDED_AFTER: &str = include_str!("../../../tests/fixtures/ts/added_arrow/after.ts");
    const MODIFIED_BEFORE: &str =
        include_str!("../../../tests/fixtures/ts/modified_method/before.ts");
    const MODIFIED_AFTER: &str =
        include_str!("../../../tests/fixtures/ts/modified_method/after.ts");
    const DELETED_BEFORE: &str =
        include_str!("../../../tests/fixtures/ts/deleted_interface/before.ts");
    const DELETED_AFTER: &str =
        include_str!("../../../tests/fixtures/ts/deleted_interface/after.ts");
    const COMPONENT_BEFORE: &str = include_str!("../../../tests/fixtures/ts/component/before.tsx");
    const COMPONENT_AFTER: &str = include_str!("../../../tests/fixtures/ts/component/after.tsx");

    fn analyzer(dialect: Dialect) -> TsJsAnalyzer {
        TsJsAnalyzer::new(dialect)
    }

    /// Every symbol rendered as `name | qualified_name | kind | start-end`, so
    /// expectations read as a table and mismatches diff line by line.
    fn outline_with(dialect: Dialect, src: &str) -> Vec<String> {
        analyzer(dialect)
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

    fn outline(src: &str) -> Vec<String> {
        outline_with(Dialect::TypeScript, src)
    }

    fn row(name: &str, qualified_name: &str, kind: &str, start: u32, end: u32) -> String {
        format!("{name} | {qualified_name} | {kind} | {start}-{end}")
    }

    fn calls_with(dialect: Dialect, src: &str, start: u32, end: u32) -> Vec<String> {
        analyzer(dialect).calls_in_span(src, &Span::whole(LineRange::inclusive(start, end)))
    }

    fn calls(src: &str, start: u32, end: u32) -> Vec<String> {
        calls_with(Dialect::TypeScript, src, start, end)
    }

    #[test]
    fn added_fixture_gains_an_arrow_binding() {
        assert_eq!(
            outline(ADDED_BEFORE),
            vec![row("greet", "greet", "function", 1, 3)]
        );
        assert_eq!(
            outline(ADDED_AFTER),
            vec![
                row("greet", "greet", "function", 1, 3),
                // Starts at the doc comment on line 5, not at `export const` on 6,
                // and runs to the end of the statement on line 7.
                row("greetAll", "greetAll", "function", 5, 7),
            ]
        );
    }

    #[test]
    fn methods_are_qualified_by_their_enclosing_class() {
        assert_eq!(
            outline(MODIFIED_BEFORE),
            vec![
                row("Counter", "Counter", "class", 1, 11),
                row("constructor", "Counter.constructor", "method", 4, 6),
                row("bump", "Counter.bump", "method", 8, 10),
            ]
        );
        assert_eq!(
            outline(MODIFIED_AFTER),
            vec![
                row("Counter", "Counter", "class", 1, 14),
                row("constructor", "Counter.constructor", "method", 4, 6),
                row("bump", "Counter.bump", "method", 8, 11),
                row("log", "Counter.log", "method", 13, 13),
                row("step", "step", "function", 16, 18),
            ]
        );
    }

    #[test]
    fn interfaces_and_type_aliases_are_symbols() {
        assert_eq!(
            outline(DELETED_BEFORE),
            vec![
                row("Shape", "Shape", "interface", 1, 3),
                row("Name", "Name", "type_alias", 5, 5),
                row("describe", "describe", "function", 7, 9),
            ]
        );
        assert_eq!(
            outline(DELETED_AFTER),
            vec![row("describe", "describe", "function", 1, 3)]
        );
    }

    #[test]
    fn the_tsx_dialect_parses_jsx() {
        assert_eq!(
            outline_with(Dialect::Tsx, COMPONENT_BEFORE),
            vec![
                row("Badge", "Badge", "function", 3, 6),
                // `export default function` is unwrapped like any other export.
                row("Panel", "Panel", "function", 8, 14),
            ]
        );
        assert_eq!(
            outline_with(Dialect::Tsx, COMPONENT_AFTER),
            vec![
                row("BadgeProps", "BadgeProps", "interface", 3, 6),
                row("Badge", "Badge", "function", 8, 11),
                row("Panel", "Panel", "function", 13, 19),
            ]
        );
    }

    #[test]
    fn the_typescript_dialect_rejects_jsx() {
        let error = analyzer(Dialect::TypeScript)
            .symbols(COMPONENT_BEFORE)
            .unwrap_err();
        assert_eq!(
            error,
            AnalysisError::Parse {
                path: "typescript".to_string(),
                reason: "source contains syntax errors".to_string(),
            },
            "a `.tsx` source must be analyzed with the tsx grammar"
        );
    }

    #[test]
    fn the_typescript_dialect_reads_angle_brackets_as_type_arguments() {
        let src = "export function first<T>(items: T[]): T {\n  return identity<T>(items[0]);\n}\n";
        assert_eq!(outline(src), vec![row("first", "first", "function", 1, 3)]);
        assert_eq!(calls(src, 1, 3), vec!["identity"]);
    }

    #[test]
    fn enums_classes_and_class_fields_are_symbols() {
        let src = "\
enum Color {
  Red,
  Green,
}

abstract class Base {
  @Input() name = \"x\";
  handle = (event: Event) => {
    track(event);
  };
  abstract run(): void;
  static make(): Base | null {
    return null;
  }
}
";
        assert_eq!(
            outline(src),
            vec![
                row("Color", "Color", "enum", 1, 4),
                row("Base", "Base", "class", 6, 15),
                // The decorated `name` field holds data, not a function.
                row("handle", "Base.handle", "method", 8, 10),
                // A bodyless abstract member is still a reviewable one.
                row("run", "Base.run", "method", 11, 11),
                row("make", "Base.make", "method", 12, 14),
            ]
        );
    }

    #[test]
    fn a_shared_statement_gives_each_binding_its_own_span() {
        let src = "const label = \"x\",\n  wrap = () => label,\n  unwrap = function () {\n    return label;\n  };\n";
        assert_eq!(
            outline(src),
            vec![
                row("wrap", "wrap", "function", 2, 2),
                row("unwrap", "unwrap", "function", 3, 5),
            ]
        );
    }

    #[test]
    fn ambient_declarations_are_symbols() {
        assert_eq!(
            outline("declare function ambient(): void;\n"),
            vec![row("ambient", "ambient", "function", 1, 1)]
        );
    }

    #[test]
    fn re_exports_declare_nothing() {
        assert_eq!(
            outline("export { a, b } from \"./mod\";\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            outline("import { c } from \"./mod\";\nexport default c;\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn call_shapes_all_reduce_to_a_bare_name() {
        let src = "\
function caller() {
  plain();
  obj.method();
  a.b.c();
  const made = new Widget(1);
  items.map((n) => double(n));
  (wrapped!)();
}
";
        assert_eq!(
            calls(src, 1, 8),
            vec!["plain", "method", "c", "Widget", "map", "double", "wrapped"]
        );
    }

    #[test]
    fn calls_are_limited_to_the_requested_span() {
        assert_eq!(calls(MODIFIED_AFTER, 8, 11), vec!["step", "log"]);
        assert_eq!(calls(MODIFIED_AFTER, 13, 13), Vec::<String>::new());
        assert_eq!(calls(MODIFIED_AFTER, 16, 18), Vec::<String>::new());
    }

    #[test]
    fn a_nested_callback_belongs_to_the_enclosing_symbol() {
        assert_eq!(
            calls(ADDED_AFTER, 5, 7),
            vec!["map", "greet"],
            "the callback's call is attributed to `greetAll`"
        );
    }

    #[test]
    fn tsx_calls_are_extracted_through_jsx() {
        assert_eq!(
            calls_with(Dialect::Tsx, COMPONENT_AFTER, 8, 11),
            vec!["classNames"]
        );
    }

    #[test]
    fn a_method_call_on_a_parameter_resolves_to_the_method_name() {
        assert_eq!(calls(DELETED_BEFORE, 7, 9), vec!["area"]);
    }

    #[test]
    fn repeated_callees_are_reported_once() {
        let src = "function f() {\n  g();\n  g();\n}\n";
        assert_eq!(calls(src, 1, 4), vec!["g"]);
    }

    #[test]
    fn syntax_errors_surface_as_a_parse_error() {
        let broken = "function broken( {\n";
        let error = analyzer(Dialect::TypeScript).symbols(broken).unwrap_err();
        assert!(matches!(error, AnalysisError::Parse { .. }), "{error:?}");
        // Calls degrade to nothing rather than propagating the failure.
        assert_eq!(calls(broken, 1, 2), Vec::<String>::new());
    }
}
