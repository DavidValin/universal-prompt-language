// Unit tests for the per-prompt tags TUI helpers.

use upl::manager::ui_prompt_tags::input_focus_has_text;

#[test]
fn input_focus_helper() {
    assert!(!input_focus_has_text(""));
    assert!(input_focus_has_text("a"));
}
