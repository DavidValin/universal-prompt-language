// Tests for the prompt builder: pure rendering, integration rendering, and
// build_from_json.
//
// These exercise `PromptBuilder::render` and `PromptBuilder::build_from_json`
// over pre-parsed templates and full UPL documents. They cover the constructs
// defined in upl-spec/upl-1.0-rfc.md §4: placeholders, ternaries, for-loops
// and if-blocks, plus the operators in §5.

use std::collections::HashMap;

use universal_prompt_language::upl::builder::{BuilderError, PromptBuilder, ValueMap};
use universal_prompt_language::upl::parser::{
    ObjectMap, Prompt, PromptParseError, PromptParser, Template, VariableDefinitions,
    VariableValue,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn prompt_with(body: &str) -> Prompt {
    let template = Template::parse(body).expect("template body should parse");
    Prompt {
        name: String::new(),
        title: None,
        desc: None,
        source: None,
        prompt: body.to_string(),
        template,
        variable_definitions: VariableDefinitions::new(),
        variable_defaults: HashMap::new(),
    }
}

fn render_str(body: &str, values: &[(&str, VariableValue)]) -> String {
    let map: ValueMap = values
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    PromptBuilder::new(prompt_with(body)).render(&map).unwrap()
}

fn parse(upl: &str) -> Prompt {
    PromptParser::parse(upl).expect("UPL should parse")
}

fn render(upl: &str, values: ValueMap) -> String {
    let prompt = parse(upl);
    PromptBuilder::new(prompt)
        .render(&values)
        .expect("render should succeed")
}

fn vmap(pairs: &[(&str, VariableValue)]) -> ValueMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn build(upl: &str, json: &str) -> Result<String, BuilderError> {
    let prompt = parse(upl);
    PromptBuilder::new(prompt).build_from_json(json)
}

// ---------------------------------------------------------------------------
// Pure rendering unit tests (originally in builder.rs)
// ---------------------------------------------------------------------------

#[test]
fn test_simple_placeholder() {
    let out = render_str("Hello, [[[NAME]]]!", &[("name", VariableValue::String("Alice".into()))]);
    assert_eq!(out, "Hello, Alice!");
}

#[test]
fn test_case_insensitive_placeholder() {
    // Placeholders MUST be uppercase (§4.1); lookup against the declared
    // (lowercase/mixed-case) variable name is case-insensitive.
    let out = render_str("[[[USERNAME]]] vs [[[USERNAME]]]", &[("Username", VariableValue::String("bob".into()))]);
    assert_eq!(out, "bob vs bob");
}

#[test]
fn test_number_and_boolean_placeholders() {
    let out = render_str(
        "size=[[[SIZE]]], flag=[[[FLAG]]]",
        &[
            ("size", VariableValue::Number(42.0)),
            ("flag", VariableValue::Boolean(true)),
        ],
    );
    assert_eq!(out, "size=42, flag=true");
}

#[test]
fn test_number_rendering_edge_cases() {
    let body = "[[[N]]]";
    assert_eq!(render_str(body, &[("n", VariableValue::Number(80.0))]), "80");
    assert_eq!(render_str(body, &[("n", VariableValue::Number(-0.0))]), "0");
    assert_eq!(render_str(body, &[("n", VariableValue::Number(3.14))]), "3.14");
    assert_eq!(render_str(body, &[("n", VariableValue::Number(-470.0))]), "-470");
    // Large integers must not saturate to i64::MAX.
    assert_eq!(
        render_str(body, &[("n", VariableValue::Number(1e20))]),
        "100000000000000000000"
    );
    assert_eq!(
        render_str(body, &[("n", VariableValue::Number(-1e20))]),
        "-100000000000000000000"
    );
}

#[test]
fn test_nested_object_placeholder() {
    let mut obj = ObjectMap::new();
    let mut auth = ObjectMap::new();
    auth.insert("type".into(), VariableValue::String("bearer".into()));
    auth.insert("token".into(), VariableValue::String("sekret".into()));
    obj.insert("base_url".into(), VariableValue::String("https://api.example.com".into()));
    obj.insert("auth".into(), VariableValue::Object(auth));
    let out = render_str(
        "url=[[[API_CONFIG.BASE_URL]]] auth=[[[API_CONFIG.AUTH.TYPE]]]",
        &[("api_config", VariableValue::Object(obj))],
    );
    assert_eq!(out, "url=https://api.example.com auth=bearer");
}

#[test]
fn test_ternary_true_branch() {
    let out = render_str(
        "{{{AGE >= 18 ? \"adult\" : \"minor\"}}}",
        &[("age", VariableValue::Number(21.0))],
    );
    assert_eq!(out, "adult");
}

#[test]
fn test_ternary_false_branch() {
    let out = render_str(
        "{{{AGE >= 18 ? \"adult\" : \"minor\"}}}",
        &[("age", VariableValue::Number(12.0))],
    );
    assert_eq!(out, "minor");
}

#[test]
fn test_ternary_bare_bool_cond() {
    let out = render_str(
        "{{{USE_ASYNC ? \"async\" : \"sync\"}}}",
        &[("use_async", VariableValue::Boolean(false))],
    );
    assert_eq!(out, "sync");
}

#[test]
fn test_if_block_truthy() {
    let out = render_str(
        "start\n{{{if INCLUDE_AUTH}}}\nAuth required\n{{{end if}}}\nend",
        &[("include_auth", VariableValue::Boolean(true))],
    );
    assert_eq!(out, "start\nAuth required\nend");
}

#[test]
fn test_if_block_falsy() {
    let out = render_str(
        "start\n{{{if INCLUDE_AUTH}}}\nAuth required\n{{{end if}}}\nend",
        &[("include_auth", VariableValue::Boolean(false))],
    );
    assert_eq!(out, "start\nend");
}

#[test]
fn test_if_block_with_comparison() {
    let body = "{{{if BODY != \"{}\"}}}has body{{{end if}}}";
    let with_body = render_str(body, &[("body", VariableValue::String("{\"x\":1}".into()))]);
    assert_eq!(with_body, "has body");
    let empty = render_str(body, &[("body", VariableValue::String("{}".into()))]);
    assert_eq!(empty, "");
}

#[test]
fn test_for_loop_over_strings() {
    let body = "{{{for ITEM in ITEMS}}}- [[[ITEM]]]\n{{{end for}}}";
    let out = render_str(
        body,
        &[(
            "items",
            VariableValue::List(vec![
                VariableValue::String("a".into()),
                VariableValue::String("b".into()),
                VariableValue::String("c".into()),
            ]),
        )],
    );
    assert_eq!(out, "- a\n- b\n- c\n");
}

#[test]
fn test_for_loop_over_objects() {
    let body = "{{{for ENDPOINT in ENDPOINTS}}}- [[[ENDPOINT.METHOD]]] [[[ENDPOINT.PATH]]]\n{{{end for}}}";
    let mut e1 = ObjectMap::new();
    e1.insert("method".into(), VariableValue::String("GET".into()));
    e1.insert("path".into(), VariableValue::String("/users".into()));
    let mut e2 = ObjectMap::new();
    e2.insert("method".into(), VariableValue::String("POST".into()));
    e2.insert("path".into(), VariableValue::String("/users".into()));
    let out = render_str(
        body,
        &[(
            "endpoints",
            VariableValue::List(vec![
                VariableValue::Object(e1),
                VariableValue::Object(e2),
            ]),
        )],
    );
    assert_eq!(out, "- GET /users\n- POST /users\n");
}

#[test]
fn test_loop_with_nested_if() {
    let body = "{{{for ENDPOINT in ENDPOINTS}}}- [[[ENDPOINT.PATH]]]\n{{{if ENDPOINT.BODY != \"{}\"}}}\n  has body\n{{{end if}}}{{{end for}}}";
    let mut e1 = ObjectMap::new();
    e1.insert("path".into(), VariableValue::String("/a".into()));
    e1.insert("body".into(), VariableValue::String("{}".into()));
    let mut e2 = ObjectMap::new();
    e2.insert("path".into(), VariableValue::String("/b".into()));
    e2.insert("body".into(), VariableValue::String("{\"x\":1}".into()));
    let out = render_str(
        body,
        &[(
            "endpoints",
            VariableValue::List(vec![
                VariableValue::Object(e1),
                VariableValue::Object(e2),
            ]),
        )],
    );
    assert_eq!(out, "- /a\n- /b\n  has body\n");
}

#[test]
fn test_not_operator() {
    let body = "{{{if !FLAG}}}off{{{end if}}}";
    let off = render_str(body, &[("flag", VariableValue::Boolean(false))]);
    assert_eq!(off, "off");
    let on = render_str(body, &[("flag", VariableValue::Boolean(true))]);
    assert_eq!(on, "");
}

#[test]
fn test_string_operators() {
    assert_eq!(
        render_str("{{{TEXT contains \"hello\" ? \"yes\" : \"no\"}}}", &[("text", VariableValue::String("say hello world".into()))]),
        "yes"
    );
    assert_eq!(
        render_str("{{{PATH starts_with \"/home\" ? \"yes\" : \"no\"}}}", &[("path", VariableValue::String("/home/me".into()))]),
        "yes"
    );
    assert_eq!(
        render_str("{{{EXT ends_with \".js\" ? \"yes\" : \"no\"}}}", &[("ext", VariableValue::String("app.ts".into()))]),
        "no"
    );
}

#[test]
fn test_contains_list_membership_requires_list_on_left() {
    // §5: `contains` performs list membership testing only when the LEFT
    // operand is a list — `TAGS contains "api"`, never `"api" contains
    // TAGS`. The list-on-right form must fail with a type error rather
    // than silently doing something else.
    let tags = VariableValue::List(vec![
        VariableValue::String("api".into()),
        VariableValue::String("web".into()),
    ]);
    assert_eq!(
        render_str(
            "{{{TAGS contains \"api\" ? \"yes\" : \"no\"}}}",
            &[("tags", tags.clone())]
        ),
        "yes"
    );

    let mut m: ValueMap = HashMap::new();
    m.insert("tags".to_string(), tags);
    let res = PromptBuilder::new(prompt_with(
        "{{{\"api\" contains TAGS ? \"yes\" : \"no\"}}}",
    ))
    .render(&m);
    assert!(matches!(res, Err(BuilderError::TypeError(_))), "{:?}", res);
}

#[test]
fn test_contains_list_membership_element_may_be_any_scalar_or_object() {
    // §5: the right operand of `contains` (when the left is a list) may be
    // any single element — string, number, boolean, or object, compared by
    // value — but never itself a list.
    let mut plato = ObjectMap::new();
    plato.insert("name".into(), VariableValue::String("Plato".into()));
    plato.insert("era".into(), VariableValue::Number(-428.0));
    let mut aristotle = ObjectMap::new();
    aristotle.insert("name".into(), VariableValue::String("Aristotle".into()));
    aristotle.insert("era".into(), VariableValue::Number(-384.0));
    let philosophers = VariableValue::List(vec![
        VariableValue::Object(plato.clone()),
        VariableValue::Object(aristotle),
    ]);

    let mut needle_match = ObjectMap::new();
    needle_match.insert("name".into(), VariableValue::String("Plato".into()));
    needle_match.insert("era".into(), VariableValue::Number(-428.0));

    let mut needle_miss = ObjectMap::new();
    needle_miss.insert("name".into(), VariableValue::String("Socrates".into()));
    needle_miss.insert("era".into(), VariableValue::Number(-470.0));

    let body = "{{{PHILOSOPHERS contains NEEDLE ? \"yes\" : \"no\"}}}";
    assert_eq!(
        render_str(body, &[
            ("philosophers", philosophers.clone()),
            ("needle", VariableValue::Object(needle_match)),
        ]),
        "yes"
    );
    assert_eq!(
        render_str(body, &[
            ("philosophers", philosophers),
            ("needle", VariableValue::Object(needle_miss)),
        ]),
        "no"
    );
}

#[test]
fn test_contains_string_left_requires_string_right() {
    // §5: `contains` is overloaded on the LEFT operand's type. When the
    // left operand is a `string`, the right operand must also be a
    // string — a list on the right does NOT fall back to membership
    // testing (only a list on the LEFT triggers that form); it's a type
    // error, same as any other non-string right operand.
    let text = VariableValue::String("hello world".into());
    let items = VariableValue::List(vec![
        VariableValue::String("a".into()),
        VariableValue::String("b".into()),
    ]);
    let mut m: ValueMap = HashMap::new();
    m.insert("text".to_string(), text);
    m.insert("items".to_string(), items);
    let res =
        PromptBuilder::new(prompt_with("{{{TEXT contains ITEMS ? \"yes\" : \"no\"}}}")).render(&m);
    assert!(matches!(res, Err(BuilderError::TypeError(_))), "{:?}", res);
}

#[test]
fn test_contains_list_membership_rejects_list_element() {
    // The right operand must never itself be a list, even when the left
    // operand is a list too.
    let numbers = VariableValue::List(vec![VariableValue::Number(1.0), VariableValue::Number(2.0)]);
    let mut m: ValueMap = HashMap::new();
    m.insert("a".to_string(), numbers.clone());
    m.insert("b".to_string(), numbers);
    let res = PromptBuilder::new(prompt_with("{{{A contains B ? \"yes\" : \"no\"}}}")).render(&m);
    assert!(matches!(res, Err(BuilderError::TypeError(_))), "{:?}", res);
}

#[test]
fn test_equality_operators() {
    assert_eq!(
        render_str("{{{A = B ? \"eq\" : \"ne\"}}}", &[
            ("a", VariableValue::Number(5.0)),
            ("b", VariableValue::Number(5.0)),
        ]),
        "eq"
    );
    assert_eq!(
        render_str("{{{A = B ? \"eq\" : \"ne\"}}}", &[
            ("a", VariableValue::String("x".into())),
            ("b", VariableValue::String("y".into())),
        ]),
        "ne"
    );
}

#[test]
fn test_comparison_operators() {
    assert_eq!(
        render_str("{{{N > 10 ? \"big\" : \"small\"}}}", &[("n", VariableValue::Number(3.0))]),
        "small"
    );
    assert_eq!(
        render_str("{{{N <= 10 ? \"ok\" : \"no\"}}}", &[("n", VariableValue::Number(10.0))]),
        "ok"
    );
}

#[test]
fn test_operator_precedence_not_vs_comparison() {
    // a = b contains c  =>  (a = b) contains c  (since `=` binds tighter
    // than `contains` per §5.2). a="x", b="x" => (a=b) => Boolean(true);
    // Boolean contains "x" => type error.
    let body = "{{{A = B contains \"x\" ? \"yes\" : \"no\"}}}";
    // a="x", b="x" => (a=b) => true (Boolean) => Boolean contains "x" => type error
    let mut m: ValueMap = HashMap::new();
    m.insert("a".to_string(), VariableValue::String("x".into()));
    m.insert("b".to_string(), VariableValue::String("x".into()));
    let res = PromptBuilder::new(prompt_with(body)).render(&m);
    assert!(res.is_err());
}

// --- Parenthesized grouping and `and`/`or`/`not` (RFC §5.1, §5.2) ---

#[test]
fn test_parenthesized_condition_renders() {
    // E2 regression: parentheses previously always failed to parse.
    let body = "{{{(HOURS > 10) ? \"ample\" : \"limited\"}}}";
    assert_eq!(
        render_str(body, &[("hours", VariableValue::Number(12.0))]),
        "ample"
    );
    assert_eq!(
        render_str(body, &[("hours", VariableValue::Number(3.0))]),
        "limited"
    );
}

#[test]
fn test_and_operator_truthiness() {
    // `and`/`or` operate on truthiness (§4.6.2), not typed equality, so
    // operands of different kinds (number, string) combine freely.
    let body = "{{{N > 0 and S ? \"yes\" : \"no\"}}}";
    assert_eq!(
        render_str(body, &[
            ("n", VariableValue::Number(1.0)),
            ("s", VariableValue::String("hi".into())),
        ]),
        "yes"
    );
    assert_eq!(
        render_str(body, &[
            ("n", VariableValue::Number(1.0)),
            ("s", VariableValue::String("".into())),
        ]),
        "no"
    );
}

#[test]
fn test_or_operator_truthiness() {
    let body = "{{{TIER = \"pro\" or TIER = \"enterprise\" ? \"full\" : \"limited\"}}}";
    assert_eq!(
        render_str(body, &[("tier", VariableValue::String("enterprise".into()))]),
        "full"
    );
    assert_eq!(
        render_str(body, &[("tier", VariableValue::String("free".into()))]),
        "limited"
    );
}

#[test]
fn test_and_short_circuits_right_operand() {
    // When the left operand of `and` is falsy, the right operand must not
    // be evaluated — so a reference to an undeclared variable there is
    // safe rather than a MissingValue error.
    let body = "{{{HAS_TAGS and TAGS contains \"api\" ? \"yes\" : \"no\"}}}";
    assert_eq!(
        render_str(body, &[("has_tags", VariableValue::Boolean(false))]),
        "no"
    );
}

#[test]
fn test_or_short_circuits_right_operand() {
    // When the left operand of `or` is truthy, the right operand must not
    // be evaluated.
    let body = "{{{IS_ADMIN or PERMS contains \"write\" ? \"yes\" : \"no\"}}}";
    assert_eq!(
        render_str(body, &[("is_admin", VariableValue::Boolean(true))]),
        "yes"
    );
}

#[test]
fn test_not_keyword_binds_looser_than_bang() {
    // `not` (§5.1) binds to the whole comparison/string-op expression that
    // follows, unlike `!` which binds only to a single primary (§5.2).
    // not A contains "hello"  =>  not (A contains "hello")  =>  no type error.
    let not_body = "{{{not A contains \"hello\" ? \"yes\" : \"no\"}}}";
    assert_eq!(
        render_str(not_body, &[("a", VariableValue::String("hello world".into()))]),
        "no"
    );
    // !A contains "hello"  =>  (!A) contains "hello"  =>  Boolean contains
    // String is a type error.
    let bang_body = "{{{!A contains \"hello\" ? \"yes\" : \"no\"}}}";
    let mut m: ValueMap = HashMap::new();
    m.insert("a".to_string(), VariableValue::String("hello world".into()));
    let res = PromptBuilder::new(prompt_with(bang_body)).render(&m);
    assert!(res.is_err());
}

#[test]
fn test_parentheses_override_default_and_or_precedence() {
    // Default precedence: `and` binds tighter than `or`, so
    // `A or B and C` parses as `A or (B and C)`.
    let default_body = "{{{A or B and C ? \"T\" : \"F\"}}}";
    // Explicit grouping flips the result: `(A or B) and C`.
    let grouped_body = "{{{(A or B) and C ? \"T\" : \"F\"}}}";
    let values = [
        ("a", VariableValue::Boolean(true)),
        ("b", VariableValue::Boolean(false)),
        ("c", VariableValue::Boolean(false)),
    ];
    assert_eq!(render_str(default_body, &values), "T"); // true or (false and false) = true
    assert_eq!(render_str(grouped_body, &values), "F"); // (true or false) and false = false
}

#[test]
fn test_rfc_grouped_and_or_not_example() {
    // RFC §5.2 worked example, in an if-block.
    let body = "{{{if (TIER = \"pro\" or TIER = \"enterprise\") and not SUSPENDED}}}Full access enabled.{{{end if}}}";
    assert_eq!(
        render_str(body, &[
            ("tier", VariableValue::String("pro".into())),
            ("suspended", VariableValue::Boolean(false)),
        ]),
        "Full access enabled."
    );
    assert_eq!(
        render_str(body, &[
            ("tier", VariableValue::String("pro".into())),
            ("suspended", VariableValue::Boolean(true)),
        ]),
        ""
    );
    assert_eq!(
        render_str(body, &[
            ("tier", VariableValue::String("free".into())),
            ("suspended", VariableValue::Boolean(false)),
        ]),
        ""
    );
}

#[test]
fn test_safety_unmatched_braces_emitted_verbatim() {
    // A lone }}} with no preceding {{{ should pass through untouched.
    let out = render_str("code: foo}}}bar", &[]);
    assert_eq!(out, "code: foo}}}bar");
}

#[test]
fn test_escaped_delimiters_render_literally() {
    // RFC §4.5: `\{{{` and `\[[[` render as literal `{{{`/`[[[`, and the
    // rest of a matched-but-escaped group (e.g. the trailing `}}}`) is
    // then just plain text, needing no escape of its own.
    let out = render_str("Mustache: \\{{{value}}}", &[]);
    assert_eq!(out, "Mustache: {{{value}}}");

    let out = render_str("Literal: \\[[[ not a var ]]]", &[]);
    assert_eq!(out, "Literal: [[[ not a var ]]]");
}

#[test]
fn test_double_backslash_escapes_the_escape() {
    // `\\{{{` is a literal `\` (the first backslash escapes the second)
    // followed by an escaped `{{{` — not a real construct either.
    let out = render_str("\\\\{{{X}}}", &[]);
    assert_eq!(out, "\\{{{X}}}");
}

#[test]
fn test_escape_does_not_suppress_real_placeholder_later_on_line() {
    let out = render_str(
        "\\[[[ literal ]]] then [[[NAME]]]",
        &[("name", VariableValue::String("Ada".into()))],
    );
    assert_eq!(out, "[[[ literal ]]] then Ada");
}

#[test]
fn test_rest_client_example() {
    let body = r#"Please write a Node.js client that calls the following endpoints using fetch:

{{{for ENDPOINT in ENDPOINTS}}}
- [[[ENDPOINT.METHOD]]] [[[ENDPOINT.PATH]]] (body: [[[ENDPOINT.BODY]]])
{{{if INCLUDE_AUTH}}}
  Note: this endpoint must send an Authorization header.
{{{end if}}}
{{{if ENDPOINT.BODY != "{}"}}}
  Note: this endpoint expects a request body.
{{{end if}}}
{{{end for}}}

Explain how the client should handle errors and retries for each call.
"#;
    let mut e1 = ObjectMap::new();
    e1.insert("method".into(), VariableValue::String("GET".into()));
    e1.insert("path".into(), VariableValue::String("/users".into()));
    e1.insert("body".into(), VariableValue::String("{}".into()));
    let mut e2 = ObjectMap::new();
    e2.insert("method".into(), VariableValue::String("POST".into()));
    e2.insert("path".into(), VariableValue::String("/orders".into()));
    e2.insert("body".into(), VariableValue::String("{\"item\":\"x\"}".into()));
    let out = render_str(body, &[
        ("endpoints", VariableValue::List(vec![VariableValue::Object(e1), VariableValue::Object(e2)])),
        ("include_auth", VariableValue::Boolean(true)),
    ]);
    assert!(out.contains("- GET /users (body: {})"));
    assert!(out.contains("- POST /orders (body: {\"item\":\"x\"})"));
    // Auth note appears for both endpoints (include_auth is true).
    assert_eq!(out.matches("must send an Authorization header").count(), 2);
    // "expects a request body" only for the POST endpoint (body != "{}").
    assert_eq!(out.matches("expects a request body").count(), 1);
    assert!(out.contains("Note: this endpoint expects a request body."));
}

#[test]
fn test_block_tag_newline_trimming() {
    // RFC §4.7: exactly one newline is trimmed immediately after each of
    // `{{{for ...}}}`, `{{{end for}}}`, `{{{if ...}}}` and `{{{end if}}}`,
    // so a loop/if written on its own line doesn't leave a blank line
    // behind in the output.
    let body = "Servers:\n{{{for SERVER in SERVERS}}}\n- [[[SERVER]]]\n{{{end for}}}\nDone.\n";
    let out = render_str(body, &[(
        "servers",
        VariableValue::List(vec![
            VariableValue::String("a".into()),
            VariableValue::String("b".into()),
        ]),
    )]);
    assert_eq!(out, "Servers:\n- a\n- b\nDone.\n");
}

#[test]
fn test_ternary_and_placeholder_do_not_trim_newlines() {
    // Unlike `for`/`if` block tags, ternaries and placeholders consume no
    // surrounding whitespace at all (§4.7).
    let body = "a\n{{{FLAG ? \"yes\" : \"no\"}}}\nb\n[[[FLAG]]]\nc\n";
    let out = render_str(body, &[("flag", VariableValue::Boolean(true))]);
    assert_eq!(out, "a\nyes\nb\ntrue\nc\n");
}

// ---------------------------------------------------------------------------
// Integration rendering tests (originally in builder_render.rs)
// ---------------------------------------------------------------------------

#[test]
fn integration_simple_string_placeholder() {
    let upl = "\
--
name: hello
title: Hello
params:
  name:
    type: string
    def: \"guest\"
--
Hello, [[[NAME]]]!
--
";
    let out = render(upl, vmap(&[("name", VariableValue::String("Ada".into()))]));
    assert_eq!(out, "Hello, Ada!\n");
}

#[test]
fn integration_file_handling_example() {
    // Based on upl-spec/upl-1.0-rfc.md §8.2.
    let upl = "\
--
name: ask_file_handling
title: Ask for File Handling Advice
params:
  file_type:
    type: option_single
    opts:
      - \"js\"
      - \"json\"
      - \"html\"
    def: \"js\"
  file_size:
    type: number
    def: 100
  use_async:
    type: boolean
    def: true
--
I have a [[[FILE_TYPE]]] file of roughly [[[FILE_SIZE]]] KB.

{{{FILE_SIZE > 100 ? \"It is a large file, so memory usage matters.\" : \"It is a small file, so simplicity matters.\"}}}

{{{USE_ASYNC ? \"Please recommend an async/await approach.\" : \"Please recommend a synchronous approach.\"}}}

Describe the best way to read and process this file in Node.js, and explain why.
--
";
    let out = render(
        upl,
        vmap(&[
            ("file_type", VariableValue::String("json".into())),
            ("file_size", VariableValue::Number(250.0)),
            ("use_async", VariableValue::Boolean(true)),
        ]),
    );
    assert!(out.starts_with("I have a json file of roughly 250 KB.\n"));
    assert!(out.contains("It is a large file, so memory usage matters."));
    assert!(out.contains("Please recommend an async/await approach."));
}

#[test]
fn integration_api_config_nested_object() {
    // Based on upl-spec/upl-1.0-rfc.md §8.3 (trimmed).
    let upl = "\
--
name: ask_api_config_review
params:
  api_config:
    type: object
    ofields:
      base_url:
        type: string
        def: \"https://api.example.com\"
      timeout:
        type: number
        def: 30
      auth:
        type: object
        ofields:
          type:
            type: option_single
            opts:
              - \"bearer\"
              - \"basic\"
              - \"none\"
            def: \"bearer\"
          token:
            type: string
            def: \"\"
--
- Base URL: [[[API_CONFIG.BASE_URL]]]
- Timeout (seconds): [[[API_CONFIG.TIMEOUT]]]
- Auth type: [[[API_CONFIG.AUTH.TYPE]]]
- Auth token: [[[API_CONFIG.AUTH.TOKEN]]]
--
";

    let mut auth = ObjectMap::new();
    auth.insert("type".into(), VariableValue::String("basic".into()));
    auth.insert("token".into(), VariableValue::String("abc123".into()));
    let mut cfg = ObjectMap::new();
    cfg.insert("base_url".into(), VariableValue::String("https://x.test".into()));
    cfg.insert("timeout".into(), VariableValue::Number(5.0));
    cfg.insert("auth".into(), VariableValue::Object(auth));

    let out = render(upl, vmap(&[("api_config", VariableValue::Object(cfg))]));
    assert!(out.contains("- Base URL: https://x.test"));
    assert!(out.contains("- Timeout (seconds): 5"));
    assert!(out.contains("- Auth type: basic"));
    assert!(out.contains("- Auth token: abc123"));
}

#[test]
fn integration_rest_client_loop_and_if() {
    // Based on upl-spec/upl-1.0-rfc.md §8.1 (trimmed body).
    let upl = "\
--
name: ask_rest_client
params:
  endpoints:
    type: list
    etype: object
    ofields:
      method:
        type: option_single
        opts:
          - \"GET\"
          - \"POST\"
        def: \"GET\"
      path:
        type: string
        def: \"/api/users\"
      body:
        type: long_string
        def: \"{}\"
  include_auth:
    type: boolean
    def: true
--
{{{for ENDPOINT in ENDPOINTS}}}
- [[[ENDPOINT.METHOD]]] [[[ENDPOINT.PATH]]] (body: [[[ENDPOINT.BODY]]])
{{{if INCLUDE_AUTH}}}
  Note: send an Authorization header.
{{{end if}}}
{{{if ENDPOINT.BODY != \"{}\"}}}
  Note: expects a request body.
{{{end if}}}
{{{end for}}}
--
";

    let mut e1 = ObjectMap::new();
    e1.insert("method".into(), VariableValue::String("GET".into()));
    e1.insert("path".into(), VariableValue::String("/users".into()));
    e1.insert("body".into(), VariableValue::String("{}".into()));
    let mut e2 = ObjectMap::new();
    e2.insert("method".into(), VariableValue::String("POST".into()));
    e2.insert("path".into(), VariableValue::String("/orders".into()));
    e2.insert("body".into(), VariableValue::String("{\"item\":1}".into()));

    let out = render(
        upl,
        vmap(&[
            (
                "endpoints",
                VariableValue::List(vec![
                    VariableValue::Object(e1),
                    VariableValue::Object(e2),
                ]),
            ),
            ("include_auth", VariableValue::Boolean(true)),
        ]),
    );

    assert!(out.contains("- GET /users (body: {})"));
    assert!(out.contains("- POST /orders (body: {\"item\":1})"));
    // Auth note appears for both (include_auth is true).
    assert_eq!(out.matches("send an Authorization header").count(), 2);
    // Body note only for the second endpoint.
    assert_eq!(out.matches("expects a request body").count(), 1);
}

#[test]
fn integration_loop_disabled_auth() {
    let upl = "\
--
name: p
params:
  items:
    type: list
    etype: object
    ofields:
      path:
        type: string
  include_auth:
    type: boolean
    def: false
--
{{{for ENDPOINT in ITEMS}}}
- [[[ENDPOINT.PATH]]]
{{{if INCLUDE_AUTH}}}AUTH{{{end if}}}
{{{end for}}}
--
";
    let mut e1 = ObjectMap::new();
    e1.insert("path".into(), VariableValue::String("/a".into()));
    let mut e2 = ObjectMap::new();
    e2.insert("path".into(), VariableValue::String("/b".into()));
    let out = render(
        upl,
        vmap(&[
            (
                "items",
                VariableValue::List(vec![
                    VariableValue::Object(e1),
                    VariableValue::Object(e2),
                ]),
            ),
            ("include_auth", VariableValue::Boolean(false)),
        ]),
    );
    assert!(!out.contains("AUTH"));
    assert!(out.contains("- /a"));
    assert!(out.contains("- /b"));
}

#[test]
fn integration_missing_variable_is_error() {
    let upl = "\
--
name: p
params:
  name:
    type: string
--
Hi [[[NAME]]]
--
";
    let prompt = parse(upl);
    let res = PromptBuilder::new(prompt).render(&ValueMap::new());
    assert!(matches!(res, Err(universal_prompt_language::upl::builder::BuilderError::MissingValue(_))));
}

#[test]
fn integration_type_mismatch_in_comparison() {
    let upl = "\
--
name: p
params:
  n:
    type: number
  s:
    type: string
--
{{{N > S ? \"x\" : \"y\"}}}
--
";
    let prompt = parse(upl);
    let res = PromptBuilder::new(prompt).render(&vmap(&[
        ("n", VariableValue::Number(1.0)),
        ("s", VariableValue::String("a".into())),
    ]));
    assert!(matches!(res, Err(universal_prompt_language::upl::builder::BuilderError::TypeError(_))));
}

#[test]
fn integration_unmatched_loop_is_error() {
    let upl = "\
--
name: p
params: {}
--
{{{for X in X}}}
body
--
";
    // Unmatched constructs are now caught at parse time, before the builder
    // ever sees the body.
    let res = PromptParser::parse(upl);
    assert!(matches!(res, Err(PromptParseError::UnmatchedConstruct(_))));
}

#[test]
fn integration_code_snippet_with_lookalike_delimiters() {
    // Per §4.5, unmatched `[[[` sequences inside code must pass through verbatim.
    let upl = "\
--
name: p
params:
  lang:
    type: string
--
```[[[LANG]]]
const x = arr[0];
const y = [[1,2,3]];
```
--
";
    let out = render(upl, vmap(&[("lang", VariableValue::String("js".into()))]));
    assert!(out.contains("```js"));
    assert!(out.contains("const y = [[1,2,3]];"));
}

#[test]
fn integration_option_multi_value() {
    // An option_multi rendered directly as a placeholder joins its chosen
    // values with ", ".
    let upl = "\
--
name: p
params:
  tags:
    type: option_multi
    etype: string
    opts:
      - \"red\"
      - \"green\"
      - \"blue\"
--
Tags: [[[TAGS]]]
--
";
    let out = render(
        upl,
        vmap(&[(
            "tags",
            VariableValue::List(vec![
                VariableValue::String("red".into()),
                VariableValue::String("green".into()),
            ]),
        )]),
    );
    assert_eq!(out, "Tags: red, green\n");
}

#[test]
fn integration_list_projection() {
    // `[[[MODEL.FIELDS.NAME]]]` projects `name` across a list of objects,
    // producing a comma-joined list of names — useful for destructuring.
    let upl = "\
--
name: p
params:
  model:
    type: object
    ofields:
      fields:
        type: list
        etype: object
        ofields:
          name:
            type: string
          type:
            type: string
--
const { [[[MODEL.FIELDS.NAME]]] } = req.body;
--
";
    let mut f1 = ObjectMap::new();
    f1.insert("name".into(), VariableValue::String("username".into()));
    f1.insert("type".into(), VariableValue::String("string".into()));
    let mut f2 = ObjectMap::new();
    f2.insert("name".into(), VariableValue::String("email".into()));
    f2.insert("type".into(), VariableValue::String("string".into()));
    let mut model = ObjectMap::new();
    model.insert(
        "fields".into(),
        VariableValue::List(vec![VariableValue::Object(f1), VariableValue::Object(f2)]),
    );
    let out = render(upl, vmap(&[("model", VariableValue::Object(model))]));
    assert_eq!(out, "const { username, email } = req.body;\n");
}

#[test]
fn integration_render_with_defaults() {
    // `render_with_defaults` uses the declared `def:` values without a TUI.
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
  n:
    type: number
    def: 7
  flag:
    type: boolean
    def: true
--
Hello, [[[NAME]]]! n=[[[N]]] flag={{{FLAG ? \"on\" : \"off\"}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Hello, world! n=7 flag=on\n");
}

#[test]
fn integration_render_with_defaults_nested_object() {
    let upl = "\
--
name: p
params:
  api:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
--
host=[[[API.HOST]]] port=[[[API.PORT]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "host=localhost port=8080\n");
}

#[test]
fn integration_render_with_defaults_list_of_objects() {
    // A list with etype=object and no inline `def:` defaults to an empty
    // list (RFC §3: `def` optional, list falls back to `[]`); the loop body
    // renders nothing.
    let upl = "\
--
name: p
params:
  endpoints:
    type: list
    etype: object
    ofields:
      method:
        type: string
        def: \"GET\"
      path:
        type: string
        def: \"/health\"
--
{{{for E in ENDPOINTS}}}- [[[E.METHOD]]] [[[E.PATH]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    // No `def:` list → empty list → loop renders nothing.
    assert_eq!(out, "");
}

#[test]
fn integration_single_quoted_if_condition() {
    let upl = "\
--
name: single_quote_if
params:
  expected_tone:
    type: string
    def: 'tone'
--
{{{if EXPECTED_TONE == 'tone'}}}(tone expected){{{end if}}}
{{{if EXPECTED_TONE == 'academic'}}}(academic expected){{{end if}}}
--
";
    let out = render(upl, vmap(&[("expected_tone", VariableValue::String("tone".into()))]));
    assert!(out.contains("(tone expected)"));
    assert!(!out.contains("academic expected"));
}

#[test]
fn integration_single_quoted_ternary_branch() {
    let upl = "\
--
name: single_quote_ternary
params:
  flag:
    type: boolean
    def: false
--
{{{FLAG ? 'yes' : 'no'}}}
--
";
    let out = render(upl, vmap(&[("flag", VariableValue::Boolean(true))]));
    assert_eq!(out.trim_end(), "yes");
}

#[test]
fn integration_single_quoted_default_strips_quotes() {
    let upl = "\
--
name: single_quote_default
params:
  tone:
    type: string
    def: 'casual'
--
tone=[[[TONE]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out.trim_end(), "tone=casual");
}

// ---------------------------------------------------------------------------
// Object type reuse via `etype: <object>` (RFC §3.4)
// ---------------------------------------------------------------------------

#[test]
fn integration_element_ref_list_of_objects() {
    // `etype: server` reuses the `server` object's ofields for the list.
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
      port:
        type: number
        def: 8080
  servers:
    type: list
    etype: server
    def:
      - { host: \"localhost\", port: 8080 }
      - { host: \"db.local\", port: 5432 }
--
Hosts:
{{{for S in SERVERS}}}
- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(
        out,
        "Hosts:\n- localhost:8080\n- db.local:5432\n"
    );
}

#[test]
fn integration_list_def_partial_elements_use_shape_field_defaults() {
    // RFC §3.4: a list element supplied only partially (here via `def:`)
    // falls back to the shape's field default for the fields it omits —
    // the same as the JSON path already did.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  servers:
    type: list
    etype: host
    def:
      - { host: \"only\" }
      - { port: 1 }
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let out = PromptBuilder::new(parse(upl)).render_with_defaults().unwrap();
    assert_eq!(out, "- only:8080\n- localhost:1\n");
}

#[test]
fn integration_list_def_partial_elements_inline_object_etype() {
    // Same rule for an inline `etype: object` element shape, including a
    // nested object field merged recursively.
    let upl = "\
--
name: p
params:
  items:
    type: list
    etype: object
    ofields:
      name:
        type: string
        def: \"n\"
      meta:
        type: object
        ofields:
          qty:
            type: number
            def: 1
          unit:
            type: string
            def: \"kg\"
    def:
      - { name: \"a\", meta: { qty: 5 } }
--
{{{for I in ITEMS}}}[[[I.NAME]]]=[[[I.META.QTY]]][[[I.META.UNIT]]]{{{end for}}}
--
";
    let out = PromptBuilder::new(parse(upl)).render_with_defaults().unwrap();
    assert_eq!(out, "a=5kg");
}

#[test]
fn integration_element_ref_forward_declaration() {
    // The list is declared before the object it references.
    let upl = "\
--
name: p
params:
  servers:
    type: list
    etype: server
  server:
    type: object_shape
    ofields:
      host:
        type: string
      port:
        type: number
        def: 8080
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    // No `def:` list → empty list → loop renders nothing.
    assert_eq!(out, "");
}

#[test]
fn integration_element_ref_with_supplied_values() {
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
      port:
        type: number
  servers:
    type: list
    etype: server
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let mut s1 = ObjectMap::new();
    s1.insert("host".into(), VariableValue::String("a.local".into()));
    s1.insert("port".into(), VariableValue::Number(80.0));
    let mut s2 = ObjectMap::new();
    s2.insert("host".into(), VariableValue::String("b.local".into()));
    s2.insert("port".into(), VariableValue::Number(443.0));
    let out = render(
        upl,
        vmap(&[(
            "servers",
            VariableValue::List(vec![
                VariableValue::Object(s1),
                VariableValue::Object(s2),
            ]),
        )]),
    );
    assert_eq!(out, "- a.local:80\n- b.local:443\n");
}

#[test]
fn integration_element_ref_case_insensitive() {
    // Placeholder/variable lookup is case-insensitive; element refs follow
    // the same rule (`etype: Server` resolves to `server`).
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
  servers:
    type: list
    etype: Server
--
{{{for S in SERVERS}}}- [[[S.HOST]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    // No `def:` list → empty list → loop renders nothing.
    assert_eq!(out, "");
}

#[test]
fn integration_element_ref_list_projection() {
    // Field projection (§4.1.5) works through an element-ref list.
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
  servers:
    type: list
    etype: server
    def:
      - { host: \"a\" }
      - { host: \"b\" }
--
Hosts: [[[SERVERS.HOST]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Hosts: a, b\n");
}

#[test]
fn integration_element_ref_missing_is_parse_error() {
    let upl = "\
--
name: p
params:
  servers:
    type: list
    etype: nope
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(matches!(
        res,
        Err(universal_prompt_language::upl::parser::PromptParseError::UnresolvedElementRef { .. })
    ));
}

#[test]
fn integration_element_ref_cycle_is_parse_error() {
    let upl = "\
--
name: p
params:
  node:
    type: object_shape
    ofields:
      children:
        type: list
        etype: node
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(matches!(
        res,
        Err(universal_prompt_language::upl::parser::PromptParseError::CircularElementRef { .. })
    ));
}

// ---------------------------------------------------------------------------
// `object_shape` type definitions and `type: <name>` inheritance
// (RFC §3.1 / §3.4 / §3.4.2)
// ---------------------------------------------------------------------------

#[test]
fn integration_object_shape_not_asked_type_ref_shape_into_object() {
    // `host` is an object_shape (never asked on its own); `cfg` is an object that
    // reuses host's shape via `type: host` and IS asked.
    // Rendering with defaults uses host's field defaults for cfg.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  cfg:
    type: host
--
Host: [[[CFG.HOST]]] Port: [[[CFG.PORT]]]
--
";
    let prompt = parse(upl);
    // cfg's ofields were spliced in from host.
    let cfg = prompt.variable_definitions.get("cfg").unwrap();
    assert_eq!(cfg.type_ref.as_deref(), Some("host"));
    assert!(cfg.ofields_definitions.as_ref().unwrap().contains_key("host"));
    assert!(cfg.ofields_definitions.as_ref().unwrap().contains_key("port"));
    // host is an object_shape → not collectible.
    let builder = PromptBuilder::new(prompt.clone());
    assert!(builder.referenced_type_defs().contains("host"));
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Host: localhost Port: 8080\n");
}

#[test]
fn integration_object_type_ref_with_def() {
    // An inheriting object's top-level object-literal `def` (RFC §3, E4)
    // overrides the shape's field-level defaults for the keys it declares;
    // here it declares both fields, so both are overridden.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  cfg:
    type: host
    def: { host: \"db.local\", port: 5432 }
--
Host: [[[CFG.HOST]]] Port: [[[CFG.PORT]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Host: db.local Port: 5432\n");
}

#[test]
fn integration_object_type_ref_with_partial_def_falls_back_per_field() {
    // A key the object-literal `def` doesn't mention keeps the shape's
    // field-level default for that key (E4).
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  cfg:
    type: host
    def: { host: \"db.local\" }
--
Host: [[[CFG.HOST]]] Port: [[[CFG.PORT]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Host: db.local Port: 8080\n");
}

#[test]
fn integration_object_def_literal_merges_recursively_into_nested_object_field() {
    // A nested object field within an object-level `def` literal merges
    // recursively against that nested field's own shape defaults, rather
    // than replacing the whole nested object wholesale (E4).
    let upl = "\
--
name: p
params:
  address:
    type: object_shape
    ofields:
      city:
        type: string
        def: \"Athens\"
      zip:
        type: string
        def: \"00000\"
  philosopher:
    type: object_shape
    ofields:
      name:
        type: string
        def: \"Socrates\"
      home:
        type: address
  focal:
    type: philosopher
    def: { name: \"Plato\", home: { city: \"Rome\" } }
--
[[[FOCAL.NAME]]] from [[[FOCAL.HOME.CITY]]] [[[FOCAL.HOME.ZIP]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Plato from Rome 00000\n");
}

#[test]
fn integration_object_shape_level_def_applies_at_reference_sites() {
    // RFC §3 (`def` row): an object_shape's declared def/field defaults are
    // applied at every site that references it. The shape's object-level
    // literal wins per key over its field defaults; a site's own literal
    // still wins over both.
    let upl = "\
--
name: p
params:
  ph:
    type: object_shape
    ofields:
      name:
        type: string
        def: \"Socrates\"
      era:
        type: number
        def: -470
    def: { name: \"Plato\" }
  focal:
    type: ph
  own:
    type: ph
    def: { era: 1 }
  many:
    type: list
    etype: ph
    def:
      - { era: 2 }
  pick:
    type: option_single
    etype: ph
    label: name
    opts:
      - { name: \"Plato\", era: 3 }
      - { name: \"Zeno\", era: 4 }
--
[[[FOCAL.NAME]]]/[[[FOCAL.ERA]]] [[[OWN.NAME]]]/[[[OWN.ERA]]] {{{for P in MANY}}}[[[P.NAME]]]/[[[P.ERA]]]{{{end for}}} [[[PICK.NAME]]]/[[[PICK.ERA]]]
--
";
    let out = PromptBuilder::new(parse(upl)).render_with_defaults().unwrap();
    assert_eq!(out, "Plato/-470 Plato/1 Plato/2 Plato/3\n");
}

#[test]
fn integration_object_type_ref_forward_reference() {
    // The inheriting object may be declared before the object_shape it names.
    let upl = "\
--
name: p
params:
  cfg:
    type: host
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
--
Host: [[[CFG.HOST]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Host: localhost\n");
}

#[test]
fn integration_object_type_ref_from_object_shape_ok() {
    // An `object` may inherit from a declared `object_shape` — no cycle here.
    let upl = "\
--
name: p
params:
  a:
    type: b
  b:
    type: object_shape
    ofields:
      x:
        type: string
--
x
--
";
    // `a` (object) reuses `b` (object_shape) — this is fine, no cycle.
    let prompt = parse(upl);
    assert!(prompt.variable_definitions.get("a").unwrap().ofields_definitions.is_some());
}

// Note: a pure `type: <name>` cycle is not constructible — the inheritance
// target must be an `object_shape`, and `object_shape` cannot itself use
// `type: <name>` (only `object` can reuse a shape). Cycle
// detection is therefore exercised through `etype` references instead; see
// `integration_element_ref_cycle_is_parse_error` and the parser's
// `test_element_ref_cycle_is_error`.

#[test]
fn integration_object_type_ref_target_must_be_object_shape() {
    // `type: <name>` naming a declared `object` (not object_shape)
    // is a parse error.
    let upl = "\
--
name: p
params:
  cfg:
    type: host
  host:
    type: object
    ofields:
      host:
        type: string
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(matches!(
        res,
        Err(universal_prompt_language::upl::parser::PromptParseError::InvalidTypeRef { .. })
    ));
}

#[test]
fn integration_object_shape_requires_ofields() {
    // An object_shape without an ofields block (and not referenced, so the
    // reuse path doesn't fire first) is a parse error.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
  cfg:
    type: object
    ofields:
      x:
        type: string
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(matches!(
        res,
        Err(universal_prompt_language::upl::parser::PromptParseError::ObjectShapeMissingOfields { .. })
    ));
}

#[test]
fn integration_object_shape_as_nested_field_is_allowed() {
    // A nested field may be `type: object_shape` (or `type: object`); when
    // nested it is collected inline during parent collection, identical to a
    // nested `object`. The object_shape-vs-object distinction only matters at
    // root level (object is asked as a param; object_shape is not).
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      sub:
        type: object_shape
        ofields:
          x:
            type: string
            def: \"x\"
--
sub.x=[[[CFG.SUB.X]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "sub.x=x\n");
}

#[test]
fn integration_object_type_ref_and_ofields_mutually_exclusive() {
    // `type: <shape>` (a shape reference) must not also declare inline
    // `ofields` — the referenced object_shape provides the fields.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
  cfg:
    type: host
    ofields:
      extra:
        type: string
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(res.is_err());
}

#[test]
fn integration_object_ofields_before_type_ref_is_also_rejected() {
    // The mutual exclusion must hold regardless of key order: `ofields`
    // written before `type: <shape>` used to be silently discarded.
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
  cfg:
    ofields:
      extra:
        type: string
    type: host
--
x
--
";
    let res = universal_prompt_language::upl::parser::PromptParser::parse(upl);
    assert!(
        matches!(res, Err(PromptParseError::InvalidOfieldsForType { .. })),
        "{:?}",
        res
    );
}

#[test]
fn integration_object_field_reuses_object_shape() {
    // A nested object field may inherit an object_shape's shape via
    // `type: <name>` (RFC §3.4.2 inside an object).
    let upl = "\
--
name: p
params:
  host:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  cfg:
    type: object
    ofields:
      primary:
        type: host
--
Host: [[[CFG.PRIMARY.HOST]]] Port: [[[CFG.PRIMARY.PORT]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Host: localhost Port: 8080\n");
}

// ---------------------------------------------------------------------------
// Typed option_single / option_multi (RFC §3.1, §3.3, §3.6, §4.1.3, §4.1.4)
// ---------------------------------------------------------------------------

#[test]
fn integration_option_single_number_etype_render() {
    let upl = "\
--
name: p
params:
  port:
    type: option_single
    etype: number
    opts:
      - 80
      - 443
      - 8080
    def: 443
--
Listening on port [[[PORT]]].
--
";
    let out = render(upl, vmap(&[("port", VariableValue::Number(80.0))]));
    assert_eq!(out, "Listening on port 80.\n");
}

#[test]
fn integration_option_single_number_default() {
    let upl = "\
--
name: p
params:
  port:
    type: option_single
    etype: number
    opts:
      - 80
      - 443
      - 8080
    def: 443
--
Listening on port [[[PORT]]].
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Listening on port 443.\n");
}

#[test]
fn integration_option_single_long_string_etype_render() {
    let upl = "\
--
name: p
params:
  body:
    type: option_single
    etype: long_string
    opts:
      - \"line one\"
      - \"line two\"
    def: \"line one\"
--
Body: [[[BODY]]]
--
";
    let out = render(
        upl,
        vmap(&[("body", VariableValue::LongString("line two".into()))]),
    );
    assert_eq!(out, "Body: line two\n");
}

#[test]
fn integration_option_multi_number_etype_render() {
    let upl = "\
--
name: p
params:
  ports:
    type: option_multi
    etype: number
    opts:
      - 80
      - 443
      - 8080
    def: [443, 8080]
--
Ports: [[[PORTS]]]
--
";
    let out = render(
        upl,
        vmap(&[(
            "ports",
            VariableValue::List(vec![
                VariableValue::Number(80.0),
                VariableValue::Number(443.0),
            ]),
        )]),
    );
    assert_eq!(out, "Ports: 80, 443\n");
}

#[test]
fn integration_option_multi_number_default() {
    let upl = "\
--
name: p
params:
  ports:
    type: option_multi
    etype: number
    opts:
      - 80
      - 443
      - 8080
    def: [443, 8080]
--
Ports: [[[PORTS]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Ports: 443, 8080\n");
}

#[test]
fn integration_option_single_object_etype_render() {
    // option_single whose etype is a referenced object: the chosen value is
    // a whole object, accessible via dotted paths.
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
        def: false
  pick:
    type: option_single
    etype: feature
    label: name
    opts:
      - { name: \"auth\", enabled: true }
      - { name: \"logs\", enabled: false }
    def: { name: \"auth\", enabled: true }
--
Picked: [[[PICK.NAME]]] (enabled=[[[PICK.ENABLED]]])
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Picked: auth (enabled=true)\n");
}

#[test]
fn integration_option_single_object_etype_supplied_value() {
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
  pick:
    type: option_single
    etype: feature
    label: name
    opts:
      - { name: \"auth\", enabled: true }
      - { name: \"logs\", enabled: false }
--
Picked: [[[PICK.NAME]]]
--
";
    let mut obj = ObjectMap::new();
    obj.insert("name".into(), VariableValue::String("logs".into()));
    obj.insert("enabled".into(), VariableValue::Boolean(false));
    let out = render(upl, vmap(&[("pick", VariableValue::Object(obj))]));
    assert_eq!(out, "Picked: logs\n");
}

#[test]
fn integration_option_multi_object_etype_loop() {
    // option_multi with object etype: chosen values are objects iterated in
    // a for loop.
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
        def: false
  selected:
    type: option_multi
    etype: feature
    label: name
    opts:
      - { name: \"auth\", enabled: true }
      - { name: \"logs\", enabled: false }
      - { name: \"metrics\", enabled: true }
    def: [{ name: \"auth\", enabled: true }, { name: \"metrics\", enabled: true }]
--
Enabled:
{{{for F in SELECTED}}}
- [[[F.NAME]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "Enabled:\n- auth\n- metrics\n");
}

#[test]
fn integration_option_multi_object_etype_supplied_values() {
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
  selected:
    type: option_multi
    etype: feature
    label: name
    opts:
      - { name: \"auth\" }
      - { name: \"logs\" }
--
Names: [[[SELECTED.NAME]]]
--
";
    let mut a = ObjectMap::new();
    a.insert("name".into(), VariableValue::String("auth".into()));
    let mut b = ObjectMap::new();
    b.insert("name".into(), VariableValue::String("logs".into()));
    let out = render(
        upl,
        vmap(&[(
            "selected",
            VariableValue::List(vec![
                VariableValue::Object(a),
                VariableValue::Object(b),
            ]),
        )]),
    );
    // List field projection (§4.1.5) across the chosen objects.
    assert_eq!(out, "Names: auth, logs\n");
}

#[test]
fn integration_option_single_string_default_falls_back_to_first_opt() {
    // No `def` supplied: default_value returns the first option.
    let upl = "\
--
name: p
params:
  env:
    type: option_single
    opts:
      - \"dev\"
      - \"prod\"
--
env=[[[ENV]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "env=dev\n");
}

// ---------------------------------------------------------------------------
// Default fallbacks when `def` is absent (RFC §3)
// ---------------------------------------------------------------------------

#[test]
fn integration_default_fallbacks_when_def_absent() {
    // Covers every type's no-`def` fallback:
    //   string "", long_string "", number 0, boolean false,
    //   list [], option_single first opt, option_multi [], object {field defaults}
    let upl = "\
--
name: p
params:
  s:
    type: string
  ls:
    type: long_string
  n:
    type: number
  b:
    type: boolean
  lst:
    type: list
    etype: string
  os:
    type: option_single
    opts:
      - \"first\"
      - \"second\"
  om:
    type: option_multi
    etype: string
    opts:
      - \"a\"
      - \"b\"
  obj:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
      unset:
        type: string
--
s=[[[S]]] ls=[[[LS]]] n=[[[N]]] b=[[[B]]] lst=[[[LST]]] os=[[[OS]]] om=[[[OM]]] obj.host=[[[OBJ.HOST]]] obj.port=[[[OBJ.PORT]]] obj.unset=[[[OBJ.UNSET]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(
        out.trim_end(),
        "s= ls= n=0 b=false lst= os=first om= obj.host=localhost obj.port=8080 obj.unset="
    );
}

#[test]
fn integration_default_fallback_list_empty_loop() {
    // A list with no `def` defaults to []; a loop over it renders nothing.
    let upl = "\
--
name: p
params:
  items:
    type: list
    etype: string
--
{{{for X in ITEMS}}}- [[[X]]]
{{{end for}}}
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "");
}

#[test]
fn integration_default_fallback_option_multi_empty() {
    let upl = "\
--
name: p
params:
  tags:
    type: option_multi
    etype: string
    opts:
      - \"a\"
      - \"b\"
--
tags: [[[TAGS]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "tags: \n");
}

#[test]
fn integration_default_fallback_object_recurses_field_defaults() {
    // Object has one field with a `def` and one without; the latter falls
    // back to its own type default (string -> "").
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
      name:
        type: string
--
host=[[[CFG.HOST]]] port=[[[CFG.PORT]]] name=[[[CFG.NAME]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "host=localhost port=8080 name=\n");
}

// --- Build-time `condition` field (RFC §3.7) ---
//
// Conditions are evaluated during interactive collection and JSON building,
// not during rendering. `render_with_defaults` uses all defaults regardless
// of conditions; `render` with explicit values renders whatever was supplied.
// These tests verify that prompts with conditions parse and render correctly.

#[test]
fn integration_condition_render_with_defaults() {
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    // Both defaults are used — condition doesn't affect rendering.
    assert!(out.contains("Card: visa"));
    assert!(out.contains("Expiry: 12/25"));
}

#[test]
fn integration_condition_render_with_explicit_values() {
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    let values = vmap(&[
        ("credit_card_type", VariableValue::String("mastercard".into())),
        ("visa_card_expiry_date", VariableValue::String("06/28".into())),
    ]);
    let out = render(upl, values);
    assert!(out.contains("Card: mastercard"));
    assert!(out.contains("Expiry: 06/28"));
}

#[test]
fn integration_condition_not_operator() {
    let upl = "\
--
name: p
params:
  flag:
    type: boolean
    def: true
  other:
    type: string
    exclude_condition: !FLAG
    def: \"hidden\"
--
Flag: [[[FLAG]]] Other: [[[OTHER]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert!(out.contains("Flag: true"));
    assert!(out.contains("Other: hidden"));
}

#[test]
fn integration_condition_with_comparison() {
    let upl = "\
--
name: p
params:
  port:
    type: number
    def: 443
  use_ssl:
    type: boolean
    exclude_condition: PORT != 443
    def: false
--
Port: [[[PORT]]] SSL: [[[USE_SSL]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert!(out.contains("Port: 443"));
    assert!(out.contains("SSL: false"));
}

#[test]
fn integration_condition_chained_params() {
    // Multiple parameters with conditions, each referencing earlier ones.
    let upl = "\
--
name: p
params:
  a:
    type: string
    def: \"x\"
  b:
    type: string
    exclude_condition: A = \"x\"
    def: \"b_default\"
  c:
    type: string
    exclude_condition: B = \"b_default\"
    def: \"c_default\"
--
A=[[[A]]] B=[[[B]]] C=[[[C]]]
--
";
    let prompt = parse(upl);
    let out = PromptBuilder::new(prompt).render_with_defaults().unwrap();
    assert_eq!(out, "A=x B=b_default C=c_default\n");
}



// ---------------------------------------------------------------------------
// build_from_json integration tests (originally in build_from_json.rs)
// ---------------------------------------------------------------------------

#[test]
fn json_basic_string_placeholder() {
    let upl = "\
--
name: hello
title: Hello
params:
  name:
    type: string
    def: \"guest\"
--
Hello, [[[NAME]]]!
--
";
    let out = build(upl, r#"{"name": "Ada"}"#).unwrap();
    assert_eq!(out, "Hello, Ada!\n");
}

#[test]
fn json_all_types_at_once() {
    let upl = "\
--
name: p
params:
  s:
    type: string
    def: \"default_s\"
  ls:
    type: long_string
    def: \"default_ls\"
  n:
    type: number
    def: 0
  b:
    type: boolean
    def: false
  os:
    type: option_single
    opts:
      - \"x\"
      - \"y\"
    def: \"x\"
  om:
    type: option_multi
    etype: string
    opts:
      - \"a\"
      - \"b\"
    def: [\"a\"]
--
s=[[[S]]] ls=[[[LS]]] n=[[[N]]] b={{{B ? \"T\" : \"F\"}}}
os=[[[OS]]] om=[[[OM]]]
--
";
    let json = r#"{
        "s": "hello",
        "ls": "multi\nline",
        "n": 42,
        "b": true,
        "os": "y",
        "om": ["a", "b"]
    }"#;
    let out = build(upl, json).unwrap();
    assert!(out.contains("s=hello"));
    assert!(out.contains("ls=multi"));
    assert!(out.contains("n=42"));
    assert!(out.contains("b=T"));
    assert!(out.contains("os=y"));
    assert!(out.contains("om=a, b"));
}

#[test]
fn json_ternary_and_if() {
    let upl = "\
--
name: p
params:
  file_size:
    type: number
    def: 100
  use_async:
    type: boolean
    def: true
--
Size: [[[FILE_SIZE]]]
{{{FILE_SIZE > 100 ? \"large\" : \"small\"}}}
{{{if USE_ASYNC}}}async mode{{{end if}}}
--
";
    let json = r#"{"file_size": 250, "use_async": false}"#;
    let out = build(upl, json).unwrap();
    assert!(out.contains("Size: 250"));
    assert!(out.contains("large"));
    assert!(!out.contains("async mode"));
}

#[test]
fn json_nested_object() {
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
--
host=[[[CFG.HOST]]] port=[[[CFG.PORT]]]
--
";
    let json = r#"{"cfg": {"host": "db.local", "port": 5432}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "host=db.local port=5432\n");
}

#[test]
fn json_nested_object_partial_defaults() {
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
--
host=[[[CFG.HOST]]] port=[[[CFG.PORT]]]
--
";
    let json = r#"{"cfg": {"port": 3000}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "host=localhost port=3000\n");
}

#[test]
fn json_list_of_strings_loop() {
    let upl = "\
--
name: p
params:
  items:
    type: list
    etype: string
    def: []
--
{{{for I in ITEMS}}}- [[[I]]]
{{{end for}}}
--
";
    let json = r#"{"items": ["alpha", "beta", "gamma"]}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "- alpha\n- beta\n- gamma\n");
}

#[test]
fn json_list_of_objects_with_object_shape() {
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
      port:
        type: number
  servers:
    type: list
    etype: server
    def: []
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let json = r#"{"servers": [
        {"host": "a.local", "port": 80},
        {"host": "b.local", "port": 443}
    ]}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "- a.local:80\n- b.local:443\n");
}

#[test]
fn json_list_of_objects_partial_field_defaults() {
    // A list element object missing a field: the missing field falls back to
    // a type-appropriate zero (matching interactive collection behaviour).
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
      port:
        type: number
  servers:
    type: list
    etype: server
    def: []
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let json = r#"{"servers": [{"host": "only-host"}]}"#;
    let out = build(upl, json).unwrap();
    // port falls back to 0 (number zero, since no def declared for server.port)
    assert_eq!(out, "- only-host:0\n");
}

#[test]
fn json_list_of_objects_partial_uses_shape_field_default() {
    // E5 regression: when the object_shape DOES declare a field-level
    // `def`, a list element missing that field must fall back to the
    // shape's declared default, not the type-appropriate zero. Previously
    // `etype` reference sites never received the shape's field defaults at
    // all (only `type: <name>` reuse did), so this fell back to `0`.
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  servers:
    type: list
    etype: server
    def: []
--
{{{for S in SERVERS}}}- [[[S.HOST]]]:[[[S.PORT]]]
{{{end for}}}
--
";
    let json = r#"{"servers": [{"host": "only-host"}]}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "- only-host:8080\n");
}

#[test]
fn json_option_single_object_etype_partial_uses_shape_field_default() {
    // E5 regression for the `option_single`/`option_multi` etype site: a
    // partially-supplied chosen value falls back to the shape's field
    // default for the field it omits, so it can still reconstruct and match
    // a declared `opts` entry.
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: \"localhost\"
      port:
        type: number
        def: 8080
  chosen:
    type: option_single
    etype: server
    label: host
    opts:
      - { host: \"localhost\", port: 8080 }
      - { host: \"other\", port: 2 }
--
[[[CHOSEN.HOST]]]:[[[CHOSEN.PORT]]]
--
";
    let json = r#"{"chosen": {"host": "localhost"}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "localhost:8080\n");
}

#[test]
fn json_option_object_partial_opts_entries_are_completed() {
    // `opts` object literals are completed from the shape's field defaults
    // at parse time, so a JSON value (which is completed the same way)
    // matches an option written partially, and a partial `def` does too.
    let upl = "\
--
name: p
params:
  f:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
        def: false
  pick:
    type: option_single
    etype: f
    label: name
    opts:
      - { name: \"auth\" }
      - { name: \"logs\", enabled: true }
    def: { name: \"logs\", enabled: true }
--
[[[PICK.NAME]]]:[[[PICK.ENABLED]]]
--
";
    assert_eq!(build(upl, r#"{"pick": {"name": "auth"}}"#).unwrap(), "auth:false\n");
    assert_eq!(build(upl, r#"{"pick": {"name": "auth", "enabled": false}}"#).unwrap(), "auth:false\n");
    assert_eq!(build(upl, "{}").unwrap(), "logs:true\n");
    let prompt = parse(upl);
    let opts = prompt.variable_definitions["pick"].options.as_ref().unwrap();
    assert!(matches!(&opts[0], VariableValue::Object(m) if m.len() == 2), "{:?}", opts[0]);
}

#[test]
fn json_option_single_number() {
    let upl = "\
--
name: p
params:
  port:
    type: option_single
    etype: number
    opts:
      - 80
      - 443
      - 8080
    def: 443
--
port=[[[PORT]]]
--
";
    let json = r#"{"port": 80}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "port=80\n");
}

#[test]
fn json_option_single_object_etype() {
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
  pick:
    type: option_single
    etype: feature
    label: name
    opts:
      - { name: \"auth\", enabled: true }
      - { name: \"logs\", enabled: false }
    def: { name: \"auth\", enabled: true }
--
picked: [[[PICK.NAME]]] enabled={{{PICK.ENABLED ? \"yes\" : \"no\"}}}
--
";
    let json = r#"{"pick": {"name": "logs", "enabled": false}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "picked: logs enabled=no\n");
}

#[test]
fn json_option_multi_object_etype_loop() {
    let upl = "\
--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
  selected:
    type: option_multi
    etype: feature
    label: name
    opts:
      - { name: \"auth\" }
      - { name: \"logs\" }
      - { name: \"metrics\" }
    def: []
--
{{{for F in SELECTED}}}- [[[F.NAME]]]
{{{end for}}}
--
";
    let json = r#"{"selected": [{"name": "auth"}, {"name": "metrics"}]}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "- auth\n- metrics\n");
}

#[test]
fn json_empty_object_all_defaults() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
  n:
    type: number
    def: 7
--
Hi [[[NAME]]]! n=[[[N]]]
--
";
    let out = build(upl, "{}").unwrap();
    assert_eq!(out, "Hi world! n=7\n");
}

#[test]
fn json_null_value_uses_default() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"fallback\"
--
Hi [[[NAME]]]
--
";
    let out = build(upl, r#"{"name": null}"#).unwrap();
    assert_eq!(out, "Hi fallback\n");
}

#[test]
fn json_case_insensitive_keys() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"default\"
--
Hi [[[NAME]]]
--
";
    let out = build(upl, r#"{"NAME": "Bob"}"#).unwrap();
    assert_eq!(out, "Hi Bob\n");
}

#[test]
fn json_error_wrong_type_for_number() {
    let upl = "\
--
name: p
params:
  n:
    type: number
--
n=[[[N]]]
--
";
    let res = build(upl, r#"{"n": "not a number"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_unknown_parameter() {
    let upl = "\
--
name: p
params:
  a:
    type: string
--
x
--
";
    let res = build(upl, r#"{"unknown_param": "x"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_object_shape_in_json() {
    let upl = "\
--
name: p
params:
  shape:
    type: object_shape
    ofields:
      x:
        type: string
  items:
    type: list
    etype: shape
    def: []
--
x
--
";
    let res = build(upl, r#"{"shape": {"x": "y"}}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_nested_object_shape_field_is_settable() {
    // A field declared `type: object_shape` inside an object is collected
    // like a nested object (RFC §3.1); it used to be rejected via JSON.
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      sub:
        type: object_shape
        ofields:
          x:
            type: string
            def: \"x\"
          y:
            type: string
            def: \"y\"
--
[[[CFG.SUB.X]]][[[CFG.SUB.Y]]]
--
";
    let out = build(upl, r#"{"cfg": {"sub": {"x": "J"}}}"#).unwrap();
    assert_eq!(out, "Jy\n");
}

#[test]
fn json_error_option_single_invalid_value() {
    let upl = "\
--
name: p
params:
  env:
    type: option_single
    opts:
      - \"dev\"
      - \"prod\"
    def: \"dev\"
--
env=[[[ENV]]]
--
";
    let res = build(upl, r#"{"env": "staging"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_option_multi_invalid_element() {
    let upl = "\
--
name: p
params:
  tags:
    type: option_multi
    etype: string
    opts:
      - \"a\"
      - \"b\"
--
tags: [[[TAGS]]]
--
";
    let res = build(upl, r#"{"tags": ["a", "z"]}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_invalid_json_syntax() {
    let upl = "\
--
name: p
params:
  a:
    type: string
--
x
--
";
    let res = build(upl, "{broken json");
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_non_object_root() {
    let upl = "\
--
name: p
params:
  a:
    type: string
--
x
--
";
    let res = build(upl, "[1, 2, 3]");
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_wrong_type_for_boolean() {
    let upl = "\
--
name: p
params:
  flag:
    type: boolean
--
{{{FLAG ? \"on\" : \"off\"}}}
--
";
    let res = build(upl, r#"{"flag": "yes"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_error_array_for_object() {
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      host:
        type: string
--
host=[[[CFG.HOST]]]
--
";
    let res = build(upl, r#"{"cfg": [1, 2]}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_deeply_nested_object() {
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      api:
        type: object
        ofields:
          auth:
            type: object
            ofields:
              type:
                type: option_single
                opts:
                  - \"bearer\"
                  - \"basic\"
                def: \"bearer\"
              token:
                type: string
                def: \"\"
--
auth.type=[[[CFG.API.AUTH.TYPE]]] token=[[[CFG.API.AUTH.TOKEN]]]
--
";
    let json = r#"{"cfg": {"api": {"auth": {"type": "basic", "token": "xyz123"}}}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "auth.type=basic token=xyz123\n");
}

#[test]
fn json_deeply_nested_object_partial() {
    let upl = "\
--
name: p
params:
  cfg:
    type: object
    ofields:
      api:
        type: object
        ofields:
          version:
            type: string
            def: \"v1\"
          auth:
            type: object
            ofields:
              type:
                type: string
                def: \"bearer\"
              token:
                type: string
                def: \"default-token\"
--
v=[[[CFG.API.VERSION]]] auth.type=[[[CFG.API.AUTH.TYPE]]] token=[[[CFG.API.AUTH.TOKEN]]]
--
";
    // Only provide api.auth.token; everything else uses defaults.
    let json = r#"{"cfg": {"api": {"auth": {"token": "my-token"}}}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "v=v1 auth.type=bearer token=my-token\n");
}

#[test]
fn json_list_of_objects_with_nested_list() {
    let upl = "\
--
name: p
params:
  resource:
    type: object_shape
    ofields:
      name:
        type: string
      actions:
        type: option_multi
        etype: string
        opts:
          - \"GET\"
          - \"POST\"
          - \"PUT\"
          - \"DELETE\"
        def: []
  resources:
    type: list
    etype: resource
    def: []
--
{{{for R in RESOURCES}}}- [[[R.NAME]]] ([[[R.ACTIONS]]])
{{{end for}}}
--
";
    let json = r#"{"resources": [
        {"name": "users", "actions": ["GET", "POST", "DELETE"]},
        {"name": "posts", "actions": ["GET", "PUT"]}
    ]}"#;
    let out = build(upl, json).unwrap();
    assert!(out.contains("- users (GET, POST, DELETE)"));
    assert!(out.contains("- posts (GET, PUT)"));
}

#[test]
fn json_object_with_nested_list_of_objects() {
    let upl = "\
--
name: p
params:
  field:
    type: object_shape
    ofields:
      name:
        type: string
      type:
        type: string
  model:
    type: object
    ofields:
      fields:
        type: list
        etype: field
        def: []
--
{{{for F in MODEL.FIELDS}}}- [[[F.NAME]]] : [[[F.TYPE]]]
{{{end for}}}
--
";
    let json = r#"{"model": {"fields": [
        {"name": "id", "type": "string"},
        {"name": "email", "type": "string"}
    ]}}"#;
    let out = build(upl, json).unwrap();
    assert_eq!(out, "- id : string\n- email : string\n");
}

#[test]
fn json_full_rest_api_prompt() {
    // A trimmed version of the create_rest_api sample, exercising list of
    // objects (with object_shape reuse), nested lists, option_multi, and
    // option_single — all driven from JSON.
    let upl = "\
--
name: p
params:
  language:
    type: option_single
    opts:
      - \"ruby\"
      - \"node.js\"
    def: \"node.js\"
  api_name:
    type: string
    def: \"My API\"
  resources:
    type: list
    etype: resource
    def: []
  resource:
    type: object_shape
    ofields:
      name:
        type: string
        def: \"users\"
      actions:
        type: option_multi
        etype: string
        opts:
          - \"POST\"
          - \"GET\"
          - \"PUT\"
          - \"PATCH\"
          - \"DELETE\"
        def: [\"POST\", \"GET\"]
      fields:
        type: list
        etype: field
        def: []
  field:
    type: object_shape
    ofields:
      name:
        type: string
        def: \"id\"
      type:
        type: option_single
        opts:
          - \"string\"
          - \"number\"
          - \"boolean\"
        def: \"string\"
      required:
        type: boolean
        def: false
--
API: [[[API_NAME]]] ([[[LANGUAGE]]])
{{{for R in RESOURCES}}}
- [[[R.NAME]]]: [[[R.ACTIONS]]]
{{{for F in R.FIELDS}}}  - [[[F.NAME]]] (type: [[[F.TYPE]]], required: [[[F.REQUIRED]]])
{{{end for}}}
{{{end for}}}
--
";
    let json = r#"{
        "language": "ruby",
        "api_name": "Blog API",
        "resources": [
            {
                "name": "users",
                "actions": ["GET", "POST", "DELETE"],
                "fields": [
                    {"name": "id", "type": "string", "required": true},
                    {"name": "email", "type": "string", "required": true},
                    {"name": "age", "type": "number", "required": false}
                ]
            },
            {
                "name": "posts",
                "actions": ["GET", "POST"],
                "fields": [
                    {"name": "id", "type": "string", "required": true},
                    {"name": "title", "type": "string", "required": true}
                ]
            }
        ]
    }"#;
    let out = build(upl, json).unwrap();
    assert!(out.contains("API: Blog API (ruby)"));
    assert!(out.contains("- users: GET, POST, DELETE"));
    assert!(out.contains("  - id (type: string, required: true)"));
    assert!(out.contains("  - email (type: string, required: true)"));
    assert!(out.contains("  - age (type: number, required: false)"));
    assert!(out.contains("- posts: GET, POST"));
    assert!(out.contains("  - title (type: string, required: true)"));
}

// --- Build-time `exclude_condition` field (RFC §3.7) ---
//
// condition truthy → parameter is hidden (excluded from build): skipped
// during interactive collection, rejected if supplied via JSON.
// condition falsy → parameter is shown (asked) normally.

#[test]
fn json_condition_hidden_param_absent_uses_default() {
    // credit_card_type defaults to "visa"; condition `CREDIT_CARD_TYPE = "visa"`
    // is truthy → expiry is hidden. Expiry absent from JSON → OK (uses default).
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    // JSON doesn't include the hidden param — should succeed.
    let out = build(upl, r#"{"credit_card_type": "visa"}"#).unwrap();
    assert!(out.contains("Card: visa"));
    assert!(out.contains("Expiry: 12/25"));
}

#[test]
fn json_condition_hidden_param_present_is_error() {
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    // JSON includes the hidden param with a non-null value — should error.
    let res = build(upl, r#"{"credit_card_type": "visa", "visa_card_expiry_date": "99/99"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_condition_hidden_param_null_is_ok() {
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    // JSON includes the hidden param as null — should be OK (null = use default).
    let out = build(upl, r#"{"credit_card_type": "visa", "visa_card_expiry_date": null}"#).unwrap();
    assert!(out.contains("Expiry: 12/25"));
}

#[test]
fn json_condition_falsy_param_shown_accepts_value() {
    // credit_card_type is "mastercard"; condition `CREDIT_CARD_TYPE = "visa"`
    // is falsy → expiry is shown. JSON provides a value → accepted.
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    let out = build(upl, r#"{"credit_card_type": "mastercard", "visa_card_expiry_date": "06/28"}"#).unwrap();
    assert!(out.contains("Card: mastercard"));
    assert!(out.contains("Expiry: 06/28"));
}

#[test]
fn json_condition_falsy_param_absent_uses_default() {
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    let out = build(upl, r#"{"credit_card_type": "mastercard"}"#).unwrap();
    assert!(out.contains("Card: mastercard"));
    assert!(out.contains("Expiry: 12/25"));
}

#[test]
fn json_condition_depends_on_json_value() {
    // The condition is evaluated against JSON-supplied values, not just defaults.
    // Default for credit_card_type is "visa" (condition truthy → hidden).
    // But JSON overrides it to "mastercard" (condition falsy → shown).
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    // JSON sets credit_card_type to "mastercard" → condition is falsy →
    // expiry is shown → providing a value is OK.
    let out = build(upl, r#"{"credit_card_type": "mastercard", "visa_card_expiry_date": "06/28"}"#).unwrap();
    assert!(out.contains("Card: mastercard"));
    assert!(out.contains("Expiry: 06/28"));
}

#[test]
fn json_condition_depends_on_json_value_hidden() {
    // Default for credit_card_type is "visa" → condition is truthy → hidden.
    // JSON sets credit_card_type to "visa" → condition still truthy →
    // expiry is hidden → providing a value is an error.
    let upl = "\
--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - \"visa\"
      - \"mastercard\"
    def: \"visa\"
  visa_card_expiry_date:
    type: string
    exclude_condition: CREDIT_CARD_TYPE = \"visa\"
    def: \"12/25\"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
";
    let res = build(upl, r#"{"credit_card_type": "visa", "visa_card_expiry_date": "06/28"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_condition_with_number_comparison() {
    let upl = "\
--
name: p
params:
  port:
    type: number
    def: 80
  use_ssl:
    type: boolean
    exclude_condition: PORT = 443
    def: false
--
Port: [[[PORT]]] SSL: [[[USE_SSL]]]
--
";
    // Default port=80 → condition is falsy → use_ssl is shown.
    // JSON sets port=443 → condition is truthy → use_ssl is hidden.
    // Providing use_ssl when hidden → error.
    let res = build(upl, r#"{"port": 443, "use_ssl": true}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));

    // Not providing use_ssl when hidden → OK (uses default).
    let out = build(upl, r#"{"port": 443}"#).unwrap();
    assert!(out.contains("Port: 443"));
    assert!(out.contains("SSL: false"));

    // port=80 (default) → condition falsy → use_ssl shown → can provide value.
    let out = build(upl, r#"{"port": 80, "use_ssl": true}"#).unwrap();
    assert!(out.contains("Port: 80"));
    assert!(out.contains("SSL: true"));
}

#[test]
fn json_condition_no_condition_param_works_normally() {
    let upl = "\
--
name: p
params:
  a:
    type: string
    def: \"x\"
  b:
    type: string
    def: \"y\"
--
A=[[[A]]] B=[[[B]]]
--
";
    let out = build(upl, r#"{"a": "1", "b": "2"}"#).unwrap();
    assert_eq!(out, "A=1 B=2\n");
}

// ---------------------------------------------------------------------------
// Method-call form operators (RFC §5) and list membership
// ---------------------------------------------------------------------------

#[test]
fn test_method_call_contains() {
    let out = render_str(
        "{{{TEXT.contains(\"hello\") ? \"yes\" : \"no\"}}}",
        &[("text", VariableValue::String("say hello world".into()))],
    );
    assert_eq!(out, "yes");
}

#[test]
fn test_method_call_form_ignores_string_literals() {
    // Only the operator position is rewritten; a literal containing
    // ".contains(" stays a literal.
    let out = render_str(
        "{{{S = \"x.contains(y)\" ? \"eq\" : \"ne\"}}}",
        &[("s", VariableValue::String("x.contains(y)".into()))],
    );
    assert_eq!(out, "eq");
}

#[test]
fn test_method_call_starts_with() {
    let out = render_str(
        "{{{PATH.starts_with(\"/home\") ? \"yes\" : \"no\"}}}",
        &[("path", VariableValue::String("/home/me".into()))],
    );
    assert_eq!(out, "yes");
}

#[test]
fn test_method_call_ends_with() {
    let out = render_str(
        "{{{EXT.ends_with(\".js\") ? \"yes\" : \"no\"}}}",
        &[("ext", VariableValue::String("app.js".into()))],
    );
    assert_eq!(out, "yes");
}

#[test]
fn test_contains_list_membership() {
    // `contains` is overloaded: when one operand is a list, it tests
    // membership (RFC §5).
    let out = render_str(
        "{{{TAGS contains \"api\" ? \"yes\" : \"no\"}}}",
        &[("tags", VariableValue::List(vec![
            VariableValue::String("api".into()),
            VariableValue::String("v2".into()),
        ]))],
    );
    assert_eq!(out, "yes");
}

#[test]
fn test_contains_list_membership_not_found() {
    let out = render_str(
        "{{{TAGS contains \"xyz\" ? \"yes\" : \"no\"}}}",
        &[("tags", VariableValue::List(vec![
            VariableValue::String("api".into()),
            VariableValue::String("v2".into()),
        ]))],
    );
    assert_eq!(out, "no");
}

#[test]
fn test_less_than_operator() {
    assert_eq!(
        render_str("{{{N < 10 ? \"small\" : \"big\"}}}", &[("n", VariableValue::Number(3.0))]),
        "small"
    );
    assert_eq!(
        render_str("{{{N < 10 ? \"small\" : \"big\"}}}", &[("n", VariableValue::Number(15.0))]),
        "big"
    );
}

#[test]
fn test_negative_number_in_condition() {
    assert_eq!(
        render_str("{{{N < 0 ? \"negative\" : \"non-negative\"}}}", &[("n", VariableValue::Number(-5.0))]),
        "negative"
    );
    assert_eq!(
        render_str("{{{N < 0 ? \"negative\" : \"non-negative\"}}}", &[("n", VariableValue::Number(0.0))]),
        "non-negative"
    );
}
