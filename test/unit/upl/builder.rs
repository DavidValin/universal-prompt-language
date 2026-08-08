use super::*;
use crate::upl::parser::PromptParser;

fn parse(upl: &str) -> Prompt {
    PromptParser::parse(upl).expect("UPL should parse")
}

/// `create_rest_api`-style prompt: `resource` is an `object_shape` reused
/// by the top-level `resources` list, and `field` is an `object_shape`
/// reused by the `fields` list nested inside `resource`'s ofields. Both
/// are `object_shape` type definitions and must be skipped during
/// collection. A plain standalone `object` (`materials`) that nothing
/// references must NOT be skipped — it is a collectible parameter.
#[test]
fn referenced_type_defs_skips_referenced_objects_only() {
    let upl = "\
--
name: p
params:
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
          - \"GET\"
          - \"POST\"
        def: [\"GET\"]
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
        type: string
        def: \"string\"
  materials:
    type: object
    ofields:
      has_calculator:
        type: boolean
        def: true
--
x
--
";
    let builder = PromptBuilder::new(parse(upl));
    let referenced = builder.referenced_type_defs();
    assert!(
        referenced.contains("resource"),
        "resource is an object_shape and should be a type definition: {:?}",
        referenced
    );
    assert!(
        referenced.contains("field"),
        "field is an object_shape (referenced by a nested list) and should be a type definition: {:?}",
        referenced
    );
    assert!(
        !referenced.contains("materials"),
        "materials is a plain object and should remain collectible: {:?}",
        referenced
    );
    assert!(
        !referenced.contains("api_name"),
        "non-object variables are never type definitions: {:?}",
        referenced
    );
}

/// A list with an inline `etype: object` (its own `ofields`, no named
/// reference) produces no element_ref; a standalone `object` declared
/// alongside it is still collected, and nothing is skipped.
#[test]
fn referenced_type_defs_empty_for_inline_object_etype() {
    let upl = "\
--
name: p
params:
  endpoints:
    type: list
    etype: object
    ofields:
      path:
        type: string
  config:
    type: object
    ofields:
      host:
        type: string
        def: \"localhost\"
--
x
--
";
    let builder = PromptBuilder::new(parse(upl));
    let referenced = builder.referenced_type_defs();
    assert!(
        referenced.is_empty(),
        "inline object etype should not reference any type definition: {:?}",
        referenced
    );
}

/// A top-level `object_shape` is skipped (never asked) even when nothing
/// references it — it is a pure type definition. Element references are
/// case-insensitive (`etype: Server` resolves to `server`); the
/// referenced set is lowercased so the skip check matches
/// case-insensitively.
#[test]
fn referenced_type_defs_case_insensitive() {
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
    def: []
  unused:
    type: object_shape
    ofields:
      x:
        type: string
--
x
--
";
    let builder = PromptBuilder::new(parse(upl));
    let referenced = builder.referenced_type_defs();
    assert!(referenced.contains("server"), "matched case-insensitively: {:?}", referenced);
    assert!(referenced.contains("unused"), "unreferenced object_shape is still a type def: {:?}", referenced);
}

/// `option_single`/`option_multi` with a referenced `object_shape` etype:
/// the object_shape is skipped during collection (it is a type def).
#[test]
fn referenced_type_defs_includes_option_object_etype() {
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
x
--
";
    let builder = PromptBuilder::new(parse(upl));
    let referenced = builder.referenced_type_defs();
    assert!(referenced.contains("feature"), "option_single etype should flag feature: {:?}", referenced);
}

// --- build_from_json unit tests ---

fn jbuild(upl: &str, json: &str) -> Result<String, BuilderError> {
    let prompt = parse(upl);
    PromptBuilder::new(prompt).build_from_json(json)
}

#[test]
fn json_simple_string_override() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
--
Hello, [[[NAME]]]!
--
";
    let out = jbuild(upl, r#"{"name": "Ada"}"#).unwrap();
    assert_eq!(out, "Hello, Ada!\n");
}

#[test]
fn json_missing_param_uses_default() {
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
    let out = jbuild(upl, r#"{"name": "Bob"}"#).unwrap();
    assert_eq!(out, "Hi Bob! n=7\n");
}

#[test]
fn json_null_uses_default() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
--
Hi [[[NAME]]]
--
";
    let out = jbuild(upl, r#"{"name": null}"#).unwrap();
    assert_eq!(out, "Hi world\n");
}

#[test]
fn json_wrong_type_is_error() {
    let upl = "\
--
name: p
params:
  n:
    type: number
    def: 0
--
n=[[[N]]]
--
";
    let res = jbuild(upl, r#"{"n": "hello"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_unknown_param_is_error() {
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
    let res = jbuild(upl, r#"{"b": "y"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_object_shape_in_json_is_error() {
    let upl = "\
--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
  servers:
    type: list
    etype: server
--
x
--
";
    let res = jbuild(upl, r#"{"server": {"host": "x"}}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_option_single_valid() {
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
    let out = jbuild(upl, r#"{"env": "prod"}"#).unwrap();
    assert_eq!(out, "env=prod\n");
}

#[test]
fn json_option_single_invalid_value() {
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
    let res = jbuild(upl, r#"{"env": "staging"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_option_multi_valid() {
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
      - \"c\"
    def: [\"a\"]
--
tags: [[[TAGS]]]
--
";
    let out = jbuild(upl, r#"{"tags": ["b", "c"]}"#).unwrap();
    assert_eq!(out, "tags: b, c\n");
}

#[test]
fn json_option_multi_invalid_element() {
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
    let res = jbuild(upl, r#"{"tags": ["a", "z"]}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_object_partial_uses_defaults() {
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
    let out = jbuild(upl, r#"{"cfg": {"host": "db.local"}}"#).unwrap();
    assert_eq!(out, "host=db.local port=8080\n");
}

#[test]
fn json_list_of_objects() {
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
    let json = r#"{"servers": [{"host": "a", "port": 80}, {"host": "b", "port": 443}]}"#;
    let out = jbuild(upl, json).unwrap();
    assert_eq!(out, "- a:80\n- b:443\n");
}

#[test]
fn json_list_of_strings() {
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
    let out = jbuild(upl, r#"{"items": ["x", "y"]}"#).unwrap();
    assert_eq!(out, "- x\n- y\n");
}

#[test]
fn json_invalid_json_is_error() {
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
    let res = jbuild(upl, "{not json");
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_non_object_root_is_error() {
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
    let res = jbuild(upl, "[1, 2, 3]");
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_case_insensitive_keys() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
--
Hi [[[NAME]]]
--
";
    let out = jbuild(upl, r#"{"NAME": "Bob"}"#).unwrap();
    assert_eq!(out, "Hi Bob\n");
}

#[test]
fn json_nested_object_with_subobject() {
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
      auth:
        type: object
        ofields:
          type:
            type: string
            def: \"bearer\"
          token:
            type: string
            def: \"\"
--
host=[[[CFG.HOST]]] auth.type=[[[CFG.AUTH.TYPE]]] token=[[[CFG.AUTH.TOKEN]]]
--
";
    let json = r#"{"cfg": {"host": "x.test", "auth": {"token": "abc"}}}"#;
    let out = jbuild(upl, json).unwrap();
    assert_eq!(out, "host=x.test auth.type=bearer token=abc\n");
}

#[test]
fn json_empty_object_uses_all_defaults() {
    let upl = "\
--
name: p
params:
  name:
    type: string
    def: \"world\"
  n:
    type: number
    def: 42
--
Hi [[[NAME]]]! n=[[[N]]]
--
";
    let out = jbuild(upl, "{}").unwrap();
    assert_eq!(out, "Hi world! n=42\n");
}

#[test]
fn json_option_single_number_etype() {
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
    let out = jbuild(upl, r#"{"port": 80}"#).unwrap();
    assert_eq!(out, "port=80\n");
}

// --- exclude_condition field (RFC §3.7) unit tests ---

#[test]
fn json_condition_hidden_by_default() {
    // Default credit_card_type = "visa" → condition truthy → hidden.
    // Expiry absent from JSON → uses default.
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
    let out = jbuild(upl, "{}").unwrap();
    assert!(out.contains("Card: visa"));
    assert!(out.contains("Expiry: 12/25"));
}

#[test]
fn json_condition_hidden_rejects_value() {
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
    let res = jbuild(upl, r#"{"visa_card_expiry_date": "99/99"}"#);
    assert!(matches!(res, Err(BuilderError::Validation(_))));
}

#[test]
fn json_condition_shown_accepts_value() {
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
    let out = jbuild(upl, r#"{"credit_card_type": "mastercard", "visa_card_expiry_date": "06/28"}"#).unwrap();
    assert!(out.contains("Card: mastercard"));
    assert!(out.contains("Expiry: 06/28"));
}