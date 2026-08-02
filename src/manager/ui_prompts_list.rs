// prompt_list.rs
//
// `list` subcommand: scans a folder for UPL prompt files, parses each one,
// and renders an interactive TUI table (id | title | params) with a header
// row. The user navigates with Up/Down and presses Enter to build the
// selected prompt via the existing PromptBuilder flow.

use std::fs;
use std::io::{self, Read, Write};
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

use crate::upl::builder::PromptBuilder;
use crate::upl::parser::{Prompt, PromptParser};
use crate::manager::{ui_prompt_tags, ui_tags};
use crate::editor::ui_prompt_editor;

#[derive(Error, Debug)]
pub enum ListError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("parse error in '{path}': {msg}")]
    Parse { path: String, msg: String },
    #[error("no prompt files found in '{0}'")]
    NoPrompts(String),
    #[error("TUI: {0}")]
    Tui(String),
    #[error("build error: {0}")]
    Build(String),
    #[error("tags: {0}")]
    Tags(#[from] crate::manager::ui_tags::TagsError),
}

/// One row in the list.
#[derive(Clone)]
struct Row {
    path: PathBuf,
    name: String,
    title: String,
    params: usize,
    sha256: String,
    repository: String,
}

/// Read and parse a prompt file.
fn load_prompt(path: &Path) -> Result<(Prompt, String), ListError> {
    let mut file = fs::File::open(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let prompt = PromptParser::parse(&content).map_err(|e| ListError::Parse {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;
    // RFC §2: the file must use the `.txt`/`.upl` extension and its base
    // name must equal the prompt's `name` field.
    crate::upl::parser::validate_prompt_file(&prompt, path).map_err(|e| ListError::Parse {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;
    Ok((prompt, content))
}

/// Resolve the folder argument: explicit > ~/.upl/prompts.
pub fn resolve_folder(folder: Option<&str>) -> Result<PathBuf, ListError> {
    if let Some(f) = folder {
        return Ok(PathBuf::from(f));
    }
    let home = std::env::var("HOME").map_err(|_| {
        ListError::Io(io::Error::new(io::ErrorKind::NotFound, "HOME not set"))
    })?;
    Ok(PathBuf::from(home).join(".upl").join("prompts"))
}

/// Collect every `.txt` or `.upl` file in `folder` (recursively), parse each,
/// and return the rows that parsed successfully.
///
/// Top-level files are treated as locally-authored prompts (repository =
/// "none"). Files nested under `<host>/<user>/<name>.txt` (the layout used by
/// `upl pull`) are tagged with the `<host>` as their repository.
fn collect_rows(folder: &Path) -> Result<Vec<Row>, ListError> {
    let mut rows = Vec::new();
    let mut first_error: Option<ListError> = None;
    collect_rows_recursive(folder, folder, &mut rows, &mut first_error)?;

    rows.sort_by(|a, b| a.name.cmp(&b.name));

    if rows.is_empty() {
        return Err(first_error.unwrap_or_else(|| {
            ListError::NoPrompts(folder.display().to_string())
        }));
    }
    Ok(rows)
}

/// Recursive helper. `root` is the top-level prompts folder; `dir` is the
/// directory currently being scanned. The repository label is derived from
/// the path components between `root` and the file: if the file is directly
/// in `root`, the label is "none"; otherwise the first component (the host)
/// is used.
fn collect_rows_recursive(
    root: &Path,
    dir: &Path,
    rows: &mut Vec<Row>,
    first_error: &mut Option<ListError>,
) -> Result<(), ListError> {
    let entries = fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rows_recursive(root, &path, rows, first_error)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let is_prompt = crate::upl::parser::has_valid_extension(&path);
        if !is_prompt {
            continue;
        }
        // Derive the repository label from the path relative to root.
        // Top-level file  -> "none"
        // <host>/<user>/file -> "<host>"
        let repository = match path.strip_prefix(root) {
            Ok(rel) if rel.components().count() > 1 => {
                rel.components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str())
                    .unwrap_or("none")
                    .to_string()
            }
            _ => "none".to_string(),
        };
        match load_prompt(&path) {
            Ok((prompt, content)) => rows.push(Row {
                path: path.clone(),
                name: prompt.name,
                title: prompt
                    .title
                    .clone()
                    .unwrap_or_else(|| path.display().to_string()),
                params: prompt.variable_definitions.len(),
                sha256: ui_tags::sha256(&content),
                repository,
            }),
            Err(e) => {
                if first_error.is_none() {
                    *first_error = Some(e);
                }
            }
        }
    }
    Ok(())
}

// Layout constants.
pub const INDENT: usize = 1; // left margin for all content
pub const TRAILING: usize = 1; // right margin for all content
pub const SEP: &str = " | "; // column separator
pub const W_TAGS: usize = 5; // fixed width for the tags counter column
const W_PARAMS: usize = 7; // fixed width for the params counter column
const W_REPO: usize = 20; // fixed width for the repository column
const W_NAME_MIN: usize = 8; // minimum width reserved for the name column
pub const W_NAME_MAX: usize = 40; // maximum width the name column can grow to

/// A computed column layout that always fits inside `total_width`.
pub struct Layout {
    pub w_name: usize,
    pub w_title: usize,
    pub w_tags: usize,
    pub w_params: usize,
    pub w_repo: usize,
}

/// Compute the column widths so the five columns plus separators exactly
/// fill `total_width`. Title has width priority: it gets the largest share
/// of the remaining space after reserving a minimum for name and fixed chunks
/// for tags, params and repository.
pub fn compute_layout(total_width: usize, longest_title: usize) -> Layout {
    // content width after the left indent and right trailing margin
    let content = total_width.saturating_sub(INDENT).saturating_sub(TRAILING);
    let sep_width = SEP.chars().count() * 4; // four separators between five cols
    let avail = content
        .saturating_sub(sep_width)
        .saturating_sub(W_TAGS)
        .saturating_sub(W_PARAMS)
        .saturating_sub(W_REPO);

    // Title gets priority: as much as it needs (capped at avail - W_NAME_MIN).
    let w_title_base = longest_title.min(avail.saturating_sub(W_NAME_MIN)).max(1);
    let w_name_raw = avail.saturating_sub(w_title_base).max(W_NAME_MIN);
    // Cap the name column at W_NAME_MAX and hand any leftover back to title.
    let w_name = w_name_raw.min(W_NAME_MAX);
    let w_title = w_title_base + (w_name_raw - w_name);

    // Safety clamp: on very narrow terminals the fixed columns plus the name
    // minimum can exceed the content width. Shrink title first, then name, so
    // the rendered line never overflows.
    let total_cols = w_name + w_title + W_TAGS + W_PARAMS + W_REPO + sep_width;
    let (w_name, w_title) = if total_cols > content {
        let excess = total_cols - content;
        if w_title >= excess {
            (w_name, w_title - excess)
        } else {
            (w_name.saturating_sub(excess - w_title), 0)
        }
    } else {
        (w_name, w_title)
    };

    Layout {
        w_name,
        w_title,
        w_tags: W_TAGS,
        w_params: W_PARAMS,
        w_repo: W_REPO,
    }
}

pub fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else if w <= 3 {
        s.chars().take(w).collect()
    } else {
        let kept: String = s.chars().take(w - 3).collect();
        format!("{kept}...")
    }
}

pub fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

fn header_line(layout: &Layout) -> String {
    format!(
        "{}{SEP}{}{SEP}{}{SEP}{}{SEP}{} ",
        pad("NAME", layout.w_name),
        pad("TITLE", layout.w_title),
        pad("TAGS", layout.w_tags),
        pad("PARAMS", layout.w_params),
        pad("REPOSITORY", layout.w_repo),
    )
}

fn separator_line(layout: &Layout) -> String {
    let dash = |w: usize| "-".repeat(w);
    format!(
        "{}{SEP}{}{SEP}{}{SEP}{}{SEP}{} ",
        dash(layout.w_name),
        dash(layout.w_title),
        dash(layout.w_tags),
        dash(layout.w_params),
        dash(layout.w_repo),
    )
}

fn row_line(row: &Row, tags_count: usize, layout: &Layout) -> String {
    format!(
        "{}{SEP}{}{SEP}{}{SEP}{}{SEP}{} ",
        pad(&truncate(&row.name, layout.w_name), layout.w_name),
        pad(&truncate(&row.title, layout.w_title), layout.w_title),
        pad(&tags_count.to_string(), layout.w_tags),
        pad(&row.params.to_string(), layout.w_params),
        pad(&truncate(&row.repository, layout.w_repo), layout.w_repo),
    )
}

/// Render the full prompt list (header, body, optional selected-tags bar,
/// footer) and clamp the cursor/scroll offset to the filtered list.
///
/// Returns `(filtered_len, body_height)` so the caller can use them for input
/// handling. All I/O errors propagate as `io::Error` so both `ListError` and
/// `TagsError` can convert them via their `From<io::Error>` impls.
fn render_list<W: Write>(
    stdout: &mut W,
    rows: &[Row],
    store: &ui_tags::TagStore,
    selected_tags: &[String],
    selected: &mut usize,
    top: &mut usize,
    longest_title: usize,
    cols: u16,
    lines: u16,
) -> io::Result<(usize, usize)> {
    // Filter rows by selected tags (AND: a row matches only if it has every
    // selected tag).
    let filtered: Vec<&Row> = if selected_tags.is_empty() {
        rows.iter().collect()
    } else {
        rows.iter()
            .filter(|r| selected_tags.iter().all(|t| store.tag_has(t, &r.sha256)))
            .collect()
    };

    let layout = compute_layout(cols as usize, longest_title);
    let header_y = 0u16;
    let separator_y = header_y + 1;
    let body_start = header_y + 2;
    let footer_y = lines.saturating_sub(1);
    // When a tag filter is active, a selected-tags bar is drawn on top of the
    // footer, shrinking the body by one row.
    let tags_bar_y = if selected_tags.is_empty() {
        None
    } else {
        Some(footer_y.saturating_sub(1))
    };
    let body_bottom = tags_bar_y.unwrap_or(footer_y);
    let body_height = (body_bottom as usize).saturating_sub(body_start as usize);

    // Keep the cursor inside the filtered list.
    if filtered.is_empty() {
        *selected = 0;
        *top = 0;
    } else {
        if *selected >= filtered.len() {
            *selected = filtered.len() - 1;
        }
        if *selected < *top {
            *top = *selected;
        }
        if *selected >= *top + body_height && body_height > 0 {
            *top = *selected + 1 - body_height;
        }
        if *top > 0 && *top + body_height > filtered.len() {
            *top = filtered.len().saturating_sub(body_height);
        }
    }

    // Render
    queue! {
        stdout,
        Clear(ClearType::All),
        cursor::MoveTo(INDENT as u16, header_y),
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print(header_line(&layout)),
        ResetColor,
        SetAttribute(Attribute::Reset),
        cursor::MoveTo(INDENT as u16, separator_y),
        SetForegroundColor(Color::DarkGrey),
        Print(separator_line(&layout)),
        ResetColor,
    }?;

    if filtered.is_empty() {
        queue!(
            stdout,
            cursor::MoveTo(INDENT as u16, body_start),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(Color::DarkGrey),
            Print("(no results)"),
            ResetColor,
        )?;
    }

    for (i, row) in filtered.iter().copied().enumerate().skip(*top).take(body_height) {
        let y = body_start + (i - *top) as u16;
        let tags_count = store.tags_for_prompt(&row.sha256).len();
        let line = row_line(row, tags_count, &layout);
        queue!(
            stdout,
            cursor::MoveTo(INDENT as u16, y),
            Clear(ClearType::CurrentLine),
        )?;
        if i == *selected {
            queue!(
                stdout,
                SetBackgroundColor(Color::White),
                SetForegroundColor(Color::Black),
                SetAttribute(Attribute::Bold),
                Print(&line),
                ResetColor,
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(Color::White),
                Print(&line),
                ResetColor,
            )?;
        }
    }

    // Selected-tags bar (drawn on top of the footer when filtering).
    if let Some(ty) = tags_bar_y {
        let bar = format!("Tags: {}", selected_tags.join(", "));
        let w = (cols as usize).saturating_sub(INDENT * 2);
        queue!(
            stdout,
            cursor::MoveTo(INDENT as u16, ty),
            Clear(ClearType::CurrentLine),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            SetAttribute(Attribute::Bold),
            Print(pad(&truncate(&bar, w), w)),
            ResetColor,
            SetAttribute(Attribute::Reset),
        )?;
    }

    // Footer
    let instructions = format!(
        "{} prompts · ↑/↓ navigate · Enter build · e edit · n new · t filter tags · Ctrl+T prompt tags · Ctrl+R UPL Help · q/Esc quit",
        filtered.len()
    );
    queue!(
        stdout,
        cursor::MoveTo(INDENT as u16, footer_y),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(instructions),
        ResetColor,
        cursor::MoveTo(0, footer_y),
    )?;

    Ok((filtered.len(), body_height))
}

/// Outcome of the interactive list TUI.
enum TuiOutcome {
    /// The user selected a prompt to build.
    Selected(Row),
    /// The user quit without selecting.
    Quit,
    /// The editor saved a prompt; the list should be re-scanned.
    Reload,
}

/// Run the interactive list TUI. Returns the path of the prompt the user
/// selected with Enter, or `None` if they quit without selecting.
fn run_tui(rows: &[Row]) -> Result<TuiOutcome, ListError> {
    // Render the TUI on stderr (not stdout) so that stdout stays clean for
    // the final rendered prompt, allowing `upl list > out.txt` to capture
    // the build while the UI is still visible on the terminal.
    let mut stdout = io::stderr();
    execute!(
        stdout,
        EnterAlternateScreen,
        DisableLineWrap,
        cursor::Hide
    )
    .map_err(|e| ListError::Tui(e.to_string()))?;
    terminal::enable_raw_mode().map_err(|e| ListError::Tui(e.to_string()))?;

    let longest_title = rows
        .iter()
        .map(|r| r.title.chars().count())
        .max()
        .unwrap_or(0)
        .max("TITLE".len());

    let mut selected: usize = 0;
    let mut top: usize = 0; // first visible row (scroll offset)
    let mut selected_tags: Vec<String> = Vec::new();
    let mut store = ui_tags::TagStore::load()?;
    let (mut cols, mut lines) =
        terminal::size().map_err(|e| ListError::Tui(e.to_string()))?;

    let result = (|| -> Result<TuiOutcome, ListError> {
        loop {
            let (filtered_len, body_height) = render_list(
                &mut stdout,
                rows,
                &store,
                &selected_tags,
                &mut selected,
                &mut top,
                longest_title,
                cols,
                lines,
            )?;

            stdout.flush().map_err(|e| ListError::Tui(e.to_string()))?;

            // Input
            match event::read().map_err(|e| ListError::Tui(e.to_string()))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if selected + 1 < filtered_len {
                            selected += 1;
                        }
                    }
                    KeyCode::Home => selected = 0,
                    KeyCode::End => selected = filtered_len.saturating_sub(1),
                    KeyCode::PageUp => {
                        selected = selected.saturating_sub(body_height.max(1));
                    }
                    KeyCode::PageDown => {
                        selected = (selected + body_height.max(1))
                            .min(filtered_len.saturating_sub(1).max(selected));
                    }
                    KeyCode::Enter => {
                        // Re-derive the selected row from the filtered list.
                        let filtered: Vec<&Row> = if selected_tags.is_empty() {
                            rows.iter().collect()
                        } else {
                            rows.iter()
                                .filter(|r| selected_tags.iter().all(|t| store.tag_has(t, &r.sha256)))
                                .collect()
                        };
                        if let Some(row) = filtered.get(selected) {
                            return Ok(TuiOutcome::Selected((*row).clone()));
                        }
                    }
                    KeyCode::Char('e') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Open the editor on the selected prompt. The editor
                        // reuses the alternate screen + raw mode already
                        // active here.
                        let filtered: Vec<&Row> = if selected_tags.is_empty() {
                            rows.iter().collect()
                        } else {
                            rows.iter()
                                .filter(|r| selected_tags.iter().all(|t| store.tag_has(t, &r.sha256)))
                                .collect()
                        };
                        if let Some(row) = filtered.get(selected) {
                            let _ = execute!(stdout, cursor::Show);
                            let saved = ui_prompt_editor::run_editor(&row.path)
                                .map_err(|e| ListError::Tui(e.to_string()))?;
                            let _ = execute!(stdout, cursor::Hide);
                            if saved {
                                return Ok(TuiOutcome::Reload);
                            }
                        }
                    }
                    KeyCode::Char('n') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Create a new prompt: open the editor with the
                        // skeleton template. On save the editor writes the
                        // prompt to ~/.upl/prompts/<name>.txt; reload the
                        // list so the new file shows up.
                        let _ = execute!(stdout, cursor::Show);
                        let saved = ui_prompt_editor::run_editor_with_content(
                            ui_prompt_editor::SKELETON,
                        )
                        .map_err(|e| ListError::Tui(e.to_string()))?;
                        let _ = execute!(stdout, cursor::Hide);
                        if saved {
                            return Ok(TuiOutcome::Reload);
                        }
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Open the UPL Help popup (RFC reference), then
                        // restore the list cursor + repaint on return.
                        let _ = execute!(stdout, cursor::Show);
                        ui_prompt_editor::show_rfc_popup(&mut stdout)
                            .map_err(|e| ListError::Tui(e.to_string()))?;
                        let _ = execute!(stdout, cursor::Hide);
                    }
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                        return Ok(TuiOutcome::Quit);
                    }
                    KeyCode::Char('t') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Open the tag-filter popup. The closure re-renders
                        // the prompt list behind the popup on every frame so
                        // the filtered list updates the instant a tag is
                        // toggled.
                        let _ = execute!(stdout, cursor::Show);
                        let valid_set: std::collections::HashSet<String> =
                            rows.iter().map(|r| r.sha256.clone()).collect();
                        if store.prune(&valid_set) {
                            store.save()?;
                        }
                        ui_tags::run_popup(&mut stdout, &mut selected_tags, &mut store, |out, sel, st, c, l| {
                            render_list(
                                out, rows, st, sel, &mut selected, &mut top,
                                longest_title, c, l,
                            )?;
                            Ok(())
                        })?;
                        let _ = execute!(stdout, cursor::Hide);
                    }
                    KeyCode::Char('t') | KeyCode::Char('T')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        // Open the per-prompt tags view for the selected row.
                        let filtered: Vec<&Row> = if selected_tags.is_empty() {
                            rows.iter().collect()
                        } else {
                            rows.iter()
                                .filter(|r| selected_tags.iter().all(|t| store.tag_has(t, &r.sha256)))
                                .collect()
                        };
                        if let Some(row) = filtered.get(selected) {
                            let _ = execute!(stdout, cursor::Show);
                            ui_prompt_tags::run_tui(
                                &mut stdout,
                                &row.sha256,
                                &row.name,
                                &row.title,
                            )?;
                            let _ = execute!(stdout, cursor::Hide);
                            // Tags may have changed; reload the store.
                            store = ui_tags::TagStore::load()?;
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

    // Restore terminal regardless of outcome, but stay in the alternate
    // screen so the caller can draw the build header without flickering
    // (leaving and re-entering the alt screen would flash the user's
    // original terminal content). The caller is responsible for leaving
    // the alternate screen.
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        stdout,
        cursor::Show,
        EnableLineWrap,
    );

    result
}

