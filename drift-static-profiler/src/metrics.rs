use crate::Language;
use tree_sitter::Node;

#[derive(Debug, Clone, Default)]
pub struct SymbolMetrics {
    pub loc: usize,
    pub complexity: usize,
    pub nesting_depth: usize,
    pub parameter_count: usize,
    pub is_async: bool,
    /// Byte ranges of loop AST nodes inside this symbol's body.
    /// Used by Phase D N+1 detection.
    pub loop_ranges: Vec<(usize, usize)>,
    /// Byte ranges of `await_expression` (or equivalent) nodes inside the body.
    /// Used by Phase D blocking-in-async detection.
    pub await_ranges: Vec<(usize, usize)>,
}

/// Compute metrics for a defined symbol given its top-level AST node (the
/// `function_definition` / `method_declaration` / `class_declaration`) and the
/// source bytes.
pub fn compute(node: Node, source: &str, lang: Language) -> SymbolMetrics {
    let body = body_of(node, lang).unwrap_or(node);
    let loc = count_lines_in_range(source, body.start_byte(), body.end_byte());
    let complexity = 1 + count_decision_points(body, lang);
    let nesting_depth = max_nesting(body, lang);
    let parameter_count = count_parameters(node, lang);
    let is_async = detect_async(node, source, lang);
    let (loop_ranges, await_ranges) = collect_loop_and_await_ranges(body);
    SymbolMetrics {
        loc,
        complexity,
        nesting_depth,
        parameter_count,
        is_async,
        loop_ranges,
        await_ranges,
    }
}

fn body_of<'a>(node: Node<'a>, _lang: Language) -> Option<Node<'a>> {
    // Try child by field name first (works across all four languages).
    node.child_by_field_name("body")
}

fn count_lines_in_range(source: &str, start: usize, end: usize) -> usize {
    // Inclusive line count: at least 1 if any bytes.
    if end <= start {
        return 0;
    }
    let slice = &source.as_bytes()[start..end.min(source.len())];
    let nl = slice.iter().filter(|&&b| b == b'\n').count();
    nl + 1
}

fn count_decision_points(root: Node, lang: Language) -> usize {
    let mut count = 0;
    walk(root, &mut |n| {
        if is_decision_point(n, lang) {
            count += 1;
        }
    });
    count
}

fn is_decision_point(n: Node, lang: Language) -> bool {
    let k = n.kind();
    let common = matches!(
        k,
        "if_statement"
            | "while_statement"
            | "for_statement"
            | "do_statement"
            | "case_clause"
            | "switch_case"
            | "switch_label"
            | "catch_clause"
            | "except_clause"
            | "conditional_expression"
            | "ternary_expression"
            | "elif_clause"
            | "elif_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "enhanced_for_statement"
            // Go: expression-switch / type-switch / select branches.
            | "expression_switch_statement"
            | "type_switch_statement"
            | "type_case"
            | "default_case"
            | "communication_case"
            // Rust expression-flavored control flow.
            | "if_expression"
            | "if_let_expression"
            | "while_expression"
            | "while_let_expression"
            | "for_expression"
            | "loop_expression"
            | "match_expression"
            | "match_arm"
            // Scala mirrors most of the Rust-style expression names.
            | "case_block"
    );
    if common {
        return true;
    }
    // Logical and/or for languages where it's a separate node kind
    match lang {
        Language::Python => k == "boolean_operator",
        Language::Java
        | Language::TypeScript
        | Language::JavaScript
        | Language::Go
        | Language::Rust
        | Language::Scala => {
            // tree-sitter exposes && / || as the operator field on binary_expression
            // Recognize the binary_expression itself and check the operator text.
            if k == "binary_expression" {
                if let Some(op) = n.child_by_field_name("operator") {
                    let kk = op.kind();
                    return kk == "&&" || kk == "||" || kk == "??";
                }
            }
            false
        }
    }
}

fn max_nesting(root: Node, lang: Language) -> usize {
    fn rec(node: Node, depth: usize, lang: Language) -> usize {
        let inc = is_nesting_kind(node, lang);
        let depth_here = if inc { depth + 1 } else { depth };
        let mut max = depth_here;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let d = rec(child, depth_here, lang);
            if d > max {
                max = d;
            }
        }
        max
    }
    rec(root, 0, lang)
}

