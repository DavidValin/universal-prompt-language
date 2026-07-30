// tags.rs
//
// Tag management for upl prompts.
//
// - `TagStore` persists a tag -> [sha256] mapping in `~/.upl/tags_db`
//   using bincode (fast binary format).
// - `run_tui` renders the "all tags" view: a search bar at the top, the list
//   of tags with prompt counts in grey, and a footer with shortcuts
//   (Ctrl+D delete, Ctrl+E rename, q/Esc back).
//
// Each prompt is identified by the sha256 of its file contents.

use std::collections::HashMap;
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
    terminal::{self},
};
use serde::{Deserialize, Serialize};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TagsError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("storage: {0}")]
    Storage(String),
    #[error("TUI: {0}")]
    Tui(String),
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct TagStore {
    // tag name -> list of prompt sha256 hashes (no duplicates)
    tags: HashMap<String, Vec<String>>,
}

impl TagStore {
    pub fn load() -> Result<Self, TagsError> {
        let path = db_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        bincode::deserialize(&bytes)
            .map_err(|e| TagsError::Storage(e.to_string()))
    }

    pub fn save(&self) -> Result<(), TagsError> {
        let path = db_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes =
            bincode::serialize(self).map_err(|e| TagsError::Storage(e.to_string()))?;
        fs::write(&path, bytes)?;
        Ok(())
    }

    /// Sorted list of all tag names.
    pub fn tag_names_sorted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tags.keys().cloned().collect();
        v.sort();
        v
    }

    /// Number of prompts currently associated with `tag`.
    pub fn count_for(&self, tag: &str) -> usize {
        self.tags.get(tag).map(|v| v.len()).unwrap_or(0)
    }

    /// Number of tags currently stored.
    pub fn total(&self) -> usize {
        self.tags.len()
    }

    /// Create a tag if it does not exist (empty).
    pub fn ensure_tag(&mut self, name: &str) {
        self.tags.entry(name.to_string()).or_default();
    }

    /// Rename a tag, preserving its associations.
    pub fn rename(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        if let Some(v) = self.tags.remove(old) {
            let entry = self.tags.entry(new.to_string()).or_default();
            for hash in v {
                if !entry.iter().any(|m| m == &hash) {
                    entry.push(hash);
                }
            }
        }
    }

    /// Delete a tag entirely, untaging every prompt that had it.
    pub fn delete(&mut self, name: &str) {
        self.tags.remove(name);
    }

/// Associate a prompt (by sha256) with a tag, creating the tag if needed.
    pub fn associate(&mut self, tag: &str, hash: &str) {
        let v = self.tags.entry(tag.to_string()).or_default();
        if !v.iter().any(|m| m == hash) {
            v.push(hash.to_string());
        }
    }

    /// Remove a single prompt<->tag association.
    pub fn disassociate(&mut self, tag: &str, hash: &str) {
        if let Some(v) = self.tags.get_mut(tag) {
            v.retain(|m| m != hash);
            if v.is_empty() {
                self.tags.remove(tag);
            }
        }
    }

    /// Sorted list of tag names associated with the prompt identified by `hash`.
    pub fn tags_for_prompt(&self, hash: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .tags
            .iter()
            .filter(|(_, v)| v.iter().any(|m| m == hash))
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }

    /// Whether `tag` is associated with the prompt identified by `hash`.
    pub fn tag_has(&self, tag: &str, hash: &str) -> bool {
        self.tags
            .get(tag)
            .map(|v| v.iter().any(|m| m == hash))
            .unwrap_or(false)
    }

    /// Remove every association whose hash is not in `valid`. Drops tags that
    /// become empty as a side effect. Returns true if anything was pruned
    /// (so the caller can persist the trimmed store).
    pub fn prune(&mut self, valid: &std::collections::HashSet<String>) -> bool {
        let mut changed = false;
        let names: Vec<String> = self.tags.keys().cloned().collect();
        for name in names {
            let v = self.tags.get_mut(&name).unwrap();
            let before = v.len();
            v.retain(|h| valid.contains(h));
            if v.len() != before {
                changed = true;
            }
            if v.is_empty() {
                self.tags.remove(&name);
            }
        }
        changed
    }
}

