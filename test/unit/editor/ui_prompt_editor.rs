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
fn block_bgs_ignore_escaped_tags() {
    // An escaped `\{{{for ...}}}` is literal text describing the syntax,
    // not a real loop opener, so it must not start a highlighted block.
    let body = "example: \\{{{for X in Y}}}\nplain line\n";
    let lines = chars(body);
    let bgs = compute_block_bgs(&lines);
    assert_eq!(bgs, vec![Block::None, Block::None, Block::None]);
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
fn escaped_placeholder_not_detected() {
    // RFC §4.5: `\[[[` renders as literal text, not a placeholder, so it
    // must not be highlighted as one — but a real placeholder later on the
    // same line still is.
    let line: Vec<char> = "a \\[[[ literal ]]] b [[[REAL]]]".chars().collect();
    let spans = find_placeholders(&line);
    assert_eq!(spans, vec![(21, 31)]);
}

#[test]
fn double_backslash_still_escapes_placeholder() {
    // RFC §4.5: there is no general backslash escape, so `\\[[[` is a
    // literal `\` followed by an *escaped* `[[[` — not a placeholder. The
    // parser renders it as `\[[[VAR]]]`; the editor must not highlight it.
    let line: Vec<char> = "\\\\[[[VAR]]]".chars().collect();
    let spans = find_placeholders(&line);
    assert_eq!(spans, Vec::<(usize, usize)>::new());
    let body = "\\\\{{{for X in Y}}}\nplain\n";
    assert_eq!(compute_block_bgs(&chars(body)), vec![Block::None, Block::None, Block::None]);
}

#[test]
fn opens_block_ignores_escaped_tag() {
    assert!(!opens_block("\\{{{for X in Y}}}", "for"));
    assert!(!opens_block("\\{{{if Z}}}", "if"));
}

#[test]
fn sanitize_keeps_safe_chars() {
    assert_eq!(sanitize_filename("my-prompt.txt"), "my-prompt.txt");
    assert_eq!(sanitize_filename("a/b/c"), "a_b_c");
    assert_eq!(sanitize_filename("hi there!"), "hi_there_");
}

#[test]
fn sanitize_keeps_unicode_name_chars() {
    // A prompt named `café_ñ_3` is valid (RFC §2.1); mangling it would make
    // the saved file's base name mismatch its `name`.
    assert_eq!(sanitize_filename("café_ñ_3.txt"), "café_ñ_3.txt");
}

#[test]
fn save_path_prefers_origin_file() {
    use std::path::{Path, PathBuf};
    let prompts = PathBuf::from("/home/u/.upl/prompts");
    // New prompt: top-level prompts dir.
    assert_eq!(save_path_for("new_prompt", None, prompts.clone()), prompts.join("new_prompt.txt"));
    // Opened from a file with a matching name: written back in place
    // (extension and location preserved).
    let orig = Path::new("/lib/team/my_prompt.upl");
    assert_eq!(save_path_for("my_prompt", Some(orig), prompts.clone()), orig);
    let pulled = Path::new("/home/u/.upl/prompts/host_7654/alice/my_prompt.txt");
    assert_eq!(save_path_for("my_prompt", Some(pulled), prompts.clone()), pulled);
    // Renamed: next to the original.
    assert_eq!(
        save_path_for("renamed", Some(orig), prompts.clone()),
        PathBuf::from("/lib/team/renamed.txt")
    );
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