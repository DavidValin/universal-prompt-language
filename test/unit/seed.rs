use super::*;
use crate::manager::ui_tags::sha256;
use std::collections::HashMap;

#[test]
fn tags_db_links_to_embedded_prompts() {
    let store: HashMap<String, Vec<String>> =
        bincode::deserialize(TAGS_DB).unwrap();
    for (name, contents) in SAMPLES {
        let h = sha256(contents);
        let tags: Vec<&String> = store
            .iter()
            .filter_map(|(t, v)| {
                if v.iter().any(|x| x == &h) {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        println!("{name} {h} -> {tags:?}");
        assert!(
            !tags.is_empty(),
            "no tag links to {name} ({h}); the tags_db is out of sync"
        );
    }
}