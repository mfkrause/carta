//! Engine unit tests: every construct, pipe, and whitespace rule, pinned to the documented behavior.

use std::collections::BTreeMap;

use super::Value;
use super::node::Template;

fn no_partials(_: &str) -> Option<String> {
    None
}

/// Render `src` against `ctx` with no partials.
fn render(src: &str, ctx: &Value) -> String {
    Template::parse(src)
        .expect("template should parse")
        .render(ctx, &no_partials)
}

/// Render with an in-memory partial set.
fn render_with(src: &str, ctx: &Value, partials: &[(&str, &str)]) -> String {
    let owned: Vec<(String, String)> = partials
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let resolve = |name: &str| {
        owned
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    Template::parse(src)
        .expect("template should parse")
        .render(ctx, &resolve)
}

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn map(entries: &[(&str, Value)]) -> Value {
    let mut m = BTreeMap::new();
    for (k, v) in entries {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Map(m)
}

fn list(items: &[Value]) -> Value {
    Value::List(items.to_vec())
}

#[test]
fn literal_and_escaped_dollar() {
    assert_eq!(render("a $$ b", &map(&[])), "a $ b");
    assert_eq!(render("$$body$$", &map(&[])), "$body$");
}

#[test]
fn variable_present_and_absent() {
    let ctx = map(&[("x", s("hi"))]);
    assert_eq!(render("[$x$]", &ctx), "[hi]");
    assert_eq!(render("[$missing$]", &ctx), "[]");
}

#[test]
fn nested_field_walks_maps() {
    let ctx = map(&[("a", map(&[("b", s("deep"))]))]);
    assert_eq!(render("$a.b$", &ctx), "deep");
    assert_eq!(render("$a.missing$", &ctx), "");
    assert_eq!(render("$a.b.c$", &ctx), "");
}

#[test]
fn bool_renders_and_is_conditionally_falsy() {
    assert_eq!(render("$x$", &map(&[("x", Value::Bool(true))])), "true");
    assert_eq!(render("$x$", &map(&[("x", Value::Bool(false))])), "false");
    assert_eq!(
        render("$if(x)$Y$else$N$endif$", &map(&[("x", Value::Bool(false))])),
        "N"
    );
    assert_eq!(
        render("$if(x)$Y$else$N$endif$", &map(&[("x", Value::Bool(true))])),
        "Y"
    );
}

#[test]
fn truthiness_of_empty_values() {
    let falsy = "$if(x)$Y$else$N$endif$";
    assert_eq!(render(falsy, &map(&[])), "N"); // absent
    assert_eq!(render(falsy, &map(&[("x", s(""))])), "N"); // empty string
    assert_eq!(render(falsy, &map(&[("x", list(&[]))])), "N"); // empty list
    assert_eq!(render(falsy, &map(&[("x", s("v"))])), "Y");
    assert_eq!(render(falsy, &map(&[("x", list(&[s("v")]))])), "Y");
}

#[test]
fn elseif_chain() {
    let t = "$if(a)$A$elseif(b)$B$elseif(c)$C$else$none$endif$";
    assert_eq!(render(t, &map(&[("b", Value::Bool(true))])), "B");
    assert_eq!(render(t, &map(&[("c", Value::Bool(true))])), "C");
    assert_eq!(render(t, &map(&[])), "none");
    assert_eq!(render(t, &map(&[("a", s("x"))])), "A");
}

#[test]
fn for_loop_and_separator() {
    let ctx = map(&[("xs", list(&[s("a"), s("b"), s("c")]))]);
    assert_eq!(render("$for(xs)$$it$$sep$, $endfor$", &ctx), "a, b, c");
    assert_eq!(render("$for(xs)$$xs$$endfor$", &ctx), "abc"); // bound name == current item
}

#[test]
fn direct_list_interpolation_has_no_separator() {
    let ctx = map(&[("xs", list(&[s("a"), s("b")]))]);
    assert_eq!(render("$xs$", &ctx), "ab");
}

#[test]
fn for_scalar_is_single_element() {
    let ctx = map(&[("x", s("solo"))]);
    assert_eq!(render("$for(x)$[$it$]$endfor$", &ctx), "[solo]");
}

#[test]
fn for_over_list_of_maps() {
    let ctx = map(&[(
        "people",
        list(&[
            map(&[("name", s("Ann")), ("email", s("ann@x"))]),
            map(&[("name", s("Bob")), ("email", s("bob@y"))]),
        ]),
    )]);
    assert_eq!(
        render("$for(people)$$it.name$ <$it.email$> $endfor$", &ctx),
        "Ann <ann@x> Bob <bob@y> "
    );
}

#[test]
fn nested_for_rebinds_it_to_inner() {
    let ctx = map(&[(
        "groups",
        list(&[
            map(&[("name", s("g1")), ("items", list(&[s("a"), s("b")]))]),
            map(&[("name", s("g2")), ("items", list(&[s("c")]))]),
        ]),
    )]);
    let t = "$for(groups)$[$it.name$:$for(it.items)$$it$,$endfor$]$endfor$";
    assert_eq!(render(t, &ctx), "[g1:a,b,][g2:c,]");
}

#[test]
fn for_pairs_is_key_sorted() {
    let ctx = map(&[("m", map(&[("z", s("1")), ("a", s("2")), ("m", s("3"))]))]);
    assert_eq!(
        render("$for(m/pairs)$$it.key$=$it.value$ $endfor$", &ctx),
        "a=2 m=3 z=1 "
    );
}

#[test]
fn pipes_string_case() {
    let ctx = map(&[("x", s("Hello World"))]);
    assert_eq!(render("$x/uppercase$", &ctx), "HELLO WORLD");
    assert_eq!(render("$x/lowercase$", &ctx), "hello world");
    assert_eq!(render("$x/uppercase/reverse$", &ctx), "DLROW OLLEH");
}

#[test]
fn pipes_length_and_list_ops() {
    let ctx = map(&[("xs", list(&[s("a"), s("b"), s("c")])), ("w", s("hello"))]);
    assert_eq!(render("$xs/length$", &ctx), "3");
    assert_eq!(render("$w/length$", &ctx), "5");
    assert_eq!(render("$xs/first$", &ctx), "a");
    assert_eq!(render("$xs/last$", &ctx), "c");
    assert_eq!(render("$xs/rest$", &ctx), "bc");
    assert_eq!(render("$xs/allbutlast$", &ctx), "ab");
    assert_eq!(render("$w/reverse$", &ctx), "olleh");
}

#[test]
fn list_selection_pipes_pass_strings_through() {
    // `first`/`last`/`rest`/`allbutlast` act on lists only; a string is returned unchanged.
    let ctx = map(&[("w", s("hello"))]);
    assert_eq!(render("$w/first$", &ctx), "hello");
    assert_eq!(render("$w/last$", &ctx), "hello");
    assert_eq!(render("$w/rest$", &ctx), "hello");
    assert_eq!(render("$w/allbutlast$", &ctx), "hello");
    // An empty list selects to the empty string.
    assert_eq!(render("[$e/first$]", &map(&[("e", list(&[]))])), "[]");
}

#[test]
fn alpha_is_single_letter_cyclic() {
    let a = |n: &str| render("$n/alpha$", &map(&[("n", s(n))]));
    assert_eq!(a("1"), "a");
    assert_eq!(a("3"), "c");
    assert_eq!(a("25"), "y");
    assert_eq!(a("26"), "`"); // cycle boundary lands just before 'a'
    assert_eq!(a("27"), "a");
    assert_eq!(a("0"), "`");
    assert_eq!(a("-1"), "-1"); // negative passes through
    assert_eq!(a("abc"), "abc"); // non-integer passes through
}

#[test]
fn roman_numbers_and_passthrough() {
    let r = |n: &str| render("$n/roman$", &map(&[("n", s(n))]));
    assert_eq!(r("1"), "i");
    assert_eq!(r("4"), "iv");
    assert_eq!(r("2024"), "mmxxiv");
    assert_eq!(r("3999"), "mmmcmxcix");
    assert_eq!(r("0"), ""); // zero is empty
    assert_eq!(r("-1"), "-1"); // negative passes through
    assert_eq!(r("abc"), "abc"); // non-integer passes through
}

#[test]
fn pipes_chomp_and_nowrap() {
    assert_eq!(render("$x/chomp$", &map(&[("x", s("line\n\n"))])), "line");
    assert_eq!(render("$x/nowrap$", &map(&[("x", s("a b"))])), "a b");
}

#[test]
fn pipes_block_padding() {
    let ctx = map(&[("x", s("Hello World"))]);
    assert_eq!(
        render(r#"$x/left 20 "[" "]"$"#, &ctx),
        "[Hello World         ]"
    );
    assert_eq!(
        render(r#"$x/right 20 "[" "]"$"#, &ctx),
        "[         Hello World]"
    );
    assert_eq!(render(r#"$x/center 13 "[" "]"$"#, &ctx), "[ Hello World ]");
    // No right border: trailing pad dropped.
    assert_eq!(render("[$x/left 20$]", &ctx), "[Hello World]");
}

#[test]
fn partial_plain_and_mapped() {
    let ctx = map(&[
        ("who", s("World")),
        ("names", list(&[s("x"), s("y"), s("z")])),
    ]);
    assert_eq!(
        render_with("$greet()$", &ctx, &[("greet", "Hi $who$")]),
        "Hi World"
    );
    assert_eq!(
        render_with("$names:item()[, ]$", &ctx, &[("item", "($it$)")]),
        "(x), (y), (z)"
    );
}

#[test]
fn missing_partial_renders_empty() {
    assert_eq!(render_with("[$gone()$]", &map(&[]), &[]), "[]");
}

// --- Whitespace and standalone-line rules ---

#[test]
fn comment_to_end_of_line() {
    // Column-zero comment swallows its newline; an indented one keeps the space and newline.
    assert_eq!(render("X\n$-- c\nY\n", &map(&[])), "X\nY\n");
    assert_eq!(render("X\n $-- c\nY\n", &map(&[])), "X\n \nY\n");
    assert_eq!(render("X $-- c\nY\n", &map(&[])), "X \nY\n");
}

#[test]
fn standalone_control_directive_consumes_its_line() {
    let t = "START\n$if(a)$\nLINE-A\n$endif$\nEND\n";
    assert_eq!(
        render(t, &map(&[("a", Value::Bool(true))])),
        "START\nLINE-A\nEND\n"
    );
    assert_eq!(render(t, &map(&[])), "START\nEND\n");
}

#[test]
fn non_standalone_control_keeps_line() {
    // An opening directive sharing its line with content makes the whole construct inline: no
    // directive of it strips its line, so the `$endif$`'s own newline survives as a blank line.
    assert_eq!(
        render("$if(a)$X\n$endif$\n", &map(&[("a", Value::Bool(true))])),
        "X\n\n"
    );
    let ctx = map(&[("xs", list(&[s("a"), s("b"), s("c")]))]);
    let t = "$for(xs)$\n$if(it)$[$it$]$endif$\n$endfor$\n";
    assert_eq!(render(t, &ctx), "[a]\n[b]\n[c]\n");
}

#[test]
fn block_ness_is_decided_by_the_opening_directive() {
    let ctx = map(&[("xs", list(&[s("a"), s("b")]))]);
    // Opening shares its line (inline construct): the lone `$endfor$` keeps its newline, so a blank
    // line trails the loop even though the closing sits by itself.
    assert_eq!(
        render("$for(xs)$- $it$\n$endfor$\nZ\n", &ctx),
        "- a\n- b\n\nZ\n"
    );
    // Opening ends its line (block construct): both the opening and the lone closing are stripped,
    // leaving no blank — a leading literal before the opening is preserved.
    assert_eq!(
        render("P$for(xs)$\n- $it$\n$endfor$\nZ\n", &ctx),
        "P- a\n- b\nZ\n"
    );
}

#[test]
fn for_body_literal_indent_repeats() {
    let ctx = map(&[("xs", list(&[s("a"), s("b"), s("c")]))]);
    let t = "$for(xs)$\n  - $it$\n$endfor$\n";
    assert_eq!(render(t, &ctx), "  - a\n  - b\n  - c\n");
}

#[test]
fn space_prefixed_variable_indents_continuations() {
    let ctx = map(&[("body", s("<p>p1</p>\n<p>p2</p>"))]);
    assert_eq!(
        render(
            "$if(a)$\n  $body$\n$endif$\n",
            &map_merge(&ctx, "a", Value::Bool(true))
        ),
        "  <p>p1</p>\n  <p>p2</p>\n"
    );
}

#[test]
fn non_space_prefix_suppresses_indent() {
    let ctx = map(&[("body", s("<p>p1</p>\n<p>p2</p>"))]);
    // Non-whitespace prefix: continuation lines are not indented.
    assert_eq!(render("XY: $body$\n", &ctx), "XY: <p>p1</p>\n<p>p2</p>\n");
    // Tab prefix: not all spaces, so no indent.
    assert_eq!(render("\t$body$\n", &ctx), "\t<p>p1</p>\n<p>p2</p>\n");
}

#[test]
fn output_is_verbatim_no_added_newline() {
    assert_eq!(render("no newline", &map(&[])), "no newline");
    assert_eq!(render("one newline\n", &map(&[])), "one newline\n");
}

/// Build a copy of `base` (a map) with one extra key.
fn map_merge(base: &Value, key: &str, value: Value) -> Value {
    let Value::Map(m) = base else {
        return base.clone();
    };
    let mut m = m.clone();
    m.insert(key.to_string(), value);
    Value::Map(m)
}

#[test]
fn parse_errors_are_reported() {
    assert!(Template::parse("$if(a)$ no end").is_err());
    assert!(Template::parse("$for(a)$ no end").is_err());
    assert!(Template::parse("$endif$").is_err());
    assert!(Template::parse("$x/boguspipe$").is_err());
    assert!(Template::parse("$unterminated").is_err());
}
