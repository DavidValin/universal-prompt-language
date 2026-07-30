// builder.rs
//
// PromptBuilder: collects values for a parsed Prompt's variables via an
// interactive TUI (powered by `inquire`), then renders the final prompt
// string by substituting `[[[VAR]]]` placeholders and evaluating the
// `{{{cond ? a : b}}}` ternaries, `{{{for x in list}}}...{{{end for}}}`
// loops and `{{{if cond}}}...{{{end if}}}` conditional blocks defined by
// the UPL specification (see upl-spec/upl-1.0-rfc.md).
//
// The rendering logic is intentionally decoupled from the interactive
// collection: `PromptBuilder::render` is a pure function over a pre-built
// value map, which makes it straightforward to unit-test.

use std::collections::HashMap;

use thiserror::Error;

use crate::upl::parser::{
    CondExpr, Node, ObjectMap, Prompt, VariableDefinition, VariableType, VariableValue,
};

/// Map of top-level variable name -> collected value.
pub type ValueMap = HashMap<String, VariableValue>;

/// A single frame in the lookup scope (variable name -> value).
type Frame = HashMap<String, VariableValue>;

#[derive(Error, Debug)]
pub enum BuilderError {
    #[error("No value provided for variable '{0}'")]
    MissingValue(String),
    #[error("Type error: {0}")]
    TypeError(String),
    #[error("Invalid condition syntax: {0}")]
    InvalidCondition(String),
    #[error("Variable '{0}' is not a list")]
    NotAList(String),
    #[error("TUI error: {0}")]
    Tui(String),
    #[error("cancelled")]
    Cancelled,
    /// User asked to go back to the previous parameter. Only used internally
    /// by `collect_values` to drive the back-navigation loop; it is converted
    /// to `Cancelled` when there is no previous field to go back to.
    #[error("back")]
    Back,
}

