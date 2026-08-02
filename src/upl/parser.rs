// prompt_parser.rs
//
// Strictly validated parser for universal prompt files.
//
// Features:
//   - Strict parsing of header, params, body
//   - Enforces indentation (only spaces)
//   - Validates all types, nested structures, conditionals, loops
//   - Supports `[[[var]]]`, `{{{cond ? a : b}}}`, `{{{for x in list}}}`
//   - Raises descriptive errors on invalid input
//   - Includes `print_prompt` for debugging

use std::collections::{HashMap, HashSet};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Data Model

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum VariableType {
    String,
    LongString,
    Number,
    Boolean,
    List,
    Object,
    ObjectShape,
    OptionSingle,
    OptionMulti,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum VariableValue {
    String(String),
    LongString(String),
    Number(f64),
    Boolean(bool),
    List(Vec<VariableValue>),
    // An ordered map preserving field declaration order (RFC §7.3 / §4.6.1).
    // `IndexMap` (not `BTreeMap`) is used so object rendering and iteration
    // follow the order fields were authored in `ofields` / an object literal.
    Object(IndexMap<String, VariableValue>),
}

/// Ordered map of object field name → value, preserving declaration order.
pub type ObjectMap = IndexMap<String, VariableValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableDefinition {
    pub r#type: VariableType,
    pub desc: Option<String>,
    pub options: Option<Vec<VariableValue>>,
    pub element_type: Option<VariableType>,
    /// Name of a declared `object_shape` variable referenced via
    /// `etype: <name>` (RFC §3.4). Resolved into `element_type`/
    /// `ofields_definitions` at parse time. `None` after successful resolution
    /// (kept populated until `resolve_element_refs` runs).
    pub element_ref: Option<String>,
    /// For `option_single`/`option_multi` with an `object_shape` etype: the
    /// name of a field on the referenced object_shape whose value is shown as
    /// the menu label for each option (RFC §3.6). Required when the etype is a
    /// referenced object_shape; ignored for scalar etypes and the inline
    /// `object` etype.
    pub label: Option<String>,
    /// Name of a declared `object_shape` variable whose `ofields` an `object`
    /// Name of a declared `object_shape` variable referenced via
    /// `type: <name>` (RFC §3.4.2). When set, the variable is an object whose
    /// `ofields` are spliced in from the referenced object_shape at parse
    /// time. Resolved into `ofields_definitions`. Kept populated for
    /// downstream consumers.
    pub type_ref: Option<String>,
    pub ofields_definitions: Option<VariableDefinitions>,
}

pub type VariableDefinitions = IndexMap<String, VariableDefinition>;
pub type VariableDefaults = HashMap<String, VariableValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub title: Option<String>,
    pub desc: Option<String>,
    /// Optional provenance field set when a prompt is pulled from a remote
    /// repository (see `repository_client::pull`). Format:
    /// `<host>/<username>/<prompt_name>`.
    pub source: Option<String>,
    pub prompt: String,
    #[serde(skip, default)]
    pub template: Template,
    pub variable_definitions: VariableDefinitions,
    pub variable_defaults: VariableDefaults,
}

// --- Template AST (body of a prompt) ---
//
// `Template` is the fully parsed, validated representation of a prompt body.
// It is produced by `Template::parse`, which is invoked from `PromptParser`
// so that every `Prompt` handed to the builder already carries a structured,
// error-free body. The builder therefore only collects values and renders the
// nodes; it never re-parses the body string at render time.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Template {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Node {
    Text(String),
    Placeholder(String),
    Ternary {
        cond: CondExpr,
        true_branch: String,
        false_branch: String,
    },
    Loop {
        item: String,
        list: String,
        body: Vec<Node>,
    },
    If {
        cond: CondExpr,
        body: Vec<Node>,
    },
}

/// Condition expression AST. Parsed and validated at template-parse time so
/// the evaluator only needs to walk it against runtime values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CondExpr {
    Literal(VariableValue),
    Var(String),
    Not(Box<CondExpr>),
    Bin {
        op: String,
        left: Box<CondExpr>,
        right: Box<CondExpr>,
    },
}

// --- Error Types ---
#[derive(Error, Debug, Clone)]
pub enum PromptParseError {
    #[error("File missing header delimiter '--' at start")]
    MissingHeaderDelimiter,
    #[error("File missing closing delimiter '--' after header")]
    MissingClosingHeaderDelimiter,
    #[error("Invalid header key: '{0}'")]
    InvalidHeaderKey(String),
    #[error("Missing required 'name' metadata field")]
    MissingName,
    #[error("Invalid 'name' value '{name}': must be non-empty and contain only lowercase alphanumeric (UTF-8) characters and underscores")]
    InvalidName { name: String },
    #[error("prompt file must use the '.txt' or '.upl' extension")]
    InvalidExtension { path: String },
    #[error("prompt name '{name}' does not match file base name '{base}'")]
    NameFileMismatch { name: String, base: String },
    #[error("Expected key-value pair but found: '{0}'")]
    UnexpectedLine(String),
    #[error("Expected '{expected}' but got '{got}'")]
    ExpectedButGot { expected: String, got: String },
    #[error("Invalid indentation: expected {expected} spaces but got {actual}")]
    IndentationError { expected: usize, actual: usize },
    #[error("Nested 'ofields' must only appear in 'object' or 'object_shape' type")]
    InvalidNestedInNonObject { field: String },
    #[error("Field '{field}' has invalid type '{value}'")]
    InvalidTypeField { field: String, value: String },
    #[error("Missing 'etype' for type '{type_name}'")]
    MissingElementType { type_name: String },
    #[error("Element reference '{name}' does not resolve to a declared variable")]
    UnresolvedElementRef { name: String },
    #[error("Element reference '{name}' must point to an 'object_shape' variable with 'ofields' (referencing an 'object' is not allowed; only object_shape is referenceable by name)")]
    InvalidElementRef { name: String },
    #[error("`type: {name}` must name a declared `object_shape` variable with `ofields` (referencing an `object` is not allowed; built-in type names are reserved)")]
    InvalidTypeRef { name: String },
    #[error("Circular element reference involving '{name}'")]
    CircularElementRef { name: String },
    #[error("Option list 'opts' only allowed for 'option_single' and 'option_multi'")]
    InvalidOptsForType,
    #[error("Option type '{0}' requires an 'opts' list with at least two entries")]
    MissingOpts(String),
    #[error("Invalid etype '{etype}' for option type '{kind}': allowed etypes are string, long_string, number, object (inline), or a referenced object_shape")]
    InvalidOptionEtype { etype: String, kind: String },
    #[error("Option entry #{index} does not match etype '{etype}': {value}")]
    OptionEntryTypeMismatch { index: usize, etype: String, value: String },
    #[error("Option 'label' is only allowed for 'option_single' and 'option_multi' with an object_shape etype")]
    InvalidLabelForType,
    #[error("Option 'label' is required for '{path}' because etype is a referenced object_shape")]
    MissingLabelForObjectEtype { path: String },
    #[error("Option 'label' field '{label}' is not declared on referenced object_shape '{obj}'")]
    UnknownLabelField { label: String, obj: String },
    #[error("Option 'label' field '{label}' on object_shape '{obj}' must be string or long_string, got {got:?}")]
    InvalidLabelFieldType { label: String, obj: String, got: VariableType },
    #[error("'ofields' is only allowed on 'object' and 'object_shape' types (got {type_name})")]
    InvalidOfieldsForType { type_name: String },
    #[error("'object_shape' variable '{name}' requires an 'ofields' block")]
    ObjectShapeMissingOfields { name: String },
    #[error("Invalid value for 'def': {value} (expected type: {expected_type:?})")]
    InvalidDefaultValue { value: String, expected_type: VariableType },
    #[error("Default for '{path}' (type {declared:?}) has wrong value kind: {value}")]
    DefTypeMismatch {
        path: String,
        declared: VariableType,
        value: String,
    },
    #[error("Heredoc 'def: >>>' is only allowed for 'long_string' variables")]
    HeredocNotLongString { field: String },
    #[error("Heredoc 'def: >>>' is missing its terminating '<<<' line")]
    MissingHeredocTerminator,
    #[error("Loop 'for' missing 'end for'")]
    MissingEndFor,
    #[error("Unmatched '{{{{{{' found in body")]
    UnmatchedBegin,
    #[error("Unmatched '}}}}}}' found in body")]
    UnmatchedEnd,
    #[error("Missing 'for' in loop pattern")]
    MissingForIn,
    #[error("Invalid condition syntax: {0}")]
    InvalidConditionSyntax(String),
    #[error("Unmatched template construct: {0}")]
    UnmatchedConstruct(String),
    #[error("Body references undeclared variable '{name}' (in {ctx})")]
    UndeclaredVariable { name: String, ctx: String },
    #[error("Body references unknown field '{field}' on '{object}' (in {ctx})")]
    UnknownField {
        field: String,
        object: String,
        ctx: String,
    },
    #[error("Expected variable name after 'in' in loop")]
    MissingListVariable,
    #[error("Unexpected '{0}' inside variable placeholder")]
    UnexpectedInVariable(String),
    #[error("Identifier '{name}' in {ctx} must be uppercase (e.g. [[[VAR_NAME]]]); variable declarations stay lowercase")]
    LowercaseIdentifier { name: String, ctx: String },
    #[error("File could not be read: {0}")]
    Io(String),
}

// --- Parsing Context ---
struct ParseContext {
    line_num: usize,
    content: Vec<String>,
    pos: usize,
    errors: Vec<PromptParseError>,
}