/// Resolve the tags_db file path (`~/.upl/tags_db`).
pub fn db_path() -> Result<PathBuf, TagsError> {
    let home = std::env::var("HOME").map_err(|_| {
        TagsError::Io(io::Error::new(io::ErrorKind::NotFound, "HOME not set"))
    })?;
    Ok(PathBuf::from(home).join(".upl").join("tags_db"))
}

/// Compute the sha256 of a prompt file's contents, as a lowercase hex string.
pub fn sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------- TUI (filter popup) ----------------

/// What the popup is currently doing.
enum Mode {
    Browse,
    /// Modal confirming deletion of the tag at `index`.
    DeleteConfirm { index: usize },
    /// Modal editing the name of the tag at `index`.
    EditName { index: usize, text: String },
}

/// Run the tag-filter popup as an overlay on top of the prompt list.
///
/// `selected` holds the currently active filter (tag names); it is toggled in
/// place while the popup is open. On `Enter` the popup closes keeping the
/// selection; on `Esc` the selection is cleared and the popup closes.
///
/// `store` is the caller's in-memory `TagStore`; the popup mutates it in
/// place (rename/delete) and persists changes, so the caller's background
/// renders see updates immediately.
///
/// `render_bg` is called at the start of every frame (and after every toggle)
/// so the caller can re-render the prompt list behind the popup — this keeps
/// the filtered list in sync the instant a tag is toggled. It receives the
/// current `selected` tags plus the terminal size.
pub fn run_popup<W, F>(
    stdout: &mut W,
    selected: &mut Vec<String>,
    store: &mut TagStore,
    mut render_bg: F,
) -> Result<(), TagsError>
where
    W: Write,
    F: FnMut(&mut W, &[String], &TagStore, u16, u16) -> Result<(), TagsError>,
{
    let mut query: String = String::new();
    let mut cursor_idx: usize = 0;
    let mut top: usize = 0;
    let mut mode = Mode::Browse;

    let (mut cols, mut lines) =
        terminal::size().map_err(|e| TagsError::Tui(e.to_string()))?;

    execute!(stdout, cursor::Show).map_err(|e| TagsError::Tui(e.to_string()))?;

    let result = (|| -> Result<(), TagsError> {
        loop {
            let names = store.tag_names_sorted();
            let q = query.trim().to_lowercase();
            let filtered: Vec<String> = if q.is_empty() {
                names.clone()
            } else {
                names
                    .iter()
                    .filter(|n| n.to_lowercase().contains(&q))
                    .cloned()
                    .collect()
            };

            if filtered.is_empty() {
                cursor_idx = 0;
            } else if cursor_idx >= filtered.len() {
                cursor_idx = filtered.len() - 1;
            }

            // Re-render the prompt list behind the popup so the filtered
            // list reflects the current selection immediately.
            render_bg(stdout, selected, store, cols, lines)?;

            render_popup(
                stdout, &store, &filtered, &query, selected,
                cursor_idx, &mut top, cols, lines, &mode,
            )?;
            stdout.flush().map_err(|e| TagsError::Tui(e.to_string()))?;

            let ev = event::read().map_err(|e| TagsError::Tui(e.to_string()))?;
            match &mut mode {
                Mode::Browse => match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match key.code {
                            KeyCode::Char('c') if ctrl => {
                                selected.clear();
                                return Ok(());
                            }
                            KeyCode::Char('d') if ctrl && !filtered.is_empty() => {
                                mode = Mode::DeleteConfirm { index: cursor_idx };
                            }
                            KeyCode::Char('e') if ctrl && !filtered.is_empty() => {
                                let text = filtered[cursor_idx].clone();
                                mode = Mode::EditName { index: cursor_idx, text };
                            }
                            KeyCode::Up => {
                                cursor_idx = cursor_idx.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                if cursor_idx + 1 < filtered.len() {
                                    cursor_idx += 1;
                                }
                            }
                            KeyCode::Home => cursor_idx = 0,
                            KeyCode::End => cursor_idx = filtered.len().saturating_sub(1),
                            KeyCode::PageUp => {
                                let h = popup_list_height(lines);
                                cursor_idx = cursor_idx.saturating_sub(h.max(1));
                            }
                            KeyCode::PageDown => {
                                let h = popup_list_height(lines);
                                cursor_idx = (cursor_idx + h.max(1))
                                    .min(filtered.len().saturating_sub(1).max(cursor_idx));
                            }
                            KeyCode::Char(' ') if !filtered.is_empty() => {
                                let name = &filtered[cursor_idx];
                                if let Some(pos) = selected.iter().position(|t| t == name) {
                                    selected.remove(pos);
                                } else {
                                    selected.push(name.clone());
                                }
                            }
                            KeyCode::Backspace => {
                                query.pop();
                                cursor_idx = 0;
                            }
                            KeyCode::Char(c) if !ctrl && c != ' ' => {
                                query.push(c);
                                cursor_idx = 0;
                            }
                            KeyCode::Enter => return Ok(()),
                            KeyCode::Esc => {
                                selected.clear();
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    Event::Resize(c, l) => {
                        cols = c;
                        lines = l;
                    }
                    _ => {}
                },
                Mode::DeleteConfirm { index } => match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                            if let Some(name) = filtered.get(*index).cloned() {
                                store.delete(&name);
                                store.save()?;
                                selected.retain(|t| t != &name);
                            }
                            mode = Mode::Browse;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            mode = Mode::Browse;
                        }
                        _ => {}
                    },
                    _ => {}
                },
                Mode::EditName { index, text } => match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        match key.code {
                            KeyCode::Enter => {
                                let new = text.trim().to_string();
                                if let Some(old) = filtered.get(*index).cloned() {
                                    if !new.is_empty() && new != old {
                                        store.rename(&old, &new);
                                        store.save()?;
                                        if let Some(pos) = selected.iter().position(|t| t == &old) {
                                            selected[pos] = new.clone();
                                        }
                                    }
                                }
                                mode = Mode::Browse;
                            }
                            KeyCode::Esc => mode = Mode::Browse,
                            KeyCode::Backspace => {
                                text.pop();
                            }
                            KeyCode::Char('c') if ctrl => mode = Mode::Browse,
                            KeyCode::Char(c) if !ctrl => text.push(c),
                            _ => {}
                        }
                    }
                    _ => {}
                },
            }
        }
    })();

    let _ = execute!(stdout, cursor::Hide);
    result
}

