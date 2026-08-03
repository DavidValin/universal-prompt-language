// Integration tests for `PromptBuilder::build_from_json`.
//
// These tests exercise the full pipeline: parse a UPL document, feed it a
// JSON string of parameter values, validate/convert the values, and render
// the final prompt. They cover the constructs defined in the RFC (§4):
// placeholders, ternaries, for-loops, if-blocks, nested objects, lists of
// objects (with object_shape reuse), option_single/option_multi, and the
// default-fallback behaviour for missing parameters.

use universal_prompt_language::upl::builder::{BuilderError, PromptBuilder};
use universal_prompt_language::upl::parser::PromptParser;

fn parse(upl: &str) -> universal_prompt_language::upl::parser::Prompt {
    PromptParser::parse(upl).expect("UPL should parse")
}

fn build(upl: &str, json: &str) -> Result<String, BuilderError> {
    let prompt = parse(upl);
    PromptBuilder::new(prompt).build_from_json(json)
}

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