// --- Body reference validation helpers (RFC §9 step 4) ---
//
// `Shape` is the resolved form a binding exposes to dotted-path traversal. It
// holds references into the stable `variable_definitions` maps (owned by the
// caller for the duration of validation), so it borrows without allocation.
#[derive(Clone, Copy)]
enum Shape<'a> {
    /// A scalar value of the given type, or a list of scalars (the element
    /// type). No further dotted segment can resolve against it.
    Scalar(VariableType),
    /// An object whose fields are the given `ofields` map. Either a declared
    /// `object`, an object field of an `object`, or the element shape of a
    /// `list`/`option_multi` whose etype is an object.
    Object(&'a VariableDefinitions),
}

/// A name visible in the body (a declared variable or a loop variable) paired
/// with its resolved `Shape`.
struct Binding<'a> {
    name: String,
    shape: Shape<'a>,
}

// --- Extract value from key-value line ---
fn extract_kv(line: &str) -> Result<(String, String), PromptParseError> {
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(PromptParseError::UnexpectedLine(line.to_string()));
    }
    let key = parts[0].trim().to_string();
    let val = parts[1].trim().to_string();
    Ok((key, val))
}

// --- Name validation (RFC §2.1) ---
//
// A valid `name` is non-empty and contains only lowercase alphanumeric
// (UTF-8) characters and underscores. Uppercase letters, titlecase letters,
// hyphens, dots and any other punctuation are rejected. Digits of any script
// are allowed (they have no case).
pub fn is_valid_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars().all(is_valid_name_char)
}

fn is_valid_name_char(c: char) -> bool {
    if c == '_' {
        return true;
    }
    if !c.is_alphanumeric() {
        return false;
    }
    if c.is_uppercase() {
        return false;
    }
    // Reject titlecase letters (alphabetic but neither upper nor lower).
    if c.is_alphabetic() && !c.is_lowercase() {
        return false;
    }
    true
}

// --- File name / extension helpers (RFC §2) ---
//
// The `name` field MUST match the file's base name. The base name is the
// file stem with a trailing `.prompt` segment removed (so legacy
// `<name>.prompt.txt` files produced by `upl pull` resolve to `<name>`),
// and the file MUST use the `.txt` or `.upl` extension.
pub fn has_valid_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "txt" || e == "upl")
        .unwrap_or(false)
}

/// Return the base name a UPL file's `name` field must match: the file stem
/// with a trailing `.prompt` segment removed. Returns `None` if the path has
/// no file stem.
pub fn prompt_file_base_name(path: &std::path::Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let base = stem.strip_suffix(".prompt").unwrap_or(stem);
    Some(base.to_string())
}

/// Validate that the file at `path` has a permitted extension (`.txt` or
/// `.upl`) and that its base name matches the prompt's `name` field.
pub fn validate_prompt_file(prompt: &Prompt, path: &std::path::Path) -> Result<(), PromptParseError> {
    if !has_valid_extension(path) {
        return Err(PromptParseError::InvalidExtension {
            path: path.display().to_string(),
        });
    }
    match prompt_file_base_name(path) {
        Some(base) if base == prompt.name => Ok(()),
        Some(base) => Err(PromptParseError::NameFileMismatch {
            name: prompt.name.clone(),
            base,
        }),
        None => Err(PromptParseError::InvalidExtension {
            path: path.display().to_string(),
        }),
    }
}

/// Split `s` on `sep` at the top nesting level, ignoring occurrences inside
/// string literals or nested `{}`/`[]` brackets. Used to parse inline list
/// and object literals whose values may themselves contain commas/colons.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            ch if ch == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

// --- Main Parser ---
pub struct PromptParser;

impl PromptParser {
    pub fn parse(content: &str) -> Result<Prompt, PromptParseError> {
        let mut ctx = ParseContext {
            line_num: 0,
            content: content.lines().map(|l| l.to_string()).collect(),
            pos: 0,
            errors: vec![],
        };

        // Validate header
        let header = Self::validate_header(&mut ctx)?;

        // Validate the `name` metadata field (RFC §2.1): required, lowercase
        // alphanumeric (UTF-8) + underscores only.
        let name = match header.get("name") {
            Some(n) => {
                let trimmed = n.trim();
                if !is_valid_name(trimmed) {
                    return Err(PromptParseError::InvalidName {
                        name: n.to_string(),
                    });
                }
                trimmed.to_string()
            }
            None => return Err(PromptParseError::MissingName),
        };

        // Parse params
        let (var_defs, defaults) = Self::validate_params(&header, &mut ctx)?;

        // Parse body
        let body = Self::validate_body(&mut ctx)?;

        // Parse and validate the body template so the builder receives a fully
        // structured, error-free representation.
        let template = Template::parse(&body)?;

        // Validate that every variable reference in the body resolves to a
        // declared variable (or an in-scope loop variable) and that every
        // dotted-path segment names a real field of the referenced object's
        // resolved shape (RFC §9 step 4).
        Self::validate_body_references(&template, &var_defs)?;

        // Finalize prompt
        let prompt = Prompt {
            name,
            title: header.get("title").map(|s| s.to_string()),
            desc: header.get("desc").map(|s| s.to_string()),
            source: header.get("source").map(|s| s.to_string()),
            prompt: body,
            template,
            variable_definitions: var_defs,
            variable_defaults: defaults,
        };

        // Ensure no errors
        if !ctx.errors.is_empty() {
            return Err(ctx.errors[0].clone()); // Return first error
        }

        Ok(prompt)
    }

    // --- HEADER PARSING ---
    fn validate_header(ctx: &mut ParseContext) -> Result<HashMap<String, String>, PromptParseError> {
        let mut header = HashMap::new();

        // The opening '--' delimiter is optional: a file may begin directly
        // with header keys.
        if ctx.pos < ctx.content.len() && ctx.content[ctx.pos].starts_with("--") {
            ctx.pos += 1;
            ctx.line_num += 1;
        }

        // Read until next '--' or until 'params:' (which hands off to validate_params)
        while ctx.pos < ctx.content.len() {
            let line = &ctx.content[ctx.pos];
            ctx.line_num += 1;

            if line.trim() == "--" {
                ctx.pos += 1;
                return Ok(header);
            }

            if line.trim().is_empty() {
                ctx.pos += 1;
                continue;
            }

            if let Ok((k, v)) = extract_kv(line) {
                match k.as_str() {
                    "params" => {
                        header.insert(k, v);
                        ctx.pos += 1;
                        return Ok(header); // validate_params consumes the indented block
                    }
                    "name" | "title" | "desc" | "source" => {
                        header.insert(k, v);
                    }
                    _ => return Err(PromptParseError::InvalidHeaderKey(k)),
                }
            } else {
                return Err(PromptParseError::UnexpectedLine(line.to_string()));
            }
            ctx.pos += 1;
        }

        Err(PromptParseError::MissingClosingHeaderDelimiter)
    }

    // --- PARAMS PARSING ---
    fn validate_params(
        header: &HashMap<String, String>,
        ctx: &mut ParseContext,
    ) -> Result<(VariableDefinitions, VariableDefaults), PromptParseError> {
        let mut var_defs = VariableDefinitions::new();
        let mut defaults = VariableDefaults::new();

        if !header.contains_key("params") {
            // No params block; consume the closing delimiter if present
            if ctx.pos < ctx.content.len() && ctx.content[ctx.pos].trim() == "--" {
                ctx.pos += 1;
                ctx.line_num += 1;
            }
            return Ok((var_defs, defaults));
        }

        // First-level variables live at indent 2
        let new_pos = Self::parse_definitions_block(
            &ctx.content,
            ctx.pos,
            2,
            String::new(),
            &mut var_defs,
            &mut defaults,
        )?;

        ctx.pos = new_pos;

        // Resolve element references (`etype: <object_shape>`, RFC §3.4) and
        // `type: <name>` inheritances (RFC §3.4.2) now that
        // all top-level variable definitions are known. Inheritance also
        // copies the referenced object_shape's field `def` defaults into the
        // inheriting object's dotted path so `render_with_defaults` resolves
        // them under the inheriting object.
        Self::resolve_element_refs(&mut var_defs, &mut defaults)?;

        // Validate cross-field consistency (opts/etype/label/def) now that
        // element references have been resolved into
        // `element_type`/`ofields_definitions`.
        Self::validate_all_definitions(&var_defs, &defaults)?;

        // Consume the closing '--' delimiter that ends the params block
        if ctx.pos < ctx.content.len() && ctx.content[ctx.pos].trim() == "--" {
            ctx.pos += 1;
            ctx.line_num += 1;
        }

        Ok((var_defs, defaults))
    }

    /// Recursively resolve `element_ref` entries (RFC §3.4) and
    /// `type: <name>` references (RFC §3.4.2) against the top-level `root`
    /// definitions. A by-name `etype` reference copies the referenced
    /// `object_shape` variable's `ofields` (deep, with its own nested refs
    /// resolved) into the referring definition and sets
    /// `element_type = Object`. An `type: <name>` inheritance
    /// on an `object` copies the referenced `object_shape`'s `ofields` into the
    /// object's `ofields_definitions`, AND copies the object_shape's field
    /// `def` defaults into the inheriting object's dotted path so
    /// `render_with_defaults` resolves them under the inheriting object.
    /// `visiting` tracks ref names on the current resolution path to detect
    /// cycles. References are case-insensitive, matching placeholder lookup.
    fn resolve_element_refs(
        defs: &mut VariableDefinitions,
        defaults: &mut VariableDefaults,
    ) -> Result<(), PromptParseError> {
        let root_snapshot: Vec<(String, VariableDefinition)> =
            defs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut visiting: HashSet<String> = HashSet::new();
        for (name, def) in defs.iter_mut() {
            Self::resolve_one(def, name, &root_snapshot, &mut visiting, defaults)?;
        }
        Ok(())
    }

