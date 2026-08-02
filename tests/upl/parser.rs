// Unit tests for the UPL prompt parser: header/params/body parsing, element
// reference resolution, and template-body validation.

use universal_prompt_language::upl::parser::{PromptParseError, PromptParser, Template, VariableType};

#[test]
fn test_all_repository_samples_parse() {
    // Every bundled sample in `samples/` must parse cleanly against the
    // current parser (guards against spec drift, e.g. the object_shape split).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples");
    let entries = std::fs::read_dir(&root).expect("samples/ dir");
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".txt"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no samples found");
    for n in &names {
        let path = root.join(n);
        let content = std::fs::read_to_string(&path).unwrap();
        let res = PromptParser::parse(&content);
        assert!(res.is_ok(), "sample {} failed to parse: {:?}", n, res.err());
    }
}

#[test]
fn test_parse_simple_prompt() {
    let content = r#"--
name: test
title: Test
desc: A test prompt
params:
  name:
    type: string
    def: "John"
    desc: "User name"
--
Hello, [[name]]!
"#;
    let result = PromptParser::parse(content);
    assert!(result.is_ok());
}

#[test]
fn test_parse_nested_config() {
    let content = r#"--
name: api_config
title: API Config
params:
  api_config:
    type: object
    ofields:
      base_url:
        type: string
        def: "https://api.example.com"
      auth:
        type: object
        ofields:
          type:
            type: option_single
            opts:
              - "bearer"
              - "basic"
            def: "bearer"
---
const config = { base_url: [[api_config.base_url]] };
"#;

    let result = PromptParser::parse(content);
    assert!(result.is_ok());
}

#[test]
fn test_element_ref_resolves_object_shape() {
    let content = r#"--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: "localhost"
      port:
        type: number
        def: 8080
  servers:
    type: list
    etype: server
--
hosts
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let servers = prompt.variable_definitions.get("servers").unwrap();
    assert_eq!(servers.element_type, Some(VariableType::Object));
    assert_eq!(servers.element_ref.as_deref(), Some("server"));
    let ofields = servers.ofields_definitions.as_ref().expect("resolved ofields");
    assert!(ofields.contains_key("host"));
    assert!(ofields.contains_key("port"));
}

#[test]
fn test_element_ref_forward_reference_resolves() {
    // List declared before the object_shape it references.
    let content = r#"--
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
--
hosts
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let servers = prompt.variable_definitions.get("servers").unwrap();
    assert_eq!(servers.element_type, Some(VariableType::Object));
    assert!(servers.ofields_definitions.as_ref().unwrap().contains_key("host"));
}

#[test]
fn test_element_ref_unresolved_is_error() {
    let content = r#"--
name: p
params:
  servers:
    type: list
    etype: nonexistent
--
hosts
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnresolvedElementRef { .. })));
}

#[test]
fn test_element_ref_to_non_object_shape_is_error() {
    // Referencing a declared `object` (not `object_shape`) by name is an error:
    // only `object_shape` is referenceable via `etype`. Referencing a non-object
    // (e.g. a string) is likewise an error.
    let content = r#"--
name: p
params:
  name:
    type: string
  names:
    type: list
    etype: name
--
hosts
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidElementRef { .. })));
}

#[test]
fn test_element_ref_to_object_is_error() {
    // Naming a declared `object` (not `object_shape`) via etype is an error.
    let content = r#"--
name: p
params:
  server:
    type: object
    ofields:
      host:
        type: string
  servers:
    type: list
    etype: server
--
x
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidElementRef { .. })));
}

#[test]
fn test_element_ref_cycle_is_error() {
    // `node` is an object_shape containing a list whose etype is `node` itself —
    // a self-referential cycle through nested fields.
    let content = r#"--
name: p
params:
  node:
    type: object_shape
    ofields:
      children:
        type: list
        etype: node
--
hosts
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::CircularElementRef { .. })));
}

// --- Heredoc long_string defaults (RFC §3.5) ---