/// Entry point for the `list` subcommand. Lists prompts in `folder` (or
/// `~/.upl/prompts` if `None`), lets the user pick one, and builds it.
pub fn run(folder: Option<&str>) -> Result<(), ListError> {
    let folder = resolve_folder(folder)?;
    let mut rows = collect_rows(&folder)?;

    loop {
        // run_tui enters the alternate screen and leaves it active on
        // return (whether or not a selection was made) so we can transition
        // straight into the build header without flickering. We must leave
        // the alt screen on every exit path from this point on.
        let choice = run_tui(&rows);
        let choice = match choice {
            Ok(TuiOutcome::Selected(c)) => c,
            Ok(TuiOutcome::Quit) => {
                let _ = execute!(io::stderr(), LeaveAlternateScreen);
                return Ok(());
            }
            Ok(TuiOutcome::Reload) => {
                // The editor saved a prompt; rescan the folder so the new
                // content (and any newly-created file) shows up.
                rows = collect_rows(&folder)?;
                continue;
            }
            Err(e) => {
                let _ = execute!(io::stderr(), LeaveAlternateScreen);
                return Err(e);
            }
        };

        // Build the chosen prompt via the existing flow.
        let (prompt, _) = load_prompt(&choice.path)?;

        // Announce the build with a styled header on stderr so it never
        // pollutes the rendered prompt body that downstream programs consume
        // via stdout. We're already in the alternate screen from run_tui;
        // just clear it and move the cursor to the top.
        let mut err = io::stderr();
        let _ = execute!(err, Clear(ClearType::All), cursor::MoveTo(0, 0));
        let _ = execute!(
            err,
            SetBackgroundColor(Color::DarkGreen),
            SetForegroundColor(Color::Black),
            SetAttribute(Attribute::Bold),
            Print(" Building prompt "),
            ResetColor,
            SetAttribute(Attribute::Reset),
            Print(" "),
            SetForegroundColor(Color::Yellow),
            Print(&choice.title),
            ResetColor,
            Print("\n\n"),
        );
        let _ = err.flush();

        let rendered = PromptBuilder::new(prompt).build_interactive();
        match rendered {
            // Cancelled (Esc / Ctrl+C): go back to the prompt list.
            Err(crate::upl::builder::BuilderError::Cancelled) => {
                continue;
            }
            Err(e) => {
                let _ = execute!(err, LeaveAlternateScreen);
                let _ = err.flush();
                return Err(ListError::Build(e.to_string()));
            }
            Ok(rendered) => {
                let _ = execute!(err, LeaveAlternateScreen);
                let _ = err.flush();
                let mut out = io::stdout();
                let _ = out.write_all(rendered.as_bytes());
                let _ = out.write_all(b"\n");
                let _ = out.flush();
                return Ok(());
            }
        }
    }
}
