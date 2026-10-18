use super::*;

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("kota-rule-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap(); Self(path)
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn prepare_string_keeps_all_existing_bytes_and_model_and_only_adds_missing_explanation_and_empty_group() {
    let dir = Directory::new(); let path = dir.0.join("SHELL.yaml");
    let before = "# My notes stay\nprovider: kota-local\nmodel: fable/reason\neffort: high\nargs: [--unchanged]\ncustom: value\n";
    fs::write(&path, before).unwrap();
    prepare_string(&dir.0).unwrap();
    let first = fs::read_to_string(&path).unwrap();
    assert!(first.contains(before)); assert!(first.starts_with(STRING_COMMENTS));
    assert!(first.contains("# threshold: 0.05-0.95 in steps of 0.05."));
    let parsed = crate::parse_shell_yaml(&first).unwrap();
    assert_eq!(parsed.model.as_deref(), Some("fable/reason"));
    assert_eq!(parsed.effort.as_deref(), Some("high"));
    assert_eq!(parsed.auto_route.as_ref().unwrap().threshold, 0.5);
    assert!(parsed.auto_route.unwrap().is_empty());
    prepare_string(&dir.0).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), first);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "no credential/default/version files");
}
#[test]
fn prepare_keeps_a_saved_group_and_user_comments_without_reformatting() {
    let dir = Directory::new(); let path = dir.0.join("SHELL.yaml");
    let before = "provider: kota-local\nmodel: fable/plain\n# user explanation\nauto-route: {threshold: 0.85, default-model: model-1, models: []}\n";
    fs::write(&path, before).unwrap(); prepare_string(&dir.0).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{STRING_COMMENTS}{before}"));
    // Explicit null is an existing value, not a missing key to duplicate/overwrite.
    let null = "provider: kota-local\nauto-route: null\n";
    fs::write(&path, null).unwrap(); prepare_string(&dir.0).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{STRING_COMMENTS}{null}"));
}
#[test]
fn missing_invalid_or_nonlocal_shell_is_not_created_or_changed() {
    let dir = Directory::new(); let path = dir.0.join("SHELL.yaml");
    assert!(prepare_string(&dir.0).is_err()); assert!(!path.exists());
    for before in ["[broken", "provider: codex\nmodel: default\n", "provider: pi\n"] {
        fs::write(&path, before).unwrap(); assert!(prepare_string(&dir.0).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }
}
#[test]
fn new_files_are_published_whole_and_existing_files_are_never_replaced() {
    let dir = Directory::new(); let path = dir.0.join("rule.json");
    let handles: Vec<_> = (0..8).map(|i| {
        let path = path.clone(); std::thread::spawn(move || create_if_absent(&path, format!("{{\"winner\":{i}}}").as_bytes()).unwrap())
    }).collect();
    for handle in handles { handle.join().unwrap(); }
    let before = fs::read(&path).unwrap();
    serde_json::from_slice::<serde_json::Value>(&before).unwrap();
    create_if_absent(&path, b"replacement").unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn json_comments_are_string_aware_and_never_join_tokens() {
    let source = r#"/* lead */ {"url":"https://a/b/*c*/", "text":"quote \" // still a string", "escaped":"\\", // a comment
"unicode":"雀/*燕*/", "n": 1} // tail"#;
    let value: serde_json::Value = serde_json::from_slice(&json_without_comments(source.as_bytes()).unwrap()).unwrap();
    assert_eq!(value["url"], "https://a/b/*c*/");
    assert_eq!(value["text"], "quote \" // still a string");
    assert_eq!(value["unicode"], "雀/*燕*/");
    assert!(json_without_comments(b"{} /* unfinished").is_err());
    for invalid in [r#"{"n":1/*comment*/0}"#, r#"{'n':1}"#, r#"{n:1}"#] {
        assert!(serde_json::from_slice::<serde_json::Value>(&json_without_comments(invalid.as_bytes()).unwrap()).is_err());
    }
}
