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
fn test_bracketed_variable_in_if_condition_is_error() {
    // `[[[VAR]]]` is not valid in condition expressions (RFC §4.2 / §4.4):
    // conditions use bare uppercase identifiers. Only placeholders in the
    // prompt body and ternary branch value references use `[[[...]]]`.
    let res = Template::parse("{{{if [[[FLAG]]}}}yes{{{end if}}}");
    assert!(matches!(res, Err(PromptParseError::InvalidConditionSyntax(_))));
}

#[test]
fn test_bracketed_variable_in_ternary_condition_is_error() {
    // Same rule for ternary conditions: `[[[AGE]]]` is not allowed; use `AGE`.
    let res = Template::parse("{{{[[[AGE]]] >= 18 ? \"adult\" : \"minor\"}}}");
    assert!(matches!(res, Err(PromptParseError::InvalidConditionSyntax(_))));
}

#[test]
fn test_bracketed_variable_in_operator_condition_is_error() {
    // `[[[VAR]]]` must not appear on either side of a binary operator.
    let res = Template::parse("{{{TEXT contains [[[NEEDLE]]] ? \"y\" : \"n\"}}}");
    assert!(matches!(res, Err(PromptParseError::InvalidConditionSyntax(_))));
}

#[test]
fn test_uppercase_identifiers_parse_clean() {
    let res = Template::parse("{{{for ITEM in ITEMS}}}- [[[ITEM]]]\n{{{end for}}}");
    assert!(res.is_ok());
}

// --- Escaping matched delimiters (RFC §4.5) ---

