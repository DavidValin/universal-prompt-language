use super::*;
use crate::upl::parser::VariableValue;

#[test]
fn uuid_is_well_formed() {
    let uuid = generate_uuid();
    assert_eq!(uuid.len(), 36);
    assert_eq!(uuid.chars().nth(8), Some('-'));
    assert_eq!(uuid.chars().nth(13), Some('-'));
    assert_eq!(uuid.chars().nth(18), Some('-'));
    assert_eq!(uuid.chars().nth(23), Some('-'));
    // Version 4.
    assert_eq!(uuid.chars().nth(14), Some('4'));
}

#[test]
fn record_roundtrip() {
    let mut rec = BuildRecord::new("abc123", "my_prompt", "/tmp/p.txt", 5);
    rec.values
        .insert("name".to_string(), VariableValue::String("test".to_string()));
    rec.collected_fields = 1;
    rec.status = BuildStatus::Built;

    let json = serde_json::to_string(&rec).unwrap();
    let rec2: BuildRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec2.uuid, rec.uuid);
    assert_eq!(rec2.prompt_name, "my_prompt");
    assert_eq!(rec2.status, BuildStatus::Built);
    assert_eq!(rec2.collected_fields, 1);
}

#[test]
fn history_upsert() {
    let mut h = BuildHistory::default();
    let rec = BuildRecord::new("a", "p", "/p", 3);
    h.upsert(rec.clone());
    assert_eq!(h.records().len(), 1);

    // Upsert with same uuid replaces.
    let mut rec2 = rec.clone();
    rec2.collected_fields = 2;
    h.upsert(rec2);
    assert_eq!(h.records().len(), 1);
    assert_eq!(h.records()[0].collected_fields, 2);
}

#[test]
fn format_date_basic() {
    // 2024-01-01 00:00:00 UTC = 1704067200
    let s = format_date(1704067200);
    assert!(s.starts_with("2024-01-01"));
}

#[test]
fn format_date_for_filename_basic() {
    // 2024-01-01 00:00:00 UTC = 1704067200
    let s = format_date_for_filename(1704067200);
    assert!(s.starts_with("20240101_0000"));
    // No spaces or colons.
    assert!(!s.contains(' ') && !s.contains(':'));
}

#[test]
fn export_to_json_writes_file() {
    // Use a fake HOME so the test never touches the real one.
    let tmp = std::env::temp_dir().join("upl_export_test");
    std::fs::create_dir_all(&tmp).unwrap();
    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &tmp);

    let mut rec = BuildRecord::new("deadbeef", "my_prompt", "/tmp/p.txt", 2);
    rec.values
        .insert("name".to_string(), VariableValue::String("hello".to_string()));
    rec.values
        .insert("count".to_string(), VariableValue::Number(42.0));
    rec.collected_fields = 2;
    rec.status = BuildStatus::Built;

    let path = rec.export_to_json().unwrap();

    // Filename should contain the sha256 and end with .json.
    let fname = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(fname.starts_with("deadbeef_"));
    assert!(fname.ends_with(".json"));

    // File content should be valid JSON with the two values.
    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["name"], "hello");
    assert_eq!(parsed["count"], 42.0);

    // Cleanup.
    let _ = std::fs::remove_dir_all(&tmp);
    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    }
}