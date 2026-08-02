// Unit tests for the prompt list UI: layout computation, text helpers, and
// folder resolution.

use std::path::PathBuf;

use universal_prompt_language::manager::ui_prompts_list::{
    center, compute_layout, pad, resolve_folder, truncate, INDENT, SEP, TRAILING, W_NAME,
};

#[test]
fn pad_truncates_long_strings() {
    assert_eq!(pad("hello", 3), "hel");
}

#[test]
fn pad_pads_short_strings() {
    assert_eq!(pad("hi", 5), "hi   ");
}

#[test]
fn truncate_adds_ellipsis() {
    assert_eq!(truncate("abcdef", 4), "a...");
}

#[test]
fn truncate_keeps_short() {
    assert_eq!(truncate("abc", 5), "abc");
}

#[test]
fn center_pads_evenly() {
    assert_eq!(center("ab", 6), "  ab  ");
}

#[test]
fn center_extra_space_goes_right() {
    assert_eq!(center("ab", 5), " ab  ");
}

#[test]
fn center_truncates_long() {
    assert_eq!(center("abcdef", 3), "abc");
}

#[test]
fn resolve_folder_uses_explicit() {
    let p = resolve_folder(Some("/tmp/prompts")).unwrap();
    assert_eq!(p, PathBuf::from("/tmp/prompts"));
}

#[test]
fn layout_fits_total_width() {
    let layout = compute_layout(100, 50);
    let total = INDENT
        + layout.w_name
        + SEP.len()
        + layout.w_title
        + SEP.len()
        + layout.w_tags
        + SEP.len()
        + layout.w_params
        + SEP.len()
        + layout.w_repo
        + TRAILING;
    assert_eq!(total, 100);
}

#[test]
fn layout_name_is_fixed() {
    // The name column is always rendered fully visible up to W_NAME chars,
    // regardless of terminal width or title length.
    let layout = compute_layout(220, 10);
    assert_eq!(layout.w_name, W_NAME);
    let layout = compute_layout(100, 90);
    assert_eq!(layout.w_name, W_NAME);
    let total = INDENT
        + layout.w_name
        + SEP.len()
        + layout.w_title
        + SEP.len()
        + layout.w_tags
        + SEP.len()
        + layout.w_params
        + SEP.len()
        + layout.w_repo
        + TRAILING;
    assert_eq!(total, 100);
}

#[test]
fn layout_fills_full_width() {
    // The list is always responsive: regardless of how short the longest
    // title is, the rendered line spans the full terminal width.
    for &w in &[60usize, 100, 140, 220] {
        let layout = compute_layout(w, 1);
        let total = INDENT
            + layout.w_name
            + SEP.len()
            + layout.w_title
            + SEP.len()
            + layout.w_tags
            + SEP.len()
            + layout.w_params
            + SEP.len()
            + layout.w_repo
            + TRAILING;
        assert_eq!(total, w, "width {w} not filled");
    }
}

#[test]
fn layout_never_overflows_narrow_width() {
    // With five columns the fixed minimum (tags + params + repo +
    // separators + margins) is ~46 chars; pick a width just above it.
    let layout = compute_layout(50, 200);
    let total = INDENT
        + layout.w_name
        + SEP.len()
        + layout.w_title
        + SEP.len()
        + layout.w_tags
        + SEP.len()
        + layout.w_params
        + SEP.len()
        + layout.w_repo
        + TRAILING;
    assert!(total <= 50);
}