#[test]
fn test_escaped_matched_braces_parses() {
    // G2 regression: a matched `{{{...}}}` that isn't a valid construct
    // used to always be a hard parse error, with no way to write it
    // literally (e.g. documenting Mustache's `{{{value}}}` syntax).
    let res = Template::parse("code snippet: \\{{{ hello }}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_escaped_matched_brackets_parses() {
    let res = Template::parse("literal: \\[[[ not a real var ]]]");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_escaped_block_tags_inside_loop_body_are_literal() {
    // An escaped `\{{{end for}}}` / `\{{{for ...}}}` inside a loop body is
    // literal text (§4.5) and must not close or open a block.
    let res = Template::parse(
        "{{{for X in XS}}}\nsyntax: \\{{{end for}}} closes a loop\n{{{end for}}}\n",
    );
    assert!(res.is_ok(), "{:?}", res);
    let res = Template::parse(
        "{{{for X in XS}}}\nsyntax: \\{{{for Y in YS}}} opens a loop\n{{{end for}}}\n",
    );
    assert!(res.is_ok(), "{:?}", res);
    let res = Template::parse(
        "{{{if A}}}\nsyntax: \\{{{end if}}} closes a block\n{{{end if}}}\n",
    );
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_unescaped_matched_braces_still_a_parse_error() {
    // The escape is opt-in; a matched `{{{...}}}` that isn't a valid
    // construct and ISN'T escaped remains a parse error (§4.5 baseline).
    let res = Template::parse("code snippet: {{{ hello }}}");
    assert!(matches!(res, Err(PromptParseError::InvalidConditionSyntax(_))));
}

// --- Parenthesized grouping and `and`/`or`/`not` (RFC §5.1, §5.2) ---

#[test]
fn test_parenthesized_condition_parses() {
    // Regression: parentheses previously always failed to parse (the paren
    // branch of `parse_primary` enforced end-of-input before consuming the
    // closing `)`), even though the tokenizer and ternary splitter already
    // supported them.
    let res = Template::parse("{{{(HOURS > 10) ? \"ample\" : \"limited\"}}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_nested_parentheses_condition_parses() {
    let res = Template::parse("{{{((A > 1)) ? \"y\" : \"n\"}}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_and_operator_condition_parses() {
    let res = Template::parse("{{{A > 1 and B < 2 ? \"y\" : \"n\"}}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_or_operator_condition_parses() {
    let res = Template::parse("{{{A = \"pro\" or A = \"enterprise\" ? \"y\" : \"n\"}}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_not_keyword_condition_parses() {
    let res = Template::parse("{{{if not SUSPENDED}}}ok{{{end if}}}");
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_grouped_and_or_not_condition_parses() {
    // The RFC §5.2 example verbatim.
    let res = Template::parse(
        "{{{if (TIER = \"pro\" or TIER = \"enterprise\") and not SUSPENDED}}}ok{{{end if}}}",
    );
    assert!(res.is_ok(), "{:?}", res);
}

#[test]
fn test_lowercase_variable_inside_parens_is_error() {
    // Uppercase enforcement (§4.1) must still apply to variables nested
    // inside a parenthesized group.
    let res = Template::parse("{{{(hours > 10) ? \"y\" : \"n\"}}}");
    assert!(matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })));
}

#[test]
fn test_exclude_condition_with_and_or_not_parens_parses() {
    let content = r#"--
name: p
params:
  tier:
    type: string
    def: "free"
  suspended:
    type: boolean
    def: false
  other:
    type: string
    exclude_condition: (TIER = "pro" or TIER = "enterprise") and not SUSPENDED
    def: "x"
--
[[[OTHER]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("other").unwrap();
    assert!(vd.exclude_condition.is_some());
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
params:
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
params:
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
fn test_label_on_scalar_etype_option_is_error() {
    // RFC §3.3: `label` is not allowed for scalar etypes.
    let content = r#"--
name: p
params:
  env:
    type: option_single
    label: whatever
    opts:
      - "a"
      - "b"
--
[[[ENV]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::InvalidLabelForType)), "{:?}", res);
}

#[test]
fn test_option_default_must_be_one_of_opts() {
    let content = r#"--
name: p
params:
  env:
    type: option_single
    opts:
      - "dev"
      - "prod"
    def: "staging"
--
[[[ENV]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::DefaultNotInOpts { .. })), "{:?}", res);

    let content = r#"--
name: p
params:
  tags:
    type: option_multi
    etype: string
    opts:
      - "a"
      - "b"
    def: ["a", "z"]
--
[[[TAGS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::DefaultNotInOpts { .. })), "{:?}", res);

    // Object-etype defaults are compared by value.
    let content = r#"--
name: p
params:
  feature:
    type: object_shape
    ofields:
      name:
        type: string
  pick:
    type: option_single
    etype: feature
    label: name
    opts:
      - { name: "auth" }
      - { name: "logs" }
    def: { name: "nope" }
--
[[[PICK.NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::DefaultNotInOpts { .. })), "{:?}", res);
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

// --- Same `label` rules for the INLINE `object` etype (RFC §3.3/§3.6, E6) ---
//
// The RFC used to say `label` is "ignored" for the inline `object` etype,
// but the interactive builder needs one just as much as it does for a
// referenced `object_shape` — there's nothing about writing the shape
// inline instead of by name that removes the need for a menu label.

#[test]
fn test_option_inline_object_etype_without_label_is_error() {
    // E6 regression: this used to parse successfully (and non-interactive
    // builds succeeded too), only crashing later when actually building
    // interactively, since the picker had no field to display.
    let content = r#"--
name: p
params:
  choice:
    type: option_single
    etype: object
    ofields:
      name:
        type: string
    opts:
      - { name: "a" }
      - { name: "b" }
--
[[[CHOICE.NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingLabelForObjectEtype { .. })), "{:?}", res);
}

#[test]
fn test_option_inline_object_etype_with_unknown_label_field_is_error() {
    let content = r#"--
name: p
params:
  choice:
    type: option_single
    etype: object
    label: bogus
    ofields:
      name:
        type: string
    opts:
      - { name: "a" }
      - { name: "b" }
--
[[[CHOICE.NAME]]]
"#;
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnknownLabelField { .. })), "{:?}", res);
}

#[test]
fn test_option_inline_object_etype_with_label_parses() {
    let content = r#"--
name: p
params:
  choice:
    type: option_single
    etype: object
    label: name
    ofields:
      name:
        type: string
    opts:
      - { name: "a" }
      - { name: "b" }
--
[[[CHOICE.NAME]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let choice = prompt.variable_definitions.get("choice").unwrap();
    assert_eq!(choice.label.as_deref(), Some("name"));
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

#[test]
fn test_list_inline_object_etype_without_ofields_is_error() {
    let content = r#"--
name: p
params:
  items:
    type: list
    etype: object
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::MissingOfieldsForObjectEtype { .. })),
        "inline etype: object without ofields should be a clean error, not a panic; got {res:?}"
    );
}

#[test]
fn test_option_multi_inline_object_etype_without_ofields_is_error() {
    let content = r#"--
name: p
params:
  items:
    type: option_multi
    etype: object
    opts:
      - {}
      - {}
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::MissingOfieldsForObjectEtype { .. })),
        "option_multi with inline etype: object without ofields should be a clean error; got {res:?}"
    );
}

#[test]
fn test_list_inline_object_etype_with_ofields_parses() {
    let content = r#"--
name: p
params:
  items:
    type: list
    etype: object
    ofields:
      name:
        type: string
      qty:
        type: number
--
{{{for I in ITEMS}}}- [[[I.NAME]]]: [[[I.QTY]]]
{{{end for}}}
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let items = prompt.variable_definitions.get("items").unwrap();
    assert_eq!(items.element_type, Some(VariableType::Object));
    assert!(items.ofields_definitions.is_some());
    let ofields = items.ofields_definitions.as_ref().unwrap();
    assert!(ofields.contains_key("name"));
    assert!(ofields.contains_key("qty"));
}

// --- Bare literal tokens (RFC §3.3.1) ---

#[test]
fn test_bare_nan_and_inf_tokens_are_strings() {
    // Rust's float parser accepts these words, but per §3.3.1 a bare token
    // that is not a number is a string.
    for word in ["nan", "NaN", "inf", "infinity", "Infinity", "-inf"] {
        let content = format!("--\nname: p\nparams:\n  s:\n    type: string\n    def: {word}\n--\n[[[S]]]\n");
        let prompt = PromptParser::parse(&content).unwrap_or_else(|e| panic!("{word}: {e}"));
        assert_eq!(
            prompt.variable_defaults.get("s"),
            Some(&universal_prompt_language::upl::parser::VariableValue::String(word.to_string()))
        );
    }
    // Real numbers still parse as numbers.
    let content = "--\nname: p\nparams:\n  n:\n    type: number\n    def: -1.5e3\n--\n[[[N]]]\n";
    let prompt = PromptParser::parse(content).unwrap();
    assert_eq!(
        prompt.variable_defaults.get("n"),
        Some(&universal_prompt_language::upl::parser::VariableValue::Number(-1500.0))
    );
}

// --- Duplicate declarations ---

#[test]
fn test_duplicate_param_name_is_error() {
    // A second declaration used to silently replace the first (while the
    // first's `def` lingered in the defaults map).
    let content = "--\nname: p\nparams:\n  x:\n    type: string\n    def: \"a\"\n  x:\n    type: string\n    def: \"b\"\n--\n[[[X]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::DuplicateVariable { .. })), "{:?}", res);
}

#[test]
fn test_duplicate_nested_field_name_is_error() {
    let content = "--\nname: p\nparams:\n  o:\n    type: object\n    ofields:\n      f:\n        type: string\n      f:\n        type: number\n--\n[[[O.F]]]\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::DuplicateVariable { .. })), "{:?}", res);
}

// --- `name` metadata field validation (RFC §2.1) ---

#[test]
fn test_uppercase_or_punctuated_param_declaration_is_error() {
    // RFC §2: declarations in params are lowercase identifiers.
    for bad in ["UserName", "my-var", "my var", "x.y"] {
        let content = format!("--\nname: p\nparams:\n  {bad}:\n    type: string\n--\nbody\n");
        let res = PromptParser::parse(&content);
        assert!(
            matches!(res, Err(PromptParseError::InvalidVariableName { .. })),
            "declaration '{bad}' should be rejected: {:?}",
            res
        );
    }
    // Nested field names follow the same rule.
    let content = "--\nname: p\nparams:\n  o:\n    type: object\n    ofields:\n      Field:\n        type: string\n--\nbody\n";
    assert!(matches!(PromptParser::parse(content), Err(PromptParseError::InvalidVariableName { .. })));
    // Lowercase unicode and digits are fine.
    let content = "--\nname: p\nparams:\n  año_2:\n    type: string\n    def: \"x\"\n--\n[[[AÑO_2]]]\n";
    assert!(PromptParser::parse(content).is_ok());
}

#[test]
fn test_missing_params_is_error() {
    // RFC §2.1: `params` is required (it may be empty, but must be present).
    let content = "--\nname: p\ntitle: t\n--\nhello\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::MissingParams)), "{:?}", res);
    // Empty params blocks are fine.
    assert!(PromptParser::parse("--\nname: p\nparams:\n--\nhello\n").is_ok());
    assert!(PromptParser::parse("--\nname: p\nparams: {}\n--\nhello\n").is_ok());
}

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

// --- Build-time `exclude_condition` field validation (RFC §3.7 / §9.5a) ---

#[test]
fn test_condition_valid_parses() {
    let content = r#"--
name: p
params:
  credit_card_type:
    type: option_single
    opts:
      - "visa"
      - "mastercard"
    def: "visa"
  visa_card_expiry_date:
    type: string
    desc: "Expiry date for Visa card"
    exclude_condition: CREDIT_CARD_TYPE != "visa"
    def: "12/25"
--
Card: [[[CREDIT_CARD_TYPE]]] Expiry: [[[VISA_CARD_EXPIRY_DATE]]]
--
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("visa_card_expiry_date").unwrap();
    assert!(vd.exclude_condition.is_some(), "condition should be parsed");
}

#[test]
fn test_condition_with_multiple_vars_parses() {
    let content = r#"--
name: p
params:
  a:
    type: string
    def: "x"
  b:
    type: number
    def: 10
  c:
    type: string
    exclude_condition: B > 5
    def: "c"
--
[[[C]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("c").unwrap();
    assert!(vd.exclude_condition.is_some());
}

// --- List `etype` validation (RFC §3.1 / §3.3) ---

#[test]
fn test_list_without_etype_is_error() {
    // RFC §3.1: `list` requires `etype`.
    let content = r#"--
name: p
params:
  items:
    type: list
    def: ["a", "b"]
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::MissingElementType { .. })),
        "list without etype should be error: {:?}",
        res
    );
}

