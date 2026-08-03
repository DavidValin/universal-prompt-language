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

use std::collections::{HashMap, HashSet};

use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::execute;
use std::io::Write as _;
use thiserror::Error;

use crate::upl::parser::{
    CondExpr, Node, ObjectMap, Prompt, VariableDefinition, VariableType, VariableValue,
};
use crate::manager::build_history::{
    self, BuildRecord, HistoryContext, SidebarOutcome,
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
    #[error("validation error: {0}")]
    Validation(String),
    /// The user opened the build-history sidebar (Ctrl+H) and chose to
    /// resume or rebuild a different build. The caller should load the
    /// prompt identified by the record with this uuid and start a new
    /// build. The current build's state has already been persisted as
    /// in-progress.
    #[error("switch to build {uuid}")]
    SwitchBuild { uuid: String },
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
        let values = self.collect_values(&mut None, None, false)?;
        self.render(&values)
    }

    /// Like `build_interactive` but tracks progress in the build-history store
    /// (`~/.upl/build_history.json`). After each field is collected the record
    /// is updated and persisted, so an interrupted build can be resumed.
    /// Between fields, Ctrl+H opens the build-history sidebar.
    pub fn build_interactive_tracked(
        &self,
        prompt_path: &str,
        prompt_sha256: &str,
    ) -> Result<String, BuilderError> {
        let total = self.collectible_field_count();
        let mut hctx = Some(HistoryContext::new(
            prompt_sha256,
            &self.prompt.name,
            prompt_path,
            total,
        ));
        let values = self.collect_values(&mut hctx, None, false)?;
        if let Some(h) = hctx.as_mut() {
            h.mark_built();
        }
        self.render(&values)
    }

    /// Resume a previously interrupted build. The saved values are used as
    /// defaults for already-collected fields; collection continues from the
    /// first un-collected field. History tracking continues so the resumed
    /// build can be interrupted again.
    ///
    /// When `rebuild` is `true` (the record was fully built), all fields are
    /// pre-filled and the cursor is positioned at the **last** field so the
    /// user can review it and immediately build (or go back to edit earlier
    /// fields).
    pub fn resume_interactive(
        &self,
        record: &BuildRecord,
        rebuild: bool,
    ) -> Result<String, BuilderError> {
        let mut hctx = Some(HistoryContext::from_record(record));
        let values = self.collect_values(&mut hctx, Some(&record.values), rebuild)?;
        if let Some(h) = hctx.as_mut() {
            h.mark_built();
        }
        self.render(&values)
    }

    /// Number of collectible top-level fields (excluding `object_shape` type
    /// definitions, which are never prompted).
    pub fn collectible_field_count(&self) -> usize {
        let referenced = self.referenced_type_defs();
        self.prompt
            .variable_definitions
            .iter()
            .filter(|(k, _)| !referenced.contains(&k.to_lowercase()))
            .count()
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
            Object | ObjectShape => {
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
    // JSON-based building (non-interactive)
    // -----------------------------------------------------------------------

    /// Build the final prompt from a JSON string of parameter values.
    ///
    /// The JSON root must be an object whose keys are parameter names
    /// (matched case-insensitively against the declared names). Each value
    /// is converted to the declared type and validated:
    ///
    /// - `string` / `long_string` ← JSON string
    /// - `number` ← JSON number
    /// - `boolean` ← JSON boolean
    /// - `object` ← JSON object (fields matched case-insensitively; missing
    ///   fields fall back to declared defaults)
    /// - `list` ← JSON array (each element converted per the list's etype)
    /// - `option_single` ← a single value matching the etype and one of the
    ///   declared `opts`
    /// - `option_multi` ← a JSON array where each element matches the etype
    ///   and one of the declared `opts`
    ///
    /// Parameters absent from the JSON fall back to declared `def:` defaults
    /// (or type-appropriate zeros when no `def` is declared). `object_shape`
    /// type definitions are never expected in the JSON and are always seeded
    /// from defaults. A JSON `null` for a key means "use the default".
    pub fn build_from_json(&self, json: &str) -> Result<String, BuilderError> {
        let values = self.values_from_json(json)?;
        self.render(&values)
    }

    /// Parse a JSON string into a validated `ValueMap`, merged with declared
    /// defaults. This is the non-interactive counterpart of `collect_values`.
    pub fn values_from_json(&self, json: &str) -> Result<ValueMap, BuilderError> {
        let root: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| BuilderError::Validation(format!("invalid JSON: {}", e)))?;
        let obj = root
            .as_object()
            .ok_or_else(|| BuilderError::Validation("JSON root must be an object".into()))?;

        let referenced = self.referenced_type_defs();

        // Start with defaults for all declared variables (including
        // object_shape type definitions, which are seeded but never expected
        // in the JSON).
        let mut values = self.defaults_to_values();

        for (key, jval) in obj {
            // Find the matching variable definition (case-insensitive).
            let (declared_name, def) = self
                .prompt
                .variable_definitions
                .iter()
                .find(|(k, _)| k.to_lowercase() == key.to_lowercase())
                .ok_or_else(|| {
                    BuilderError::Validation(format!(
                        "unknown parameter '{}' in JSON (not declared in prompt)",
                        key
                    ))
                })?;

            // object_shape types are not settable.
            if referenced.contains(&declared_name.to_lowercase()) {
                return Err(BuilderError::Validation(format!(
                    "parameter '{}' is an object_shape type definition and cannot be set via JSON",
                    key
                )));
            }

            // null means "use default" — skip overriding.
            if jval.is_null() {
                continue;
            }

            let val = self.json_to_variable_value(jval, def, declared_name)?;
            values.insert(declared_name.clone(), val);
        }

        Ok(values)
    }

    /// Convert a JSON value into a `VariableValue` according to the declared
    /// variable definition. For objects, missing fields fall back to declared
    /// defaults; for lists, each element is converted per the list's etype;
    /// for options, the converted value is validated against the declared
    /// `opts`.
    fn json_to_variable_value(
        &self,
        json: &serde_json::Value,
        def: &VariableDefinition,
        path: &str,
    ) -> Result<VariableValue, BuilderError> {
        use serde_json::Value as J;
        use VariableType as T;

        match def.r#type {
            T::String => match json {
                J::String(s) => Ok(VariableValue::String(s.clone())),
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects a string, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::LongString => match json {
                J::String(s) => Ok(VariableValue::LongString(s.clone())),
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects a long_string, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::Number => match json {
                J::Number(n) => n.as_f64().map(VariableValue::Number).ok_or_else(|| {
                    BuilderError::Validation(format!(
                        "parameter '{}' expects a number, got {}",
                        path,
                        json
                    ))
                }),
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects a number, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::Boolean => match json {
                J::Bool(b) => Ok(VariableValue::Boolean(*b)),
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects a boolean, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::OptionSingle => {
                let val = self.json_to_option_element(json, def, path)?;
                if let Some(opts) = &def.options {
                    if !opts.iter().any(|o| values_equal(o, &val)) {
                        return Err(BuilderError::Validation(format!(
                            "value for '{}' is not one of the declared opts",
                            path
                        )));
                    }
                }
                Ok(val)
            }
            T::OptionMulti => match json {
                J::Array(arr) => {
                    let mut items = Vec::with_capacity(arr.len());
                    for (i, elem) in arr.iter().enumerate() {
                        let ipath = format!("{}[{}]", path, i);
                        let val = self.json_to_option_element(elem, def, &ipath)?;
                        if let Some(opts) = &def.options {
                            if !opts.iter().any(|o| values_equal(o, &val)) {
                                return Err(BuilderError::Validation(format!(
                                    "element {} of '{}' is not one of the declared opts",
                                    i,
                                    path
                                )));
                            }
                        }
                        items.push(val);
                    }
                    Ok(VariableValue::List(items))
                }
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects an array, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::Object => match json {
                J::Object(obj) => {
                    let ofields = def.ofields_definitions.as_ref().ok_or_else(|| {
                        BuilderError::TypeError(format!(
                            "object '{}' has no ofields block",
                            path
                        ))
                    })?;
                    let mut map = ObjectMap::new();
                    for (fname, fdef) in ofields {
                        let fpath = format!("{}.{}", path, fname);
                        match obj
                            .iter()
                            .find(|(k, _)| k.to_lowercase() == fname.to_lowercase())
                            .map(|(_, v)| v)
                        {
                            Some(jv) if !jv.is_null() => {
                                let val = self.json_to_variable_value(jv, fdef, &fpath)?;
                                map.insert(fname.clone(), val);
                            }
                            _ => {
                                map.insert(fname.clone(), self.default_value(&fpath, fdef));
                            }
                        }
                    }
                    Ok(VariableValue::Object(map))
                }
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects an object, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::List => match json {
                J::Array(arr) => {
                    let etype = def.element_type.unwrap_or(VariableType::String);
                    let elem_def = synthesize_elem_def(def, etype);
                    let mut items = Vec::with_capacity(arr.len());
                    for (i, elem) in arr.iter().enumerate() {
                        let ipath = format!("{}[{}]", path, i);
                        let val = self.json_to_variable_value(elem, &elem_def, &ipath)?;
                        items.push(val);
                    }
                    Ok(VariableValue::List(items))
                }
                _ => Err(BuilderError::Validation(format!(
                    "parameter '{}' expects an array, got {}",
                    path,
                    json_type_name(json)
                ))),
            },
            T::ObjectShape => Err(BuilderError::Validation(format!(
                "parameter '{}' is an object_shape and cannot be set via JSON",
                path
            ))),
        }
    }

    /// Convert a JSON value into a `VariableValue` according to an option
    /// type's element type. The element definition is synthesized from the
    /// option's etype and resolved ofields.
    fn json_to_option_element(
        &self,
        json: &serde_json::Value,
        def: &VariableDefinition,
        path: &str,
    ) -> Result<VariableValue, BuilderError> {
        let etype = def.element_type.unwrap_or(VariableType::String);
        let elem_def = synthesize_elem_def(def, etype);
        self.json_to_variable_value(json, &elem_def, path)
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
    fn collect_values(
        &self,
        hctx: &mut Option<HistoryContext>,
        resume: Option<&ValueMap>,
        rebuild: bool,
    ) -> Result<ValueMap, BuilderError> {
        // Top-level `object_shape` variables are pure type definitions (RFC
        // §3.1/§3.4): they declare a reusable shape and are never prompted
        // for on their own — only the site that references them (a
        // `list`/`option_*` element, or an `object` inheriting the shape via
        // `type: <name>`) is collected. They must be skipped
        // here; otherwise the builder would prompt for them as if they were
        // standalone fields.
        let referenced = self.referenced_type_defs();
        let defs: Vec<(String, &VariableDefinition)> = self
            .prompt
            .variable_definitions
            .iter()
            .filter(|(k, _)| !referenced.contains(&k.to_lowercase()))
            .map(|(k, v)| (k.clone(), v))
            .collect();
        let mut values: Vec<VariableValue> = Vec::with_capacity(defs.len());
        let mut idx = 0usize;

        // Pre-fill from resume values so we skip already-collected fields.
        if let Some(resume_map) = resume {
            for (key, _) in &defs {
                if let Some(v) = resume_map.get(key) {
                    values.push(v.clone());
                    idx += 1;
                } else {
                    break;
                }
            }
        }

        // Show already-collected fields before continuing, and position
        // the cursor for the rebuild case.
        if idx > 0 {
            print_collected_summary(&defs, &values, idx);
        }
        if rebuild && idx >= defs.len() {
            // All fields were previously collected — position at the
            // last field so the user can review it before building.
            idx = defs.len() - 1;
        }

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

                    // Update build history and check Ctrl+H between fields.
                    if let Some(h) = hctx.as_mut() {
                        let stored = &values[idx - 1];
                        h.update_field(key, stored, idx);
                        let mut stderr = std::io::stderr();
                        if let Ok(Some(SidebarOutcome::Select(uuid))) =
                            build_history::check_ctrl_h(&mut stderr, &mut h.history, 100)
                        {
                            if uuid != h.record.uuid {
                                return Err(BuilderError::SwitchBuild { uuid });
                            }
                        }
                    }
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
        // Seed defaults for skipped (type-definition) objects so render
        // never reports them missing if the template happens to reference
        // one directly.
        for (key, def) in &self.prompt.variable_definitions {
            if referenced.contains(&key.to_lowercase()) {
                map.insert(key.clone(), self.default_value(key, def));
            }
        }
        for ((key, _), v) in defs.iter().zip(values) {
            map.insert(key.clone(), v);
        }
        Ok(map)
    }

    /// Set of top-level variable names (lowercased) that are **not** collectible
    /// — i.e. must be skipped during interactive collection. Under the RFC
    /// (§3.1/§3.4) every top-level `object_shape` is a pure type definition: it
    /// is never prompted for on its own, only at the site that references it
    /// (a `list`/`option_*` element, or an `object` reusing its shape via
    /// `type: <name>`). A top-level `object`, by contrast, is
    /// always collectible (asked in declaration order) even if something
    /// references it — but the parser rejects by-name references to an
    /// `object`, so only `object_shape` ends up here.
    pub fn referenced_type_defs(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for (name, def) in &self.prompt.variable_definitions {
            if def.r#type == VariableType::ObjectShape {
                out.insert(name.to_lowercase());
            }
        }
        out
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
            Object | ObjectShape => self.collect_object(path, def),
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
        let mut err = std::io::stderr();
        let _ = execute!(
            err,
            SetForegroundColor(Color::AnsiValue(10)),
            Print(&lbl),
            ResetColor
        );
        if let Some(desc) = desc(def) {
            let _ = execute!(
                err,
                SetForegroundColor(Color::AnsiValue(248)),
                Print(format!(" - {}", desc)),
                ResetColor
            );
        }
        let _ = execute!(err, Print("\n"));
        if let Some(d) = default.and_then(|v| match v {
            VariableValue::String(s) | VariableValue::LongString(s) => Some(s.clone()),
            _ => None,
        }) {
            let _ = execute!(
                err,
                SetForegroundColor(Color::AnsiValue(248)),
                Print(format!("(default: leave empty to use \"{}\")\n", d)),
                ResetColor
            );
        }
        let _ = execute!(
            err,
            SetForegroundColor(Color::AnsiValue(248)),
            Print("(enter your text; finish with two consecutive lines containing only '.')\n"),
            Print("(type ':back' as the first line to go back to the previous parameter)\n"),
            ResetColor
        );
        // Flush so the prompt appears before reading.
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
        _default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let etype = def
            .element_type
            .ok_or_else(|| BuilderError::TypeError(format!("list '{}' has no etype", path)))?;

        // Synthesize a definition for the element type.
        let elem_def = VariableDefinition {
            r#type: etype,
            desc: def.desc.clone(),
            options: def.options.clone(),
            element_type: def.element_type,
            element_ref: None,
            label: None,
            type_ref: None,
            ofields_definitions: def.ofields_definitions.clone(),
        };

        let mut items: Vec<VariableValue> = Vec::new();
        let help = format!("add or finish items{BACK_HINT}");
        loop {
            let menu_lbl = format!("{} ({} items added)", path, items.len());
            let mut select = inquire::Select::new(&menu_lbl, vec!["add item".to_string(), "done".to_string()]);
            select = select.with_help_message(&help);
            let choice = select.prompt().map_err(map_inquire_err)?;
            if choice == "done" {
                break;
            }
            // "add item" — collect a new item at index `items.len()`. Pressing
            // Esc (Back) while entering an item moves back to re-edit the
            // previous item (keeping its current value as the default) instead
            // of discarding the list; Back at item 0 returns to this menu.
            let mut idx = items.len();
            loop {
                let ipath = format!("{}[{}]", path, idx);
                let default = items.get(idx).cloned();
                match self.collect_definition(&ipath, &elem_def, default.as_ref()) {
                    Ok(v) => {
                        if idx < items.len() {
                            items[idx] = v;
                        } else {
                            items.push(v);
                        }
                        break;
                    }
                    Err(BuilderError::Back) => {
                        if idx == 0 {
                            break;
                        }
                        idx -= 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(VariableValue::List(items))
    }
}

// ---------------------------------------------------------------------------
// Helpers shared by collection and rendering
// ---------------------------------------------------------------------------

fn label(path: &str, _def: &VariableDefinition) -> String {
    format!("> {}", path)
}

/// Print a summary of already-collected fields on stderr so the user can
/// review previous answers before the current field prompt appears.
/// `up_to` is the number of fields (from the start of `defs`) that have
/// already been collected; their values are taken from `values`.
fn print_collected_summary(
    defs: &[(String, &VariableDefinition)],
    values: &[VariableValue],
    up_to: usize,
) {
    if up_to == 0 {
        return;
    }
    let mut err = std::io::stderr();
    let _ = execute!(
        err,
        SetForegroundColor(Color::DarkGrey),
        Print("Previously collected:\n"),
        ResetColor,
    );
    for ((key, _), val) in defs.iter().zip(values).take(up_to) {
        let val_str = stringify(val).unwrap_or_default();
        let display: String = if val_str.chars().count() > 60 {
            let kept: String = val_str.chars().take(57).collect();
            format!("{kept}...")
        } else {
            val_str
        };
        let _ = execute!(
            err,
            SetForegroundColor(Color::DarkGrey),
            Print(format!("  {key} = ")),
            SetForegroundColor(Color::White),
            Print(&display),
            Print("\n"),
            ResetColor,
        );
    }
    let _ = execute!(err, Print("\n"));
    let _ = err.flush();
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

/// Synthesize a `VariableDefinition` for a list/option element from the
/// parent definition's resolved etype and ofields. This mirrors the element
/// definition synthesized by `collect_list`.
fn synthesize_elem_def(def: &VariableDefinition, etype: VariableType) -> VariableDefinition {
    VariableDefinition {
        r#type: etype,
        desc: None,
        options: def.options.clone(),
        element_type: def.element_type,
        element_ref: None,
        label: None,
        type_ref: None,
        ofields_definitions: def.ofields_definitions.clone(),
    }
}

/// Human-readable name for a JSON value type, used in validation error messages.
fn json_type_name(json: &serde_json::Value) -> &'static str {
    match json {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
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

#[cfg(test)]
mod tests {
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
}
