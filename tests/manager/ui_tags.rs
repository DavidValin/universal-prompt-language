// Unit tests for the tag store and sha256 helper.

use universal_prompt_language::manager::ui_tags::{sha256, TagStore};

#[test]
fn sha256_is_stable_and_hex() {
    let a = sha256("hello world");
    let b = sha256("hello world");
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn associate_dedupes() {
    let mut s = TagStore::default();
    let m = "abc";
    s.associate("rust", m);
    s.associate("rust", m);
    s.associate("rust", m);
    assert_eq!(s.count_for("rust"), 1);
    assert_eq!(s.total(), 1);
}

#[test]
fn disassociate_removes_tag_when_empty() {
    let mut s = TagStore::default();
    s.associate("rust", "a");
    s.disassociate("rust", "a");
    assert_eq!(s.count_for("rust"), 0);
    assert_eq!(s.total(), 0);
}

#[test]
fn rename_preserves_associations() {
    let mut s = TagStore::default();
    s.associate("old", "a");
    s.associate("old", "b");
    s.rename("old", "new");
    assert_eq!(s.count_for("new"), 2);
    assert_eq!(s.count_for("old"), 0);
    assert_eq!(s.tags_for_prompt("a"), vec!["new".to_string()]);
}

#[test]
fn tags_for_prompt_sorted_and_deduped() {
    let mut s = TagStore::default();
    s.associate("zebra", "x");
    s.associate("alpha", "x");
    s.associate("mango", "x");
    assert_eq!(
        s.tags_for_prompt("x"),
        vec!["alpha".to_string(), "mango".to_string(), "zebra".to_string()]
    );
}