/// Map an `inquire::InquireError` to a `BuilderError`.
///
/// - Ctrl+C (`OperationInterrupted`) cancels the whole build.
/// - Esc (`OperationCanceled`) asks to go back to the previous parameter;
///   `collect_values` turns this into `Cancelled` when on the first field.
fn map_inquire_err(e: inquire::InquireError) -> BuilderError {
    match e {
        inquire::InquireError::OperationInterrupted => BuilderError::Cancelled,
        inquire::InquireError::OperationCanceled => BuilderError::Back,
        other => BuilderError::Tui(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub struct PromptBuilder {
    prompt: Prompt,
}

impl PromptBuilder {
    pub fn new(prompt: Prompt) -> Self {
        Self { prompt }
    }

    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// Collect values interactively via the TUI, then render the final prompt.
    pub fn build_interactive(&self) -> Result<String, BuilderError> {
        let values = self.collect_values()?;
        self.render(&values)
    }

    /// Pure render: substitute placeholders and evaluate constructs using the
    /// supplied top-level `values`. No TUI interaction. The body is already
    /// parsed into a `Template` by the parser, so this only walks the nodes.
    pub fn render(&self, values: &ValueMap) -> Result<String, BuilderError> {
        let mut scope: Vec<Frame> = vec![values.clone()];
        render_nodes(&self.prompt.template.nodes, &mut scope)
    }

    /// Render non-interactively using the `def:` defaults declared in the
    /// prompt file. Useful for testing / piping the rendered prompt without
    /// driving the TUI. Missing defaults fall back to type-appropriate zeros.
    pub fn render_with_defaults(&self) -> Result<String, BuilderError> {
        let values = self.defaults_to_values();
        self.render(&values)
    }

    /// Reconstruct a hierarchical `ValueMap` (top-level name -> value) from
    /// the flat, dotted-key `variable_defaults` map produced by the parser.
    pub fn defaults_to_values(&self) -> ValueMap {
        let mut map = ValueMap::new();
        for (key, def) in &self.prompt.variable_definitions {
            map.insert(key.clone(), self.default_value(key, def));
        }
        map
    }

    fn default_value(&self, path: &str, def: &VariableDefinition) -> VariableValue {
        use VariableType::*;
        match def.r#type {
            String | LongString => self
                .prompt
                .variable_defaults
                .get(path)
                .cloned()
                .unwrap_or(VariableValue::String(::std::string::String::new())),
            Number => self
                .prompt
                .variable_defaults
                .get(path)
                .cloned()
                .unwrap_or(VariableValue::Number(0.0)),
            Boolean => self
                .prompt
                .variable_defaults
                .get(path)
                .cloned()
                .unwrap_or(VariableValue::Boolean(false)),
            OptionSingle => {
                let etype = def.element_type.unwrap_or(VariableType::String);
                if let Some(v) = self.prompt.variable_defaults.get(path) {
                    return v.clone();
                }
                if let Some(opts) = &def.options {
                    if let Some(first) = opts.first() {
                        return first.clone();
                    }
                }
                option_type_zero(etype)
            }
            OptionMulti => {
                if let Some(v) = self.prompt.variable_defaults.get(path) {
                    return v.clone();
                }
                VariableValue::List(vec![])
            }
            Object => {
                let mut map = ObjectMap::new();
                if let Some(nested) = &def.ofields_definitions {
                    for (k, nd) in nested {
                        let npath = format!("{}.{}", path, k);
                        map.insert(k.clone(), self.default_value(&npath, nd));
                    }
                }
                VariableValue::Object(map)
            }
            List => {
                // Honor an inline `def:` list verbatim; otherwise default to
                // an empty list (RFC §3 — `def` is optional, list falls back
                // to `[]`). The element type's shape is still declared so the
                // interactive collector can prompt for items when invoked.
                if let Some(VariableValue::List(items)) = self.prompt.variable_defaults.get(path)
                {
                    return VariableValue::List(items.clone());
                }
                VariableValue::List(vec![])
            }
        }
    }

    // -----------------------------------------------------------------------
    // Interactive collection
    // -----------------------------------------------------------------------

    /// Collect values interactively via the TUI.
    ///
    /// The user can go back to the previous parameter at any time:
    ///   - on an `inquire` prompt, press Esc (Ctrl+C cancels the whole build);
    ///   - on a `long_string` prompt, type `:back` as the first line.
    ///
    /// Already-collected values are retained in `values` and passed back as
    /// the default when a field is re-collected after going back, so previous
    /// answers are preserved (not reset). Going back from the first parameter
    /// cancels the build.
    fn collect_values(&self) -> Result<ValueMap, BuilderError> {
        let defs: Vec<(String, &VariableDefinition)> = self
            .prompt
            .variable_definitions
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect();
        let mut values: Vec<VariableValue> = Vec::with_capacity(defs.len());
        let mut idx = 0usize;
        while idx < defs.len() {
            let (key, def) = &defs[idx];
            // Prefer the previously collected value (so going back keeps it),
            // falling back to the prompt's declared `def:` default.
            let default = values
                .get(idx)
                .cloned()
                .or_else(|| self.prompt.variable_defaults.get(key).cloned());
            match self.collect_definition(key, def, default.as_ref()) {
                Ok(v) => {
                    if idx < values.len() {
                        values[idx] = v;
                    } else {
                        values.push(v);
                    }
                    idx += 1;
                }
                Err(BuilderError::Back) => {
                    if idx == 0 {
                        return Err(BuilderError::Cancelled);
                    }
                    idx -= 1;
                }
                Err(e) => return Err(e),
            }
        }
        let mut map = ValueMap::new();
        for ((key, _), v) in defs.iter().zip(values) {
            map.insert(key.clone(), v);
        }
        Ok(map)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_definition(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        use VariableType::*;
        match def.r#type {
            String => self.collect_string(path, def, default),
            LongString => self.collect_long_string(path, def, default),
            Number => self.collect_number(path, def, default),
            Boolean => self.collect_boolean(path, def, default),
            OptionSingle => self.collect_option_single(path, def, default),
            OptionMulti => self.collect_option_multi(path, def, default),
            Object => self.collect_object(path, def),
            List => self.collect_list(path, def, default),
        }
    }

    fn collect_string(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let def_str = default.and_then(|v| match v {
            VariableValue::String(s) => Some(s.clone()),
            _ => None,
        });
        let lbl = label(path, def);
        let help = help_with_back(def);
        let text = inquire::Text::new(&lbl)
            .with_default(def_str.as_deref().unwrap_or(""))
            .with_help_message(&help);
        let ans = text
            .prompt()
            .map_err(map_inquire_err)?;
        Ok(VariableValue::String(ans))
    }

    /// Multiline input for `long_string` fields. Prints the label on its own
    /// line and leaves the cursor at the beginning of the next line so the
    /// user can freely paste long text. Input continues until the user enters
    /// two consecutive lines, each containing exactly a single '.' (and
    /// nothing else). Those two terminator lines are stripped from the
    /// result.
    fn collect_long_string(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        use std::io::BufRead;
        let lbl = label(path, def);
        eprint!("{}", lbl);
        if let Some(desc) = desc(def) {
            eprint!(" - {}", desc);
        }
        eprintln!();
        if let Some(d) = default.and_then(|v| match v {
            VariableValue::String(s) | VariableValue::LongString(s) => Some(s.clone()),
            _ => None,
        }) {
            eprintln!("(default: leave empty to use \"{}\")", d);
        }
        eprintln!("(enter your text; finish with two consecutive lines containing only '.')");
        eprintln!("(type ':back' as the first line to go back to the previous parameter)");
        // Flush so the prompt appears before reading.
        use std::io::Write;
        let _ = std::io::stderr().flush();

        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        let mut buf = String::new();
        let mut prev_dot = false;
        let mut got_any = false;
        let mut first = true;
        while let Some(Ok(line)) = lines.next() {
            // `:back` on the first line (before any content) goes back.
            if first && line.trim() == ":back" {
                return Err(BuilderError::Back);
            }
            first = false;
            got_any = true;
            if line == "." {
                if prev_dot {
                    // Remove the lone '.' line we appended on the previous
                    // iteration (it was the first of the two terminator lines).
                    if buf.ends_with(".\n") {
                        buf.truncate(buf.len() - 2);
                    }
                    break;
                }
                prev_dot = true;
                buf.push_str(&line);
                buf.push('\n');
            } else {
                prev_dot = false;
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        // EOF / Ctrl+D without a terminator: keep what we have, but if nothing
        // at all was entered fall back to the default.
        if !got_any || (buf.is_empty() && prev_dot == false) {
            if let Some(d) = default.and_then(|v| match v {
                VariableValue::String(s) | VariableValue::LongString(s) => Some(s.clone()),
                _ => None,
            }) {
                if buf.is_empty() {
                    return Ok(VariableValue::LongString(d));
                }
            }
        }
        // Strip a single trailing newline (the one after the last content line).
        if buf.ends_with('\n') {
            buf.truncate(buf.len() - 1);
        }
        Ok(VariableValue::LongString(buf))
    }

    fn collect_number(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let def_str = default.and_then(|v| match v {
            VariableValue::Number(n) => Some(number_to_string(*n)),
            _ => None,
        });
        let lbl = label(path, def);
        let mut text = inquire::Text::new(&lbl)
            .with_default(def_str.as_deref().unwrap_or("0"))
            .with_validator(|s: &str| -> Result<inquire::validator::Validation, inquire::CustomUserError> {
                if s.parse::<f64>().is_ok() {
                    Ok(inquire::validator::Validation::Valid)
                } else {
                    Ok(inquire::validator::Validation::Invalid("not a number".into()))
                }
            });
        let help = help_with_back(def);
        text = text.with_help_message(&help);
        let ans = text
            .prompt()
            .map_err(map_inquire_err)?;
        Ok(VariableValue::Number(ans.parse().unwrap_or(0.0)))
    }

    fn collect_boolean(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let def_b = default
            .and_then(|v| match v {
                VariableValue::Boolean(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false);
        let lbl = label(path, def);
        let mut confirm = inquire::Confirm::new(&lbl)
            .with_default(def_b);
        let help = match desc(def) {
            Some(desc) => format!("{} · true / false{BACK_HINT}", desc),
            None => format!("true / false{BACK_HINT}"),
        };
        confirm = confirm.with_help_message(&help);
        let ans = confirm
            .prompt()
            .map_err(map_inquire_err)?;
        Ok(VariableValue::Boolean(ans))
    }

    fn collect_option_single(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let etype = def.element_type.unwrap_or(VariableType::String);
        let opts = option_values(def)?;
        let labels = option_labels(&opts, def)?;
        let def_idx = default.and_then(|v| option_match_index(v, &opts, etype));
        let lbl = label(path, def);
        let mut select = inquire::Select::new(&lbl, labels.clone());
        let help = match (desc(def), &def_idx) {
            (Some(desc), Some(i)) => format!("{} · default: {}{BACK_HINT}", desc, labels[*i]),
            (Some(desc), None) => format!("{} · select one{BACK_HINT}", desc),
            (None, Some(i)) => format!("default: {}{BACK_HINT}", labels[*i]),
            (None, None) => format!("select one{BACK_HINT}"),
        };
        select = select.with_help_message(&help);
        let ans = select
            .prompt()
            .map_err(map_inquire_err)?;
        let idx = labels.iter().position(|l| l == &ans).unwrap_or(0);
        Ok(opts[idx].clone())
    }

    fn collect_option_multi(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let etype = def.element_type.unwrap_or(VariableType::String);
        let opts = option_values(def)?;
        let labels = option_labels(&opts, def)?;
        let preselected: Vec<usize> = match default {
            Some(VariableValue::List(l)) => l
                .iter()
                .filter_map(|v| option_match_index(v, &opts, etype))
                .collect(),
            _ => vec![],
        };
        let lbl = label(path, def);
        let help = help_with_back(def);
        let mut mselect = inquire::MultiSelect::new(&lbl, labels.clone());
        mselect = mselect.with_help_message(&help);
        // Pre-select default indices.
        if !preselected.is_empty() {
            mselect = mselect.with_default(&preselected);
        }
        let ans = mselect
            .prompt()
            .map_err(map_inquire_err)?;
        let chosen: Vec<VariableValue> = ans
            .into_iter()
            .filter_map(|label| labels.iter().position(|l| *l == label).map(|i| opts[i].clone()))
            .collect();
        Ok(VariableValue::List(chosen))
    }

    fn collect_object(&self, path: &str, def: &VariableDefinition) -> Result<VariableValue, BuilderError> {
        let nested = def
            .ofields_definitions
            .as_ref()
            .ok_or_else(|| BuilderError::TypeError(format!("object '{}' has no ofields block", path)))?;
        let mut map = ObjectMap::new();
        for (k, nd) in nested {
            let npath = format!("{}.{}", path, k);
            let ndefault = self.prompt.variable_defaults.get(&npath);
            let v = self.collect_definition(&npath, nd, ndefault)?;
            map.insert(k.clone(), v);
        }
        Ok(VariableValue::Object(map))
    }

    fn collect_list(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let etype = def
            .element_type
            .ok_or_else(|| BuilderError::TypeError(format!("list '{}' has no etype", path)))?;
        let def_count = match default {
            Some(VariableValue::List(l)) => l.len(),
            _ => 0,
        };
        let count_lbl = format!("{} (number of items)", path);
        let def_count_str = def_count.to_string();
        let mut text = inquire::Text::new(&count_lbl)
            .with_default(&def_count_str)
            .with_validator(|s: &str| -> Result<inquire::validator::Validation, inquire::CustomUserError> {
                if s.parse::<usize>().is_ok() {
                    Ok(inquire::validator::Validation::Valid)
                } else {
                    Ok(inquire::validator::Validation::Invalid("not a number".into()))
                }
            });
        let help = help_with_back(def);
        text = text.with_help_message(&help);
        let count_str = text
            .prompt()
            .map_err(map_inquire_err)?;
        let count: usize = count_str.parse().unwrap_or(0);

        // Synthesize a definition for the element type.
        let elem_def = VariableDefinition {
            r#type: etype,
            desc: def.desc.clone(),
            options: def.options.clone(),
            element_type: def.element_type,
            element_ref: None,
            label: None,
            ofields_definitions: def.ofields_definitions.clone(),
        };
        let mut items = Vec::with_capacity(count);
        for idx in 0..count {
            let ipath = format!("{}[{}]", path, idx);
            let v = self.collect_definition(&ipath, &elem_def, None)?;
            items.push(v);
        }
        Ok(VariableValue::List(items))
    }
}

// ---------------------------------------------------------------------------
// Helpers shared by collection and rendering
// ---------------------------------------------------------------------------

fn label(path: &str, _def: &VariableDefinition) -> String {
    path.to_string()
}

fn desc(def: &VariableDefinition) -> Option<&str> {
    def.desc.as_ref().filter(|d| !d.is_empty()).map(|d| d.as_str())
}

/// Help-message suffix advertising the back/cancel keys for `inquire` prompts.
const BACK_HINT: &str = " · Esc: back · Ctrl+C: cancel";

/// Build a help message for an `inquire` prompt: the field's `desc` (if any)
/// followed by the back/cancel key hint.
fn help_with_back(def: &VariableDefinition) -> String {
    match desc(def) {
        Some(d) => format!("{d}{BACK_HINT}"),
        None => format!("Esc: back · Ctrl+C: cancel"),
    }
}

/// Type-appropriate zero value for an option etype, used as a last-resort
/// fallback when no `def` and no `opts` are present (parser normally
/// requires `opts`).
fn option_type_zero(etype: VariableType) -> VariableValue {
    match etype {
        VariableType::String => VariableValue::String(String::new()),
        VariableType::LongString => VariableValue::LongString(String::new()),
        VariableType::Number => VariableValue::Number(0.0),
        VariableType::Object => VariableValue::Object(ObjectMap::new()),
        _ => VariableValue::String(String::new()),
    }
}

/// Return the raw, etype-typed option values for an `option_single` /
/// `option_multi` variable. The parser already validated that every entry
/// matches `def.element_type` (defaulting to `string` for `option_single`),
/// so this only rejects an empty/missing `opts` list.
fn option_values(def: &VariableDefinition) -> Result<Vec<VariableValue>, BuilderError> {
    def.options
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| BuilderError::TypeError("option type has no opts".into()))
}

/// Produce a display label for each option value, for use in the
/// `inquire::Select`/`MultiSelect` menus.
///
/// - Scalar etypes (`string`, `long_string`, `number`) render verbatim via
///   `stringify`.
/// - Object etype: the value of the field named by `def.label` on each option
///   object. The parser guarantees `label` is set and the field exists and is
///   string-valued, so this never fails for a conformant file.
fn option_labels(
    opts: &[VariableValue],
    def: &VariableDefinition,
) -> Result<Vec<String>, BuilderError> {
    let is_object = def.element_type == Some(VariableType::Object);
    if is_object {
        let label_field = def
            .label
            .as_deref()
            .ok_or_else(|| BuilderError::TypeError("object etype option has no label".into()))?;
        let lbl_lc = label_field.to_lowercase();
        let mut out = Vec::with_capacity(opts.len());
        for v in opts {
            if let VariableValue::Object(map) = v {
                let entry = map.iter().find(|(k, _)| k.to_lowercase() == lbl_lc);
                let label = match entry {
                    Some((_, VariableValue::String(s) | VariableValue::LongString(s))) => {
                        s.clone()
                    }
                    _ => {
                        return Err(BuilderError::TypeError(format!(
                            "option object missing label field '{}'",
                            label_field
                        )));
                    }
                };
                out.push(label);
            } else {
                return Err(BuilderError::TypeError(
                    "object-etype option is not an object".into(),
                ));
            }
        }
        Ok(out)
    } else {
        opts.iter().map(stringify).collect()
    }
}

/// Find the index of `v` within `opts` comparing by etype-appropriate equality.
/// Used to locate the default selection.
fn option_match_index(
    v: &VariableValue,
    opts: &[VariableValue],
    etype: VariableType,
) -> Option<usize> {
    use VariableValue::*;
    match (etype, v) {
        (VariableType::String, String(s)) => opts.iter().position(|o| matches!(o, String(x) if x == s)),
        (VariableType::LongString, LongString(s) | String(s)) => opts
            .iter()
            .position(|o| matches!(o, LongString(x) | String(x) if x == s)),
        (VariableType::Number, Number(n)) => opts
            .iter()
            .position(|o| matches!(o, Number(x) if x == n)),
        (VariableType::Object, Object(m)) => opts.iter().position(|o| {
            if let Object(x) = o {
                maps_equal(x, m)
            } else {
                false
            }
        }),
        _ => None,
    }
}

fn maps_equal(a: &ObjectMap, b: &ObjectMap) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (k, va) in a {
        match b.get(k) {
            Some(vb) if values_equal(va, vb) => {}
            _ => return false,
        }
    }
    true
}

fn values_equal(a: &VariableValue, b: &VariableValue) -> bool {
    use VariableValue::*;
    match (a, b) {
        (String(x), String(y)) | (LongString(x), LongString(y))
        | (String(x), LongString(y)) | (LongString(x), String(y)) => x == y,
        (Number(x), Number(y)) => x == y,
        (Boolean(x), Boolean(y)) => x == y,
        (List(x), List(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b)),
        (Object(x), Object(y)) => maps_equal(x, y),
        _ => false,
    }
}

fn number_to_string(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

fn stringify(v: &VariableValue) -> Result<String, BuilderError> {
    Ok(match v {
        VariableValue::String(s) | VariableValue::LongString(s) => s.clone(),
        VariableValue::Number(n) => number_to_string(*n),
        VariableValue::Boolean(b) => b.to_string(),
        VariableValue::List(items) => items
            .iter()
            .map(stringify)
            .collect::<Result<Vec<_>, _>>()?
            .join(", "),
        VariableValue::Object(map) => {
            let mut parts = Vec::new();
            for (k, val) in map.iter() {
                parts.push(format!("{}: {}", k, stringify(val)?));
            }
            parts.join(", ")
        }
    })
}

fn truthy(v: &VariableValue) -> bool {
    match v {
        VariableValue::Boolean(b) => *b,
        VariableValue::String(s) | VariableValue::LongString(s) => !s.is_empty(),
        VariableValue::Number(n) => *n != 0.0,
        VariableValue::List(l) => !l.is_empty(),
        VariableValue::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_nodes(nodes: &[Node], scope: &mut Vec<Frame>) -> Result<String, BuilderError> {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Placeholder(p) => {
                let v = lookup(p, scope)
                    .ok_or_else(|| BuilderError::MissingValue(p.clone()))?;
                out.push_str(&stringify(&v)?);
            }
            Node::Ternary {
                cond,
                true_branch,
                false_branch,
            } => {
                let cv = eval(cond, scope)?;
                let branch = if truthy(&cv) { true_branch } else { false_branch };
                out.push_str(&render_value_expr(branch, scope)?);
            }
            Node::If { cond, body } => {
                let cv = eval(cond, scope)?;
                if truthy(&cv) {
                    out.push_str(&render_nodes(body, scope)?);
                }
            }
            Node::Loop { item, list, body } => {
                let list_val = lookup(list, scope)
                    .ok_or_else(|| BuilderError::MissingValue(list.clone()))?;
                if let VariableValue::List(items) = list_val {
                    for elem in items {
                        let mut frame = Frame::new();
                        frame.insert(item.clone(), elem.clone());
                        scope.push(frame);
                        out.push_str(&render_nodes(body, scope)?);
                        scope.pop();
                    }
                } else {
                    return Err(BuilderError::NotAList(list.clone()));
                }
            }
        }
    }
    Ok(out)
}

/// Render a ternary branch: either a `"literal"`, a `[[[var]]]` reference,
/// a bare number/boolean literal, or plain text.
fn render_value_expr(s: &str, scope: &[Frame]) -> Result<String, BuilderError> {
    let s = s.trim();
    if s.starts_with("[[[") && s.ends_with("]]]") && s[3..].find("]]]") == Some(s.len() - 6) {
        let name = &s[3..s.len() - 3];
        let v = lookup(name, scope)
            .ok_or_else(|| BuilderError::MissingValue(name.to_string()))?;
        return stringify(&v);
    }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return Ok(s[1..s.len() - 1].to_string());
    }
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        return Ok(s[1..s.len() - 1].to_string());
    }
    Ok(s.to_string())
}

/// Case-insensitive lookup of a dotted path against the scope stack.
fn lookup(path: &str, scope: &[Frame]) -> Option<VariableValue> {
    let parts: Vec<&str> = path.split('.').collect();
    let first = parts[0].to_lowercase();
    for frame in scope.iter().rev() {
        for (k, v) in frame.iter() {
            if k.to_lowercase() == first {
                return traverse(v, &parts[1..]);
            }
        }
    }
    None
}

fn traverse(value: &VariableValue, parts: &[&str]) -> Option<VariableValue> {
    let mut cur = value.clone();
    for p in parts {
        let pl = p.to_lowercase();
        match &cur {
            VariableValue::Object(map) => {
                let mut found = None;
                for (k, v) in map.iter() {
                    if k.to_lowercase() == pl {
                        found = Some(v.clone());
                        break;
                    }
                }
                cur = found?;
            }
            // List projection: when traversing into a list of objects with a
            // field name, project that field across every element and return
            // the resulting list. Lets `[[[model.fields.name]]]` expand to the
            // list of names from a list-of-objects variable.
            VariableValue::List(items) => {
                let mut projected = Vec::with_capacity(items.len());
                for item in items {
                    if let VariableValue::Object(_) = item {
                        projected.push(traverse(item, &[p])?);
                    } else {
                        return None;
                    }
                }
                cur = VariableValue::List(projected);
            }
            _ => return None,
        }
    }
    Some(cur)
}

// ---------------------------------------------------------------------------
// Condition evaluation (the condition AST is parsed by the parser)
// ---------------------------------------------------------------------------

fn eval(e: &CondExpr, scope: &[Frame]) -> Result<VariableValue, BuilderError> {
    match e {
        CondExpr::Literal(v) => Ok(v.clone()),
        CondExpr::Var(name) => lookup(name, scope)
            .ok_or_else(|| BuilderError::MissingValue(name.clone())),
        CondExpr::Not(inner) => {
            let v = eval(inner, scope)?;
            Ok(VariableValue::Boolean(!truthy(&v)))
        }
        CondExpr::Bin { op, left, right } => {
            let l = eval(left, scope)?;
            let r = eval(right, scope)?;
            eval_bin(op, &l, &r)
        }
    }
}

fn eval_bin(op: &str, l: &VariableValue, r: &VariableValue) -> Result<VariableValue, BuilderError> {
    use VariableValue::*;
    match op {
        "=" => Ok(Boolean(values_eq(l, r)?)),
        "!=" => Ok(Boolean(!values_eq(l, r)?)),
        ">" | "<" | ">=" | "<=" => {
            let (a, b) = as_numbers(l, r, op)?;
            let res = match op {
                ">" => a > b,
                "<" => a < b,
                ">=" => a >= b,
                "<=" => a <= b,
                _ => unreachable!(),
            };
            Ok(Boolean(res))
        }
        "contains" | "starts_with" | "ends_with" => {
            // `contains` is overloaded: works on strings and on lists.
            if op == "contains" {
                if let (List(items), _) | (_, List(items)) = (l, r) {
                    let needle = match r {
                        String(s) | LongString(s) => s.clone(),
                        Number(n) => number_to_string(*n),
                        Boolean(b) => b.to_string(),
                        _ => return Err(BuilderError::TypeError(format!(
                            "cannot check membership of {:?} in list",
                            r
                        ))),
                    };
                    let found = items.iter().any(|v| match v {
                        String(s) | LongString(s) => s == &needle,
                        Number(n) => number_to_string(*n) == needle,
                        Boolean(b) => b.to_string() == needle,
                        _ => false,
                    });
                    return Ok(Boolean(found));
                }
            }
            let (a, b) = as_strings(l, r, op)?;
            let res = match op {
                "contains" => a.contains(&b),
                "starts_with" => a.starts_with(&b),
                "ends_with" => a.ends_with(&b),
                _ => unreachable!(),
            };
            Ok(Boolean(res))
        }
        _ => Err(BuilderError::InvalidCondition(format!(
            "unknown operator '{}'",
            op
        ))),
    }
}

fn values_eq(l: &VariableValue, r: &VariableValue) -> Result<bool, BuilderError> {
    use VariableValue::*;
    match (l, r) {
        (Number(a), Number(b)) => Ok(a == b),
        (String(a), String(b)) | (LongString(a), LongString(b)) | (String(a), LongString(b))
        | (LongString(a), String(b)) => Ok(a == b),
        (Boolean(a), Boolean(b)) => Ok(a == b),
        _ => Err(BuilderError::TypeError(format!(
            "cannot compare {:?} with {:?}",
            l, r
        ))),
    }
}

fn as_numbers(
    l: &VariableValue,
    r: &VariableValue,
    op: &str,
) -> Result<(f64, f64), BuilderError> {
    use VariableValue::*;
    match (l, r) {
        (Number(a), Number(b)) => Ok((*a, *b)),
        _ => Err(BuilderError::TypeError(format!(
            "operator '{}' requires numbers, got {:?} and {:?}",
            op, l, r
        ))),
    }
}

fn as_strings(
    l: &VariableValue,
    r: &VariableValue,
    op: &str,
) -> Result<(String, String), BuilderError> {
    use VariableValue::*;
    match (l, r) {
        (String(a) | LongString(a), String(b) | LongString(b)) => Ok((a.clone(), b.clone())),
        _ => Err(BuilderError::TypeError(format!(
            "operator '{}' requires strings, got {:?} and {:?}",
            op, l, r
        ))),
    }
}
