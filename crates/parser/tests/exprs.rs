//! Expression parsing: the precedence table of DESIGN 3.6, every expression
//! kind, and the generic-call disambiguation of DESIGN 3.9.

mod common;

use common::{expr, expr_dump, expr_with_diags};

#[test]
fn precedence_table() {
    // Ordered from the loosest to the tightest binding, exactly as in
    // DESIGN.md section 3.6.
    let cases = [
        // or < and
        ("a or b and c", "(or a (and b c))"),
        ("a and b or c", "(or (and a b) c)"),
        // and < not
        ("not a and b", "(and (not a) b)"),
        ("a and not b", "(and a (not b))"),
        // not < comparison
        ("not a == b", "(not (cmp a == b))"),
        // comparison < |
        ("a | b == c", "(cmp (| a b) == c)"),
        ("a == b | c", "(cmp a == (| b c))"),
        // | < ^
        ("a | b ^ c", "(| a (^ b c))"),
        // ^ < &
        ("a ^ b & c", "(^ a (& b c))"),
        // & < shift
        ("a & b << c", "(& a (<< b c))"),
        ("a & b >> c", "(& a (>> b c))"),
        // shift < additive
        ("a << b + c", "(<< a (+ b c))"),
        // additive < multiplicative
        ("a + b * c", "(+ a (* b c))"),
        ("a - b / c", "(- a (/ b c))"),
        ("a + b // c", "(+ a (// b c))"),
        ("a + b % c", "(+ a (% b c))"),
        // multiplicative < unary
        ("-a * b", "(* (- a) b)"),
        ("~a & b", "(& (~ a) b)"),
        ("+a - b", "(- (+ a) b)"),
        // unary < **
        ("-a ** b", "(- (** a b))"),
        ("a ** -b", "(** a (- b))"),
        // ** < call/index/attribute
        ("a ** b.c", "(** a (. b c))"),
        ("-x.y", "(- (. x y))"),
        ("a ** b * c", "(* (** a b) c)"),
    ];
    for (src, expected) in cases {
        assert_eq!(expr(src), expected, "while parsing {src:?}");
    }
}

#[test]
fn associativity() {
    let cases = [
        ("a + b - c", "(- (+ a b) c)"),
        ("a - b + c", "(+ (- a b) c)"),
        ("a * b // c % d", "(% (// (* a b) c) d)"),
        ("a | b | c", "(| (| a b) c)"),
        ("a and b and c", "(and (and a b) c)"),
        ("a or b or c", "(or (or a b) c)"),
        ("a << b << c", "(<< (<< a b) c)"),
        // `**` is the only right associative operator.
        ("a ** b ** c", "(** a (** b c))"),
        ("2 ** 3 ** 2", "(** 2 (** 3 2))"),
    ];
    for (src, expected) in cases {
        assert_eq!(expr(src), expected, "while parsing {src:?}");
    }
}

#[test]
fn parentheses_regroup_without_creating_a_node() {
    assert_eq!(expr("(a + b) * c"), "(* (+ a b) c)");
    assert_eq!(expr("a * (b + c)"), "(* a (+ b c))");
    assert_eq!(expr("(a)"), "a");
    assert_eq!(expr("((a))"), "a");
    // The span of a parenthesised expression is the span of the inner
    // expression, not of the parentheses.
    assert_eq!(expr_dump("(a)"), "Name `a` @1..2");
}

#[test]
fn comparison_operators() {
    let cases = [
        ("a == b", "(cmp a == b)"),
        ("a != b", "(cmp a != b)"),
        ("a < b", "(cmp a < b)"),
        ("a <= b", "(cmp a <= b)"),
        ("a > b", "(cmp a > b)"),
        ("a >= b", "(cmp a >= b)"),
        ("a is None", "(cmp a is None)"),
        ("a is not None", "(cmp a is not None)"),
        ("a in b", "(cmp a in b)"),
        ("a not in b", "(cmp a not in b)"),
    ];
    for (src, expected) in cases {
        assert_eq!(expr(src), expected, "while parsing {src:?}");
    }
}

#[test]
fn comparison_chains_parse_and_are_left_for_sema() {
    // DESIGN 3.6: chains are an M3 feature; the parser accepts them so sema can
    // reject them with a proper diagnostic.
    assert_eq!(expr("a < b < c"), "(cmp a < b < c)");
    assert_eq!(expr("a < b <= c == d"), "(cmp a < b <= c == d)");
}

