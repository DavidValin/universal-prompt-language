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

    /// Look up a declared `def` for `path`, falling back from a specific
    /// list/option element instance path (e.g. `servers[0].port`) to the
    /// canonical per-field default `etype: <object_shape>` copies onto the
    /// list/option's own path (`servers.port`) during element-ref
    /// resolution (RFC §3, E5) — every element shares the same field
    /// defaults, so there's no reason to key them per-instance.
    fn lookup_default(&self, path: &str) -> Option<VariableValue> {
        self.prompt
            .variable_defaults
            .get(path)
            .or_else(|| {
                strip_index_segment(path).and_then(|canonical| self.prompt.variable_defaults.get(&canonical))
            })
            .cloned()
    }

    fn default_value(&self, path: &str, def: &VariableDefinition) -> VariableValue {
        use VariableType::*;
        match def.r#type {
            String | LongString => self
                .lookup_default(path)
                .unwrap_or(VariableValue::String(::std::string::String::new())),
            Number => self
                .lookup_default(path)
                .unwrap_or(VariableValue::Number(0.0)),
            Boolean => self
                .lookup_default(path)
                .unwrap_or(VariableValue::Boolean(false)),
            OptionSingle => {
                let etype = def.element_type.unwrap_or(VariableType::String);
                if let Some(v) = self.lookup_default(path) {
                    return self.fill_element_defaults(path, def, &v);
                }
                if let Some(opts) = &def.options {
                    if let Some(first) = opts.first() {
                        return first.clone();
                    }
                }
                option_type_zero(etype)
            }
            OptionMulti => {
                if let Some(VariableValue::List(items)) = self.lookup_default(path) {
                    return VariableValue::List(
                        items.iter().map(|e| self.fill_element_defaults(path, def, e)).collect(),
                    );
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
                // An object-level `def` literal (RFC §3) overrides the
                // shape's field-level defaults on a per-key basis: a key the
                // literal declares wins; any key it doesn't mention falls
                // back to the shape's own default for that field. Nested
                // object fields merge recursively for the same reason.
                if let Some(VariableValue::Object(overrides)) = self.lookup_default(path) {
                    map = merge_object_default(map, &overrides);
                }
                VariableValue::Object(map)
            }
            List => {
                // Honor an inline `def:` list, filling any field an object
                // element omits from the element shape's own defaults (RFC
                // §3.4 / §3.3); otherwise default to an empty list (RFC §3 —
                // `def` is optional, list falls back to `[]`).
                if let Some(VariableValue::List(items)) = self.lookup_default(path) {
                    return VariableValue::List(
                        items.iter().map(|e| self.fill_element_defaults(path, def, e)).collect(),
                    );
                }
                VariableValue::List(vec![])
            }
        }
    }

    /// Complete a list/option element taken from a `def:` literal. For an
    /// object-shaped element, every field the literal doesn't mention falls
    /// back to the element shape's field default (RFC §3.4: "any field the
    /// element's value doesn't mention falls back to the shape's own field
    /// default"), merged recursively like an object-level `def`. Scalar
    /// elements are returned unchanged.
    fn fill_element_defaults(
        &self,
        path: &str,
        def: &VariableDefinition,
        elem: &VariableValue,
    ) -> VariableValue {
        let is_object_etype =
            def.element_type == Some(VariableType::Object) || def.element_ref.is_some();
        match (is_object_etype, elem, &def.ofields_definitions) {
            (true, VariableValue::Object(overrides), Some(ofields)) => {
                let mut base = ObjectMap::new();
                for (k, nd) in ofields {
                    let npath = format!("{}.{}", path, k);
                    base.insert(k.clone(), self.default_value(&npath, nd));
                }
                VariableValue::Object(merge_object_default(base, overrides))
            }
            _ => elem.clone(),
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

        // Validate all JSON keys correspond to declared parameters.
        for (key, _) in obj {
            let found = self
                .prompt
                .variable_definitions
                .iter()
                .any(|(k, _)| k.to_lowercase() == key.to_lowercase());
            if !found {
                return Err(BuilderError::Validation(format!(
                    "unknown parameter '{}' in JSON (not declared in prompt)",
                    key
                )));
            }
        }

        // Start with defaults for all declared variables (including
        // object_shape type definitions, which are seeded but never expected
        // in the JSON).
        let mut values = self.defaults_to_values();

        // Process in declaration order so condition expressions can reference
        // already-processed parameters (RFC §3.7 / §9 step 5a: a condition may
        // only reference parameters declared before the one carrying it).
        for (declared_name, def) in &self.prompt.variable_definitions {
            // object_shape types are not settable.
            if referenced.contains(&declared_name.to_lowercase()) {
                if let Some((_, jv)) = obj
                    .iter()
                    .find(|(k, _)| k.to_lowercase() == declared_name.to_lowercase())
                {
                    if !jv.is_null() {
                        return Err(BuilderError::Validation(format!(
                            "parameter '{}' is an object_shape type definition and cannot be set via JSON",
                            declared_name
                        )));
                    }
                }
                continue;
            }

            // Evaluate build-time condition (RFC §3.7).
            let is_hidden = if let Some(cond) = &def.exclude_condition {
                is_condition_hidden(cond, &values)?
            } else {
                false
            };

            // Find the matching JSON value (case-insensitive).
            let json_val = obj
                .iter()
                .find(|(k, _)| k.to_lowercase() == declared_name.to_lowercase())
                .map(|(_, v)| v);

            if is_hidden {
                // Parameter is hidden by its condition — reject if present
                // and non-null in the JSON.
                if let Some(jv) = json_val {
                    if !jv.is_null() {
                        return Err(BuilderError::Validation(format!(
                            "parameter '{}' is hidden by its condition and cannot be set via JSON",
                            declared_name
                        )));
                    }
                }
                // Keep the default value already in `values`.
                continue;
            }

            // Not hidden — accept JSON value if present and non-null.
            if let Some(jv) = json_val {
                // null means "use default" — skip overriding.
                if jv.is_null() {
                    continue;
                }
                let val = self.json_to_variable_value(jv, def, declared_name)?;
                values.insert(declared_name.clone(), val);
            }
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
            // A nested `object_shape` field is collected exactly like a
            // nested `object` (RFC §3.1); only a *top-level* object_shape is
            // a pure type definition, and those never reach this function
            // (`values_from_json` filters them out first).
            T::Object | T::ObjectShape => match json {
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

            // Evaluate build-time condition (RFC §3.7): a truthy condition
            // hides (excludes) the parameter from the build — skip it and
            // use its default value. The condition can only reference
            // parameters declared before this one, so their values are
            // already collected and available in `values`.
            if let Some(cond) = &def.exclude_condition {
                let mut current = ValueMap::new();
                for ((k, _), v) in defs.iter().zip(&values) {
                    current.insert(k.clone(), v.clone());
                }
                if is_condition_hidden(cond, &current)? {
                    let default = self.default_value(key, def);
                    if idx < values.len() {
                        values[idx] = default;
                    } else {
                        values.push(default);
                    }
                    idx += 1;
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
                    continue;
                }
            }

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
            Object | ObjectShape => self.collect_object(path, def, default),
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
        // Start the cursor on the default so Enter picks it (the help text
        // already advertised it, but the cursor sat on the first option).
        if let Some(i) = def_idx {
            select = select.with_starting_cursor(i);
        }
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

    /// Collect an object's fields in declaration order. Each field's default
    /// is, in priority order: the field's value in `default` (a previously
    /// collected/resumed object, so going back keeps the user's answers),
    /// then the declared `def:` for the field — looked up via
    /// `lookup_default` so list-element paths (`servers[0].host`) fall back
    /// to the shape's canonical field defaults (`servers.host`).
    fn collect_object(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
    ) -> Result<VariableValue, BuilderError> {
        let nested = def
            .ofields_definitions
            .as_ref()
            .ok_or_else(|| BuilderError::TypeError(format!("object '{}' has no ofields block", path)))?;
        let prev = match default {
            Some(VariableValue::Object(m)) => Some(m),
            _ => None,
        };
        let mut map = ObjectMap::new();
        for (k, nd) in nested {
            let npath = format!("{}.{}", path, k);
            let kl = k.to_lowercase();
            let ndefault = prev
                .and_then(|m| m.iter().find(|(pk, _)| pk.to_lowercase() == kl).map(|(_, v)| v.clone()))
                .or_else(|| self.lookup_default(&npath));
            let v = self.collect_definition(&npath, nd, ndefault.as_ref())?;
            map.insert(k.clone(), v);
        }
        Ok(VariableValue::Object(map))
    }

    /// Collect a list interactively. The list starts from `default` — the
    /// declared `def:` items, or the previously collected/resumed value when
    /// coming back to this field — so existing items can be kept, edited or
    /// removed rather than always starting from an empty list.
    fn collect_list(
        &self,
        path: &str,
        def: &VariableDefinition,
        default: Option<&VariableValue>,
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
            exclude_condition: None,
        };

        let mut items: Vec<VariableValue> = match default {
            Some(VariableValue::List(l)) => l.clone(),
            _ => Vec::new(),
        };
        let help = format!("add, edit, remove or finish items{BACK_HINT}");
        loop {
            let menu_lbl = format!("{} ({} items)", path, items.len());
            let mut choices = vec!["add item".to_string()];
            if !items.is_empty() {
                choices.push("edit item".to_string());
                choices.push("remove item".to_string());
            }
            choices.push("done".to_string());
            let mut select = inquire::Select::new(&menu_lbl, choices);
            select = select.with_help_message(&help);
            let choice = match select.prompt() {
                Ok(c) => c,
                // Esc on the list menu goes back to the previous parameter.
                Err(e) => return Err(map_inquire_err(e)),
            };
            if choice == "done" {
                break;
            }
            if choice == "edit item" || choice == "remove item" {
                let labels: Vec<String> = items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("{}: {}", i + 1, item_summary(v)))
                    .collect();
                let pick = inquire::Select::new(&format!("{} > {}", path, choice), labels.clone())
                    .with_help_message("Esc: back to the list menu")
                    .prompt();
                let idx = match pick {
                    Ok(l) => labels.iter().position(|x| *x == l).unwrap_or(0),
                    Err(inquire::InquireError::OperationCanceled) => continue,
                    Err(e) => return Err(map_inquire_err(e)),
                };
                if choice == "remove item" {
                    items.remove(idx);
                    continue;
                }
                let ipath = format!("{}[{}]", path, idx);
                match self.collect_definition(&ipath, &elem_def, Some(&items[idx])) {
                    Ok(v) => items[idx] = v,
                    Err(BuilderError::Back) => {}
                    Err(e) => return Err(e),
                }
                continue;
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

/// One-line summary of a list item for the edit/remove pickers.
fn item_summary(v: &VariableValue) -> String {
    let s = stringify(v).unwrap_or_default().replace('\n', " ");
    if s.chars().count() > 60 {
        format!("{}...", s.chars().take(57).collect::<String>())
    } else {
        s
    }
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
        exclude_condition: None,
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

/// Strip a `[N]` list/option-element index from `path`'s first segment, if
/// present, so a specific element instance path (e.g. `servers[0].port` or
/// `servers[0]`) maps back to the list/option's own canonical path
/// (`servers.port` / `servers`) — where `etype: <object_shape>` field
/// defaults are copied during element-ref resolution (RFC §3, E5). The
/// index only ever appears in the leftmost segment, since only a list/
/// option_multi's own elements are indexed. Returns `None` if there's no
/// index to strip (nothing to fall back to beyond the exact path already
/// tried).
fn strip_index_segment(path: &str) -> Option<String> {
    let open = path.find('[')?;
    let close = path[open..].find(']')? + open;
    let mut out = String::with_capacity(path.len() - (close - open + 1));
    out.push_str(&path[..open]);
    out.push_str(&path[close + 1..]);
    Some(out)
}

/// Merge an object-level `def` literal's declared fields over a base map of
/// shape-derived field defaults (RFC §3, E4): a key `overrides` declares
/// wins; a key it doesn't mention keeps its value from `base`. When both
/// sides have an object at the same key, they're merged recursively rather
/// than the override replacing the whole nested object, so a partial nested
/// override still inherits the rest of that nested shape's field defaults.
/// `base`'s key order (the shape's declaration order, §4.6.1/§7.3) is
/// preserved — `IndexMap::insert` on an existing key updates its value
/// without moving it.
fn merge_object_default(mut base: ObjectMap, overrides: &ObjectMap) -> ObjectMap {
    for (k, v) in overrides {
        match (base.get(k), v) {
            (Some(VariableValue::Object(base_obj)), VariableValue::Object(override_obj)) => {
                let merged = merge_object_default(base_obj.clone(), override_obj);
                base.insert(k.clone(), VariableValue::Object(merged));
            }
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
    base
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
    // Integer-valued numbers render without a fractional part (RFC §4.6.1).
    // Only cast to i64 while the value is exactly representable there;
    // beyond that the cast saturates (1e20 used to print as i64::MAX), so
    // fall back to Rust's own formatting, which prints the full integer.
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
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

/// Evaluate a build-time `condition` expression (RFC §3.7) against the
/// supplied top-level values. Returns `true` when the condition is truthy,
/// meaning the parameter carrying the condition is **hidden** (excluded from
/// the build). Returns `false` when the parameter should be shown (asked).
fn is_condition_hidden(cond: &CondExpr, values: &ValueMap) -> Result<bool, BuilderError> {
    let scope: Vec<Frame> = vec![values.clone()];
    let result = eval(cond, &scope)?;
    Ok(truthy(&result))
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
        // `and`/`or` (RFC §5.1) operate on truthiness rather than typed
        // operand comparison, and are short-circuiting: the right operand is
        // only evaluated (and looked up) when it can affect the result, so a
        // right-hand reference that would otherwise fail to resolve is safe
        // to write behind a left operand that already determines the value.
        CondExpr::Bin { op, left, right } if op == "and" => {
            let l = eval(left, scope)?;
            if !truthy(&l) {
                return Ok(VariableValue::Boolean(false));
            }
            let r = eval(right, scope)?;
            Ok(VariableValue::Boolean(truthy(&r)))
        }
        CondExpr::Bin { op, left, right } if op == "or" => {
            let l = eval(left, scope)?;
            if truthy(&l) {
                return Ok(VariableValue::Boolean(true));
            }
            let r = eval(right, scope)?;
            Ok(VariableValue::Boolean(truthy(&r)))
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
            // `contains` is overloaded: list membership when the LEFT
            // operand is a list (§5) — `TAGS contains "api"`, never
            // `"api" contains TAGS`. A list on the right with a non-list
            // left operand falls through to `as_strings` below, which
            // reports a clear type error rather than a confusing
            // "membership of List in list" message.
            if op == "contains" {
                if let List(items) = l {
                    // The right operand is a single element — string,
                    // number, boolean, or object, compared by value — never
                    // itself a list (§5).
                    if let List(_) = r {
                        return Err(BuilderError::TypeError(
                            "operator 'contains': right operand must be a single element (string, number, boolean, or object), not a list".into(),
                        ));
                    }
                    let found = items.iter().any(|item| values_equal(item, r));
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
#[path = "../../test/unit/upl/builder.rs"]
mod tests;

