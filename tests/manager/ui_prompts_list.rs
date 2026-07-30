// Unit tests for the prompt list UI: layout computation, text helpers, and
// folder resolution.

use std::path::PathBuf;

use universal_prompt_language::manager::ui_prompts_list::{
    compute_layout, pad, resolve_folder, truncate, INDENT, SEP, TRAILING, W_NAME_MAX,
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
fn layout_caps_name_at_max() {
    // Wide terminal with short titles: name is still capped at W_NAME_MAX,
    // and the leftover width goes to the title column.
    let layout = compute_layout(220, 10);
    assert_eq!(layout.w_name, W_NAME_MAX);
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
    assert_eq!(total, 220);
}

#[test]
fn layout_title_has_priority() {
    // Wide terminal, very long titles: title takes the lion's share.
    let layout = compute_layout(140, 90);
    assert!(layout.w_title > layout.w_name);
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