#[test]
fn test_heredoc_long_string_default() {
    let content = r#"--
name: p
params:
  body:
    type: long_string
    desc: Default request body
    def: >>>
{
  "name": "example",
  "active": true
}
<<<
--
POST [[[URL]]]
[[[BODY]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let val = prompt.variable_defaults.get("body").expect("body has default");
    match val {
        universal_prompt_language::upl::parser::VariableValue::LongString(s) | universal_prompt_language::upl::parser::VariableValue::String(s) => {
            assert_eq!(s, "{\n  \"name\": \"example\",\n  \"active\": true\n}");
        }
        other => panic!("unexpected value kind: {other:?}"),
    }
}

#[test]
fn test_heredoc_allows_any_indentation_in_content() {
    let content = r#"--
name: p
params:
  note:
    type: long_string
    def: >>>
    indented line
plain line
<<<
--
[[[NOTE]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let val = prompt.variable_defaults.get("note").unwrap();
    match val {
        universal_prompt_language::upl::parser::VariableValue::LongString(s) | universal_prompt_language::upl::parser::VariableValue::String(s) => {
            assert_eq!(s, "    indented line\nplain line");
        }
        other => panic!("unexpected value kind: {other:?}"),
    }
}

#[test]
fn test_heredoc_terminator_can_be_indented() {
    let content = "--\nname: p\nparams:\n  note:\n    type: long_string\n    def: >>>\nhello\n    <<<\n--\n[[[NOTE]]]\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    let val = prompt.variable_defaults.get("note").unwrap();
    match val {
        universal_prompt_language::upl::parser::VariableValue::LongString(s) | universal_prompt_language::upl::parser::VariableValue::String(s) => {
            assert_eq!(s, "hello");
        }
        other => panic!("unexpected value kind: {other:?}"),
    }
}

#[test]
fn test_heredoc_on_non_long_string_is_error() {
    let content = r#"--
name: p
params:
  name:
    type: string
    def: >>>
hello
<<<
--
[[[NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::HeredocNotLongString { .. })));
}

#[test]
fn test_heredoc_missing_terminator_is_error() {
    let content = "--\nname: p\nparams:\n  note:\n    type: long_string\n    def: >>>\nhello\n--\n[[[NOTE]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingHeredocTerminator)));
}

// --- Template-body validation tests ---

#[test]
fn test_unmatched_loop_is_error() {
    let res = Template::parse("{{{for X in X}}}body");
    assert!(matches!(res, Err(PromptParseError::UnmatchedConstruct(_))));
}

// --- Uppercase-identifier enforcement (RFC §4.1) ---

#[test]
fn test_lowercase_placeholder_is_error() {
    let res = Template::parse("Hello, [[[name]]]!");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_lowercase_dotted_path_segment_is_error() {
    let res = Template::parse("[[[SERVER.host]]]");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_lowercase_loop_variable_is_error() {
    let res = Template::parse("{{{for item in ITEMS}}}- [[[ITEM]]]\n{{{end for}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_lowercase_loop_list_reference_is_error() {
    let res = Template::parse("{{{for ITEM in items}}}- [[[ITEM]]]\n{{{end for}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_bracketed_loop_list_still_accepted() {
    // `[[[...]]]` wrapping is tolerated for backward compatibility.
    let res = Template::parse("{{{for ITEM in [[[ITEMS]]]}}}- [[[ITEM]]]\n{{{end for}}}");
    assert!(res.is_ok());
}

#[test]
fn test_lowercase_condition_variable_is_error() {
    let res = Template::parse("{{{if include_auth}}}yes{{{end if}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_lowercase_ternary_condition_variable_is_error() {
    let res = Template::parse("{{{file_size > 100 ? \"big\" : \"small\"}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_lowercase_ternary_branch_variable_is_error() {
    let res = Template::parse("{{{FLAG ? [[[name]]] : \"x\"}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_uppercase_identifiers_parse_clean() {
    let res = Template::parse("{{{for ITEM in ITEMS}}}- [[[ITEM]]]\n{{{end for}}}");
    assert!(res.is_ok());
}

#[test]
fn test_unmatched_if_is_error() {
    let res = Template::parse("{{{if true}}}body");
    assert!(matches!(res, Err(PromptParseError::UnmatchedConstruct(_))));
}

#[test]
fn test_unmatched_loop_in_full_prompt_is_error() {
    let content = r#"--
name: p
--
{{{for X in X}}}body
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnmatchedConstruct(_))));
}

#[test]
fn test_unmatched_if_in_full_prompt_is_error() {
    let content = r#"--
name: p
--
{{{if true}}}body
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnmatchedConstruct(_))));
}

// --- Option etype / label / opts validation (RFC §3.1, §3.3, §3.6) ---

#[test]
fn test_opts_on_non_option_type_is_error() {
    let content = r#"--
name: p
params:
  name:
    type: string
    opts:
      - "a"
      - "b"
--
[[[NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidOptsForType)));
}

#[test]
fn test_label_on_non_option_type_is_error() {
    let content = r#"--
name: p
params:
  name:
    type: string
    label: foo
--
[[[NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidLabelForType)));
}

#[test]
fn test_option_single_without_etype_defaults_to_string() {
    let content = r#"--
name: p
params:
  env:
    type: option_single
    opts:
      - "dev"
      - "prod"
    def: "prod"
--
[[[ENV]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let env = prompt.variable_definitions.get("env").unwrap();
    assert_eq!(env.element_type, None); // defaults to string at use time
}

#[test]
fn test_option_multi_without_etype_is_error() {
    let content = r#"--
name: p
params:
  tags:
    type: option_multi
    opts:
      - "a"
      - "b"
--
[[[TAGS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingElementType { .. })));
}

#[test]
fn test_option_single_with_single_opt_is_error() {
    // RFC §3.1: option_* require `opts` with at least two entries.
    let content = r#"--
name: p
params:
  env:
    type: option_single
    opts:
      - "only"
--
[[[ENV]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingOpts(_))));
}

#[test]
fn test_option_multi_with_single_opt_is_error() {
    let content = r#"--
name: p
params:
  tags:
    type: option_multi
    etype: string
    opts:
      - "only"
--
[[[TAGS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingOpts(_))));
}

#[test]
fn test_option_multi_without_opts_is_error() {
    let content = r#"--
name: p
params:
  tags:
    type: option_multi
    etype: string
--
[[[TAGS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingOpts(_))));
}

#[test]
fn test_option_with_boolean_etype_is_error() {
    let content = r#"--
name: p
params:
  flag:
    type: option_single
    etype: boolean
    opts:
      - true
      - false
--
[[[FLAG]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidOptionEtype { .. })));
}

#[test]
fn test_option_single_number_etype_parses() {
    let content = r#"--
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
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let port = prompt.variable_definitions.get("port").unwrap();
    assert_eq!(port.element_type, Some(VariableType::Number));
    let opts = port.options.as_ref().unwrap();
    assert_eq!(opts.len(), 3);
}

#[test]
fn test_option_single_long_string_etype_parses() {
    let content = r#"--
name: p
params:
  body:
    type: option_single
    etype: long_string
    opts:
      - "first paragraph"
      - "second paragraph"
    def: "first paragraph"
--
[[[BODY]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let body = prompt.variable_definitions.get("body").unwrap();
    assert_eq!(body.element_type, Some(VariableType::LongString));
}

#[test]
fn test_option_number_etype_with_string_opt_is_error() {
    let content = r#"--
name: p
params:
  port:
    type: option_single
    etype: number
    opts:
      - 80
      - "not-a-number"
--
[[[PORT]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::OptionEntryTypeMismatch { .. })));
}

#[test]
fn test_option_object_etype_without_label_is_error() {
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
  selected:
    type: option_multi
    etype: feature
    opts:
      - { name: "auth", enabled: true }
      - { name: "logs", enabled: false }
--
[[[SELECTED]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingLabelForObjectEtype { .. })));
}

#[test]
fn test_option_object_etype_with_unknown_label_field_is_error() {
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
  selected:
    type: option_single
    etype: feature
    label: nope
    opts:
      - { name: "auth" }
      - { name: "logs" }
--
[[[SELECTED]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnknownLabelField { .. })));
}

#[test]
fn test_option_object_etype_with_non_string_label_field_is_error() {
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      enabled:
        type: boolean
  selected:
    type: option_single
    etype: feature
    label: enabled
    opts:
      - { enabled: true }
      - { enabled: false }
--
[[[SELECTED]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidLabelFieldType { .. })));
}

#[test]
fn test_option_object_etype_with_label_parses() {
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
      enabled:
        type: boolean
  selected:
    type: option_multi
    etype: feature
    label: name
    opts:
      - { name: "auth", enabled: true }
      - { name: "logs", enabled: false }
--
{{{for F in SELECTED}}}- [[[F.NAME]]]
{{{end for}}}
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let selected = prompt.variable_definitions.get("selected").unwrap();
    assert_eq!(selected.element_type, Some(VariableType::Object));
    assert_eq!(selected.element_ref.as_deref(), Some("feature"));
    assert_eq!(selected.label.as_deref(), Some("name"));
    assert!(selected.ofields_definitions.is_some());
}

#[test]
fn test_option_object_etype_opt_missing_label_field_is_error() {
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
  selected:
    type: option_single
    etype: feature
    label: name
    opts:
      - { enabled: true }
      - { name: "logs" }
--
[[[SELECTED]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::OptionEntryTypeMismatch { .. })));
}

// --- `name` metadata field validation (RFC §2.1) ---

#[test]
fn test_missing_name_is_error() {
    let content = "--\ntitle: t\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingName)));
}

#[test]
fn test_uppercase_name_is_error() {
    let content = "--\nname: My_Prompt\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidName { .. })));
}

#[test]
fn test_name_with_hyphen_or_dot_is_error() {
    for bad in ["my-prompt", "my.prompt", "my prompt", "prompt!"] {
        let content = format!("--\nname: {bad}\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n");
        let res = PromptParser::parse(&content);
        assert!(matches!(res, Err(PromptParseError::InvalidName { .. })), "name '{bad}' should be invalid");
    }
}

#[test]
fn test_name_lowercase_alphanumeric_and_underscore_is_ok() {
    let content = "--\nname: my_prompt_42\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(res.is_ok());
    assert_eq!(res.unwrap().name, "my_prompt_42");
}

#[test]
fn test_name_unicode_lowercase_is_ok() {
    // lowercase UTF-8 letters and digits are allowed.
    let content = "--\nname: café_ñ_3\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(res.is_ok(), "lowercase unicode name should parse");
    assert_eq!(res.unwrap().name, "café_ñ_3");
}

#[test]
fn test_name_unicode_uppercase_is_error() {
    // an uppercase UTF-8 letter is rejected.
    let content = "--\nname: Café\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidName { .. })));
}

use universal_prompt_language::upl::parser::{has_valid_extension, prompt_file_base_name, validate_prompt_file};
use std::path::Path;

#[test]
fn test_has_valid_extension() {
    assert!(has_valid_extension(Path::new("a.txt")));
    assert!(has_valid_extension(Path::new("a.upl")));
    assert!(has_valid_extension(Path::new("/x/y/a.upl")));
    assert!(!has_valid_extension(Path::new("a.md")));
}

#[test]
fn test_prompt_file_base_name() {
    assert_eq!(prompt_file_base_name(Path::new("my_prompt.txt")).as_deref(), Some("my_prompt"));
    assert_eq!(prompt_file_base_name(Path::new("my_prompt.upl")).as_deref(), Some("my_prompt"));
    // legacy .prompt.txt files resolve to the base name too.
    assert_eq!(prompt_file_base_name(Path::new("my_prompt.prompt.txt")).as_deref(), Some("my_prompt"));
}

#[test]
fn test_validate_prompt_file_matches() {
    let content = "--\nname: my_prompt\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let prompt = PromptParser::parse(content).unwrap();
    assert!(validate_prompt_file(&prompt, Path::new("/tmp/my_prompt.txt")).is_ok());
    assert!(validate_prompt_file(&prompt, Path::new("/tmp/my_prompt.upl")).is_ok());
    assert!(validate_prompt_file(&prompt, Path::new("/tmp/my_prompt.prompt.txt")).is_ok());
}

#[test]
fn test_validate_prompt_file_mismatch_is_error() {
    let content = "--\nname: my_prompt\nparams:\n  x:\n    type: string\n--\n[[[X]]]\n";
    let prompt = PromptParser::parse(content).unwrap();
    // wrong base name
    assert!(validate_prompt_file(&prompt, Path::new("/tmp/other.txt")).is_err());
    // wrong extension
    assert!(validate_prompt_file(&prompt, Path::new("/tmp/my_prompt.md")).is_err());
}

