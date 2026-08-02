// ui_prompt_editor.rs
//
// Full-screen raw-mode text editor for a single UPL prompt file.
//
// Layout:
//   ┌───────────────────────────────────────┬───────────────┐
//   │ editable content (scrollable)          │  VALID /      │
//   │  - placeholders: yellow bg / black fg │  INVALID      │
//   │  - for blocks:   dark grey bg         │  var1         │
//   │  - if blocks:    light grey bg        │  var2         │
//   │                                       │  ...          │
//   │  (when INVALID, the sidebar shows the │  (when        │
//   │   wrapped parse errors instead of the │   INVALID,    │
//   │   variable list)                      │   errors)     │
//   ├───────────────────────────────────────┴───────────────┘
//   │ Ctrl+S save · Ctrl+R UPL Help · Esc quit                │
//   └───────────────────────────────────────────────────────┘
//
// Keys:
//   arrows / Home / End / PageUp / PageDown  navigate
//   type                                     insert
//   Enter                                    new line
//   Backspace / Delete                       erase
//   Tab                                      insert 2 spaces
//   Ctrl+S                                   save (only when VALID)
//   Ctrl+R                                   open the UPL Help popup (RFC reference)
//   Esc / Ctrl+C                             quit back to the list
//
// Saves are always written to ~/.upl/prompts/<name>.txt, where <name> is the
// parsed `name` field of the prompt. Saving is blocked while the content is
// INVALID (unparsable) or has no valid `name`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{
        self, Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use thiserror::Error;

use crate::upl::parser::{PromptParser, VariableType};

/// The skeleton a brand-new prompt starts from. It is a valid, minimal UPL
/// prompt so the editor opens in the VALID state and the user can save it
/// straight away with Ctrl+S (writing it to ~/.upl/prompts/<name>.txt).
pub const SKELETON: &str = "\
--
name: new_prompt
title: New Prompt
desc: A new prompt. Edit the name, add params, and write your body below.
params:
  input:
    type: string
    desc: The main input
    def: \"\"
--
[[[INPUT]]]
";

#[derive(Error, Debug)]
pub enum EditorError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("TUI: {0}")]
    Tui(String),
    #[error("save: {0}")]
    Save(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    For,
    If,
}

/// Each variable occupies three sidebar rows: the name (yellow), its type
/// (white, no background), and a blank separator row.
const ROWS_PER_VAR: usize = 3;

pub struct Editor {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize, // char index within the current line
    top: usize,  // vertical scroll offset (line index)
    left: usize, // horizontal scroll offset (char index)
    cols: u16,
    rows: u16, // terminal height
    valid: bool,
    var_names: Vec<(String, VariableType)>,
    name: String,
    /// Parse error messages from the last `recompute`. Empty when valid.
    /// The parser currently surfaces one error per parse, but the editor
    /// renders every entry (one per line) so collecting multiple later is
    /// a drop-in change.
    errors: Vec<String>,
    saved: bool,
    message: String,
}

/// Open the file at `path` in the editor. Returns `true` if the user saved
/// the prompt (so the caller can reload the prompt list), `false` if they
/// quit without saving.
///
/// The caller is expected to have already entered the alternate screen and
/// enabled raw mode (the prompt list does so); this editor reuses that
/// state and only manages the cursor visibility.
pub fn run_editor(path: &Path) -> Result<bool, EditorError> {
    let content = std::fs::read_to_string(path).map_err(EditorError::Io)?;
    run_editor_with_content(&content)
}

/// Open the editor with the given initial `content` (instead of reading it
/// from a file). Used by the "new prompt" flows (`upl init` and the `n`
/// shortcut in the prompt list), which start from the [`SKELETON`] template.
///
/// Like [`run_editor`], the caller must have already entered the alternate
/// screen and enabled raw mode.
pub fn run_editor_with_content(content: &str) -> Result<bool, EditorError> {
    let (cols, rows) = terminal::size().map_err(|e| EditorError::Tui(e.to_string()))?;
    let mut ed = Editor {
        lines: split_lines(content),
        row: 0,
        col: 0,
        top: 0,
        left: 0,
        cols,
        rows,
        valid: false,
        var_names: Vec::new(),
        name: String::new(),
        errors: Vec::new(),
        saved: false,
        message: String::new(),
    };
    ed.recompute();
    ed.run()?;
    Ok(ed.saved)
}

/// Standalone entry point that performs the full terminal setup (enter the
/// alternate screen, enable raw mode) before delegating to
/// [`run_editor_with_content`], and restores the terminal on exit. Use this
/// from contexts that are not already inside a TUI (e.g. `upl init`).
///
/// `content` is the initial prompt text to edit (typically [`SKELETON`]).
/// Returns `true` if the user saved the prompt.
pub fn run_editor_standalone(content: &str) -> Result<bool, EditorError> {
    let mut stdout = io::stderr();
    execute!(
        stdout,
        EnterAlternateScreen,
        DisableLineWrap,
        cursor::Hide
    )
    .map_err(|e| EditorError::Tui(e.to_string()))?;
    terminal::enable_raw_mode().map_err(|e| EditorError::Tui(e.to_string()))?;

    let result = run_editor_with_content(content);

    let _ = terminal::disable_raw_mode();
    let _ = execute!(stdout, cursor::Show, EnableLineWrap, LeaveAlternateScreen);
    let _ = stdout.flush();

    result
}

