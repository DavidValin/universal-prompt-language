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