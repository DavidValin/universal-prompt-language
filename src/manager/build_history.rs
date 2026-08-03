// build_history.rs
//
// Build history for UPL prompt builds. Persists a list of `BuildRecord`
// entries to `~/.upl/build_history.json` so that in-progress builds can be
// resumed and completed builds can be rebuilt.
//
// Each record stores the values collected so far, the prompt identity (sha256
// of the file contents), the file path (so the prompt can be reloaded), and a
// status (InProgress / Built). The builder updates the record after every
// field is collected, making builds resumable from the exact point of
// interruption.
//
// `run_sidebar` renders a crossterm-based overlay listing all records. The
// user navigates with Up/Down and presses Enter to resume (InProgress) or
// rebuild (Built) the selected record.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
    terminal,
};
use serde::{Deserialize, Serialize};

use crate::upl::builder::ValueMap;
use crate::upl::parser::VariableValue;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    InProgress,
    Built,
}

impl BuildStatus {
    fn label(&self) -> &'static str {
        match self {
            BuildStatus::InProgress => "in progress",
            BuildStatus::Built => "built",
        }
    }

    fn color(&self) -> Color {
        match self {
            BuildStatus::InProgress => Color::Yellow,
            BuildStatus::Built => Color::DarkGreen,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BuildRecord {
    pub uuid: String,
    pub date: u64,
    pub prompt_sha256: String,
    pub prompt_name: String,
    pub prompt_path: String,
    pub status: BuildStatus,
    pub values: ValueMap,
    pub total_fields: usize,
    pub collected_fields: usize,
}

impl BuildRecord {
    pub fn new(
        prompt_sha256: &str,
        prompt_name: &str,
        prompt_path: &str,
        total_fields: usize,
    ) -> Self {
        Self {
            uuid: generate_uuid(),
            date: now_secs(),
            prompt_sha256: prompt_sha256.to_string(),
            prompt_name: prompt_name.to_string(),
            prompt_path: prompt_path.to_string(),
            status: BuildStatus::InProgress,
            values: ValueMap::new(),
            total_fields,
            collected_fields: 0,
        }
    }

    pub fn is_resumable(&self) -> bool {
        self.status == BuildStatus::InProgress && self.collected_fields < self.total_fields
    }

    /// Export the record's collected values as pretty JSON to
    /// `~/.upl/build_exports/<prompt_sha256>_<date>.json`, where `<date>` is
    /// the current time formatted as `YYYYMMDD_HHMM`. The exported file
    /// contains only the parameter values and is directly reusable with
    /// `upl build-from-json`. Returns the full path of the written file.
    pub fn export_to_json(&self) -> io::Result<PathBuf> {
        let dir = build_exports_dir()?;
        fs::create_dir_all(&dir)?;

        let date = format_date_for_filename(now_secs());
        let filename = format!("{}_{}.json", self.prompt_sha256, date);
        let path = dir.join(filename);

        let json = serde_json::to_string_pretty(&self.values)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&path, json)?;

        Ok(path)
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct BuildHistory {
    #[serde(default)]
    records: Vec<BuildRecord>,
}

impl BuildHistory {
    pub fn load() -> io::Result<Self> {
        let path = history_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self) -> io::Result<()> {
        let path = history_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&path, bytes)?;
        Ok(())
    }

    pub fn records(&self) -> &[BuildRecord] {
        &self.records
    }

    pub fn records_mut(&mut self) -> &mut Vec<BuildRecord> {
        &mut self.records
    }

    /// Upsert a record by uuid. If a record with the same uuid exists, it is
    /// replaced; otherwise the record is appended.
    pub fn upsert(&mut self, record: BuildRecord) {
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|r| r.uuid == record.uuid)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
    }

    pub fn get(&self, uuid: &str) -> Option<&BuildRecord> {
        self.records.iter().find(|r| r.uuid == uuid)
    }

    pub fn remove(&mut self, uuid: &str) {
        self.records.retain(|r| r.uuid != uuid);
    }

    /// Remove old completed records, keeping at most `max` records total.
    pub fn prune(&mut self, max: usize) {
        if self.records.len() <= max {
            return;
        }
        // Keep all in-progress records; prune oldest built ones first.
        self.records.sort_by_key(|r| (r.status != BuildStatus::InProgress, r.date));
        self.records.truncate(max);
    }
}

// ---------------------------------------------------------------------------
// HistoryContext — used by the builder during tracked collection
// ---------------------------------------------------------------------------

/// Bundles a `BuildRecord` with the loaded `BuildHistory` store so the builder
/// can update the record after each field and persist it.
pub struct HistoryContext {
    pub record: BuildRecord,
    pub history: BuildHistory,
}

impl HistoryContext {
    pub fn new(
        prompt_sha256: &str,
        prompt_name: &str,
        prompt_path: &str,
        total_fields: usize,
    ) -> Self {
        let record = BuildRecord::new(prompt_sha256, prompt_name, prompt_path, total_fields);
        let history = BuildHistory::load().unwrap_or_default();
        let mut ctx = Self { record, history };
        ctx.persist();
        ctx
    }

    pub fn from_record(record: &BuildRecord) -> Self {
        let history = BuildHistory::load().unwrap_or_default();
        Self { record: record.clone(), history }
    }

    /// Update the record after a field is collected and persist.
    pub fn update_field(&mut self, key: &str, value: &VariableValue, collected: usize) {
        self.record.values.insert(key.to_string(), value.clone());
        self.record.collected_fields = collected;
        self.record.date = now_secs();
        self.persist();
    }

    /// Mark the current build as completed and persist.
    pub fn mark_built(&mut self) {
        self.record.status = BuildStatus::Built;
        self.record.collected_fields = self.record.total_fields;
        self.persist();
    }

    fn persist(&mut self) {
        self.history.upsert(self.record.clone());
        let _ = self.history.save();
    }
}

// ---------------------------------------------------------------------------
// Paths & helpers
// ---------------------------------------------------------------------------

fn history_path() -> io::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| {
        io::Error::new(io::ErrorKind::NotFound, "HOME not set")
    })?;
    Ok(PathBuf::from(home).join(".upl").join("build_history.json"))
}