/// Geometry of the popup overlay. Returns (x, y, w, h) of the box.
fn popup_geom(cols: u16, lines: u16) -> (u16, u16, u16, u16) {
    // 50% of the screen, centered. The prompt list behind stays visible.
    let w = (cols / 2).max(40).min(cols);
    let h = (lines / 2).max(10).min(lines);
    let x = (cols.saturating_sub(w)) / 2;
    let y = (lines.saturating_sub(h)) / 2;
    (x, y, w, h)
}

/// Height (in rows) available for the tag list inside the popup.
fn popup_list_height(lines: u16) -> usize {
    let (_, _, _, h) = popup_geom(u16::MAX, lines);
    // inner = h-2 (border), minus title/search/sep (3) and footer (1)
    (h as usize).saturating_sub(2).saturating_sub(3).saturating_sub(1).max(1)
}

fn render_popup<W: Write>(
    stdout: &mut W,
    store: &TagStore,
    filtered: &[String],
    query: &str,
    selected: &[String],
    cursor_idx: usize,
    top: &mut usize,
    cols: u16,
    lines: u16,
    mode: &Mode,
) -> Result<(), TagsError> {
    let (bx, by, bw, bh) = popup_geom(cols, lines);
    let inner_x = bx + 1;
    let inner_y = by + 1;
    let inner_w = bw.saturating_sub(2) as usize;
    let inner_h = bh.saturating_sub(2) as usize;
    let list_h = popup_list_height(lines);

    // Adjust scroll so the cursor stays visible.
    if filtered.is_empty() {
        *top = 0;
    } else {
        if cursor_idx < *top {
            *top = cursor_idx;
        }
        if cursor_idx >= *top + list_h && list_h > 0 {
            *top = cursor_idx + 1 - list_h;
        }
        if *top > 0 && *top + list_h > filtered.len() {
            *top = filtered.len().saturating_sub(list_h);
        }
    }

    // Title row
    let title = format!("Filter by tags · {} selected", selected.len());
    queue!(
        stdout,
        cursor::MoveTo(inner_x, inner_y),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print(truncate_pad(&title, inner_w)),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;

    // Search row
    let search_label = "Search: ";
    queue!(
        stdout,
        cursor::MoveTo(inner_x, inner_y + 1),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print(search_label),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::White),
        Print(truncate_pad(query, inner_w.saturating_sub(search_label.chars().count()))),
        ResetColor
    )?;

    // Separator
    queue!(
        stdout,
        cursor::MoveTo(inner_x, inner_y + 2),
        SetForegroundColor(Color::DarkGrey),
        Print(&"─".repeat(inner_w)),
        ResetColor
    )?;

    // Tag list
    if filtered.is_empty() {
        queue!(
            stdout,
            cursor::MoveTo(inner_x, inner_y + 3),
            SetForegroundColor(Color::DarkGrey),
            Print(truncate_pad("(no tags)", inner_w)),
            ResetColor
        )?;
    } else {
        for (i, name) in filtered.iter().enumerate().skip(*top).take(list_h) {
            let ry = inner_y + 3 + (i - *top) as u16;
            let count = store.count_for(name);
            let is_selected_tag = selected.iter().any(|t| t == name);
            let check = if is_selected_tag { "[x]" } else { "[ ]" };
            let line = format!("{} {} ({})", check, name, count);

            queue!(stdout, cursor::MoveTo(inner_x, ry))?;
            if i == cursor_idx {
                queue!(
                    stdout,
                    SetBackgroundColor(Color::White),
                    SetForegroundColor(Color::Black),
                    SetAttribute(Attribute::Bold),
                    Print(truncate_pad(&line, inner_w)),
                    ResetColor,
                    SetAttribute(Attribute::Reset)
                )?;
            } else {
                // Print the colored segments, then pad the rest of the inner
                // width with spaces so leftover content from the list behind
                // is overwritten (without touching the border columns).
                let prefix = format!("{} {}", check, name);
                let count_str = format!("({})", count);
                let used = prefix.chars().count() + 1 + count_str.chars().count();
                let pad = inner_w.saturating_sub(used);
                queue!(
                    stdout,
                    SetForegroundColor(Color::White),
                    Print(&prefix),
                    Print(" "),
                    SetForegroundColor(Color::DarkGrey),
                    Print(&count_str),
                    ResetColor,
                    Print(&" ".repeat(pad))
                )?;
            }
        }
    }

    // Footer
    let footer = "↑/↓ navigate · Space toggle · Enter confirm · Esc clear+close · Ctrl+D del · Ctrl+E rename";
    let footer_y = inner_y + inner_h as u16 - 1;
    queue!(
        stdout,
        cursor::MoveTo(inner_x, footer_y),
        SetForegroundColor(Color::DarkGrey),
        Print(truncate_pad(footer, inner_w)),
        ResetColor
    )?;

    // Border — drawn last so it is never wiped by content rows above.
    draw_border(stdout, bx, by, bw, bh)?;


    // Cursor / modal overlay
    match mode {
        Mode::Browse => {
            let x = inner_x + search_label.chars().count() as u16 + query.chars().count() as u16;
            let y = inner_y + 1;
            queue!(stdout, cursor::MoveTo(x, y))?;
        }
        Mode::DeleteConfirm { index } => {
            let name = filtered.get(*index).cloned().unwrap_or_default();
            let msg = format!(" Delete tag '{}'? [y/n] ", name);
            draw_popup_modal(stdout, &msg, cols, lines)?;
        }
        Mode::EditName { text, .. } => {
            let label = " Rename to: ";
            let msg = format!("{}{} ", label, text);
            draw_bordered_popup_modal(stdout, &msg, cols, lines)?;
            let y = lines / 2;
            let w = msg.chars().count() as u16;
            let x = (cols.saturating_sub(w)) / 2 + label.chars().count() as u16
                + text.chars().count() as u16;
            queue!(stdout, cursor::MoveTo(x, y))?;
        }
    }

    Ok(())
}