fn is_nesting_kind(n: Node, _lang: Language) -> bool {
    matches!(
        n.kind(),
        "if_statement"
            | "elif_clause"
            | "else_clause"
            | "while_statement"
            | "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "do_statement"
            | "switch_statement"
            | "switch_expression"
            | "try_statement"
            | "except_clause"
            | "catch_clause"
            | "lambda"
            | "lambda_expression"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
            | "enhanced_for_statement"
            // Go
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "func_literal"
            // Rust
            | "if_expression"
            | "if_let_expression"
            | "while_expression"
            | "while_let_expression"
            | "for_expression"
            | "loop_expression"
            | "match_expression"
            | "function_item"
            | "closure_expression"
            | "impl_item"
            // Scala — function_definition already listed above.
            | "match_clause"
            | "indented_block"
    )
}

fn count_parameters(node: Node, _lang: Language) -> usize {
    // All seven supported grammars expose the parameter list via a field
    // named "parameters" on the function/method definition node.
    let Some(params) = node.child_by_field_name("parameters") else {
        return 0;
    };
    // Count parameter-bearing children. Tree-sitter exposes the list itself;
    // we filter for identifier-like nodes that represent parameters.
    let mut count = 0;
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        // Skip pure-syntax separators like commas which aren't named anyway.
        match child.kind() {
            // Python
            "identifier"
            | "typed_parameter"
            | "default_parameter"
            | "typed_default_parameter"
            | "list_splat_pattern"
            | "dictionary_splat_pattern"
            // Java
            | "formal_parameter"
            | "spread_parameter"
            | "receiver_parameter"
            // TS / JS
            | "required_parameter"
            | "optional_parameter"
            | "rest_pattern"
            | "rest_parameter"
            | "assignment_pattern"
            // Go: each `parameter_declaration` is one parameter group; we
            // count groups, not names, because `func f(a, b int)` is
            // commonly read as one logical parameter list in Go style.
            | "parameter_declaration"
            | "variadic_parameter_declaration"
            // Rust + Scala both emit `parameter` per arg inside the
            // parameter list. `self_parameter` / `variadic_parameter` are
            // Rust-only; `class_parameter` is Scala's constructor-arg form
            // (`class Foo(x: Int)`).
            | "parameter"
            | "self_parameter"
            | "variadic_parameter"
            | "class_parameter" => count += 1,
            _ => {}
        }
    }
    count
}

fn detect_async(node: Node, source: &str, lang: Language) -> bool {
    // Cheap heuristic: peek at the first ~8 bytes of the symbol's leading text.
    // True for `async def`, `async function`, `async name(`, `public async ...`.
    let start = node.start_byte();
    let bytes = source.as_bytes();
    let end = (start + 64).min(bytes.len());
    let head = match std::str::from_utf8(&bytes[start..end]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let trimmed = head.trim_start();
    match lang {
        Language::Python => trimmed.starts_with("async def") || trimmed.starts_with("async\n"),
        Language::Java => false, // CompletableFuture detection deferred
        Language::TypeScript | Language::JavaScript => {
            trimmed.starts_with("async ")
                || trimmed.starts_with("public async")
                || trimmed.starts_with("private async")
                || trimmed.starts_with("protected async")
                || trimmed.starts_with("static async")
                // method shorthand: `async create(...)`
                || (trimmed.len() > 5 && &trimmed[..5] == "async" && trimmed.as_bytes().get(5) == Some(&b' '))
        }
        // Go's concurrency (`go fn()`, channels) lives at the call site, not
        // on the function definition, so there's nothing in the symbol head
        // to flag.
        Language::Go => false,
        // Rust: `async fn name()`, optionally prefixed by `pub`/`pub(crate)`.
        Language::Rust => {
            let s = trimmed.trim_start_matches("pub").trim_start();
            // Strip pub(crate)/pub(super)/etc.
            let s = if let Some(rest) = s.strip_prefix('(') {
                rest.split_once(')').map(|(_, r)| r.trim_start()).unwrap_or(s)
            } else {
                s
            };
            s.starts_with("async fn") || s.starts_with("async unsafe")
        }
        // Scala async is library-level (Future, ZIO, cats-effect …), not a
        // keyword on the def, so we can't detect it syntactically.
        Language::Scala => false,
    }
}

fn walk<F: FnMut(Node)>(node: Node, visit: &mut F) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, visit);
    }
}

