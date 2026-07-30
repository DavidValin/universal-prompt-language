// Unit tests for the prompt builder's pure rendering logic.
//
// These exercise `PromptBuilder::render` over pre-parsed templates (the body
// is parsed by `Template::parse`, mirroring what `PromptParser` does). They
// cover the constructs defined in upl-spec/upl-1.0-rfc.md §4: placeholders,
// ternaries, for-loops and if-blocks, plus the operators in §5.

use std::collections::HashMap;

use upl::upl::builder::{PromptBuilder, ValueMap};
use upl::upl::parser::{ObjectMap, Prompt, Template, VariableDefinitions, VariableValue};

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
        "{{{[[[AGE]]] >= 18 ? \"adult\" : \"minor\"}}}",
        &[("age", VariableValue::Number(21.0))],
    );
    assert_eq!(out, "adult");
}

#[test]
fn test_ternary_false_branch() {
    let out = render_str(
        "{{{[[[AGE]]] >= 18 ? \"adult\" : \"minor\"}}}",
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
    // than `contains` per §5.1). a="x", b="x" => (a=b) => Boolean(true);
    // Boolean contains "x" => type error.
    let body = "{{{A = B contains \"x\" ? \"yes\" : \"no\"}}}";
    // a="x", b="x" => (a=b) => true (Boolean) => Boolean contains "x" => type error
    let mut m: ValueMap = HashMap::new();
    m.insert("a".to_string(), VariableValue::String("x".into()));
    m.insert("b".to_string(), VariableValue::String("x".into()));
    let res = PromptBuilder::new(prompt_with(body)).render(&m);
    assert!(res.is_err());
}

#[test]
fn test_safety_unmatched_braces_emitted_verbatim() {
    // A lone }}} with no preceding {{{ should pass through untouched.
    let out = render_str("code: foo}}}bar", &[]);
    assert_eq!(out, "code: foo}}}bar");
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