fn draw_border<W: Write>(
    stdout: &mut W,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
) -> Result<(), TagsError> {
    if w < 2 || h < 2 {
        return Ok(());
    }
    let top_row = "─".repeat((w as usize).saturating_sub(2));
    let bot_row = top_row.clone();
    queue!(
        stdout,
        cursor::MoveTo(x, y),
        SetForegroundColor(Color::Cyan),
        Print("┌"),
        Print(&top_row),
        Print("┐"),
        ResetColor
    )?;
    for ry in 1..(h as u16).saturating_sub(1) {
        queue!(
            stdout,
            cursor::MoveTo(x, y + ry),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            cursor::MoveTo(x + w - 1, y + ry),
            Print("│"),
            ResetColor
        )?;
    }
    queue!(
        stdout,
        cursor::MoveTo(x, y + h - 1),
        SetForegroundColor(Color::Cyan),
        Print("└"),
        Print(&bot_row),
        Print("┘"),
        ResetColor
    )?;
    Ok(())
}

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

fn draw_popup_modal<W: Write>(
    stdout: &mut W,
    msg: &str,
    cols: u16,
    lines: u16,
) -> Result<(), TagsError> {
    let y = lines / 2;
    let w = msg.chars().count() as u16;
    let x = (cols.saturating_sub(w)) / 2;
    queue!(
        stdout,
        cursor::MoveTo(x.saturating_sub(1), y.saturating_sub(1)),
        SetBackgroundColor(Color::DarkGrey),
        SetForegroundColor(Color::White),
        Print(&" ".repeat((w + 2) as usize)),
        cursor::MoveTo(x.saturating_sub(1), y),
        Print(" "),
        SetAttribute(Attribute::Bold),
        Print(msg),
        SetAttribute(Attribute::Reset),
        Print(" "),
        cursor::MoveTo(x.saturating_sub(1), y + 1),
        Print(&" ".repeat((w + 2) as usize)),
        ResetColor
    )?;
    Ok(())
}

/// Like `draw_popup_modal` but without a background color, wrapped in a
/// border drawn with `draw_border`. The message sits centered on the
/// middle row inside a 3-row-tall, (msg+2)-wide box.
fn draw_bordered_popup_modal<W: Write>(
    stdout: &mut W,
    msg: &str,
    cols: u16,
    lines: u16,
) -> Result<(), TagsError> {
    let y = lines / 2;
    let w = msg.chars().count() as u16;
    let x = (cols.saturating_sub(w)) / 2;
    // Box: 1 row above the message, the message row, 1 row below => height 3.
    // Width is msg + 2 (one padding column on each side).
    let bx = x.saturating_sub(1);
    let by = y.saturating_sub(1);
    let bw = w + 2;
    let bh = 3;
    draw_border(stdout, bx, by, bw, bh)?;
    queue!(
        stdout,
        cursor::MoveTo(x, y),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print(msg),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    Ok(())
}