fn collect_loop_and_await_ranges(body: Node) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let mut loops = Vec::new();
    let mut awaits = Vec::new();
    walk(body, &mut |n| {
        if is_loop_kind(n) {
            loops.push((n.start_byte(), n.end_byte()));
        }
        if is_await_kind(n) {
            awaits.push((n.start_byte(), n.end_byte()));
        }
    });
    (loops, awaits)
}

fn is_loop_kind(n: Node) -> bool {
    matches!(
        n.kind(),
        "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "enhanced_for_statement"
            | "while_statement"
            | "do_statement"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression"
            // Rust expression-loops
            | "for_expression"
            | "while_expression"
            | "while_let_expression"
            | "loop_expression"
    )
}

fn is_await_kind(n: Node) -> bool {
    // Python: `await_expression` (also called `await`?); JS/TS: `await_expression`
    matches!(n.kind(), "await_expression" | "await")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::extract_tags_from_source;
    use std::path::Path;

    fn sym(src: &str, lang: Language, name: &str) -> crate::Symbol {
        let tags = extract_tags_from_source(Path::new("t.test"), lang, src).expect("parse");
        tags.symbols.into_iter().find(|s| s.name == name).expect("symbol")
    }

    #[test]
    fn python_complexity_straight_line_is_one() {
        let s = sym("def f(x):\n    return x + 1\n", Language::Python, "f");
        assert_eq!(s.complexity, 1, "straight-line code is complexity 1");
        assert_eq!(s.nesting_depth, 0);
    }

    #[test]
    fn python_complexity_counts_if_for_and() {
        // 1 (base) + if + for + and = 4
        let src = "def f(xs, q):\n    if q:\n        for x in xs:\n            if x > 0 and x < 10:\n                print(x)\n";
        let s = sym(src, Language::Python, "f");
        // if (1) + for (1) + nested if (1) + and (1) = 4 decision points => complexity 5
        assert!(s.complexity >= 4, "expected complexity >= 4, got {}", s.complexity);
        assert!(s.nesting_depth >= 2, "expected nesting_depth >= 2, got {}", s.nesting_depth);
    }

    #[test]
    fn python_loc_is_line_count() {
        let s = sym("def f():\n    x = 1\n    return x\n", Language::Python, "f");
        // body spans 3 lines (def + 2 stmts) — exact count depends on grammar
        assert!(s.loc >= 2 && s.loc <= 4, "got loc = {}", s.loc);
    }

    #[test]
    fn python_async_detected() {
        let s = sym("async def f():\n    return 1\n", Language::Python, "f");
        assert!(s.is_async, "async def should set is_async");
    }

    #[test]
    fn python_parameter_count() {
        let s = sym("def f(a, b, c=1):\n    return a\n", Language::Python, "f");
        assert_eq!(s.parameter_count, 3);
    }

    #[test]
    fn typescript_complexity_with_ternary_and_short_circuit() {
        // 1 (base) + ternary + && = 3
        let src = "function f(x: number, y: number) { return x > 0 ? (x && y) : 0; }\n";
        let s = sym(src, Language::TypeScript, "f");
        assert!(s.complexity >= 2, "got {}", s.complexity);
    }

    #[test]
    fn typescript_async_detected() {
        let src = "async function f() { return 1; }\n";
        let s = sym(src, Language::TypeScript, "f");
        assert!(s.is_async);
    }

    #[test]
    fn javascript_for_loop_and_if_complexity() {
        let src = "function f(xs) { for (const x of xs) { if (x > 0) console.log(x); } }\n";
        let s = sym(src, Language::JavaScript, "f");
        assert!(s.complexity >= 3, "got {}", s.complexity);
        assert!(s.nesting_depth >= 2, "got {}", s.nesting_depth);
    }

    #[test]
    fn java_complexity_if_and_switch_case() {
        let src = "class C { void f(int x) { if (x > 0 && x < 10) { switch (x) { case 1: break; case 2: break; } } } }\n";
        let s = sym(src, Language::Java, "f");
        // if (1) + && (1) + 2 cases (2) = 4 decision points => complexity 5
        assert!(s.complexity >= 3, "got {}", s.complexity);
    }

    // ── Go ────────────────────────────────────────────────────────────

    #[test]
    fn go_function_complexity_counts_if_for_and() {
        let src = "package p\n\
                   func f(xs []int, q bool) int {\n\
                     if q {\n\
                       for _, x := range xs {\n\
                         if x > 0 && x < 10 {\n\
                           return x\n\
                         }\n\
                       }\n\
                     }\n\
                     return 0\n\
                   }\n";
        let s = sym(src, Language::Go, "f");
        assert!(s.complexity >= 4, "got {}", s.complexity);
        assert!(s.nesting_depth >= 3, "got {}", s.nesting_depth);
        assert!(!s.is_async, "Go has no async keyword");
    }

    #[test]
    fn go_method_declaration_parsed() {
        let src = "package p\n\
                   type T struct{}\n\
                   func (r *T) DoIt(x int) error { return nil }\n";
        let s = sym(src, Language::Go, "DoIt");
        assert_eq!(s.parameter_count, 1, "got {}", s.parameter_count);
    }

    // ── Rust ──────────────────────────────────────────────────────────

    #[test]
    fn rust_function_async_detected() {
        let src = "async fn run(x: u32) -> u32 { x + 1 }\n";
        let s = sym(src, Language::Rust, "run");
        assert!(s.is_async, "`async fn` should set is_async");
        assert_eq!(s.parameter_count, 1);
    }

    #[test]
    fn rust_pub_async_fn_detected_with_visibility() {
        let src = "pub async fn run() {}\n";
        let s = sym(src, Language::Rust, "run");
        assert!(s.is_async, "pub async fn must still detect async");
    }

    #[test]
    fn rust_complexity_counts_match_arms_and_if_let() {
        let src = "fn classify(x: Option<i32>) -> &'static str {\n\
                     if let Some(v) = x {\n\
                       match v {\n\
                         0 => \"zero\",\n\
                         1 | 2 => \"small\",\n\
                         _ => \"other\",\n\
                       }\n\
                     } else {\n\
                       \"none\"\n\
                     }\n\
                   }\n";
        let s = sym(src, Language::Rust, "classify");
        assert!(s.complexity >= 4, "got {}", s.complexity);
    }

    #[test]
    fn rust_method_inside_impl_block_gets_parent() {
        // impl T { fn m() {} } — m's parent should be T via byte containment.
        use crate::tags::extract_tags_from_source;
        use std::path::Path;
        let src = "struct T;\nimpl T {\n  fn m(&self) -> i32 { 1 }\n}\n";
        let tags = extract_tags_from_source(Path::new("t.rs"), Language::Rust, src).unwrap();
        let m = tags
            .symbols
            .iter()
            .find(|s| s.name == "m")
            .expect("m extracted");
        assert_eq!(
            m.parent.as_deref(),
            Some("T"),
            "method inside impl T should get parent=T; got {:?}",
            m.parent
        );
    }

    // ── Scala ─────────────────────────────────────────────────────────

    #[test]
    fn scala_function_definition_parsed() {
        // Plain Scala def with parens.
        let src = "object O {\n  def greet(name: String): String = \"hi \" + name\n}\n";
        let s = sym(src, Language::Scala, "greet");
        // Some scala grammars don't expose parameter list as `parameters`
        // field uniformly; we tolerate 0 here and just assert the symbol
        // was extracted. Body-derived metrics should still compute.
        assert!(s.loc >= 1, "loc should be at least 1, got {}", s.loc);
    }

    #[test]
    fn scala_parameter_count() {
        // `parameters` is the outer list; `parameter` (singular) is each arg.
        // Was previously broken because the wrong kind name was matched.
        let src = "object O {\n  def fn3(a: Int, b: Int, c: String): Int = a\n}\n";
        let s = sym(src, Language::Scala, "fn3");
        assert_eq!(s.parameter_count, 3, "got {}", s.parameter_count);
    }

    #[test]
    fn scala_class_extracted() {
        use crate::tags::extract_tags_from_source;
        use std::path::Path;
        let src = "class Foo(x: Int) {\n  def get: Int = x\n}\n";
        let tags = extract_tags_from_source(Path::new("Foo.scala"), Language::Scala, src).unwrap();
        let foo = tags
            .symbols
            .iter()
            .find(|s| s.name == "Foo")
            .expect("class Foo extracted");
        assert!(
            matches!(foo.kind, crate::SymbolKind::Class),
            "class Foo should be SymbolKind::Class"
        );
    }
}