#[test]
fn is_not_spans_both_keywords() {
    // `is not` is one operator whose span covers both keywords.
    insta::assert_snapshot!(expr_dump("a is not None"), @r"
    Compare @0..13
      left Name `a` @0..1
      cmp `is not` @2..8
        None @9..13
    ");
}

#[test]
fn literals() {
    assert_eq!(expr("42"), "42");
    assert_eq!(expr("1_000_000"), "1000000");
    assert_eq!(expr("0xff"), "255");
    assert_eq!(expr("0b1010"), "10");
    assert_eq!(expr("0o17"), "15");
    assert_eq!(expr("3.14"), "3.14");
    assert_eq!(expr("1e3"), "1000.0");
    assert_eq!(expr("1.5e-3"), "0.0015");
    assert_eq!(expr("\"hi\""), "\"hi\"");
    assert_eq!(expr("'hi'"), "\"hi\"");
    assert_eq!(expr("\"a\\nb\""), "\"a\\nb\"");
    assert_eq!(expr("True"), "True");
    assert_eq!(expr("False"), "False");
    assert_eq!(expr("None"), "None");
    assert_eq!(expr("x"), "x");
}

#[test]
fn containers() {
    assert_eq!(expr("[]"), "(list)");
    assert_eq!(expr("[1, 2]"), "(list 1 2)");
    assert_eq!(expr("[1, 2,]"), "(list 1 2)");
    assert_eq!(expr("{}"), "(dict)");
    assert_eq!(expr("{\"a\": 1}"), "(dict \"a\":1)");
    assert_eq!(expr("{\"a\": 1, \"b\": 2,}"), "(dict \"a\":1 \"b\":2)");
    assert_eq!(expr("{1, 2}"), "(set 1 2)");
    assert_eq!(expr("{1,}"), "(set 1)");
    assert_eq!(expr("()"), "(tuple)");
    assert_eq!(expr("(1, \"a\")"), "(tuple 1 \"a\")");
    assert_eq!(expr("(1,)"), "(tuple 1)");
    assert_eq!(expr("[[1], [2, 3]]"), "(list (list 1) (list 2 3))");
}

#[test]
fn containers_may_span_lines() {
    // Inside brackets newlines are implicit continuations (DESIGN 3.1).
    assert_eq!(expr("[\n    1,\n    2,\n]"), "(list 1 2)");
    assert_eq!(expr("{\n  \"a\": 1,\n}"), "(dict \"a\":1)");
}

#[test]
fn calls_indexes_and_attributes() {
    assert_eq!(expr("f()"), "(call f)");
    assert_eq!(expr("f(1, 2)"), "(call f 1 2)");
    assert_eq!(expr("f(1,)"), "(call f 1)");
    assert_eq!(expr("f(a=1)"), "(call f a=1)");
    assert_eq!(expr("f(1, b=2)"), "(call f 1 b=2)");
    assert_eq!(expr("f(a=1, b=2,)"), "(call f a=1 b=2)");
    assert_eq!(expr("f(g(1))"), "(call f (call g 1))");
    assert_eq!(expr("f(1)(2)"), "(call (call f 1) 2)");
    assert_eq!(expr("xs[0]"), "(index xs 0)");
    assert_eq!(expr("xs[0][1]"), "(index (index xs 0) 1)");
    assert_eq!(expr("xs[i + 1]"), "(index xs (+ i 1))");
    assert_eq!(expr("p.x"), "(. p x)");
    assert_eq!(expr("a.b.c"), "(. (. a b) c)");
    assert_eq!(expr("a.b(c).d"), "(. (call (. a b) c) d)");
    assert_eq!(
        expr("self.items.append(v)"),
        "(call (. (. self items) append) v)"
    );
    assert_eq!(
        expr("\"a b\".split(\" \")"),
        "(call (. \"a b\" split) \" \")"
    );
}

#[test]
fn generic_calls_use_the_csharp_rule() {
    // DESIGN 3.9 point 3.
    assert_eq!(expr("max<float>(2.5, 1.5)"), "(call max<float> 2.5 1.5)");
    assert_eq!(expr("f<int>(1)"), "(call f<int> 1)");
    assert_eq!(expr("f<int, str>(1)"), "(call f<int,str> 1)");
    assert_eq!(expr("f<list<int>>(xs)"), "(call f<list<int>> xs)");
    assert_eq!(
        expr("f<dict<str, list<int>>>(d)"),
        "(call f<dict<str,list<int>>> d)"
    );
    assert_eq!(expr("obj.method<int>(x)"), "(call (. obj method)<int> x)");
    // `a < b > (c)` is a generic call, per the documented consequence of the rule.
    assert_eq!(expr("a < b > (c)"), "(call a<b> c)");
    // ... and the way to get a comparison is to parenthesise it.
    assert_eq!(expr("(a < b) > c"), "(cmp (cmp a < b) > c)");
    assert_eq!(
        expr("f<int>(1) + g<str>(\"a\")"),
        "(+ (call f<int> 1) (call g<str> \"a\"))"
    );
}

#[test]
fn less_than_stays_a_comparison_when_the_rule_does_not_apply() {
    assert_eq!(expr("x<y"), "(cmp x < y)");
    assert_eq!(expr("a < b and c > d"), "(and (cmp a < b) (cmp c > d))");
    assert_eq!(expr("a < b > c"), "(cmp a < b > c)");
    assert_eq!(expr("f < int > 1"), "(cmp f < int > 1)");
    assert_eq!(expr("a < b + c"), "(cmp a < (+ b c))");
    // `1 < 2` never even reaches the speculative path: the callee is not a name.
    assert_eq!(expr("1 < 2"), "(cmp 1 < 2)");
    // A speculative parse that fails must not consume tokens.
    assert_eq!(expr("a < b >> c"), "(cmp a < (>> b c))");
}

#[test]
fn shift_operators_still_work_after_generic_splitting() {
    assert_eq!(expr("a >> b"), "(>> a b)");
    assert_eq!(expr("a << b"), "(<< a b)");
    assert_eq!(expr("1 << 2 >> 3"), "(>> (<< 1 2) 3)");
}

#[test]
fn f_strings() {
    assert_eq!(expr("f\"hello\""), "(fstr \"hello\")");
    assert_eq!(expr("f\"x={x}\""), "(fstr \"x=\" {x})");
    assert_eq!(expr("f\"{x:.2f}\""), "(fstr {x:.2f})");
    assert_eq!(expr("f\"{{literal}}\""), "(fstr \"{literal}\")");
    assert_eq!(expr("f\"a{x}b{y}c\""), "(fstr \"a\" {x} \"b\" {y} \"c\")");
    assert_eq!(
        expr("f\"{p.dist():.3f}\""),
        "(fstr {(call (. p dist)):.3f})"
    );
    assert_eq!(expr("f\"{a + b}\""), "(fstr {(+ a b)})");
    assert_eq!(
        expr("f\"{energy(a, b):.6f}\""),
        "(fstr {(call energy a b):.6f})"
    );
}

#[test]
fn f_string_holes_keep_the_spans_of_the_original_file() {
    // The sub-parsed expression must point at the source inside the braces.
    insta::assert_snapshot!(expr_dump("f\"x={total + 1:.2f}\""), @r#"
    FString @0..20
      literal "x=" @2..4
      hole Hole @4..19
        Binary + @5..14
          op Op @11..12
          lhs Name `total` @5..10
          rhs Int 1 @13..14
        spec ".2f" @15..18
    "#);
}

#[test]
fn unary_chains() {
    assert_eq!(expr("--a"), "(- (- a))");
    assert_eq!(expr("not not a"), "(not (not a))");
    assert_eq!(expr("-~a"), "(- (~ a))");
}

#[test]
fn trailing_tokens_after_an_expression_are_reported() {
    let (_, messages) = expr_with_diags("1 + 2 3");
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].contains("expected end of input"),
        "{messages:?}"
    );
}

#[test]
fn missing_operand_is_reported_once() {
    let (tree, messages) = expr_with_diags("1 +");
    assert_eq!(tree, "(+ 1 <error>)");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("expected an expression"),
        "{messages:?}"
    );
}

/// `i64::MIN` is only writable with the sign glued to the literal, which the
/// lexer folds; the parser must see one negative literal, not a negation.
#[test]
fn the_smallest_int_is_a_literal() {
    assert_eq!(expr("-9223372036854775808"), "-9223372036854775808");
    assert_eq!(expr("- 5"), "(- 5)");
    assert_eq!(expr("-2 ** 2"), "(- (** 2 2))");
}
