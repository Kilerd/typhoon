//! Error reporting and recovery: every diagnostic must be reported, parsing
//! must continue, and the parser must never loop forever.

mod common;

use common::{messages, module_with_diags, rendered_errors};
use typhoon_ast::Item;

/// Asserts that parsing `src` reports exactly one diagnostic containing
/// `needle`.
fn one_error(src: &str, needle: &str) {
    let messages = messages(src);
    assert_eq!(
        messages.len(),
        1,
        "expected exactly one diagnostic, got {messages:?}"
    );
    assert!(
        messages[0].contains(needle),
        "expected a diagnostic containing {needle:?}, got {messages:?}"
    );
}

/// Asserts that at least one diagnostic contains `needle`.
fn has_error(src: &str, needle: &str) {
    let messages = messages(src);
    assert!(
        messages.iter().any(|m| m.contains(needle)),
        "expected a diagnostic containing {needle:?}, got {messages:?}"
    );
}

#[test]
fn def_is_not_a_keyword() {
    has_error("def main():\n    pass\n", "unknown keyword `def`");
    let (_, diags) = module_with_diags("def main():\n    pass\n");
    let help = &diags.iter().next().expect("a diagnostic").help;
    assert_eq!(help, &vec!["use `fn` to declare a function".to_string()]);
}

#[test]
fn braces_instead_of_indentation() {
    for src in [
        "fn main() {\n    pass\n}\n",
        "fn main():\n    if x {\n        pass\n    }\n",
        "fn main():\n    while x {\n        pass\n    }\n",
    ] {
        let (_, diags) = module_with_diags(src);
        let has_brace_help = diags.iter().any(|d| {
            d.help
                .iter()
                .any(|h| h.contains("indentation blocks, not braces"))
        });
        assert!(
            has_brace_help,
            "expected the brace help for {src:?}, got {diags:?}"
        );
    }
}

#[test]
fn missing_colon_after_a_header() {
    has_error(
        "fn main()\n    pass\n",
        "expected `:` after the function header",
    );
    has_error(
        "fn main():\n    if x\n        pass\n",
        "expected `:` after the `if` condition",
    );
    has_error(
        "fn main():\n    while x\n        pass\n",
        "expected `:` after the `while` condition",
    );
    has_error(
        "fn main():\n    for i in xs\n        pass\n",
        "expected `:` after the `for` header",
    );
    has_error("class C\n    pass\n", "expected `:` after the class header");
}

#[test]
fn augmented_assignment_is_rejected_with_a_hint() {
    for (src, op) in [
        ("x += 1", "+="),
        ("x -= 1", "-="),
        ("x *= 2", "*="),
        ("x /= 2", "/="),
    ] {
        let src = format!("fn main():\n    {src}\n");
        let (_, diags) = module_with_diags(&src);
        let diag = diags.iter().next().expect("a diagnostic");
        assert_eq!(diag.message, "augmented assignment is not supported yet");
        assert!(diag.labels[0].message.contains(op), "{diag:?}");
        let plain = op.trim_end_matches('=');
        assert!(diag.help[0].contains(&format!("x = x {plain}")), "{diag:?}");
    }
}

#[test]
fn semicolons_are_rejected() {
    one_error(
        "fn main():\n    x = 1;\n",
        "semicolons are not used in typhoon",
    );
}

#[test]
fn elif_and_else_without_if() {
    one_error(
        "fn main():\n    elif x:\n        pass\n",
        "`elif` without a matching `if`",
    );
    one_error(
        "fn main():\n    else:\n        pass\n",
        "`else` without a matching `if`",
    );
}

#[test]
fn top_level_statements_are_rejected() {
    has_error("print(1)\n", "top-level statements are not allowed");
    has_error("x = 1\n", "top-level statements are not allowed");
    let (_, diags) = module_with_diags("print(1)\n");
    assert!(
        diags
            .iter()
            .any(|d| d.help.iter().any(|h| h.contains("fn main()")))
    );
}

#[test]
fn a_top_level_statement_does_not_hide_the_rest_of_the_file() {
    let (module, diags) = module_with_diags("print(1)\n\nfn main():\n    pass\n");
    assert_eq!(diags.len(), 1);
    assert_eq!(module.items.len(), 1);
    assert_eq!(module.items[0].name().name, "main");
}

#[test]
fn two_independent_errors_are_both_reported() {
    let src = "fn a():\n    x ++\n\nfn b():\n    y = = 2\n";
    let (module, diags) = module_with_diags(src);
    assert!(
        diags.len() >= 2,
        "expected at least two diagnostics, got {diags:?}"
    );
    // Both functions still made it into the tree.
    assert_eq!(module.items.len(), 2);
}