fn split_lines(content: &str) -> Vec<Vec<char>> {
    if content.is_empty() {
        return vec![Vec::new()];
    }
    content
        .split('\n')
        .map(|l| l.chars().collect::<Vec<char>>())
        .collect()
}

impl Editor {
    fn run(&mut self) -> Result<(), EditorError> {
        let mut stdout = io::stderr();
        let _ = queue!(stdout, cursor::Show);

        let res = (|| -> Result<(), EditorError> {
            loop {
                self.render(&mut stdout)?;
                stdout.flush().map_err(|e| EditorError::Tui(e.to_string()))?;

                let ev = event::read().map_err(|e| EditorError::Tui(e.to_string()))?;
                // Clear any transient message on the next event.
                self.message.clear();

                match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match key.code {
                            KeyCode::Char('s') if ctrl => self.save()?,
                            KeyCode::Char('r') if ctrl => {
                                let mut out = io::stderr();
                                show_rfc_popup(&mut out)?;
                                let _ = queue!(out, cursor::Hide);
                            }
                            KeyCode::Char('c') if ctrl => return Ok(()),
                            KeyCode::Esc => return Ok(()),
                            KeyCode::Char(c) if !ctrl => {
                                self.insert_char(c);
                                self.recompute();
                            }
                            KeyCode::Tab => {
                                self.insert_char(' ');
                                self.insert_char(' ');
                                self.recompute();
                            }
                            KeyCode::Enter => {
                                self.newline();
                                self.recompute();
                            }
                            KeyCode::Backspace => {
                                self.backspace();
                                self.recompute();
                            }
                            KeyCode::Delete => {
                                self.delete();
                                self.recompute();
                            }
                            KeyCode::Left => self.move_left(),
                            KeyCode::Right => self.move_right(),
                            KeyCode::Up => self.move_up(),
                            KeyCode::Down => self.move_down(),
                            KeyCode::Home => self.col = 0,
                            KeyCode::End => {
                                self.col = self.lines[self.row].len();
                            }
                            KeyCode::PageUp => {
                                let h = self.current_edit_height();
                                self.row = self.row.saturating_sub(h);
                                self.clamp_row();
                            }
                            KeyCode::PageDown => {
                                let h = self.current_edit_height();
                                self.row = (self.row + h).min(self.lines.len() - 1);
                            }
                            _ => {}
                        }
                    }
                    Event::Resize(c, r) => {
                        self.cols = c;
                        self.rows = r;
                    }
                    _ => {}
                }
            }
        })();

        let _ = queue!(stdout, cursor::Hide, Clear(ClearType::All));
        stdout.flush().ok();
        res
    }

    // ---- editing primitives ----

    fn insert_char(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
    }

    fn newline(&mut self) {
        let rest: Vec<char> = self.lines[self.row].drain(self.col..).collect();
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        } else if self.row > 0 {
            let prev_len = self.lines[self.row - 1].len();
            let cur: Vec<char> = self.lines.remove(self.row);
            self.lines[self.row - 1].extend(cur);
            self.row -= 1;
            self.col = prev_len;
        }
    }

    fn delete(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next: Vec<char> = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.clamp_row();
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.clamp_row();
        }
    }

    fn clamp_row(&mut self) {
        if self.col > self.lines[self.row].len() {
            self.col = self.lines[self.row].len();
        }
    }

    // ---- parse state ----

    fn content_string(&self) -> String {
        let mut s = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.extend(line.iter());
        }
        s
    }

    fn recompute(&mut self) {
        let content = self.content_string();
        match PromptParser::parse(&content) {
            Ok(p) => {
                self.valid = true;
                self.var_names = p
                    .variable_definitions
                    .iter()
                    .map(|(k, v)| (k.clone(), v.r#type))
                    .collect();
                self.name = p.name;
                self.errors = Vec::new();
            }
            Err(e) => {
                self.valid = false;
                self.var_names = Vec::new();
                self.name = String::new();
                self.errors = vec![e.to_string()];
            }
        }
    }

    // ---- save ----

    fn save(&mut self) -> Result<(), EditorError> {
        if !self.valid {
            self.message = "invalid: cannot save".to_string();
            return Ok(());
        }
        let name = if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else {
            self.message = "no `name` field: cannot save".to_string();
            return Ok(());
        };
        let home = std::env::var("HOME").map_err(|_| {
            EditorError::Save("HOME not set".to_string())
        })?;
        let dir = PathBuf::from(home).join(".upl").join("prompts");
        std::fs::create_dir_all(&dir).map_err(EditorError::Io)?;
        let fname = sanitize_filename(&format!("{name}.txt"));
        let path = dir.join(&fname);
        std::fs::write(&path, self.content_string()).map_err(EditorError::Io)?;
        self.saved = true;
        self.message = format!("saved to {}", path.display());
        Ok(())
    }

    // ---- layout helpers ----
    //
    // Layout (with borders):
    //   row 0            : top border  ┌──┬──┐
    //   row 1..rows-3    : inner area  │content││sidebar│
    //   row rows-2       : bottom border └──┴──┘
    //   row rows-1       : status bar (VALID/INVALID + hint)
    //
    // `content_w` / `sidebar_w` are the *inner* widths (excluding borders).
    // Three vertical border columns + the two panes fill the full width.

    fn inner_top(&self) -> usize {
        1
    }

    fn inner_height(&self) -> usize {
        self.rows.saturating_sub(3) as usize
    }

    fn bottom_border_y(&self) -> u16 {
        self.rows.saturating_sub(2) as u16
    }

    fn status_y(&self) -> u16 {
        self.rows.saturating_sub(1) as u16
    }

    fn sidebar_w(&self) -> usize {
        // Sidebar is 18% of the terminal width, clamped to a sane range.
        let w = (self.cols as usize) * 18 / 100;
        w.max(10).min(40)
    }

    fn content_w(&self) -> usize {
        (self.cols as usize)
            .saturating_sub(self.sidebar_w())
            .saturating_sub(3) // three vertical borders
    }

    fn content_x(&self) -> usize {
        1
    }

    fn sidebar_x(&self) -> usize {
        self.content_w() + 2
    }

    /// Wrap the stored parse errors to the sidebar inner width, producing
    /// one string per display line. Empty when the prompt is valid. Errors
    /// are shown in the sidebar (instead of variable names) when invalid.
    fn error_lines(&self) -> Vec<String> {
        let sw = self.sidebar_w().saturating_sub(2);
        if sw == 0 || self.errors.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for err in &self.errors {
            for chunk in wrap_text(err, sw) {
                out.push(chunk);
            }
        }
        out
    }

    /// Height of the editable region. The full inner height is available for
    /// editing; error messages render in the sidebar, not the content pane.
    fn edit_height(&self, _err_h: usize) -> usize {
        self.inner_height()
    }

    fn current_edit_height(&self) -> usize {
        self.inner_height()
    }

    fn scroll_to_cursor(&mut self, edit_h: usize) {
        if self.row < self.top {
            self.top = self.row;
        }
        if self.row >= self.top + edit_h && edit_h > 0 {
            self.top = self.row + 1 - edit_h;
        }
        let cw = self.content_w();
        if self.col < self.left {
            self.left = self.col;
        }
        if self.col >= self.left + cw && cw > 0 {
            self.left = self.col + 1 - cw;
        }
    }

    // ---- rendering ----

    fn render<W: Write>(&mut self, stdout: &mut W) -> io::Result<()> {
        let cols = self.cols as usize;
        let inner_h = self.inner_height();
        let cw = self.content_w();
        let sw = self.sidebar_w();
        let cx0 = self.content_x();
        let sx0 = self.sidebar_x();
        let itop = self.inner_top();

        let wrapped_errors = self.error_lines();
        let edit_h = self.edit_height(wrapped_errors.len());
        self.scroll_to_cursor(edit_h);

        let bgs = compute_block_bgs(&self.lines);
        let is_header = compute_header_flags(&self.lines);
        let is_param_name = compute_param_name_flags(&self.lines);

        // Hide the cursor while we repaint so it doesn't visibly jump to
        // every cell during the draw; it is re-shown at the edit position
        // once the frame is complete.
        queue!(stdout, cursor::Hide)?;

        // ---- top border ----
        let top = format!(
            "┌{}┬{}┐",
            "─".repeat(cw),
            "─".repeat(sw)
        );
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetForegroundColor(Color::DarkGrey),
            Print(pad_to(&top, cols)),
            ResetColor
        )?;

        // ---- content + separator + sidebar (inner rows) ----
        for vi in 0..inner_h {
            let y = (itop + vi) as u16;
            // left border
            queue!(
                stdout,
                cursor::MoveTo(0, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                ResetColor
            )?;
            // content
            queue!(stdout, cursor::MoveTo(cx0 as u16, y))?;
            let li = self.top + vi;
            if li < self.lines.len() {
                render_line(
                    stdout,
                    &self.lines[li],
                    bgs[li],
                    is_header[li],
                    is_param_name[li],
                    self.left,
                    cw,
                )?;
            } else {
                // empty line: fill with default bg
                queue!(
                    stdout,
                    SetForegroundColor(Color::White),
                    Print(" ".repeat(cw)),
                    ResetColor
                )?;
            }
            // middle border
            queue!(
                stdout,
                cursor::MoveTo((cx0 + cw) as u16, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                ResetColor
            )?;
            // sidebar
            queue!(stdout, cursor::MoveTo(sx0 as u16, y))?;
            self.render_sidebar_line(stdout, vi, sw, &wrapped_errors)?;
            // right border
            queue!(
                stdout,
                cursor::MoveTo((sx0 + sw) as u16, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                ResetColor
            )?;
        }

        // ---- bottom border ----
        let bottom = format!(
            "└{}┴{}┘",
            "─".repeat(cw),
            "─".repeat(sw)
        );
        queue!(
            stdout,
            cursor::MoveTo(0, self.bottom_border_y()),
            SetForegroundColor(Color::DarkGrey),
            Print(pad_to(&bottom, cols)),
            ResetColor
        )?;

        // ---- status bar ----
        queue!(stdout, cursor::MoveTo(0, self.status_y()), Clear(ClearType::CurrentLine))?;
        let hint = if self.message.is_empty() {
            "  Ctrl+S save · Ctrl+R UPL Help · Esc quit".to_string()
        } else {
            format!("  {}", self.message)
        };
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(hint),
            ResetColor
        )?;

        // ---- cursor ----
        let cx = (cx0 + self.col - self.left) as u16;
        let cy = (itop + self.row - self.top) as u16;
        queue!(stdout, cursor::MoveTo(cx, cy), cursor::Show)?;

        Ok(())
    }

/// Render one sidebar inner row. The sidebar has a 1-char padding on each
/// side, so the inner content width is `w - 2` and is drawn starting at
/// column 1 within the sidebar.
///
/// Row layout inside the sidebar:
///   vi == 0 : status badge header — "VALID" (green bg / black fg, bold) when
///             the prompt parses, or "INVALID" (red bg / black fg, bold) when
///             it does not. Replaces the old bottom status badge.
///   vi == 1 : empty line
///   vi >= 2 : when VALID, a variable block (3 rows): name (yellow bg / black
///             fg, with a leading and trailing space inside the background),
///             its type (white, no background), and a blank separator row.
///             When INVALID, the wrapped parse-error lines (red) take over
///             the variable rows so the user sees what is wrong.
fn render_sidebar_line<W: Write>(
    &self,
    stdout: &mut W,
    vi: usize,
    w: usize,
    errors: &[String],
) -> io::Result<()> {
    if w < 2 {
        return Ok(());
    }
    let inner_w = w - 2;

    // Left padding (1 char, no background).
    queue!(
        stdout,
        SetForegroundColor(Color::White),
        Print(" "),
        ResetColor
    )?;

    match vi {
        0 => {
            // Status badge header (moved here from the bottom status bar).
            let (label, bg) = if self.valid {
                ("VALID", Color::Green)
            } else {
                ("INVALID", Color::Red)
            };
            let label = format!(" {label} ");
            let label_w = label.chars().count().min(inner_w);
            let label: String = label.chars().take(label_w).collect();
            queue!(
                stdout,
                SetBackgroundColor(bg),
                SetForegroundColor(Color::Black),
                SetAttribute(Attribute::Bold),
                Print(&label),
                ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
            if label_w < inner_w {
                queue!(
                    stdout,
                    SetForegroundColor(Color::White),
                    Print(" ".repeat(inner_w - label_w)),
                    ResetColor
                )?;
            }
        }
        1 => {
            queue!(
                stdout,
                SetForegroundColor(Color::White),
                Print(" ".repeat(inner_w)),
                ResetColor
            )?;
        }
        _ => {
            if self.valid {
                let idx = (vi - 2) / ROWS_PER_VAR;
                let sub = (vi - 2) % ROWS_PER_VAR;
                let n = self.var_names.len();
                if idx >= n {
                    queue!(
                        stdout,
                        SetForegroundColor(Color::White),
                        Print(" ".repeat(inner_w)),
                        ResetColor
                    )?;
                } else {
                    let (name, vtype) = &self.var_names[idx];
                    match sub {
                        0 => {
                            // Variable name with a leading and trailing space,
                            // all inside the yellow background. The background
                            // ends right after the trailing space.
                            let label = format!(" {} ", name);
                            let label_w = label.chars().count().min(inner_w);
                            let label: String = label.chars().take(label_w).collect();
                            queue!(
                                stdout,
                                SetBackgroundColor(Color::Yellow),
                                SetForegroundColor(Color::Black),
                                Print(&label),
                                ResetColor
                            )?;
                            if label_w < inner_w {
                                queue!(
                                    stdout,
                                    SetForegroundColor(Color::White),
                                    Print(" ".repeat(inner_w - label_w)),
                                    ResetColor
                                )?;
                            }
                        }
                        1 => {
                            let label = format!(" {}", type_name(vtype));
                            queue!(
                                stdout,
                                SetForegroundColor(Color::White),
                                Print(pad_to(&label, inner_w)),
                                ResetColor
                            )?;
                        }
                        _ => {
                            queue!(
                                stdout,
                                SetForegroundColor(Color::White),
                                Print(" ".repeat(inner_w)),
                                ResetColor
                            )?;
                        }
                    }
                }
            } else {
                // Error lines replace the variable rows when invalid.
                let ei = vi - 2;
                let cell = errors.get(ei).map(|s| s.as_str()).unwrap_or("");
                queue!(
                    stdout,
                    SetForegroundColor(Color::Red),
                    Print(pad_to(cell, inner_w)),
                    ResetColor
                )?;
            }
        }
    }

    // Right padding (1 char, no background).
    queue!(
        stdout,
        SetForegroundColor(Color::White),
        Print(" "),
        ResetColor
    )?;
    Ok(())
}
}

// ---- free helpers ----

fn pad_to(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

/// Greedy word-wrap of `s` into lines of at most `width` chars. Words longer
/// than `width` are hard-broken. An empty input yields a single empty line so
/// callers always get at least one line to render.
fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    for line in s.split('\n') {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in line.split_whitespace() {
            let wl = word.chars().count();
            if cur.is_empty() {
                if wl <= width {
                    cur.push_str(word);
                } else {
                    let mut rest: String = word.to_string();
                    while rest.chars().count() > width {
                        let chunk: String = rest.chars().take(width).collect();
                        out.push(chunk);
                        rest = rest.chars().skip(width).collect();
                    }
                    cur = rest;
                }
            } else if cur.chars().count() + 1 + wl <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                out.push(std::mem::take(&mut cur));
                if wl <= width {
                    cur.push_str(word);
                } else {
                    let mut rest: String = word.to_string();
                    while rest.chars().count() > width {
                        let chunk: String = rest.chars().take(width).collect();
                        out.push(chunk);
                        rest = rest.chars().skip(width).collect();
                    }
                    cur = rest;
                }
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn type_name(t: &VariableType) -> &'static str {
    match t {
        VariableType::String => "string",
        VariableType::LongString => "long_string",
        VariableType::Number => "number",
        VariableType::Boolean => "boolean",
        VariableType::List => "list",
        VariableType::Object => "object",
        VariableType::OptionSingle => "option_single",
        VariableType::OptionMulti => "option_multi",
    }
}

/// Mark every line that belongs to the header/params section (the lines
/// between the optional opening `--` and the `--` delimiter that closes the
/// params block). Body lines are not header lines.
fn compute_header_flags(lines: &[Vec<char>]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    let mut start = 0usize;
    // Optional opening `--` delimiter.
    if !lines.is_empty() {
        let s: String = lines[0].iter().collect();
        if s.trim() == "--" {
            start = 1;
        }
    }
    // Find the closing `--` delimiter that ends the params block.
    let mut end = lines.len();
    for i in start..lines.len() {
        let s: String = lines[i].iter().collect();
        if s.trim() == "--" {
            end = i;
            break;
        }
    }
    for i in start..end {
        out[i] = true;
    }
    out
}

/// Compute the background block for every line. The opener line belongs to
/// the block it opens, the closer line belongs to the block it closes, and
/// the lines in between belong to the innermost enclosing block.
fn compute_block_bgs(lines: &[Vec<char>]) -> Vec<Block> {
    let mut out = vec![Block::None; lines.len()];
    let mut stack: Vec<Block> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let s: String = line.iter().collect();
        let ends_for = s.contains("{{{end for}}}");
        let ends_if = s.contains("{{{end if}}}");
        let opens_for = opens_block(&s, "for");
        let opens_if = opens_block(&s, "if");

        if ends_for || ends_if {
            out[i] = stack.last().copied().unwrap_or(Block::None);
            if ends_for {
                stack.pop();
            }
            if ends_if {
                stack.pop();
            }
            if opens_for {
                out[i] = Block::For;
                stack.push(Block::For);
            } else if opens_if {
                out[i] = Block::If;
                stack.push(Block::If);
            }
        } else if opens_for {
            out[i] = Block::For;
            stack.push(Block::For);
        } else if opens_if {
            out[i] = Block::If;
            stack.push(Block::If);
        } else {
            out[i] = stack.last().copied().unwrap_or(Block::None);
        }
    }
    out
}

/// Does `s` contain an opening block tag `<{ { { <kw> ... } } }` (with a
/// matching `}}}` after the keyword)?
fn opens_block(s: &str, kw: &str) -> bool {
    let pat = ["{{{", kw, " "].concat();
    match s.find(&pat) {
        Some(i) => s[i + pat.len()..].contains("}}}"),
        None => false,
    }
}

/// Find all `[[[...]]]` placeholder spans (start, end) in char indices.
fn find_placeholders(line: &[char]) -> Vec<(usize, usize)> {
    let s: String = line.iter().collect();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while let Some(o) = s[i..].find("[[[") {
        let start = i + o;
        match s[start + 3..].find("]]]") {
            Some(e) => {
                let end = start + 3 + e + 3;
                spans.push((start, end));
                i = end;
            }
            None => break,
        }
    }
    spans
}

fn in_placeholder(idx: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|(a, b)| idx >= *a && idx < *b)
}

fn block_color(b: Block) -> Color {
    match b {
        Block::None => Color::Reset,
        Block::For => Color::DarkGrey,
        Block::If => Color::Black,
    }
}

/// Render a single content line into the content area, clipping to the
/// horizontal scroll window `[left, left+width)` and applying:
///   - header key (the `key:` prefix) on a header line: grey foreground
///   - placeholder spans: yellow background, black foreground
///   - the line's block background for everything else
fn render_line<W: Write>(
    stdout: &mut W,
    line: &[char],
    bg: Block,
    is_header: bool,
    is_param_name: bool,
    left: usize,
    width: usize,
) -> io::Result<()> {
    let spans = find_placeholders(line);
    let key_span = if is_header { header_key_span(line) } else { None };
    let end = (left + width).min(line.len());
    let base_bg = block_color(bg);

    // Walk the visible window, grouping consecutive chars by style.
    let mut idx = left;
    let mut seg_buf = String::new();
    let mut seg_style = style_at(idx, end, &spans, &key_span, is_param_name);
    let mut written = 0usize; // visible columns emitted

    while idx < end {
        let st = style_at(idx, end, &spans, &key_span, is_param_name);
        if st != seg_style {
            emit_segment(stdout, &seg_buf, seg_style, base_bg)?;
            written += seg_buf.chars().count();
            seg_buf.clear();
            seg_style = st;
        }
        seg_buf.push(line[idx]);
        idx += 1;
    }
    if !seg_buf.is_empty() {
        emit_segment(stdout, &seg_buf, seg_style, base_bg)?;
        written += seg_buf.chars().count();
    }

    // Pad the rest of the content width with the block background.
    if written < width {
        let pad = " ".repeat(width - written);
        queue!(
            stdout,
            SetBackgroundColor(base_bg),
            SetForegroundColor(Color::White),
            Print(pad),
            ResetColor
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegStyle {
    Normal,
    Placeholder,
    HeaderKey,
    /// The param-name key (a first-level variable name under `params:`).
    /// Rendered in yellow to match the sidebar's param-name background.
    ParamName,
}

fn style_at(
    idx: usize,
    end: usize,
    spans: &[(usize, usize)],
    key_span: &Option<(usize, usize)>,
    is_param_name: bool,
) -> SegStyle {
    if idx >= end {
        return SegStyle::Normal;
    }
    if in_placeholder(idx, spans) {
        return SegStyle::Placeholder;
    }
    if let Some((a, b)) = key_span {
        if idx >= *a && idx < *b {
            return if is_param_name {
                SegStyle::ParamName
            } else {
                SegStyle::HeaderKey
            };
        }
    }
    SegStyle::Normal
}

fn emit_segment<W: Write>(
    stdout: &mut W,
    text: &str,
    style: SegStyle,
    base_bg: Color,
) -> io::Result<()> {
    match style {
        SegStyle::Placeholder => queue!(
            stdout,
            SetBackgroundColor(Color::Yellow),
            SetForegroundColor(Color::Black),
            Print(text),
            ResetColor
        ),
        SegStyle::HeaderKey => queue!(
            stdout,
            SetBackgroundColor(base_bg),
            SetForegroundColor(Color::DarkGrey),
            Print(text),
            ResetColor
        ),
        SegStyle::ParamName => queue!(
            stdout,
            SetBackgroundColor(base_bg),
            SetForegroundColor(Color::Yellow),
            Print(text),
            ResetColor
        ),
        SegStyle::Normal => queue!(
            stdout,
            SetBackgroundColor(base_bg),
            SetForegroundColor(Color::White),
            Print(text),
            ResetColor
        ),
    }
}

/// Mark every line whose `key:` is the name of a top-level param (a
/// first-level variable definition under `params:`). These keys are rendered
/// in yellow to match the sidebar's param-name background.
fn compute_param_name_flags(lines: &[Vec<char>]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    // Header section bounds (same logic as `compute_header_flags`).
    let mut start = 0usize;
    if !lines.is_empty() {
        let s: String = lines[0].iter().collect();
        if s.trim() == "--" {
            start = 1;
        }
    }
    let mut end = lines.len();
    for i in start..lines.len() {
        let s: String = lines[i].iter().collect();
        if s.trim() == "--" {
            end = i;
            break;
        }
    }

    // Find the `params:` line.
    let mut params_line = None;
    for i in start..end {
        if let Some((key, _)) = line_key(&lines[i]) {
            if key == "params" {
                params_line = Some(i);
                break;
            }
        }
    }
    let Some(params_line) = params_line else {
        return out;
    };

    // The first-level variables live at the shallowest indent among the
    // non-empty lines after `params:`.
    let mut min_indent: Option<usize> = None;
    for i in (params_line + 1)..end {
        let s: String = lines[i].iter().collect();
        if s.trim().is_empty() {
            continue;
        }
        let indent = lines[i].iter().take_while(|c| **c == ' ').count();
        min_indent = Some(min_indent.map_or(indent, |m| m.min(indent)));
    }
    let Some(min_indent) = min_indent else {
        return out;
    };

    // A param-name line is a `key:` line whose indent equals the minimum.
    // Sub-properties (`type:`, `desc:`, ...) live at deeper indents.
    for i in (params_line + 1)..end {
        let line = &lines[i];
        let indent = line.iter().take_while(|c| **c == ' ').count();
        if indent != min_indent {
            continue;
        }
        if line_key(line).is_some() {
            out[i] = true;
        }
    }
    out
}

/// For a header/params line, return the span `(start, end)` covering the
/// leading whitespace, the key identifier, and the trailing `:`. Returns
/// `None` if the line is not a `key:` line (e.g. a `- item` list entry or a
/// heredoc body line).
fn header_key_span(line: &[char]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < line.len() && line[i] == ' ' {
        i += 1;
    }
    if i >= line.len() || !(line[i].is_ascii_alphabetic() || line[i] == '_') {
        return None;
    }
    while i < line.len()
        && (line[i].is_ascii_alphanumeric() || line[i] == '_' || line[i] == '.')
    {
        i += 1;
    }
    if i < line.len() && line[i] == ':' {
        Some((0, i + 1))
    } else {
        None
    }
}

/// If `line` is a `key:` line (after leading spaces), return `(key, indent)`.
fn line_key(line: &[char]) -> Option<(String, usize)> {
    let mut i = 0;
    while i < line.len() && line[i] == ' ' {
        i += 1;
    }
    let indent = i;
    if i >= line.len() || !(line[i].is_ascii_alphabetic() || line[i] == '_') {
        return None;
    }
    let start = i;
    while i < line.len()
        && (line[i].is_ascii_alphanumeric() || line[i] == '_' || line[i] == '.')
    {
        i += 1;
    }
    if i < line.len() && line[i] == ':' {
        let key: String = line[start..i].iter().collect();
        Some((key, indent))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// UPL RFC reference popup (Ctrl+R)
// ---------------------------------------------------------------------------

/// The UPL RFC, embedded into the binary at compile time. Rebuilding the app
/// picks up any changes to the source `.md` file automatically.
const RFC_TEXT: &str = include_str!("../../upl-spec/upl-1.0-rfc.md");

/// Centered (90% × 90%) scrollable viewer for `RFC_TEXT`. The viewer takes
/// over the screen until the user presses Esc, `q`, or Ctrl+C, then returns.
/// Public so other TUIs (e.g. the prompt list) can reuse it via `Ctrl+R`.
pub fn show_rfc_popup<W: Write>(stdout: &mut W) -> io::Result<()> {
    let lines: Vec<&str> = RFC_TEXT.lines().collect();
    let mut top: usize = 0;
    let mut left: usize = 0;
    let mut cols: u16;
    let mut rows: u16;
    let (c, r) = terminal::size()?;
    cols = c;
    rows = r;

    let _ = queue!(stdout, cursor::Show);

    let res = (|| -> io::Result<()> {
        loop {
            let (win_x, win_y, win_w, win_h) = rfc_window(cols, rows);
            draw_rfc(stdout, &lines, top, left, win_x, win_y, win_w, win_h)?;
            stdout.flush()?;

            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    // Ctrl+C, plain Esc, and plain q/Q all close the popup.
                    if (ctrl && key.code == KeyCode::Char('c'))
                        || (!ctrl && key.code == KeyCode::Esc)
                        || (!ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')))
                    {
                        return Ok(());
                    }
                    match key.code {
                        KeyCode::Up => top = top.saturating_sub(1),
                        KeyCode::Down => {
                            let h = (win_h as usize).saturating_sub(2);
                            if top + h < lines.len() {
                                top += 1;
                            }
                        }
                        KeyCode::PageUp => {
                            let h = (win_h as usize).saturating_sub(2);
                            top = top.saturating_sub(h.max(1));
                        }
                        KeyCode::PageDown => {
                            let h = (win_h as usize).saturating_sub(2);
                            top = (top + h.max(1)).min(lines.len().saturating_sub(h.max(1)));
                        }
                        KeyCode::Home => top = 0,
                        KeyCode::End => {
                            let h = (win_h as usize).saturating_sub(2);
                            top = lines.len().saturating_sub(h.max(1));
                        }
                        KeyCode::Left => left = left.saturating_sub(4),
                        KeyCode::Right => left = left.saturating_add(4),
                        _ => {}
                    }
                }
                Event::Resize(c, r) => {
                    cols = c;
                    rows = r;
                }
                _ => {}
            }
        }
    })();

    let _ = queue!(stdout, cursor::Hide, Clear(ClearType::All));
    stdout.flush().ok();
    res
}

/// Compute the centered popup geometry: 90% of the terminal width and
/// height, centered, returning `(x, y, width, height)` in cells. The width
/// and height are kept odd/even so the borders line up.
fn rfc_window(cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    let w = ((cols as u32 * 90) / 100).max(20) as u16;
    let h = ((rows as u32 * 90) / 100).max(7) as u16;
    let w = w.min(cols);
    let h = h.min(rows);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    (x, y, w, h)
}

fn draw_rfc<W: Write>(
    stdout: &mut W,
    lines: &[&str],
    top: usize,
    left: usize,
    win_x: u16,
    win_y: u16,
    win_w: u16,
    win_h: u16,
) -> io::Result<()> {
    let win_w = win_w as usize;
    let inner_h = win_h.saturating_sub(3) as usize; // top + bottom borders + status line
    let inner_w = win_w.saturating_sub(2); // left + right borders

    let _ = queue!(stdout, cursor::Hide);

    // Top border with a centered title.
    let title = " UPL RFC ";
    let title_w = title.chars().count();
    let title_start = (win_w.saturating_sub(title_w)) / 2;
    let mut top_line = String::with_capacity(win_w);
    top_line.push('┌');
    for i in 0..(win_w.saturating_sub(2)) {
        if i >= title_start && i < title_start + title_w {
            top_line.push_str(&title[i - title_start..].chars().next().unwrap().to_string());
        } else {
            top_line.push('─');
        }
    }
    top_line.push('┐');
    queue!(
        stdout,
        cursor::MoveTo(win_x, win_y),
        SetForegroundColor(Color::DarkGrey),
        Print(pad_to(&top_line, win_w)),
        ResetColor
    )?;

    // Inner lines.
    for vi in 0..inner_h {
        let y = win_y + 1 + vi as u16;
        let li = top + vi;
        // left border
        queue!(
            stdout,
            cursor::MoveTo(win_x, y),
            SetForegroundColor(Color::DarkGrey),
            Print("│"),
            ResetColor
        )?;
        // content cell
        let cell = if li < lines.len() {
            let line = lines[li];
            let chars: Vec<char> = line.chars().collect();
            let end = (left + inner_w).min(chars.len());
            if left < chars.len() {
                chars[left..end].iter().collect::<String>()
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        queue!(
            stdout,
            SetForegroundColor(Color::White),
            Print(pad_to(&cell, inner_w)),
            ResetColor
        )?;
        // right border
        queue!(
            stdout,
            cursor::MoveTo(win_x + (win_w - 1) as u16, y),
            SetForegroundColor(Color::DarkGrey),
            Print("│"),
            ResetColor
        )?;
    }

    // Bottom border.
    let bottom = format!("└{}┘", "─".repeat(win_w.saturating_sub(2)));
    let by = win_y + 1 + inner_h as u16;
    queue!(
        stdout,
        cursor::MoveTo(win_x, by),
        SetForegroundColor(Color::DarkGrey),
        Print(pad_to(&bottom, win_w)),
        ResetColor
    )?;

    // Status / hint line (inside the box, below the bottom border).
    let sy = by + 1;
    let total = lines.len();
    let pct = if total == 0 {
        0
    } else {
        ((top + inner_h.min(total - top)) * 100) / total
    };
    let hint = format!(
        " line {}/{} · {}%   ↑/↓ scroll · PgUp/PgDn · Home/End · ←/→ pan · Esc/q close ",
        top.saturating_add(1).min(total.max(1)),
        total,
        pct.min(100)
    );
    queue!(
        stdout,
        cursor::MoveTo(win_x, sy),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(pad_to(&hint, win_w)),
        ResetColor
    )?;

    let _ = queue!(stdout, cursor::Show);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<Vec<char>> {
        split_lines(s)
    }

    #[test]
    fn skeleton_parses_as_valid_prompt() {
        // The "new prompt" skeleton must be a valid UPL prompt so the editor
        // opens in the VALID state and the user can save it immediately.
        let prompt = PromptParser::parse(SKELETON)
            .expect("SKELETON must be a valid UPL prompt");
        assert_eq!(prompt.name, "new_prompt");
        assert_eq!(prompt.title.as_deref(), Some("New Prompt"));
        assert!(prompt.variable_definitions.contains_key("input"));
        assert!(prompt.prompt.contains("[[[INPUT]]]"));
    }

    #[test]
    fn split_and_join_round_trips() {
        for s in ["", "a", "a\n", "a\nb", "a\nb\n", "foo\nbar\nbaz\n"] {
            let ed_lines = split_lines(s);
            let out = {
                let mut o = String::new();
                for (i, l) in ed_lines.iter().enumerate() {
                    if i > 0 {
                        o.push('\n');
                    }
                    o.extend(l.iter());
                }
                o
            };
            assert_eq!(out, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn block_bgs_for_sample_body() {
        let body = "pre\n{{{for CRITERION in CRITERIA}}}\n- [[[CRITERION.NAME]]]\n{{{end for}}}\nmid\n{{{if FLAG}}}\nyes\n{{{end if}}}\n";
        let lines = chars(body);
        let bgs = compute_block_bgs(&lines);
        assert_eq!(bgs, vec![
            Block::None,  // pre
            Block::For,   // opener
            Block::For,   // body line (with placeholder)
            Block::For,   // closer
            Block::None,  // mid
            Block::If,    // opener
            Block::If,    // body
            Block::If,    // closer
            Block::None,  // trailing empty line (from the closing \n)
        ]);
    }

    #[test]
    fn nested_if_inside_for_innermost_wins() {
        let body = "{{{for X in Y}}}\n{{{if Z}}}\nx\n{{{end if}}}\n{{{end for}}}}";
        let lines = chars(body);
        let bgs = compute_block_bgs(&lines);
        assert_eq!(bgs, vec![
            Block::For,
            Block::If,
            Block::If,
            Block::If, // end if closer belongs to if
            Block::For, // end for closer belongs to for
        ]);
    }

    #[test]
    fn placeholders_detected() {
        let line: Vec<char> = "a [[[VAR]]] b [[[X.Y]]]".chars().collect();
        let spans = find_placeholders(&line);
        assert_eq!(spans, vec![(2, 11), (14, 23)]);
    }

    #[test]
    fn opens_block_detects_tags() {
        assert!(opens_block("{{{for CRITERION in CRITERIA}}}", "for"));
        assert!(opens_block("{{{if AUDIENCE = \"debate\"}}}", "if"));
        assert!(!opens_block("{{{cond ? a : b}}}", "for"));
        assert!(!opens_block("{{{end for}}}", "for"));
        // opener needs a closing }}}
        assert!(!opens_block("{{{for X in Y", "for"));
    }

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize_filename("my-prompt.txt"), "my-prompt.txt");
        assert_eq!(sanitize_filename("a/b/c"), "a_b_c");
        assert_eq!(sanitize_filename("hi there!"), "hi_there_");
    }

    #[test]
    fn wrap_text_breaks_long_lines() {
        let out = wrap_text("hello world this is a test", 10);
        for line in &out {
            assert!(line.chars().count() <= 10, "line too long: {line:?}");
        }
        assert!(out.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn wrap_text_hard_breaks_long_word() {
        let out = wrap_text("supercalifragilistic", 5);
        for line in &out {
            assert!(line.chars().count() <= 5, "line too long: {line:?}");
        }
        let joined = out.join("");
        assert_eq!(joined, "supercalifragilistic");
    }

    #[test]
    fn wrap_text_empty_yields_one_line() {
        let out = wrap_text("", 10);
        assert_eq!(out, vec!["".to_string()]);
    }
}