/// Directory where exported build records are written.
fn build_exports_dir() -> io::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| {
        io::Error::new(io::ErrorKind::NotFound, "HOME not set")
    })?;
    Ok(PathBuf::from(home).join(".upl").join("build_exports"))
}

/// Generate a UUID v4-style string (random hex, formatted as
/// 8-4-4-4-12).
fn generate_uuid() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    // Set version 4 and variant bits (RFC 4122).
    buf[6] = (buf[6] & 0x0f) | 0x40;
    buf[8] = (buf[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        buf[0], buf[1], buf[2], buf[3],
        buf[4], buf[5],
        buf[6], buf[7],
        buf[8], buf[9],
        buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Current Unix timestamp in seconds (public for builder use).
pub fn now() -> u64 {
    now_secs()
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM` using a minimal civil-calendar
/// conversion (no external date crate needed).
fn format_date(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;

    // Convert days-since-epoch to (year, month, day) using the algorithm from
    // Howard Hinnant's date library (civil_from_days).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, m, d, hour, minute
    )
}

/// Format a Unix timestamp as `YYYYMMDD_HHMM` — filename-safe (no spaces or
/// colons), suitable for use in export filenames.
fn format_date_for_filename(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;

    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{:04}{:02}{:02}_{:02}{:02}", year, m, d, hour, minute)
}

/// Truncate or pad `s` to exactly `w` character cells.
fn truncate_pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n > w {
        if w <= 3 {
            s.chars().take(w).collect()
        } else {
            let kept: String = s.chars().take(w - 3).collect();
            format!("{kept}...")
        }
    } else {
        format!("{}{}", s, " ".repeat(w - n))
    }
}

// ---------------------------------------------------------------------------
// Sidebar TUI
// ---------------------------------------------------------------------------

/// What the user chose in the sidebar.
#[derive(Clone, Debug)]
pub enum SidebarOutcome {
    /// Close the sidebar without doing anything.
    Close,
    /// Resume or rebuild the record with this uuid.
    Select(String),
}

/// Run the build-history sidebar as a full-screen crossterm TUI.
///
/// The caller must have already entered the alternate screen **and** enabled
/// raw mode. This function shows the cursor, renders the list of build
/// records, and restores the cursor on exit. The user navigates with Up/Down
/// and presses Enter to select a record (resume if in-progress, rebuild if
/// built), or Esc/q to close. Ctrl+D deletes the selected record.
/// Ctrl+E exports the selected record's values to a JSON file.
pub fn run_sidebar<W: Write>(
    stdout: &mut W,
    history: &mut BuildHistory,
) -> io::Result<SidebarOutcome> {
    let _ = execute!(stdout, cursor::Show);

    let mut selected: usize = 0;
    let mut top: usize = 0;
    let (mut cols, mut lines) = terminal::size()?;
    let mut status_msg: Option<String> = None;

    let result = (|| -> io::Result<SidebarOutcome> {
        loop {
            render_sidebar(stdout, history, &mut selected, &mut top, cols, lines, &status_msg)?;
            stdout.flush()?;
            status_msg = None;

            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    match key.code {
                        KeyCode::Up => selected = selected.saturating_sub(1),
                        KeyCode::Down => {
                            if selected + 1 < history.records().len() {
                                selected += 1;
                            }
                        }
                        KeyCode::Home => selected = 0,
                        KeyCode::End => {
                            selected = history.records().len().saturating_sub(1);
                        }
                        KeyCode::PageUp => {
                            let h = list_height(lines);
                            selected = selected.saturating_sub(h.max(1));
                        }
                        KeyCode::PageDown => {
                            let h = list_height(lines);
                            selected = (selected + h.max(1))
                                .min(history.records().len().saturating_sub(1).max(selected));
                        }
                        KeyCode::Enter => {
                            if let Some(record) = history.records().get(selected) {
                                return Ok(SidebarOutcome::Select(record.uuid.clone()));
                            }
                        }
                        KeyCode::Char('d') if ctrl && !history.records().is_empty() => {
                            let uuid = history.records().get(selected).map(|r| r.uuid.clone());
                            if let Some(uuid) = uuid {
                                history.remove(&uuid);
                                history.save()?;
                                selected = selected.saturating_sub(1);
                            }
                        }
                        KeyCode::Char('e') if ctrl && !history.records().is_empty() => {
                            if let Some(record) = history.records().get(selected) {
                                match record.export_to_json() {
                                    Ok(path) => {
                                        status_msg = Some(format!(
                                            "Exported to {}",
                                            path.display()
                                        ));
                                    }
                                    Err(e) => {
                                        status_msg =
                                            Some(format!("Export failed: {}", e));
                                    }
                                }
                            }
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                            return Ok(SidebarOutcome::Close);
                        }
                        _ => {}
                    }
                }
                Event::Resize(c, l) => {
                    cols = c;
                    lines = l;
                }
                _ => {}
            }
        }
    })();

    let _ = execute!(stdout, cursor::Hide);
    result
}

fn list_height(lines: u16) -> usize {
    // Header (1) + separator (1) + column header (1) + column separator (1) + footer (1) = 5 rows of chrome.
    (lines as usize).saturating_sub(5).max(1)
}

fn render_sidebar<W: Write>(
    stdout: &mut W,
    history: &BuildHistory,
    selected: &mut usize,
    top: &mut usize,
    cols: u16,
    lines: u16,
    status_msg: &Option<String>,
) -> io::Result<()> {
    let records = history.records();
    let lh = list_height(lines);

    // Clamp selection.
    if records.is_empty() {
        *selected = 0;
        *top = 0;
    } else {
        if *selected >= records.len() {
            *selected = records.len() - 1;
        }
        if *selected < *top {
            *top = *selected;
        }
        if *selected >= *top + lh && lh > 0 {
            *top = *selected + 1 - lh;
        }
        if *top > 0 && *top + lh > records.len() {
            *top = records.len().saturating_sub(lh);
        }
    }

    queue!(
        stdout,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print(" Build History"),
        ResetColor,
        SetAttribute(Attribute::Reset),
    )?;

    // Separator.
    let sep_width = cols as usize;
    queue!(
        stdout,
        cursor::MoveTo(0, 1),
        SetForegroundColor(Color::DarkGrey),
        Print(&"─".repeat(sep_width)),
        ResetColor,
    )?;

    // Column layout: status(12) | date(17) | prompt(24) | progress(12)
    let w_status = 12usize;
    let w_date = 17usize;
    let w_progress = 14usize;
    let w_prompt = sep_width.saturating_sub(w_status + w_date + w_progress + 6).max(10);

    // Header row.
    let header = format!(
        " {}  {}  {}  {}",
        truncate_pad("STATUS", w_status),
        truncate_pad("DATE", w_date),
        truncate_pad("PROMPT", w_prompt),
        truncate_pad("PROGRESS", w_progress),
    );
    queue!(
        stdout,
        cursor::MoveTo(0, 2),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print(truncate_pad(&header, sep_width)),
        ResetColor,
        SetAttribute(Attribute::Reset),
    )?;

    // Separator below the column header.
    let col_sep = format!(
        " {}  {}  {}  {}",
        truncate_pad(&"-".repeat(w_status), w_status),
        truncate_pad(&"-".repeat(w_date), w_date),
        truncate_pad(&"-".repeat(w_prompt), w_prompt),
        truncate_pad(&"-".repeat(w_progress), w_progress),
    );
    queue!(
        stdout,
        cursor::MoveTo(0, 3),
        SetForegroundColor(Color::DarkGrey),
        Print(truncate_pad(&col_sep, sep_width)),
        ResetColor,
    )?;

    // Records.
    if records.is_empty() {
        queue!(
            stdout,
            cursor::MoveTo(0, 4),
            SetForegroundColor(Color::DarkGrey),
            Print("(no build history yet)"),
            ResetColor,
        )?;
    } else {
        for (i, record) in records
            .iter()
            .enumerate()
            .skip(*top)
            .take(lh)
        {
            let y = 4 + (i - *top) as u16;
            let status_str = record.status.label();
            let date_str = format_date(record.date);
            let progress_str = if record.total_fields > 0 {
                format!("{}/{} fields", record.collected_fields, record.total_fields)
            } else {
                "—".to_string()
            };
            let prompt_str = if record.is_resumable() {
                format!("{} ⟳", record.prompt_name)
            } else {
                record.prompt_name.clone()
            };

            let line = format!(
                " {}  {}  {}  {}",
                truncate_pad(status_str, w_status),
                truncate_pad(&date_str, w_date),
                truncate_pad(&prompt_str, w_prompt),
                truncate_pad(&progress_str, w_progress),
            );

            queue!(
                stdout,
                cursor::MoveTo(0, y),
                terminal::Clear(terminal::ClearType::CurrentLine),
            )?;

            if i == *selected {
                queue!(
                    stdout,
                    SetBackgroundColor(Color::White),
                    SetForegroundColor(Color::Black),
                    SetAttribute(Attribute::Bold),
                    Print(truncate_pad(&line, sep_width)),
                    ResetColor,
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                // Status with color, rest in white/grey.
                queue!(
                    stdout,
                    SetForegroundColor(record.status.color()),
                    Print(" "),
                    Print(truncate_pad(status_str, w_status)),
                    ResetColor,
                    SetForegroundColor(Color::White),
                    Print("  "),
                    Print(truncate_pad(&date_str, w_date)),
                    Print("  "),
                    Print(truncate_pad(&prompt_str, w_prompt)),
                    Print("  "),
                    SetForegroundColor(Color::DarkGrey),
                    Print(truncate_pad(&progress_str, w_progress)),
                    ResetColor,
                )?;
            }
        }
    }

    // Footer.
    let footer_y = lines.saturating_sub(1);
    let footer = "↑/↓ navigate · Enter resume/rebuild · Ctrl+E export · Ctrl+D delete · Esc/q close";
    queue!(
        stdout,
        cursor::MoveTo(0, footer_y),
        terminal::Clear(terminal::ClearType::CurrentLine),
        SetForegroundColor(if status_msg.is_some() {
            Color::Green
        } else {
            Color::DarkGrey
        }),
        Print(truncate_pad(
            status_msg.as_deref().unwrap_or(footer),
            cols as usize,
        )),
        ResetColor,
        cursor::MoveTo(0, footer_y),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Ctrl+H check (called between fields during collection)
// ---------------------------------------------------------------------------

/// Briefly enter raw mode and poll for Ctrl+H. If pressed, run the sidebar.
/// Returns `Some(SidebarOutcome)` if the sidebar was shown, `None` if no
/// key was pressed within the timeout or a non-Ctrl+H key was pressed.
///
/// The timeout controls how long we wait for the user to press Ctrl+H after
/// a field completes. 100 ms is short enough to be unnoticeable during normal
/// builds but gives the user a window to trigger the history sidebar.
pub fn check_ctrl_h<W: Write>(
    stdout: &mut W,
    history: &mut BuildHistory,
    timeout_ms: u64,
) -> io::Result<Option<SidebarOutcome>> {
    terminal::enable_raw_mode()?;

    let outcome = if event::poll(std::time::Duration::from_millis(timeout_ms))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press
                && key.code == KeyCode::Char('h')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                let result = run_sidebar(stdout, history)?;
                Some(result)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    terminal::disable_raw_mode()?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upl::parser::VariableValue;

    #[test]
    fn uuid_is_well_formed() {
        let uuid = generate_uuid();
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.chars().nth(8), Some('-'));
        assert_eq!(uuid.chars().nth(13), Some('-'));
        assert_eq!(uuid.chars().nth(18), Some('-'));
        assert_eq!(uuid.chars().nth(23), Some('-'));
        // Version 4.
        assert_eq!(uuid.chars().nth(14), Some('4'));
    }

    #[test]
    fn record_roundtrip() {
        let mut rec = BuildRecord::new("abc123", "my_prompt", "/tmp/p.txt", 5);
        rec.values
            .insert("name".to_string(), VariableValue::String("test".to_string()));
        rec.collected_fields = 1;
        rec.status = BuildStatus::Built;

        let json = serde_json::to_string(&rec).unwrap();
        let rec2: BuildRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec2.uuid, rec.uuid);
        assert_eq!(rec2.prompt_name, "my_prompt");
        assert_eq!(rec2.status, BuildStatus::Built);
        assert_eq!(rec2.collected_fields, 1);
    }

    #[test]
    fn history_upsert() {
        let mut h = BuildHistory::default();
        let rec = BuildRecord::new("a", "p", "/p", 3);
        h.upsert(rec.clone());
        assert_eq!(h.records().len(), 1);

        // Upsert with same uuid replaces.
        let mut rec2 = rec.clone();
        rec2.collected_fields = 2;
        h.upsert(rec2);
        assert_eq!(h.records().len(), 1);
        assert_eq!(h.records()[0].collected_fields, 2);
    }

    #[test]
    fn format_date_basic() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let s = format_date(1704067200);
        assert!(s.starts_with("2024-01-01"));
    }

    #[test]
    fn format_date_for_filename_basic() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let s = format_date_for_filename(1704067200);
        assert!(s.starts_with("20240101_0000"));
        // No spaces or colons.
        assert!(!s.contains(' ') && !s.contains(':'));
    }

    #[test]
    fn export_to_json_writes_file() {
        // Use a fake HOME so the test never touches the real one.
        let tmp = std::env::temp_dir().join("upl_export_test");
        std::fs::create_dir_all(&tmp).unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);

        let mut rec = BuildRecord::new("deadbeef", "my_prompt", "/tmp/p.txt", 2);
        rec.values
            .insert("name".to_string(), VariableValue::String("hello".to_string()));
        rec.values
            .insert("count".to_string(), VariableValue::Number(42.0));
        rec.collected_fields = 2;
        rec.status = BuildStatus::Built;

        let path = rec.export_to_json().unwrap();

        // Filename should contain the sha256 and end with .json.
        let fname = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(fname.starts_with("deadbeef_"));
        assert!(fname.ends_with(".json"));

        // File content should be valid JSON with the two values.
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["name"], "hello");
        assert_eq!(parsed["count"], 42.0);

        // Cleanup.
        let _ = std::fs::remove_dir_all(&tmp);
        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }
    }
}
