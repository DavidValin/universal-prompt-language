// prompt_tags.rs
//
// Per-prompt tag management view. Opened from the prompt list with Ctrl+T.
//
// Layout (top to bottom):
//   - identification of the selected prompt (id, title, sha256)
//   - the full list of tags in the store as a checklist: [x] = attached to
//     this prompt, [ ] = not attached. Navigable.
//   - a "New Tag:" input field (typing + Enter creates & attaches the tag)
//
// Keys:
//   ↑/Down   navigate the tag list
//   Space   toggle attachment of the selected tag
//   n       start entering a new tag (then type, Enter to save & attach,
//            Esc to cancel before saving)
//   Del      disassociate the selected tag from this prompt
//   q/Esc    back to the prompt list (or cancel new-tag input if active)

use std::io::Write;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
    terminal::{self, Clear, ClearType},
};

use crate::manager::ui_tags::{TagStore, TagsError};

const INDENT: u16 = 2;

/// Run the prompt-tags view inside an already-established alternate screen +
/// raw mode session.
pub fn run_tui<W: Write>(
    stdout: &mut W,
    sha256: &str,
    prompt_id: &str,
    prompt_title: &str,
) -> Result<(), TagsError> {
    let mut store = TagStore::load()?;

    let mut input: String = String::new();
    let mut input_mode: bool = false;
    let mut selected: usize = 0;
    let (mut cols, mut lines) =
        terminal::size().map_err(|e| TagsError::Tui(e.to_string()))?;

    execute!(stdout, cursor::Show).map_err(|e| TagsError::Tui(e.to_string()))?;

    let result = (|| -> Result<(), TagsError> {
        loop {
            let all_tags = store.tag_names_sorted();
            if all_tags.is_empty() {
                selected = 0;
            } else if selected >= all_tags.len() {
                selected = all_tags.len() - 1;
            }

            render(stdout, &store, &all_tags, &sha256, prompt_id, prompt_title, &input, input_mode, selected, cols, lines)?;

            stdout.flush().map_err(|e| TagsError::Tui(e.to_string()))?;

            match event::read().map_err(|e| TagsError::Tui(e.to_string()))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if selected + 1 < all_tags.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Home => selected = 0,
                    KeyCode::End => selected = all_tags.len().saturating_sub(1),
                    KeyCode::Esc => {
                        if input_mode {
                            // Cancel new-tag input without saving.
                            input_mode = false;
                            input.clear();
                        } else {
                            return Ok(());
                        }
                    }
                    KeyCode::Char('q') | KeyCode::Char('Q') if !input_mode => {
                        return Ok(());
                    }
                    // Behaviour while entering a new tag:
                    KeyCode::Backspace if input_mode => {
                        input.pop();
                    }
                    KeyCode::Enter if input_mode => {
                        let name = input.trim().to_string();
                        if !name.is_empty() {
                            store.ensure_tag(&name);
                            store.associate(&name, &sha256);
                            store.save()?;
                            input.clear();
                            input_mode = false;
                            selected = store
                                .tag_names_sorted()
                                .iter()
                                .position(|t| t == &name)
                                .unwrap_or(selected);
                        } else {
                            input_mode = false;
                            input.clear();
                        }
                    }
                    KeyCode::Char(c) if input_mode => {
                        input.push(c);
                    }
                    // Behaviour while navigating the tag list (not in input mode):
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        input_mode = true;
                        input.clear();
                    }
                    KeyCode::Char(' ') if !input_mode && !all_tags.is_empty() => {
                        if let Some(name) = all_tags.get(selected).cloned() {
                            if store.tag_has(&name, &sha256) {
                                store.disassociate(&name, &sha256);
                            } else {
                                store.ensure_tag(&name);
                                store.associate(&name, &sha256);
                            }
                            store.save()?;
                        }
                    }
                    KeyCode::Delete | KeyCode::Backspace
                        if !input_mode && !all_tags.is_empty() =>
                    {
                        if let Some(name) = all_tags.get(selected).cloned() {
                            store.disassociate(&name, &sha256);
                            store.save()?;
                        }
                    }
                    _ => {}
                },
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

pub fn input_focus_has_text(s: &str) -> bool {
    !s.is_empty()
}

fn render<W: Write>(
    stdout: &mut W,
    store: &TagStore,
    all_tags: &[String],
    sha256: &str,
    prompt_id: &str,
    prompt_title: &str,
    input: &str,
    input_mode: bool,
    selected: usize,
    cols: u16,
    lines: u16,
) -> Result<(), TagsError> {
    queue!(stdout, Clear(ClearType::All))?;

    // Header: prompt identification
    let mut y: u16 = 0;
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print("Prompt Tags"),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    y += 1;
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print("ID: "),
        SetAttribute(Attribute::Reset),
        Print(prompt_id),
        ResetColor
    )?;
    y += 1;
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print("Title: "),
        SetAttribute(Attribute::Reset),
        Print(prompt_title),
        ResetColor
    )?;
    y += 1;
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetForegroundColor(Color::DarkGrey),
        Print(format!("sha256: {}", sha256)),
        ResetColor
    )?;
    y += 1;

    // Separator
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(cols.saturating_sub(INDENT as u16 * 2) as usize)),
        ResetColor
    )?;
    y += 1;

    // Tags list header
    let attached_count = all_tags
        .iter()
        .filter(|t| store.tag_has(t, &sha256))
        .count();
    queue!(
        stdout,
        cursor::MoveTo(INDENT, y),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Yellow),
        Print(format!(
            "Tags ({}/{})",
            attached_count,
            all_tags.len()
        )),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    y += 1;

    // Tags checklist
    let list_top = y;
    let available = lines.saturating_sub(list_top) as usize; // remaining lines including footer
    // Reserve one line for the input row only while entering a new tag; always
    // reserve one line for the footer.
    let reserved = if input_mode { 2 } else { 1 };
    let list_h = available.saturating_sub(reserved).max(1);

    let top = if selected < list_h {
        0
    } else {
        selected + 1 - list_h
    };

    if all_tags.is_empty() {
        queue!(
            stdout,
            cursor::MoveTo(INDENT, list_top),
            SetForegroundColor(Color::DarkGrey),
            Print("(no tags yet — press 'n' to add one)",),
            ResetColor
        )?;
    } else {
        for (i, name) in all_tags.iter().enumerate().skip(top).take(list_h) {
            let yy = list_top + (i - top) as u16;
            let mark = if store.tag_has(name, &sha256) { "x" } else { " " };
            let line = format!("[{}] {}", mark, name);
            queue!(stdout, cursor::MoveTo(INDENT, yy), Clear(ClearType::CurrentLine))?;
            if i == selected {
                queue!(
                    stdout,
                    SetBackgroundColor(Color::White),
                    SetForegroundColor(Color::Black),
                    SetAttribute(Attribute::Bold),
                    Print(&line),
                    ResetColor,
                    SetAttribute(Attribute::Reset)
                )?;
            } else {
                queue!(
                    stdout,
                    SetForegroundColor(Color::White),
                    Print(&line),
                    ResetColor
                )?;
            }
        }
    }

    // New Tag input — only shown once the user starts adding one (presses 'n').
    let input_y = list_top + list_h as u16;
    queue!(stdout, cursor::MoveTo(INDENT, input_y), Clear(ClearType::CurrentLine))?;
    if input_mode {
        queue!(
            stdout,
            SetAttribute(Attribute::Bold),
            SetForegroundColor(Color::Green),
            Print("New Tag: "),
            SetAttribute(Attribute::Reset),
            ResetColor,
            SetForegroundColor(Color::White),
            Print(input),
            ResetColor
        )?;
    }

    // Footer
    let footer_y = lines.saturating_sub(1);
    let footer = if input_mode {
        "type tag · Enter save & attach · Esc cancel"
    } else {
        "↑/↓ navigate · Space toggle · n new · Del remove · q/Esc back"
    };
    queue!(
        stdout,
        cursor::MoveTo(INDENT, footer_y),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(footer),
        ResetColor
    )?;

    // Position cursor at end of "New Tag:" input only while entering a tag.
    if input_mode {
        let x = INDENT + "New Tag: ".chars().count() as u16 + input.chars().count() as u16;
        queue!(stdout, cursor::MoveTo(x, input_y), cursor::Show)?;
    } else {
        queue!(stdout, cursor::Hide)?;
    }

    Ok(())
}
