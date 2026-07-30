// Integration tests for the PromptBuilder render engine.
//
// These tests feed real UPL documents through `PromptParser` (exercising the
// declared variable definitions) and then render them with `PromptBuilder`,
// supplying a pre-built ValueMap. They cover the constructs defined in
// upl-spec/upl-1.0-rfc.md §4: placeholders, ternaries, for-loops and if-blocks, plus
// the operators in §5.

use universal_prompt_language::upl::builder::{PromptBuilder, ValueMap};
use universal_prompt_language::upl::parser::{ObjectMap, PromptParser, PromptParseError, VariableValue};

fn parse(upl: &str) -> universal_prompt_language::upl::parser::Prompt {
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
    type: object
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
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
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
    type: object
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
    type: object
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
    type: object
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
    type: object
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
    type: object
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
    type: object
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
    type: object
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
    type: object
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


