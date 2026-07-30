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
//   Enter    toggle attachment of the selected tag (when input is empty),
//            or create + attach the tag typed in "New Tag:" (when non-empty)
//   Del      disassociate the selected tag from this prompt
//   q/Esc    back to the prompt list

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

            render(stdout, &store, &all_tags, &sha256, prompt_id, prompt_title, &input, selected, cols, lines)?;

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
                    KeyCode::Delete | KeyCode::Backspace
                        if !all_tags.is_empty() && !input_focus_has_text(&input) =>
                    {
                        // Del detaches the selected tag.
                        // Backspace only detaches when the input is empty,
                        // otherwise it edits the input.
                        if let Some(name) = all_tags.get(selected).cloned() {
                            store.disassociate(&name, &sha256);
                            store.save()?;
                        }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Enter => {
                        if input_focus_has_text(&input) {
                            let name = input.trim().to_string();
                            if !name.is_empty() {
                                store.ensure_tag(&name);
                                store.associate(&name, &sha256);
                                store.save()?;
                                input.clear();
                                selected = store
                                    .tag_names_sorted()
                                    .iter()
                                    .position(|t| t == &name)
                                    .unwrap_or(selected);
                            }
                        } else if let Some(name) = all_tags.get(selected).cloned() {
                            if store.tag_has(&name, &sha256) {
                                store.disassociate(&name, &sha256);
                            } else {
                                store.ensure_tag(&name);
                                store.associate(&name, &sha256);
                            }
                            store.save()?;
                        }
                    }
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                    KeyCode::Char(c) => {
                        input.push(c);
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
    let list_h = available.saturating_sub(2).max(1); // reserve 2 for input + footer

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
            Print("(no tags yet — type one below)"),
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

    // New Tag input
    let input_y = list_top + list_h as u16;
    queue!(
        stdout,
        cursor::MoveTo(INDENT, input_y),
        Clear(ClearType::CurrentLine),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Green),
        Print("New Tag: "),
        SetAttribute(Attribute::Reset),
        ResetColor,
        SetForegroundColor(Color::White),
        Print(input),
        ResetColor
    )?;

    // Footer
    let footer_y = lines.saturating_sub(1);
    let footer = "↑/↓ navigate · Enter toggle / add new · Del remove · q/Esc back";
    queue!(
        stdout,
        cursor::MoveTo(INDENT, footer_y),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(footer),
        ResetColor
    )?;

    // Position cursor at end of "New Tag:" input
    let x = INDENT + "New Tag: ".chars().count() as u16 + input.chars().count() as u16;
    queue!(stdout, cursor::MoveTo(x, input_y))?;

    Ok(())
}