#[test]
fn an_error_inside_a_function_does_not_swallow_the_next_function() {
    let src = "fn broken():\n    x = \nfn ok():\n    pass\n";
    let (module, diags) = module_with_diags(src);
    assert!(!diags.is_empty());
    assert_eq!(module.items.len(), 2);
    assert_eq!(module.items[1].name().name, "ok");
}

#[test]
fn an_error_in_a_class_body_does_not_swallow_the_next_item() {
    let src = "class C:\n    1 + 1\n\nfn main():\n    pass\n";
    let (module, diags) = module_with_diags(src);
    assert!(!diags.is_empty());
    assert_eq!(module.items.len(), 2);
}

#[test]
fn positional_after_keyword_argument() {
    one_error(
        "fn main():\n    f(a=1, 2)\n",
        "positional argument follows keyword argument",
    );
}

#[test]
fn invalid_assignment_target() {
    one_error("fn main():\n    1 = 2\n", "invalid assignment target");
    one_error("fn main():\n    f() = 2\n", "invalid assignment target");
}

#[test]
fn local_declaration_without_an_initializer() {
    one_error(
        "fn main():\n    x: int\n",
        "a local variable declaration must have an initializer",
    );
}

#[test]
fn class_fields_may_omit_the_initializer() {
    let (_, diags) = module_with_diags("class C:\n    x: int\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn parameter_without_a_type_annotation() {
    has_error("fn f(a):\n    pass\n", "needs a type annotation");
}

#[test]
fn missing_indented_block() {
    has_error("fn main():\npass\n", "expected an indented block");
}

#[test]
fn unexpected_indentation() {
    has_error(
        "fn main():\n    x = 1\n        y = 2\n",
        "unexpected indentation",
    );
}

#[test]
fn unclosed_delimiters_terminate() {
    for src in [
        "fn main():\n    f(1\n",
        "fn main():\n    xs = [1, 2\n",
        "fn main():\n    d = {1: 2\n",
    ] {
        let (_, diags) = module_with_diags(src);
        assert!(!diags.is_empty(), "expected a diagnostic for {src:?}");
    }
}

#[test]
fn reserved_but_unimplemented_keywords() {
    has_error("import foo\n", "`import` is not supported yet");
    has_error("fn main():\n    raise x\n", "`raise` is not supported yet");
    has_error(
        "fn main():\n    try:\n        pass\n",
        "`try` is not supported yet",
    );
    has_error(
        "fn main():\n    match x:\n        pass\n",
        "`match` is not supported yet",
    );
}

#[test]
fn nested_functions_are_rejected() {
    has_error(
        "fn main():\n    fn inner():\n        pass\n    pass\n",
        "nested functions are not supported",
    );
}

#[test]
fn garbage_input_terminates_and_reports() {
    let inputs = [
        ")))",
        "((((",
        "fn fn fn",
        ":::",
        "fn main(:\n",
        "class:\n",
        "fn main():\n        \n",
        "]]]]",
        "= = =",
        "fn <T>():\n    pass\n",
        "fn main():\n    x = (((\n",
        "\u{1}\u{2}",
    ];
    for src in inputs {
        let (_, diags) = module_with_diags(src);
        assert!(!diags.is_empty(), "expected a diagnostic for {src:?}");
    }
}

#[test]
fn random_token_soup_always_terminates() {
    // A deterministic pseudo-random soup of tokens: the parser must always
    // finish and never panic, whatever the input looks like.
    let pieces = [
        "fn",
        "class",
        "if",
        "elif",
        "else",
        "while",
        "for",
        "in",
        "not",
        "and",
        "or",
        "return",
        "break",
        "continue",
        "pass",
        "True",
        "False",
        "None",
        "is",
        "x",
        "y",
        "1",
        "2.5",
        "\"s\"",
        "f\"{x}\"",
        "+",
        "-",
        "*",
        "/",
        "//",
        "%",
        "**",
        "==",
        "!=",
        "<",
        "<=",
        ">",
        ">=",
        "=",
        ":",
        ",",
        ".",
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
        "->",
        "|",
        "&",
        "^",
        "~",
        "<<",
        ">>",
        "+=",
        ";",
        "\n",
        "\n    ",
        "\n        ",
        "#c\n",
    ];
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for case in 0..300 {
        let mut src = String::new();
        let len = 3 + (case % 40);
        for _ in 0..len {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let index = ((state >> 33) as usize) % pieces.len();
            src.push_str(pieces[index]);
            src.push(' ');
        }
        let mut diags = typhoon_diag::Diagnostics::new();
        // The assertion is that this returns at all.
        let _ = typhoon_parser::parse_module(common::FILE, &src, &mut diags);
    }
}

#[test]
fn moderately_nested_input_parses() {
    let src = format!(
        "fn main():\n    x = {}1{}\n",
        "(".repeat(30),
        ")".repeat(30)
    );
    let (_, diags) = module_with_diags(&src);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn absurdly_nested_input_is_rejected_instead_of_overflowing_the_stack() {
    let src = format!(
        "fn main():\n    x = {}1{}\n",
        "(".repeat(5_000),
        ")".repeat(5_000)
    );
    let messages = messages(&src);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("nested too deeply"), "{messages:?}");
}

#[test]
fn deeply_nested_blocks_are_rejected_instead_of_overflowing_the_stack() {
    let mut src = String::from("fn main():\n");
    for depth in 0..2_000 {
        src.push_str(&"    ".repeat(depth + 1));
        src.push_str("if x:\n");
    }
    src.push_str(&"    ".repeat(2_001));
    src.push_str("pass\n");
    let messages = messages(&src);
    assert!(
        messages.iter().any(|m| m.contains("nested too deeply")),
        "{:?}",
        &messages[..messages.len().min(5)]
    );
}

#[test]
fn empty_file_is_an_empty_module() {
    let (module, diags) = module_with_diags("");
    assert!(module.items.is_empty());
    assert!(diags.is_empty());
    let (module, diags) = module_with_diags("\n\n# just a comment\n\n");
    assert!(module.items.is_empty());
    assert!(diags.is_empty());
}

#[test]
fn recovery_keeps_the_good_parts_of_a_class() {
    let src = "class C:\n    x: int\n    1 + 1\n    fn m(self):\n        pass\n";
    let (module, diags) = module_with_diags(src);
    assert!(!diags.is_empty());
    let Item::Class(decl) = &module.items[0] else {
        panic!("expected a class")
    };
    assert_eq!(decl.fields.len(), 1);
    assert_eq!(decl.methods.len(), 1);
}

#[test]
fn rendered_diagnostics_snapshot() {
    insta::assert_snapshot!(rendered_errors("def main():\n    pass\n"));
}

#[test]
fn rendered_brace_diagnostic_snapshot() {
    insta::assert_snapshot!(rendered_errors("fn main() {\n    print(1)\n}\n"));
}

#[test]
fn rendered_augmented_assignment_snapshot() {
    insta::assert_snapshot!(rendered_errors("fn main():\n    total += 1\n"));
}

#[test]
fn rendered_top_level_statement_snapshot() {
    insta::assert_snapshot!(rendered_errors("print(1)\n"));
}

#[test]
fn rendered_two_errors_snapshot() {
    insta::assert_snapshot!(rendered_errors(
        "fn a():\n    x = \n\nfn b():\n    y = 1;\n"
    ));
}

#[test]
fn lexer_diagnostics_reach_the_caller() {
    // `parse_module` runs the lexer, so its diagnostics land in the same sink.
    let messages = messages("fn main():\n\tpass\n");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("tabs are not allowed in indentation")),
        "{messages:?}"
    );
}

#[test]
fn parsing_continues_after_a_lexer_error() {
    let (module, diags) = module_with_diags("fn main():\n    x = \"unterminated\n    y = 1\n");
    assert!(!diags.is_empty());
    let Item::Fn(decl) = &module.items[0] else {
        panic!("expected a function")
    };
    assert_eq!(decl.body.stmts.len(), 2);
}

#[test]
fn an_unknown_character_does_not_derail_the_parse() {
    let (module, diags) = module_with_diags("fn main():\n    x = 1 @ 2\n    y = 2\n");
    assert!(!diags.is_empty());
    let Item::Fn(decl) = &module.items[0] else {
        panic!("expected a function")
    };
    assert_eq!(decl.name.name, "main");
    assert!(!decl.body.stmts.is_empty());
}

#[test]
fn f_string_hole_errors_point_into_the_file() {
    let src = "fn main():\n    print(f\"{1 +}\")\n";
    let (_, diags) = module_with_diags(src);
    let diag = diags.iter().next().expect("a diagnostic");
    let span = diag.span().expect("a span");
    // The error must point inside the f-string, not at offset 0 of a fragment.
    assert!(span.start > 20, "{diag:?}");
    assert!(span.end as usize <= src.len(), "{diag:?}");
}