    fn resolve_one(
        def: &mut VariableDefinition,
        path: &str,
        root: &[(String, VariableDefinition)],
        visiting: &mut HashSet<String>,
        defaults: &mut VariableDefaults,
    ) -> Result<(), PromptParseError> {
        // First resolve any element reference on this definition itself.
        // `element_ref` is kept populated after resolution so downstream
        // consumers (e.g. default synthesis) can recover the origin path.
        if let Some(refname) = def.element_ref.clone() {
            let key = refname.to_lowercase();
            if visiting.contains(&key) {
                return Err(PromptParseError::CircularElementRef { name: refname });
            }
            let target_entry = root
                .iter()
                .find(|(k, _)| k.to_lowercase() == key)
                .ok_or(PromptParseError::UnresolvedElementRef {
                    name: refname.clone(),
                })?;
            let target_name = target_entry.0.clone();
            let target = &target_entry.1;
            // Only `object_shape` is referenceable by name (RFC §3.4). Naming a
            // declared `object` (or any non-object_shape) is an error.
            if target.r#type != VariableType::ObjectShape || target.ofields_definitions.is_none() {
                return Err(PromptParseError::InvalidElementRef { name: refname });
            }
            visiting.insert(key);
            let mut nested = target.ofields_definitions.clone().unwrap();
            for (fname, nd) in nested.iter_mut() {
                let npath = format!("{}.{}", path, fname);
                Self::resolve_one(nd, &npath, root, visiting, defaults)?;
            }
            visiting.remove(&refname.to_lowercase());
            // Normalize the ref to the target's declared name so downstream
            // default lookup (keyed by declared name) is case-insensitive.
            def.element_ref = Some(target_name);
            def.element_type = Some(VariableType::Object);
            def.ofields_definitions = Some(nested);
        }
        // Resolve a `type: <object_shape_name>` reference (RFC §3.4.2):
        // splices the referenced object_shape's ofields in as this object's
        // own ofields, and copies the object_shape's field defaults into this
        // object's dotted path so `render_with_defaults` resolves them here.
        if let Some(refname) = def.type_ref.clone() {
            let key = refname.to_lowercase();
            if visiting.contains(&key) {
                return Err(PromptParseError::CircularElementRef { name: refname });
            }
            let target_entry = root
                .iter()
                .find(|(k, _)| k.to_lowercase() == key)
                .ok_or(PromptParseError::InvalidTypeRef {
                    name: refname.clone(),
                })?;
            let target_name = target_entry.0.clone();
            let target = &target_entry.1;
            if target.r#type != VariableType::ObjectShape || target.ofields_definitions.is_none() {
                return Err(PromptParseError::InvalidTypeRef { name: refname });
            }
            visiting.insert(key);
            let mut nested = target.ofields_definitions.clone().unwrap();
            for (fname, nd) in nested.iter_mut() {
                let npath = format!("{}.{}", path, fname);
                Self::resolve_one(nd, &npath, root, visiting, defaults)?;
            }
            visiting.remove(&refname.to_lowercase());
            def.type_ref = Some(target_name.clone());
            def.ofields_definitions = Some(nested);
            // Copy the object_shape's field defaults (every key starting with
            // `<target_name>.`) into this object's path, re-keyed under
            // `<path>.`.
            let src_prefix = format!("{}.", target_name);
            let copies: Vec<(String, VariableValue)> = defaults
                .iter()
                .filter_map(|(k, v)| {
                    if k.starts_with(&src_prefix) {
                        let rest = &k[src_prefix.len()..];
                        Some((format!("{}.{}", path, rest), v.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            for (k, v) in copies {
                defaults.entry(k).or_insert(v);
            }
        }
        // Then recurse into this definition's own ofields — but only when the
        // ofields were declared inline (no element_ref / type_ref was just
        // resolved). When a ref was resolved, the splice above already
        // recursed into the (cloned) nested ofields with path tracking, so
        // re-recursing here would only redundantly re-resolve them.
        if def.element_ref.is_none() && def.type_ref.is_none() {
            if let Some(nested) = def.ofields_definitions.as_mut() {
                for (fname, nd) in nested.iter_mut() {
                    let npath = format!("{}.{}", path, fname);
                    Self::resolve_one(nd, &npath, root, visiting, defaults)?;
                }
            }
        }
        Ok(())
    }

    // Recursive, indentation-driven parser for a block of sibling variable
    // definitions all living at exactly `indent` spaces. Returns the index of
    // the first line that does not belong to this block (less indented, blank,
    // or a `--` delimiter).
    fn parse_definitions_block(
        lines: &[String],
        start: usize,
        indent: usize,
        prefix: String,
        var_defs: &mut VariableDefinitions,
        defaults: &mut VariableDefaults,
    ) -> Result<usize, PromptParseError> {
        let mut pos = start;

        while pos < lines.len() {
            let line = &lines[pos];
            let stripped = line.trim_start();
            let cur_indent = line.len() - stripped.len();

            if stripped.is_empty() {
                pos += 1;
                continue;
            }
            // A delimiter or a line at lower indentation ends this block
            if stripped == "--" || cur_indent < indent {
                break;
            }
            if cur_indent != indent {
                return Err(PromptParseError::IndentationError {
                    expected: indent,
                    actual: cur_indent,
                });
            }

            let (key, _val) = extract_kv(stripped)?;
            let var_name = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };

            let mut def = VariableDefinition {
                r#type: VariableType::String,
                desc: None,
                options: None,
                element_type: None,
                element_ref: None,
                label: None,
                type_ref: None,
                ofields_definitions: None,
            };

            // Move to the first property line of this variable
            pos += 1;

            // Parse the properties of this variable (expected at indent + 2)
            while pos < lines.len() {
                let pline = &lines[pos];
                let pstripped = pline.trim_start();
                let pindent = pline.len() - pstripped.len();

                if pstripped.is_empty() {
                    pos += 1;
                    continue;
                }
                // Anything at or below this variable's indent ends its properties
                if pindent <= indent {
                    break;
                }
                if pindent != indent + 2 {
                    return Err(PromptParseError::IndentationError {
                        expected: indent + 2,
                        actual: pindent,
                    });
                }

                let (pk, pv) = extract_kv(pstripped)?;
                match pk.as_str() {
                    "type" => {
                        // `type` accepts either a built-in type name (§3.1)
                        // or the name of a declared `object_shape` variable
                        // (RFC §3.4.2). A non-builtin name is recorded in
                        // `type_ref` and `r#type` is tentatively set to
                        // `Object`; resolution later splices in the
                        // referenced object_shape's `ofields` and confirms
                        // the target exists and is an `object_shape`.
                        let v = pv.trim();
                        match Self::parse_type(v, "type") {
                            Ok(t) => def.r#type = t,
                            Err(_) => {
                                def.type_ref = Some(v.to_string());
                                def.r#type = VariableType::Object;
                            }
                        }
                        pos += 1;
                    }
                    "etype" => {
                        let v = pv.trim();
                        match Self::parse_type(v, "etype") {
                            Ok(t) => def.element_type = Some(t),
                            Err(_) => {
                                // Not a built-in type name: treat it as a
                                // reference to a declared `object` variable
                                // (RFC §3.4). Resolved later.
                                def.element_ref = Some(v.to_string());
                            }
                        }
                        pos += 1;
                    }
                    "desc" => {
                        def.desc = Some(pv.to_string());
                        pos += 1;
                    }
                    "label" => {
                        def.label = Some(pv.trim().to_string());
                        pos += 1;
                    }
                    "def" => {
                        if pv.trim() == ">>>" {
                            // Heredoc default for `long_string` variables (RFC §3.5).
                            // Collect raw lines until a line whose trimmed content
                            // is `<<<`, then store the value verbatim.
                            if def.r#type != VariableType::LongString {
                                return Err(PromptParseError::HeredocNotLongString {
                                    field: var_name.clone(),
                                });
                            }
                            let mut buf = String::new();
                            let mut p = pos + 1;
                            let mut found = false;
                            while p < lines.len() {
                                let l = &lines[p];
                                if l.trim() == "<<<" {
                                    found = true;
                                    p += 1;
                                    break;
                                }
                                buf.push_str(l);
                                buf.push('\n');
                                p += 1;
                            }
                            if !found {
                                return Err(PromptParseError::MissingHeredocTerminator);
                            }
                            // Drop the trailing newline added after the last
                            // content line so the value is the joined lines
                            // without a trailing terminator newline.
                            if buf.ends_with('\n') {
                                buf.pop();
                            }
                            defaults
                                .insert(var_name.clone(), VariableValue::LongString(buf));
                            pos = p;
                        } else if pv.trim().is_empty() {
                            // Could be a list of items below (indent + 4) or just empty
                            let mut items = Vec::new();
                            let mut p = pos + 1;
                            while p < lines.len() {
                                let iline = &lines[p];
                                let istripped = iline.trim_start();
                                let iindent = iline.len() - istripped.len();
                                if istripped.is_empty() {
                                    p += 1;
                                    continue;
                                }
                                if iindent != indent + 4 {
                                    break;
                                }
                                if !(istripped.starts_with("- ") || istripped == "-") {
                                    break;
                                }
                                let item_val = istripped
                                    .strip_prefix('-')
                                    .map(|s| s.trim())
                                    .unwrap_or(istripped);
                                items.push(Self::parse_value(item_val)?);
                                p += 1;
                            }
                            if items.is_empty() {
                                defaults
                                    .insert(var_name.clone(), VariableValue::String(String::new()));
                            } else {
                                defaults.insert(var_name.clone(), VariableValue::List(items));
                            }
                            pos = p;
                        } else {
                            defaults.insert(var_name.clone(), Self::parse_value(pv.trim())?);
                            pos += 1;
                        }
                    }
                    "opts" => {
                        if pv.trim().starts_with('[') {
                            def.options = Some(Self::parse_list_of_values(pv.trim())?);
                            pos += 1;
                        } else {
                            let mut items = Vec::new();
                            pos += 1; // move past the "opts:" line
                            while pos < lines.len() {
                                let iline = &lines[pos];
                                let istripped = iline.trim_start();
                                let iindent = iline.len() - istripped.len();
                                if istripped.is_empty() {
                                    pos += 1;
                                    continue;
                                }
                                if iindent != indent + 4 {
                                    break;
                                }
                                if !(istripped.starts_with("- ") || istripped == "-") {
                                    break;
                                }
                                let item_val = istripped
                                    .strip_prefix('-')
                                    .map(|s| s.trim())
                                    .unwrap_or(istripped);
                                items.push(Self::parse_value(item_val)?);
                                pos += 1;
                            }
                            def.options = Some(items);
                        }
                    }
                    "ofields" => {
                        // `ofields` is the inline field map. It may be empty
                        // (the indented `ofields` block follows) or `{}`.
                        // Shape reuse uses `type: <object_shape_name>` (§3.4.2),
                        // not `ofields`; the two are mutually exclusive (a
                        // `type: <name>` reference has no inline `ofields`).
                        if def.type_ref.is_some() {
                            return Err(PromptParseError::InvalidOfieldsForType {
                                type_name:
                                    "object (cannot have both `type: <shape>` and `ofields`)".into(),
                            });
                        }
                        pos += 1; // move past the "ofields:" line
                        let mut nested_defs = VariableDefinitions::new();
                        let new_pos = Self::parse_definitions_block(
                            lines,
                            pos,
                            indent + 4,
                            var_name.clone(),
                            &mut nested_defs,
                            defaults,
                        )?;
                        def.ofields_definitions = Some(nested_defs);
                        pos = new_pos;
                    }
                    other => {
                        return Err(PromptParseError::InvalidHeaderKey(other.to_string()));
                    }
                }
            }

            var_defs.insert(key.clone(), def);
        }

        Ok(pos)
    }

    /// Walk every definition (top-level and nested inside `ofields`) and run
    /// `validate_definition` on each. Paths are dotted so error messages can
    /// point at the offending nested field. `defaults` is the flat dotted-key
    /// map of `def` values parsed from the file, used to type-check each
    /// variable's default against its declared `type`/`element_type` (RFC §3.3).
    fn validate_all_definitions(
        defs: &VariableDefinitions,
        defaults: &VariableDefaults,
    ) -> Result<(), PromptParseError> {
        for (name, def) in defs {
            Self::validate_definition(name, def, defaults)?;
            if let Some(nested) = &def.ofields_definitions {
                Self::validate_all_definitions_nested(name, nested, defaults)?;
            }
        }
        Ok(())
    }

    fn validate_all_definitions_nested(
        parent: &str,
        defs: &VariableDefinitions,
        defaults: &VariableDefaults,
    ) -> Result<(), PromptParseError> {
        for (name, def) in defs {
            let path = format!("{}.{}", parent, name);
            Self::validate_definition(&path, def, defaults)?;
            if let Some(nested) = &def.ofields_definitions {
                Self::validate_all_definitions_nested(&path, nested, defaults)?;
            }
        }
        Ok(())
    }

    /// Validate cross-field consistency of a parsed variable definition
    /// (RFC §3.3): `opts` only on `option_*`, option etype restricted to
    /// `string`/`long_string`/`number`/object-ref, `opts` entries match the
    /// etype, `label` only on `option_*` with object etype and required in
    /// that case, and `label` names a string/long_string field on the
    /// referenced object.
    fn validate_definition(
        path: &str,
        def: &VariableDefinition,
        defaults: &VariableDefaults,
    ) -> Result<(), PromptParseError> {
        use VariableType::*;
        let is_option = matches!(def.r#type, OptionSingle | OptionMulti);

        // `ofields` (inline `ofields_definitions`) and/or a `type_ref`
        // (shape reuse via `type: <object_shape_name>`) are allowed on:
        //   - `object` / `object_shape` (their own shape), and
        //   - `list` / `option_single` / `option_multi` when `etype` is the
        //     inline `object` (the inline element shape, RFC §4.1.1).
        let has_ofields = def.ofields_definitions.is_some() || def.type_ref.is_some();
        if has_ofields
            && !matches!(
                def.r#type,
                Object | ObjectShape | List | OptionSingle | OptionMulti
            )
        {
            return Err(PromptParseError::InvalidOfieldsForType {
                type_name: format!("{:?}", def.r#type).to_lowercase(),
            });
        }

        // `type_ref` (shape reuse) is only valid on `object` — and is mutually
        // exclusive with inline `ofields` (the referenced object_shape
        // provides the fields). `r#type` is set to `Object` during parse when
        // `type_ref` is recorded, so this also rejects a `type_ref` mistakenly
        // set on a non-object.
        if def.type_ref.is_some() {
            if def.r#type != Object {
                return Err(PromptParseError::InvalidOfieldsForType {
                    type_name: format!("{:?}", def.r#type).to_lowercase(),
                });
            }
            // A `type: <name>` object must NOT also declare inline `ofields`.
            // (After resolution `ofields_definitions` holds the spliced shape,
            // so this check runs against the parse-time state captured on the
            // definition: if both were declared, both `type_ref` and
            // `ofields_definitions` are `Some` *before* resolution. We can't
            // re-detect post-resolution, so we also guard at parse time in the
            // `ofields:` key handler.)
        }

        // An `object_shape` must declare an inline `ofields` block (it cannot
        // reuse a shape via `type: <name>` — that form is for `object` only).
        if def.r#type == ObjectShape && def.ofields_definitions.is_none() {
            return Err(PromptParseError::ObjectShapeMissingOfields {
                name: path.to_string(),
            });
        }

        // An `object` must have an `ofields` (inline or via `type: <name>`).
        // After resolution, `ofields_definitions` is populated in both cases.
        if def.r#type == Object && !has_ofields {
            return Err(PromptParseError::InvalidOfieldsForType {
                type_name: "object".into(),
            });
        }

        // `etype`/`element_ref` only on list/option_* (not object/object_shape).
        if (def.element_type.is_some() || def.element_ref.is_some())
            && !matches!(def.r#type, List | OptionSingle | OptionMulti)
        {
            return Err(PromptParseError::InvalidOptionEtype {
                etype: "etype".into(),
                kind: format!("{:?}", def.r#type).to_lowercase(),
            });
        }

        // Type-check the `def` value (RFC §3.3): a `def` whose `VariableValue`
        // kind does not match the declared `type`/`element_type` is a parse
        // error. Object/list `def`s are checked recursively against the
        // resolved `ofields_definitions` / element shape.
        if let Some(default) = defaults.get(path) {
            Self::validate_default_value(path, def, default)?;
        }

        // `opts` only on option_*.
        if def.options.is_some() && !is_option {
            return Err(PromptParseError::InvalidOptsForType);
        }

        // `label` only on option_*.
        if def.label.is_some() && !is_option {
            return Err(PromptParseError::InvalidLabelForType);
        }

        if is_option {
            // `opts` is required for option_* and must contain at least two
            // entries (RFC §3.1) — a single-option menu is not meaningful.
            let opt_count = def.options.as_ref().map(|o| o.len()).unwrap_or(0);
            if opt_count < 2 {
                return Err(PromptParseError::MissingOpts(format!("{:?}", def.r#type).to_lowercase()));
            }

            // option_single etype defaults to string; option_multi requires it.
            // `element_ref` is set when etype names a declared object; after
            // resolution `element_type` is `Object` and `ofields_definitions`
            // holds the resolved shape, so either signal counts as "present".
            let has_etype = def.element_type.is_some() || def.element_ref.is_some();
            if !has_etype {
                if let OptionMulti = def.r#type {
                    return Err(PromptParseError::MissingElementType {
                        type_name: "option_multi".into(),
                    });
                }
            }
            let etype = if has_etype {
                def.element_type
            } else {
                Some(String)
            };

            // Restrict allowed option etypes. An unresolved element_ref is an
            // object etype (validated as Object once resolved).
            if let Some(et) = etype {
                if !matches!(et, String | LongString | Number | Object) {
                    return Err(PromptParseError::InvalidOptionEtype {
                        etype: format!("{:?}", et),
                        kind: format!("{:?}", def.r#type).to_lowercase(),
                    });
                }
            }
            if def.element_ref.is_some() && etype != Some(Object) {
                // element_ref resolved to a non-object: already an error from
                // resolve_one, but defensively reject here too.
                return Err(PromptParseError::InvalidOptionEtype {
                    etype: "referenced-non-object".into(),
                    kind: format!("{:?}", def.r#type).to_lowercase(),
                });
            }

            // `label` rules apply only when the etype is a referenced object.
            let is_object_etype = etype == Some(Object) && def.element_ref.is_some();
            if is_object_etype {
                let label = match &def.label {
                    Some(l) if !l.trim().is_empty() => l.trim().to_string(),
                    _ => {
                        return Err(PromptParseError::MissingLabelForObjectEtype {
                            path: path.to_string(),
                        });
                    }
                };
                // The referenced object's ofields were resolved into
                // `ofields_definitions`. Verify the label field exists and is
                // string/long_string.
                if let Some(ofields) = &def.ofields_definitions {
                    let lbl_lc = label.to_lowercase();
                    let field = ofields.iter().find(|(k, _)| k.to_lowercase() == lbl_lc);
                    match field {
                        Some((_, fdef)) => {
                            if !matches!(fdef.r#type, String | LongString) {
                                return Err(PromptParseError::InvalidLabelFieldType {
                                    label,
                                    obj: def
                                        .element_ref
                                        .clone()
                                        .unwrap_or_else(|| "<object>".into()),
                                    got: fdef.r#type,
                                });
                            }
                        }
                        None => {
                            return Err(PromptParseError::UnknownLabelField {
                                label,
                                obj: def
                                    .element_ref
                                    .clone()
                                    .unwrap_or_else(|| "<object>".into()),
                            });
                        }
                    }
                }
            }

            // Validate each opt entry matches the etype.
            if let Some(opts) = &def.options {
                let et = etype.unwrap_or(String);
                for (i, v) in opts.iter().enumerate() {
                    let ok = matches!(
                        (et, v),
                        (String, VariableValue::String(_))
                            | (LongString, VariableValue::LongString(_))
                            | (LongString, VariableValue::String(_))
                            | (Number, VariableValue::Number(_))
                            | (Object, VariableValue::Object(_))
                    );
                    if !ok {
                        return Err(PromptParseError::OptionEntryTypeMismatch {
                            index: i + 1,
                            etype: format!("{:?}", et),
                            value: format!("{:?}", v),
                        });
                    }
                }
                // For object etype, also ensure each opt object has the label
                // field present and string-valued, so the menu can render it.
                if et == Object {
                    if let Some(label) = &def.label {
                        let lbl_lc = label.trim().to_lowercase();
                        for (i, v) in opts.iter().enumerate() {
                            if let VariableValue::Object(map) = v {
                                let entry = map
                                    .iter()
                                    .find(|(k, _)| k.to_lowercase() == lbl_lc);
                                match entry {
                                    Some((_, VariableValue::String(_) | VariableValue::LongString(_))) => {}
                                    _ => {
                                        return Err(PromptParseError::OptionEntryTypeMismatch {
                                            index: i + 1,
                                            etype: format!("{:?}", et),
                                            value: format!(
                                                "missing/non-string label field '{}'",
                                                label
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate that a parsed `def` value matches the declared `type` (and
    /// `element_type` for lists), per RFC §3.3 ("All values supplied via
    /// `def`, `opts`, etc. must match the declared `type` and `etype` ... A
    /// mismatch is a parse error."). The check is structural: the top-level
    /// `VariableValue` kind must match the declared type, list elements must
    /// match `element_type`, and object `def`s must be objects. Field-level
    /// shape of object defaults is not enforced here (extra/missing nested
    /// keys are tolerated and filled from defaults at render time).
    fn validate_default_value(
        path: &str,
        def: &VariableDefinition,
        value: &VariableValue,
    ) -> Result<(), PromptParseError> {
        use VariableType as T;
        use VariableValue as V;
        let declared = def.r#type;
        let ok = match (declared, value) {
            (T::String, V::String(_)) => true,
            (T::LongString, V::LongString(_) | V::String(_)) => true,
            (T::Number, V::Number(_)) => true,
            (T::Boolean, V::Boolean(_)) => true,
            (T::Object, V::Object(_)) => true,
            (T::ObjectShape, V::Object(_)) => true,
            (T::List, V::List(items)) => {
                // Every element must match the list's etype. For object etype
                // (resolved from a reference), each element must be an object.
                let etype = def.element_type.unwrap_or(T::String);
                items.iter().all(|e| Self::value_matches_etype(etype, e))
            }
            (T::OptionSingle, _) => {
                // option_single def is one of the opts (or any scalar of the
                // etype). Validate against the etype the same way as opts.
                let etype = def.element_type.unwrap_or(T::String);
                Self::value_matches_etype(etype, value)
            }
            (T::OptionMulti, V::List(items)) => {
                let etype = def.element_type.unwrap_or(T::String);
                items.iter().all(|e| Self::value_matches_etype(etype, e))
            }
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(PromptParseError::DefTypeMismatch {
                path: path.to_string(),
                declared,
                value: format!("{:?}", value),
            })
        }
    }

    /// Whether a single value matches a scalar/object etype (used for list
    /// elements and option defaults). `string` accepts a `String` (a
    /// `long_string` default is also accepted into a `string` etype since the
    /// parser stores heredoc defaults as `LongString`).
    fn value_matches_etype(etype: VariableType, value: &VariableValue) -> bool {
        use VariableType as T;
        use VariableValue as V;
        match (etype, value) {
            (T::String, V::String(_)) => true,
            (T::LongString, V::LongString(_) | V::String(_)) => true,
            (T::Number, V::Number(_)) => true,
            (T::Boolean, V::Boolean(_)) => true,
            (T::Object, V::Object(_)) => true,
            _ => false,
        }
    }

    // Helper: parse a type string into a VariableType
    fn parse_type(s: &str, field: &str) -> Result<VariableType, PromptParseError> {
        Ok(match s {
            "string" => VariableType::String,
            "long_string" => VariableType::LongString,
            "number" => VariableType::Number,
            "boolean" => VariableType::Boolean,
            "list" => VariableType::List,
            "object" => VariableType::Object,
            "object_shape" => VariableType::ObjectShape,
            "option_single" => VariableType::OptionSingle,
            "option_multi" => VariableType::OptionMulti,
            other => {
                return Err(PromptParseError::InvalidTypeField {
                    field: field.to_string(),
                    value: other.to_string(),
                });
            }
        })
    }

    // --- BODY PARSING ---
    fn validate_body(ctx: &mut ParseContext) -> Result<String, PromptParseError> {
        let mut body = String::new();
        while ctx.pos < ctx.content.len() {
            let line = &ctx.content[ctx.pos];
            ctx.line_num += 1;

            // A standalone '--' line marks the end of the body
            if line.trim() == "--" {
                ctx.pos += 1;
                break;
            }

            body.push_str(line);
            body.push('\n');
            ctx.pos += 1;
        }

        Ok(body)
    }

    // --- Parse Value from String (literal) ---
    fn parse_value(s: &str) -> Result<VariableValue, PromptParseError> {
        let s = s.trim();
        if s.starts_with('\"') && s.ends_with('\"') && s.len() >= 2 {
            let inner = &s[1..s.len() - 1];
            Ok(VariableValue::String(inner.to_string()))
        } else if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
            let inner = &s[1..s.len() - 1];
            Ok(VariableValue::String(inner.to_string()))
        } else if s == "true" {
            Ok(VariableValue::Boolean(true))
        } else if s == "false" {
            Ok(VariableValue::Boolean(false))
        } else if let Ok(num) = s.parse::<f64>() {
            Ok(VariableValue::Number(num))
        } else if s == "{}" {
            Ok(VariableValue::Object(IndexMap::new()))
        } else if s.starts_with('{') && s.ends_with('}') {
            Self::parse_object_literal(s)
        } else if s.starts_with('[') && s.ends_with(']') {
            let inner = &s[1..s.len() - 1];
            let items: Vec<VariableValue> = split_top_level(inner, ',')
                .into_iter()
                .map(|item| item.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|s| Self::parse_value(&s).unwrap_or_else(|_| VariableValue::String(s)))
                .collect();
            Ok(VariableValue::List(items))
        } else {
            Ok(VariableValue::String(s.to_string()))
        }
    }

    /// Parse a non-empty inline object literal `{ key: value, ... }`. Keys are
    /// bare identifiers; values are any literal supported by `parse_value`,
    /// including nested objects/lists. Enables inline `def:` values like
    /// `{ host: "localhost", port: 8080 }` (RFC §3.4 / §8.3).
    fn parse_object_literal(s: &str) -> Result<VariableValue, PromptParseError> {
        let inner = s[1..s.len() - 1].trim();
        let mut map = IndexMap::new();
        if inner.is_empty() {
            return Ok(VariableValue::Object(map));
        }
        for part in split_top_level(inner, ',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let colon = part
                .find(':')
                .ok_or_else(|| PromptParseError::InvalidDefaultValue {
                    value: s.to_string(),
                    expected_type: VariableType::Object,
                })?;
            let key = part[..colon].trim().to_string();
            let val = Self::parse_value(&part[colon + 1..])?;
            map.insert(key, val);
        }
        Ok(VariableValue::Object(map))
    }

    // --- Parse List of Values ---
    fn parse_list_of_values(s: &str) -> Result<Vec<VariableValue>, PromptParseError> {
        if s.starts_with('[') && s.ends_with(']') {
            let inner = &s[1..s.len() - 1];
            split_top_level(inner, ',')
                .into_iter()
                .map(|item| item.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|s| Self::parse_value(&s))
                .collect()
        } else {
            Err(PromptParseError::InvalidDefaultValue {
                value: s.to_string(),
                expected_type: VariableType::String,
            })
        }
    }

    // --- PRINT PROMPT ---
    pub fn print_prompt(prompt: &Prompt) {
        println!("\n🔍 Parsed Prompt: {}\n", prompt.title.as_deref().unwrap_or("Unknown"));
        println!("{}: {}", Self::color_key("Name", 0), prompt.name);
        if let Some(desc) = &prompt.desc {
            println!("{}: {}", Self::color_key("Description", 0), desc);
        }
        println!("\n{}:", Self::color_key("Variables", 0));
        Self::print_definitions(&prompt.variable_definitions, 1);
        println!("\n{}:", Self::color_key("Body", 0));
        println!("{}", prompt.prompt);
    }

    fn gray_for_depth(depth: usize) -> u8 {
        // 256-color grayscale ramp: 232 (black) .. 255 (white).
        // Step ~5 codes per nesting level (≈20% of the 24-step ramp).
        let code = 232u32 + 5 * depth as u32;
        if code >= 255 { 255 } else { code as u8 }
    }

    fn color_key(key: &str, depth: usize) -> String {
        let g = Self::gray_for_depth(depth);
        // white background (256-color #15), font = grayscale ramp
        format!("\x1b[48;5;15m\x1b[38;5;{}m{}\x1b[0m", g, key)
    }

    fn print_definitions(defs: &VariableDefinitions, depth: usize) {
        for (key, def) in defs {
            let indent = "  ".repeat(depth + 1);
            println!("{}{}: {:?}", indent, Self::color_key(key, depth), def.r#type);
            if let Some(desc) = &def.desc {
                println!("{}  {}: {}", indent, Self::color_key("Description", depth), desc);
            }
            if let Some(opts) = &def.options {
                println!("{}  {}: {:?}", indent, Self::color_key("Options", depth), opts);
            }
            if let Some(et) = &def.element_type {
                println!("{}  {}: {:?}", indent, Self::color_key("EType", depth), et);
            }
            if let Some(label) = &def.label {
                println!("{}  {}: {}", indent, Self::color_key("Label", depth), label);
            }
            if let Some(ofields) = &def.ofields_definitions {
                println!("{}  {}:", indent, Self::color_key("OFields", depth));
                Self::print_definitions(ofields, depth + 1);
            }
        }
    }

    // -----------------------------------------------------------------
    // Body reference validation (RFC §9 step 4)
    // -----------------------------------------------------------------
    //
    // Every variable reference in the body (placeholders, loop list
    // references, condition variables, and ternary branch `[[[VAR]]]`
    // references) must resolve against a declared `params` variable or an
    // in-scope loop variable. For dotted paths, every segment after the
    // leading one must name a real field of the referenced object's resolved
    // shape. List field projection (§4.1.5) is supported: a segment that
    // traverses into a `list` of `object` elements resolves the remaining
    // path against the list's element `ofields`.
    // (The `Shape`/`Binding` helper types are defined at module level below.)

    fn validate_body_references<'a>(
        template: &'a Template,
        var_defs: &'a VariableDefinitions,
    ) -> Result<(), PromptParseError> {
        // Top-level bindings: one per declared variable. Lookup is
        // case-insensitive (declared names are lowercase; references are
        // uppercase per §4.1).
        let top: Vec<Binding> = var_defs
            .iter()
            .map(|(k, v)| Binding {
                name: k.clone(),
                shape: Self::shape_of(v),
            })
            .collect();
        let mut scope: Vec<Vec<Binding>> = vec![top];
        Self::validate_nodes(&template.nodes, &mut scope)
    }

    /// Resolve a variable definition into the `Shape` its references expose.
    /// `Object` and any list/option with an object etype expose
    /// `Object(ofields)`; scalars and lists/options of scalars expose
    /// `Scalar(etype)`.
    fn shape_of(def: &VariableDefinition) -> Shape<'_> {
        match def.r#type {
            VariableType::Object | VariableType::ObjectShape => Shape::Object(
                def.ofields_definitions
                    .as_ref()
                    .expect("object/object_shape has ofields after validation"),
            ),
            VariableType::List | VariableType::OptionSingle | VariableType::OptionMulti => {
                let is_object_etype = def.element_type == Some(VariableType::Object)
                    || def.element_ref.is_some();
                if is_object_etype {
                    Shape::Object(
                        def.ofields_definitions
                            .as_ref()
                            .expect("object-etype variable has resolved ofields"),
                    )
                } else {
                    Shape::Scalar(def.element_type.unwrap_or(VariableType::String))
                }
            }
            other => Shape::Scalar(other),
        }
    }

    fn validate_nodes<'a>(
        nodes: &[Node],
        scope: &mut Vec<Vec<Binding<'a>>>,
    ) -> Result<(), PromptParseError> {
        for node in nodes {
            match node {
                Node::Text(_) => {}
                Node::Placeholder(p) => {
                    Self::validate_path(p, "placeholder", scope)?;
                }
                Node::Ternary {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    Self::validate_cond(cond, scope)?;
                    Self::validate_branch_ref(true_branch, scope)?;
                    Self::validate_branch_ref(false_branch, scope)?;
                }
                Node::Loop { item, list, body } => {
                    // Resolve the full loop-list path (which may be dotted,
                    // e.g. `RESOURCE.FIELDS`) to its `Shape`. The loop variable
                    // is then bound to that shape: for a list-of-objects, the
                    // shape is `Object(ofields)` so `<ITEM>.<FIELD>` resolves;
                    // for a list-of-scalars, `Scalar(etype)`.
                    //
                    // An undeclared loop list (root variable not declared) is
                    // tolerated at parse time for consistency with §3.5 (values
                    // may be supplied at render time); the builder enforces the
                    // list requirement at render time. In that case we bind the
                    // item to `Scalar(String)` so field references inside the
                    // body are not falsely rejected (they are validated at
                    // render time against the actual element).
                    let list_shape = match Self::resolve_shape(list, "loop list", scope) {
                        Ok(s) => s,
                        Err(PromptParseError::UndeclaredVariable { .. }) => {
                            Shape::Scalar(VariableType::String)
                        }
                        Err(e) => return Err(e),
                    };
                    scope.push(vec![Binding {
                        name: item.clone(),
                        shape: list_shape,
                    }]);
                    let r = Self::validate_nodes(body, scope);
                    scope.pop();
                    r?;
                }
                Node::If { cond, body } => {
                    Self::validate_cond(cond, scope)?;
                    Self::validate_nodes(body, scope)?;
                }
            }
        }
        Ok(())
    }

    /// Resolve a full dotted path to the `Shape` of its final segment,
    /// returning an error for unknown fields on declared objects. An
    /// undeclared root is reported as `UndeclaredVariable` (callers that want
    /// to tolerate undeclared roots should use `validate_path` instead).
    fn resolve_shape<'a>(
        path: &str,
        ctx: &str,
        scope: &[Vec<Binding<'a>>],
    ) -> Result<Shape<'a>, PromptParseError> {
        let parts: Vec<&str> = path.split('.').collect();
        let first = parts[0].to_lowercase();
        let binding = scope
            .iter()
            .rev()
            .flat_map(|f| f.iter())
            .find(|b| b.name.to_lowercase() == first)
            .ok_or_else(|| PromptParseError::UndeclaredVariable {
                name: parts[0].to_string(),
                ctx: ctx.to_string(),
            })?;
        Self::resolve_dotted(&parts[1..], binding.shape, &binding.name, ctx)
    }

    fn validate_path<'a>(
        path: &str,
        ctx: &str,
        scope: &[Vec<Binding<'a>>],
    ) -> Result<(), PromptParseError> {
        let parts: Vec<&str> = path.split('.').collect();
        let first = parts[0].to_lowercase();
        // An undeclared root variable is allowed at parse time: values may be
        // supplied programmatically at render time (e.g. `[[[URL]]]` in the
        // §3.5 example). Existence of a *value* is enforced at render time by
        // the builder (`MissingValue`). We only validate the dotted field
        // shape here, against declared objects whose `ofields` are known.
        let binding = match scope
            .iter()
            .rev()
            .flat_map(|f| f.iter())
            .find(|b| b.name.to_lowercase() == first)
        {
            Some(b) => b,
            None => return Ok(()),
        };
        Self::resolve_dotted(&parts[1..], binding.shape, &binding.name, ctx)?;
        Ok(())
    }

    /// Walk the remaining path segments against the binding's `Shape` and return
    /// the `Shape` of the final segment. A segment resolves if it names a
    /// field of an `Object`'s `ofields`; the field's own `Shape` then drives
    /// the next segment. A `Scalar` shape with any remaining segment is an
    /// error. An empty `parts` slice returns the input `shape` unchanged.
    fn resolve_dotted<'a>(
        parts: &[&str],
        shape: Shape<'a>,
        owner: &str,
        ctx: &str,
    ) -> Result<Shape<'a>, PromptParseError> {
        if parts.is_empty() {
            return Ok(shape);
        }
        let p = parts[0];
        let pl = p.to_lowercase();
        match shape {
            Shape::Object(ofields) => {
                let field = ofields.iter().find(|(k, _)| k.to_lowercase() == pl);
                match field {
                    Some((k, fdef)) => {
                        let next_owner = format!("{}.{}", owner, k);
                        Self::resolve_dotted(&parts[1..], Self::shape_of(fdef), &next_owner, ctx)
                    }
                    None => Err(PromptParseError::UnknownField {
                        field: p.to_string(),
                        object: owner.to_string(),
                        ctx: ctx.to_string(),
                    }),
                }
            }
            Shape::Scalar(t) => Err(PromptParseError::UnknownField {
                field: p.to_string(),
                object: format!("{} (scalar {:?}, no fields)", owner, t),
                ctx: ctx.to_string(),
            }),
        }
    }

    fn validate_cond<'a>(cond: &CondExpr, scope: &[Vec<Binding<'a>>]) -> Result<(), PromptParseError> {
        match cond {
            CondExpr::Literal(_) => Ok(()),
            CondExpr::Var(name) => Self::validate_path(name, "condition", scope),
            CondExpr::Not(inner) => Self::validate_cond(inner, scope),
            CondExpr::Bin { left, right, .. } => {
                Self::validate_cond(left, scope)?;
                Self::validate_cond(right, scope)
            }
        }
    }

    /// A ternary branch may be a `[[[VAR]]]` reference (validated) or a
    /// literal (unvalidated). Only the `[[[VAR]]]` form is checked here.
    fn validate_branch_ref<'a>(
        branch: &str,
        scope: &[Vec<Binding<'a>>],
    ) -> Result<(), PromptParseError> {
        let s = branch.trim();
        if s.starts_with("[[[") && s.ends_with("]]]") && s.len() >= 6 {
            let inner = &s[3..s.len() - 3];
            if !inner.contains("]]]") {
                return Self::validate_path(inner.trim(), "ternary branch", scope);
            }
        }
        Ok(())
    }
}

// --- Template parsing ---
//
// Parses the prompt body into a `Template` (a tree of `Node`s). Recognizes
// `[[[var]]]` placeholders, `{{{cond ? a : b}}}` ternaries, `{{{for x in
// list}}}...{{{end for}}}` loops and `{{{if cond}}}...{{{end if}}}` blocks.
// Condition expressions are parsed into a `CondExpr` AST so the body handed
// to the builder is fully structured and validated.

const END_FOR_BRACE: &str = "{{{end for}}}";
const END_IF: &str = "{{{end if}}}";

impl Template {
    /// Parse a template string into a validated `Template`. After structural
    /// parsing succeeds, a second pass enforces that every variable reference
    /// (placeholder paths, loop variable bindings, ternary branch references,
    /// and condition variable references) is written in **uppercase**. The
    /// `params` declarations stay lowercase; the renderer matches references
    /// against declared names case-insensitively, but the *source* form of
    /// every reference MUST be uppercase.
    pub fn parse(s: &str) -> Result<Template, PromptParseError> {
        let nodes = parse_template(s)?;
        validate_identifiers(&nodes)?;
        Ok(Template { nodes })
    }
}

/// Parse a template string into a node tree.
fn parse_template(s: &str) -> Result<Vec<Node>, PromptParseError> {
    let mut nodes = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with("[[[") {
            if let Some(end) = s[i + 3..].find("]]]") {
                let inner = s[i + 3..i + 3 + end].trim().to_string();
                flush_text(&mut nodes, &mut text);
                nodes.push(Node::Placeholder(inner));
                i = i + 3 + end + 3;
                continue;
            }
            text.push_str("[[[");
            i += 3;
            continue;
        }
        if s[i..].starts_with("{{{for ") {
            let close = find_subseq(s, i + 3, "}}}")
                .ok_or_else(|| PromptParseError::UnmatchedConstruct("unterminated loop tag".into()))?;
            let inner = &s[i + 3..close];
            let (item, list) = parse_for_header(inner)?;
            let body_start = skip_one_newline(s, close + 3);
            let loop_end = find_loop_end(s, body_start).ok_or_else(|| {
                PromptParseError::UnmatchedConstruct(format!("missing '{}'", END_FOR_BRACE))
            })?;
            let body_nodes = parse_template(&s[body_start..loop_end])?;
            flush_text(&mut nodes, &mut text);
            nodes.push(Node::Loop {
                item,
                list,
                body: body_nodes,
            });
            i = skip_one_newline(s, loop_end + END_FOR_BRACE.len());
            continue;
        }
        if s[i..].starts_with(END_FOR_BRACE) {
            return Err(PromptParseError::UnmatchedConstruct(
                "unexpected loop-end tag".into(),
            ));
        }
        if s[i..].starts_with("{{{if ") {
            let close = find_subseq(s, i + 3, "}}}")
                .ok_or_else(|| PromptParseError::UnmatchedConstruct("unterminated '{{{if' tag".into()))?;
            let inner = &s[i + 3..close];
            let cond_raw = inner.trim().strip_prefix("if").unwrap_or("").trim().to_string();
            let cond = parse_condition(&cond_raw)?;
            let body_start = skip_one_newline(s, close + 3);
            let if_end = find_if_end(s, body_start)
                .ok_or_else(|| PromptParseError::UnmatchedConstruct(format!("missing '{}'", END_IF)))?;
            let body_nodes = parse_template(&s[body_start..if_end])?;
            flush_text(&mut nodes, &mut text);
            nodes.push(Node::If {
                cond,
                body: body_nodes,
            });
            i = skip_one_newline(s, if_end + END_IF.len());
            continue;
        }
        if s[i..].starts_with(END_IF) {
            return Err(PromptParseError::UnmatchedConstruct(
                "unexpected '{{{end if}}}'".into(),
            ));
        }
        if s[i..].starts_with("{{{") {
            // Ternary expression.
            if let Some(end) = s[i + 3..].find("}}}") {
                let inner = &s[i + 3..i + 3 + end];
                let (cond, t, f) = split_ternary(inner).ok_or_else(|| {
                    PromptParseError::InvalidConditionSyntax(format!("bad ternary: {}", inner))
                })?;
                let cond = parse_condition(cond.trim())?;
                flush_text(&mut nodes, &mut text);
                nodes.push(Node::Ternary {
                    cond,
                    true_branch: t.trim().to_string(),
                    false_branch: f.trim().to_string(),
                });
                i = i + 3 + end + 3;
                continue;
            }
            text.push_str("{{{");
            i += 3;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    flush_text(&mut nodes, &mut text);
    Ok(nodes)
}

fn flush_text(nodes: &mut Vec<Node>, text: &mut String) {
    if !text.is_empty() {
        nodes.push(Node::Text(std::mem::take(text)));
    }
}

// ---------------------------------------------------------------------------
// Identifier case validation
// ---------------------------------------------------------------------------

/// Walk a parsed template tree and reject any variable reference that is not
/// written in uppercase. This enforces the RFC rule (§4.1) that placeholder
/// paths, loop variable bindings, ternary branch references, and condition
/// variable references MUST be uppercase. Variable and field *declarations*
/// in `params` remain lowercase; the renderer folds case when matching, so a
/// reference `[[[USERNAME]]]` resolves a variable declared as `username`.
fn validate_identifiers(nodes: &[Node]) -> Result<(), PromptParseError> {
    for node in nodes {
        match node {
            Node::Text(_) => {}
            Node::Placeholder(p) => ensure_uppercase(p, "placeholder")?,
            Node::Ternary {
                cond,
                true_branch,
                false_branch,
            } => {
                validate_cond_identifiers(cond)?;
                ensure_branch_uppercase(true_branch)?;
                ensure_branch_uppercase(false_branch)?;
            }
            Node::Loop { item, list, body } => {
                ensure_uppercase(item, "loop variable")?;
                ensure_uppercase(list, "loop list reference")?;
                validate_identifiers(body)?;
            }
            Node::If { cond, body } => {
                validate_cond_identifiers(cond)?;
                validate_identifiers(body)?;
            }
        }
    }
    Ok(())
}

/// Validate every variable reference inside a condition expression.
fn validate_cond_identifiers(e: &CondExpr) -> Result<(), PromptParseError> {
    match e {
        CondExpr::Literal(_) => Ok(()),
        CondExpr::Var(name) => ensure_uppercase(name, "condition variable"),
        CondExpr::Not(inner) => validate_cond_identifiers(inner),
        CondExpr::Bin { left, right, .. } => {
            validate_cond_identifiers(left)?;
            validate_cond_identifiers(right)
        }
    }
}

/// A ternary branch may be a `[[[VAR]]]` reference, a quoted string literal, a
/// bare number/boolean literal, or plain text. Only the `[[[VAR]]]` form is a
/// variable reference and must be uppercase; the rest is rendered verbatim.
fn ensure_branch_uppercase(branch: &str) -> Result<(), PromptParseError> {
    let s = branch.trim();
    if s.starts_with("[[[") && s.ends_with("]]]") && s.len() >= 6 {
        let inner = &s[3..s.len() - 3];
        if !inner.contains("]]]") {
            return ensure_uppercase(inner, "ternary branch variable");
        }
    }
    Ok(())
}

/// Reject an identifier that contains any lowercase ASCII letter. Digits,
/// underscores, dots, and uppercase letters are permitted.
fn ensure_uppercase(name: &str, ctx: &str) -> Result<(), PromptParseError> {
    let trimmed = name.trim();
    if trimmed.chars().any(|c| c.is_ascii_lowercase()) {
        return Err(PromptParseError::LowercaseIdentifier {
            name: trimmed.to_string(),
            ctx: ctx.to_string(),
        });
    }
    Ok(())
}

/// "trim_blocks": skip a single newline (`\n` or `\r\n`) immediately following
/// a block tag. This lets authors put `{{{for ...}}}` and `{{{if ...}}}` tags
/// on their own line without introducing an extra blank line in the output.
fn skip_one_newline(s: &str, i: usize) -> usize {
    if s[i..].starts_with("\r\n") {
        i + 2
    } else if s[i..].starts_with('\n') {
        i + 1
    } else {
        i
    }
}

/// Find the first occurrence of `needle` in `s` at or after `from`.
fn find_subseq(s: &str, from: usize, needle: &str) -> Option<usize> {
    if from > s.len() {
        return None;
    }
    s[from..].find(needle).map(|p| from + p)
}

/// Given the inner text of a `{{{for ...}}}` header (i.e. the part between
/// `{{{` and `}}}`), parse `for <item> in <list>` into (item, list).
fn parse_for_header(inner: &str) -> Result<(String, String), PromptParseError> {
    let inner = inner.trim();
    let rest = inner
        .strip_prefix("for")
        .ok_or_else(|| PromptParseError::UnmatchedConstruct("loop missing 'for'".into()))?
        .trim();
    let in_idx = rest
        .find(" in ")
        .ok_or_else(|| PromptParseError::UnmatchedConstruct("loop missing 'in'".into()))?;
    let item = rest[..in_idx].trim().to_string();
    let list_part = rest[in_idx + 4..].trim();
    // The loop list is a bare variable reference. For backward compatibility,
    // `[[[VAR]]]` wrapping is tolerated (and stripped) but not required:
    // `[[[...]]]` is reserved for printing into the prompt body.
    let list = list_part
        .strip_prefix("[[[")
        .and_then(|s| s.strip_suffix("]]]"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| list_part.to_string());
    Ok((item, list))
}

/// Does `s[i..]` open a `{{{for ...}}}` loop?
fn is_loop_open(s: &str, i: usize) -> bool {
    s[i..].starts_with("{{{for ")
}

/// Locate the matching `{{{end for}}}` for a loop whose body starts at
/// `body_start`, accounting for nested loops and skipping over ternaries
/// and if-block tags.
fn find_loop_end(s: &str, body_start: usize) -> Option<usize> {
    let mut i = body_start;
    let mut depth = 1;
    while i < s.len() {
        if is_loop_open(s, i) {
            depth += 1;
            // skip the whole opener tag `{{{for ...}}}`
            let e = s[i + 3..].find("}}}")?;
            i = i + 3 + e + 3;
        } else if s[i..].starts_with(END_FOR_BRACE) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
            i += END_FOR_BRACE.len();
        } else if s[i..].starts_with("{{{if ") {
            // skip an if-block opener tag `{{{if ...}}}`
            let e = s[i + 3..].find("}}}")?;
            i = i + 3 + e + 3;
        } else if s[i..].starts_with(END_IF) {
            i += END_IF.len();
        } else if s[i..].starts_with("{{{") {
            if let Some(e) = s[i + 3..].find("}}}") {
                i = i + 3 + e + 3;
            } else {
                i += 3;
            }
        } else {
            let ch = s[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
    }
    None
}

/// Locate the matching `{{{end if}}}` for an if-block whose body starts at
/// `body_start`, accounting for nested if-blocks, loops, and ternaries.
fn find_if_end(s: &str, body_start: usize) -> Option<usize> {
    let mut i = body_start;
    let mut depth = 1;
    while i < s.len() {
        if s[i..].starts_with("{{{if ") {
            depth += 1;
            // skip the whole opener tag `{{{if ...}}}`
            let e = s[i + 3..].find("}}}")?;
            i = i + 3 + e + 3;
        } else if s[i..].starts_with(END_IF) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
            i += END_IF.len();
        } else if is_loop_open(s, i) {
            // Skip an entire nested loop.
            let header_close = s[i + 3..].find("}}}")?;
            let loop_body_start = skip_one_newline(s, i + 3 + header_close + 3);
            if let Some(end) = find_loop_end(s, loop_body_start) {
                i = end + END_FOR_BRACE.len();
            } else {
                return None;
            }
        } else if s[i..].starts_with("{{{") {
            if let Some(e) = s[i + 3..].find("}}}") {
                i = i + 3 + e + 3;
            } else {
                i += 3;
            }
        } else {
            let ch = s[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
    }
    None
}

/// Split a ternary body `cond ? a : b` into three parts, respecting string
/// literals and parentheses.
fn split_ternary(inner: &str) -> Option<(String, String, String)> {
    let q = find_top_level(inner, '?')?;
    let c = find_top_level_from(inner, ':', q + 1)?;
    Some((
        inner[..q].to_string(),
        inner[q + 1..c].to_string(),
        inner[c + 1..].to_string(),
    ))
}

fn find_top_level(s: &str, target: char) -> Option<usize> {
    find_top_level_from(s, target, 0)
}

fn find_top_level_from(s: &str, target: char, from: usize) -> Option<usize> {
    let mut str_quote: Option<char> = None;
    let mut depth = 0i32;
    let mut i = from;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if let Some(q) = str_quote {
            if c == q {
                str_quote = None;
            }
            i += c.len_utf8();
            continue;
        }
        match c {
            '"' | '\'' => str_quote = Some(c),
            '(' => depth += 1,
            ')' => depth -= 1,
            ch if ch == target && depth == 0 => return Some(i),
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

// --- Condition expression parsing ---

#[derive(Debug, Clone)]
enum Tok {
    Var(String),
    Str(String),
    Num(f64),
    Bool(bool),
    Op(String),
    LParen,
    RParen,
}

fn tokenize_cond(s: &str) -> Result<Vec<Tok>, PromptParseError> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if s[i..].starts_with("[[[") {
            let end = s[i + 3..].find("]]]").ok_or_else(|| {
                PromptParseError::InvalidConditionSyntax("unterminated variable in condition".into())
            })?;
            toks.push(Tok::Var(s[i + 3..i + 3 + end].trim().to_string()));
            i = i + 3 + end + 3;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            let mut j = i + 1;
            let mut buf = String::new();
            while j < s.len() {
                let ch = s[j..].chars().next().unwrap();
                if ch == quote {
                    j += 1;
                    break;
                }
                buf.push(ch);
                j += ch.len_utf8();
            }
            toks.push(Tok::Str(buf));
            i = j;
            continue;
        }
        if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
            continue;
        }
        if c == '!' {
            if s[i..].starts_with("!=") {
                toks.push(Tok::Op("!=".into()));
                i += 2;
            } else {
                toks.push(Tok::Op("!".into()));
                i += 1;
            }
            continue;
        }
        if c == '>' {
            if s[i..].starts_with(">=") {
                toks.push(Tok::Op(">=".into()));
                i += 2;
            } else {
                toks.push(Tok::Op(">".into()));
                i += 1;
            }
            continue;
        }
        if c == '<' {
            if s[i..].starts_with("<=") {
                toks.push(Tok::Op("<=".into()));
                i += 2;
            } else {
                toks.push(Tok::Op("<".into()));
                i += 1;
            }
            continue;
        }
        if c == '=' {
            if s[i..].starts_with("==") {
                toks.push(Tok::Op("=".into()));
                i += 2;
            } else {
                toks.push(Tok::Op("=".into()));
                i += 1;
            }
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            let mut buf = String::new();
            while j < s.len() {
                let ch = s[j..].chars().next().unwrap();
                if ch.is_alphanumeric() || ch == '_' || ch == '.' {
                    buf.push(ch);
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            match buf.as_str() {
                "true" => toks.push(Tok::Bool(true)),
                "false" => toks.push(Tok::Bool(false)),
                "contains" | "starts_with" | "ends_with" => toks.push(Tok::Op(buf)),
                _ => toks.push(Tok::Var(buf)),
            }
            i = j;
            continue;
        }
        if c == '-' || c.is_ascii_digit() {
            let mut j = i;
            let mut buf = String::new();
            if c == '-' {
                buf.push('-');
                j += 1;
            }
            while j < s.len() {
                let ch = s[j..].chars().next().unwrap();
                if ch.is_ascii_digit() || ch == '.' {
                    buf.push(ch);
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            let n: f64 = buf.parse().map_err(|_| {
                PromptParseError::InvalidConditionSyntax(format!("bad number '{}'", buf))
            })?;
            toks.push(Tok::Num(n));
            i = j;
            continue;
        }
        return Err(PromptParseError::InvalidConditionSyntax(format!(
            "unexpected character '{}'",
            c
        )));
    }
    Ok(toks)
}

struct CondParser {
    toks: Vec<Tok>,
    pos: usize,
}

impl CondParser {
    fn parse(&mut self) -> Result<CondExpr, PromptParseError> {
        let e = self.parse_string_op()?;
        if self.pos != self.toks.len() {
            return Err(PromptParseError::InvalidConditionSyntax("trailing tokens".into()));
        }
        Ok(e)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn parse_string_op(&mut self) -> Result<CondExpr, PromptParseError> {
        let mut left = self.parse_equality()?;
        loop {
            match self.peek() {
                Some(Tok::Op(o))
                    if o == "contains" || o == "starts_with" || o == "ends_with" =>
                {
                    let op = o.clone();
                    self.pos += 1;
                    let right = self.parse_equality()?;
                    left = CondExpr::Bin {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<CondExpr, PromptParseError> {
        let mut left = self.parse_comparison()?;
        loop {
            match self.peek() {
                Some(Tok::Op(o)) if o == "=" || o == "!=" => {
                    let op = o.clone();
                    self.pos += 1;
                    let right = self.parse_comparison()?;
                    left = CondExpr::Bin {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<CondExpr, PromptParseError> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Tok::Op(o)) if o == ">" || o == "<" || o == ">=" || o == "<=" => {
                    let op = o.clone();
                    self.pos += 1;
                    let right = self.parse_unary()?;
                    left = CondExpr::Bin {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<CondExpr, PromptParseError> {
        if let Some(Tok::Op(o)) = self.peek() {
            if o == "!" {
                self.pos += 1;
                let inner = self.parse_unary()?;
                return Ok(CondExpr::Not(Box::new(inner)));
            }
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<CondExpr, PromptParseError> {
        match self.next() {
            Some(Tok::Var(name)) => Ok(CondExpr::Var(name)),
            Some(Tok::Str(s)) => Ok(CondExpr::Literal(VariableValue::String(s))),
            Some(Tok::Num(n)) => Ok(CondExpr::Literal(VariableValue::Number(n))),
            Some(Tok::Bool(b)) => Ok(CondExpr::Literal(VariableValue::Boolean(b))),
            Some(Tok::LParen) => {
                let e = self.parse()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err(PromptParseError::InvalidConditionSyntax("expected ')'".into())),
                }
            }
            other => Err(PromptParseError::InvalidConditionSyntax(format!(
                "unexpected token in condition: {:?}",
                other
            ))),
        }
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
}

/// Parse a condition expression string into a validated `CondExpr`.
fn parse_condition(src: &str) -> Result<CondExpr, PromptParseError> {
    let normalized = normalize_method_calls(src);
    let toks = tokenize_cond(&normalized)?;
    let mut p = CondParser { toks, pos: 0 };
    p.parse()
}

/// Rewrite method-call conditions (`x.contains(y)`, `x.starts_with(y)`,
/// `x.ends_with(y)`) into the RFC infix form (`x contains y`) so the rest of
/// the evaluator only needs to handle the infix operators from §5.
fn normalize_method_calls(src: &str) -> String {
    let methods = [
        ("contains", "contains"),
        ("starts_with", "starts_with"),
        ("ends_with", "ends_with"),
    ];
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    'outer: while i < src.len() {
        for (method, op) in &methods {
            let pat = format!(".{}", method);
            if !src[i..].starts_with(&pat) {
                continue;
            }
            let mut j = i + pat.len();
            while j < src.len() && src[j..].chars().next().unwrap().is_whitespace() {
                j += 1;
            }
            if j >= src.len() || !src[j..].starts_with('(') {
                continue;
            }
            // Find the matching closing paren, skipping string literals.
            let mut k = j + 1;
            let mut str_quote: Option<char> = None;
            while k < src.len() {
                let c = src[k..].chars().next().unwrap();
                if let Some(q) = str_quote {
                    if c == q {
                        str_quote = None;
                    }
                    k += c.len_utf8();
                    continue;
                }
                if c == '"' || c == '\'' {
                    str_quote = Some(c);
                } else if c == ')' {
                    break;
                }
                k += c.len_utf8();
            }
            out.push(' ');
            out.push_str(op);
            out.push(' ');
            out.push_str(&src[j + 1..k]);
            i = k + 1;
            continue 'outer;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}