#[test]
fn test_list_with_invalid_etype_list_is_error() {
    // RFC §3.3: `list` is not a valid list etype.
    let content = r#"--
name: p
params:
  items:
    type: list
    etype: list
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::InvalidListEtype { .. })),
        "etype: list on a list should be error: {:?}",
        res
    );
}

#[test]
fn test_list_with_invalid_etype_option_single_is_error() {
    // RFC §3.3: `option_single` is not a valid list etype.
    let content = r#"--
name: p
params:
  items:
    type: list
    etype: option_single
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::InvalidListEtype { .. })),
        "etype: option_single on a list should be error: {:?}",
        res
    );
}

#[test]
fn test_list_with_invalid_etype_object_shape_literal_is_error() {
    // RFC §3.3: `object_shape` (the literal type name) is not a valid list
    // etype — one must use `etype: <object_shape_name>` (a reference).
    let content = r#"--
name: p
params:
  items:
    type: list
    etype: object_shape
--
[[[ITEMS]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::InvalidListEtype { .. })),
        "etype: object_shape (literal) on a list should be error: {:?}",
        res
    );
}

#[test]
fn test_list_with_boolean_etype_is_ok() {
    // RFC §3.3: `boolean` IS a valid list etype.
    let content = r#"--
name: p
params:
  flags:
    type: list
    etype: boolean
    def: [true, false, true]
--
[[[FLAGS]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let flags = prompt.variable_definitions.get("flags").unwrap();
    assert_eq!(flags.element_type, Some(VariableType::Boolean));
}


#[test]
fn test_condition_not_operator_parses() {
    let content = r#"--
name: p
params:
  flag:
    type: boolean
    def: true
  other:
    type: string
    exclude_condition: !FLAG
    def: "hello"
--
[[[OTHER]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("other").unwrap();
    assert!(vd.exclude_condition.is_some());
}

#[test]
fn test_condition_contains_operator_parses() {
    let content = r#"--
name: p
params:
  text:
    type: string
    def: "hello world"
  other:
    type: string
    exclude_condition: TEXT contains "world"
    def: "x"
--
[[[OTHER]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("other").unwrap();
    assert!(vd.exclude_condition.is_some());
}

#[test]
fn test_condition_forward_reference_is_error() {
    // A condition referencing a parameter declared AFTER it is a parse error.
    let content = r#"--
name: p
params:
  b:
    type: string
    exclude_condition: A = "x"
    def: "b"
  a:
    type: string
    def: "x"
--
[[[A]]] [[[B]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionRefersToLaterParam { .. })),
        "forward reference in condition should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_self_reference_is_error() {
    let content = r#"--
name: p
params:
  a:
    type: string
    exclude_condition: A = "x"
    def: "a"
--
[[[A]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionRefersToLaterParam { .. })),
        "self reference in condition should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_undeclared_variable_is_error() {
    let content = r#"--
name: p
params:
  a:
    type: string
    exclude_condition: UNDECLARED = "x"
    def: "a"
--
[[[A]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionRefersToUndeclared { .. })),
        "undeclared variable in condition should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_on_object_shape_is_error() {
    let content = r#"--
name: p
params:
  shape:
    type: object_shape
    exclude_condition: SOME_VAR = "x"
    ofields:
      x:
        type: string
--
[[[X]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOnObjectShape { .. })),
        "condition on object_shape should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_on_nested_field_is_error() {
    let content = r#"--
name: p
params:
  obj:
    type: object
    ofields:
      x:
        type: string
        exclude_condition: SOME_VAR = "y"
        def: "x"
--
[[[OBJ.X]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOnNestedField { .. })),
        "condition on nested field should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_lowercase_variable_is_error() {
    let content = r#"--
name: p
params:
  a:
    type: string
    def: "x"
  b:
    type: string
    exclude_condition: a = "x"
    def: "b"
--
[[[A]]] [[[B]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::LowercaseIdentifier { .. })),
        "lowercase variable in condition should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_invalid_syntax_is_error() {
    let content = r#"--
name: p
params:
  a:
    type: string
    def: "x"
  b:
    type: string
    exclude_condition: A =
    def: "b"
--
[[[B]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        res.is_err(),
        "invalid condition syntax should be error: {:?}",
        res
    );
}

#[test]
fn test_condition_with_number_comparison_parses() {
    let content = r#"--
name: p
params:
  port:
    type: number
    def: 80
  use_ssl:
    type: boolean
    exclude_condition: PORT != 443
    def: false
--
Port: [[[PORT]]] SSL: [[[USE_SSL]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("use_ssl").unwrap();
    assert!(vd.exclude_condition.is_some());
}

// --- Static `contains`/`starts_with`/`ends_with` operand-type checks ---
//
// These apply to prompt-body conditions (ternary/if), which are validated by
// `PromptParser::parse` -> `validate_body_references` -> `validate_cond`,
// against the declared `params` types (RFC §5). They deliberately do NOT
// apply to `exclude_condition` — that's validated by the separate,
// Shape-unaware `validate_conditions` (name/order checks only).

#[test]
fn test_contains_static_check_list_left_rejects_list_right() {
    let content = r#"--
name: p
params:
  a:
    type: list
    etype: number
    def: [1, 2]
  b:
    type: list
    etype: number
    def: [1]
--
{{{A contains B ? "yes" : "no"}}}
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOperatorTypeError { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_contains_static_check_string_left_rejects_list_right() {
    // A list on the right does NOT fall back to membership testing when the
    // left operand is a string — only a list on the LEFT does that.
    let content = r#"--
name: p
params:
  text:
    type: string
    def: "hello world"
  items:
    type: list
    etype: string
    def: ["a", "b"]
--
{{{TEXT contains ITEMS ? "yes" : "no"}}}
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOperatorTypeError { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_contains_static_check_rejects_non_list_non_string_left() {
    let content = r#"--
name: p
params:
  n:
    type: number
    def: 5
--
{{{N contains "x" ? "yes" : "no"}}}
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOperatorTypeError { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_starts_with_static_check_rejects_non_string_right() {
    let content = r#"--
name: p
params:
  path:
    type: string
    def: "/home/me"
  n:
    type: number
    def: 5
--
{{{PATH starts_with N ? "yes" : "no"}}}
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOperatorTypeError { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_contains_static_check_valid_cases_parse() {
    let content = r#"--
name: p
params:
  tags:
    type: list
    etype: string
    def: ["api", "web"]
  text:
    type: string
    def: "hello world"
--
{{{TAGS contains "api" ? "yes" : "no"}}} {{{TEXT contains "hello" ? "yes" : "no"}}}
"#;
    assert!(PromptParser::parse(content).is_ok());
}

#[test]
fn test_contains_static_check_allows_operand_of_unknown_type() {
    // A reference to an undeclared root variable is tolerated at parse time
    // (RFC §3.5: its value, and so its type, may be supplied only at render
    // time), so the static check must not flag it.
    let content = r#"--
name: p
params:
  tags:
    type: list
    etype: string
    def: ["api", "web"]
--
{{{TAGS contains NEEDLE ? "yes" : "no"}}}
"#;
    assert!(PromptParser::parse(content).is_ok());
}

#[test]
fn test_contains_static_check_resolves_dotted_object_field_as_list() {
    // A `list`-typed field nested inside an object must be recognized as a
    // list for the static check, not collapsed to its element type (which is
    // what the field's `Shape` alone would give).
    let content = r#"--
name: p
params:
  model:
    type: object
    ofields:
      tags:
        type: list
        etype: string
        def: ["x"]
    def: {}
--
{{{MODEL.TAGS contains "x" ? "yes" : "no"}}}
"#;
    assert!(PromptParser::parse(content).is_ok());
}

#[test]
fn test_contains_static_check_rejects_bare_loop_item_object() {
    // Inside a `for` loop over a list of objects, the bare loop item is an
    // object — an invalid left operand for `contains` on its own (a field of
    // it could be, e.g. `ENDPOINT.METHOD contains "G"`, but the item itself
    // is not a list or a string).
    let content = r#"--
name: p
params:
  endpoints:
    type: list
    etype: object
    ofields:
      method:
        type: string
        def: "GET"
    def: [{method: "GET"}]
--
{{{for ENDPOINT in ENDPOINTS}}}
{{{ENDPOINT contains "x" ? "yes" : "no"}}}
{{{end for}}}
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ConditionOperatorTypeError { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_condition_case_insensitive_reference_parses() {
    // Condition variable references must be uppercase; the matching against
    // the declared lowercase name is case-insensitive.
    let content = r#"--
name: p
params:
  my_var:
    type: string
    def: "test"
  other:
    type: string
    exclude_condition: MY_VAR = "test"
    def: "o"
--
[[[OTHER]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    let vd = prompt.variable_definitions.get("other").unwrap();
    assert!(vd.exclude_condition.is_some());
}

// --- Content after the body terminator (RFC §2, G3) ---

#[test]
fn test_content_after_body_terminator_is_error() {
    // G3 regression: a bare '--' line inside the body used to silently
    // truncate everything after it. It must now be a parse error instead.
    let content = r#"--
name: p
params:
  x:
    type: string
    def: "hi"
--
Divider below:
--
This part used to vanish silently.
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ContentAfterBodyTerminator(_))),
        "{:?}",
        res
    );
}

#[test]
fn test_trailing_blank_lines_after_body_terminator_are_ok() {
    // A blank line (or several) after the terminator — e.g. a final
    // newline at EOF — is harmless and must not be flagged.
    let content = "--\nname: p\nparams:\n  x:\n    type: string\n    def: \"hi\"\n--\nBody text.\n--\n\n\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    assert_eq!(prompt.prompt, "Body text.\n");
}

#[test]
fn test_body_without_trailing_terminator_still_parses() {
    // The trailing '--' is optional; a file with no closing terminator at
    // all must still parse (the whole remainder is the body).
    let content = "--\nname: p\nparams:\n  x:\n    type: string\n    def: \"hi\"\n--\nBody text.\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    assert_eq!(prompt.prompt, "Body text.\n");
}

// --- Header delimiter exactness (RFC §2, G9) ---

#[test]
fn test_leading_triple_dash_is_error() {
    // G9 regression: §2 requires the delimiter to be a line containing
    // *exactly* '--'; the opening-delimiter check used to accept any
    // '--'-prefixed line (e.g. '---'), silently treating it as the opener.
    let content = "---\nname: p\nparams:\n  x:\n    type: string\n    def: \"hi\"\n--\nBody text.\n";
    let res = PromptParser::parse(content);
    assert!(matches!(res, Err(PromptParseError::UnexpectedLine(_))), "{:?}", res);
}

#[test]
fn test_exact_leading_delimiter_still_parses() {
    let content = "--\nname: p\nparams:\n  x:\n    type: string\n    def: \"hi\"\n--\nBody text.\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    assert_eq!(prompt.prompt, "Body text.\n");
}

#[test]
fn test_omitted_leading_delimiter_still_parses() {
    // The leading '--' is optional, not just exact-or-nothing: a file may
    // begin directly with its first metadata key.
    let content = "name: p\nparams:\n  x:\n    type: string\n    def: \"hi\"\n--\nBody text.\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    assert_eq!(prompt.prompt, "Body text.\n");
}

// --- Built-in type names are reserved for object_shape names (RFC §3.3/§3.4.2, E9) ---

#[test]
fn test_object_shape_named_after_builtin_type_is_error() {
    // E9 regression: an object_shape named e.g. 'string' used to parse
    // without complaint, but was permanently unreferenceable — any
    // `type: string`/`etype: string` always resolves to the built-in type
    // first, so the shape could never actually be used. Now rejected at
    // declaration.
    let content = r#"--
name: p
params:
  string:
    type: object_shape
    ofields:
      value:
        type: string
        def: "hello"
  x:
    type: string
    def: "hi"
--
[[[X]]]
"#;
    let res = PromptParser::parse(content);
    assert!(
        matches!(res, Err(PromptParseError::ReservedObjectShapeName { .. })),
        "{:?}",
        res
    );
}

#[test]
fn test_object_shape_named_after_each_builtin_type_is_error() {
    for kw in [
        "string",
        "long_string",
        "number",
        "boolean",
        "list",
        "object",
        "object_shape",
        "option_single",
        "option_multi",
    ] {
        let content = format!(
            "--\nname: p\nparams:\n  {kw}:\n    type: object_shape\n    ofields:\n      v:\n        type: string\n--\nbody\n"
        );
        let res = PromptParser::parse(&content);
        assert!(
            matches!(res, Err(PromptParseError::ReservedObjectShapeName { .. })),
            "expected '{kw}' to be rejected as an object_shape name: {:?}",
            res
        );
    }
}

#[test]
fn test_object_shape_with_normal_name_still_parses() {
    let content = r#"--
name: p
params:
  server:
    type: object_shape
    ofields:
      host:
        type: string
        def: "localhost"
  cfg:
    type: server
--
Host: [[[CFG.HOST]]]
"#;
    let prompt = PromptParser::parse(content).expect("should parse");
    assert!(prompt.variable_definitions.get("cfg").is_some());
}

#[test]
fn test_plain_variable_named_after_builtin_type_still_parses() {
    // The reservation applies only to `object_shape` names — a plain
    // `string`-typed variable named e.g. 'list' causes no shadowing
    // ambiguity (it's never referenced *by name* as a type) and remains
    // allowed.
    let content = "--\nname: p\nparams:\n  list:\n    type: string\n    def: \"hi\"\n--\n[[[LIST]]]\n";
    let prompt = PromptParser::parse(content).expect("should parse");
    assert!(prompt.variable_definitions.get("list").is_some());
}

