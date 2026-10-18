use super::*;

struct Board(PathBuf);
impl Board {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-whiteboard-delete-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        load_whiteboard_canvas(&root, None).unwrap();
        Self(root)
    }
    fn page(&self, index: usize) -> PathBuf {
        project_memory_dir(&self.0).join(format!("canvas/pages/page-{index:03}.excalidraw"))
    }
    fn add(&self, index: usize) -> PathBuf {
        let path = self.page(index);
        fs::write(&path, default_excalidraw_json()).unwrap();
        path
    }
    fn manifest(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(project_memory_dir(&self.0).join("canvas/manifest.json")).unwrap()).unwrap()
    }
}
impl Drop for Board {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn delete_current_page_keeps_other_pages_titles_and_chat_snapshots() {
    let board = Board::new();
    let first = board.page(1);
    let second = board.add(2);
    let snapshot = project_memory_dir(&board.0).join("canvas/snapshots/kept.png");
    fs::write(&snapshot, b"snapshot bytes").unwrap();
    let mut titles = std::collections::HashMap::new();
    titles.insert(path_string(&first), "Kept title".to_string());
    titles.insert(path_string(&second), "Deleted title".to_string());
    write_whiteboard_manifest_with_title_updates(&board.0, &second, Some(&snapshot), &titles).unwrap();
    let snapshots = board.manifest()["snapshots"].clone();
    let result = delete_whiteboard_canvas_page(&board.0, second.to_str().unwrap(), second.to_str()).unwrap();
    assert!(!second.exists());
    assert_eq!(result.page_path, path_string(&first));
    assert_eq!(result.pages.len(), 1);
    assert_eq!(result.pages[0].title, "Kept title");
    assert_eq!(board.manifest()["activePageId"], "page-001");
    assert_eq!(board.manifest()["snapshots"], snapshots);
    assert_eq!(fs::read(snapshot).unwrap(), b"snapshot bytes");
    assert!(!board.manifest().to_string().contains("Deleted title"));
}

#[test]
fn delete_other_page_keeps_active_page_and_its_content() {
    let board = Board::new();
    let second = board.add(2);
    let third = board.add(3);
    let data = r#"{"elements":[{"type":"text","text":"keep drawing"}]}"#;
    fs::write(&third, data).unwrap();
    let result = delete_whiteboard_canvas_page(&board.0, second.to_str().unwrap(), third.to_str()).unwrap();
    assert_eq!(result.page_path, path_string(&third));
    assert_eq!(result.data_json, data);
    assert_eq!(result.pages.len(), 2);
    assert!(board.page(1).exists());
}

#[test]
fn delete_last_page_creates_blank_at_a_new_path_and_drops_old_title() {
    let board = Board::new();
    let first = board.page(1);
    fs::write(&first, r#"{"elements":[{"type":"text","text":"discard drawing"}]}"#).unwrap();
    let mut titles = std::collections::HashMap::new();
    titles.insert(path_string(&first), "Old page".to_string());
    write_whiteboard_manifest_with_title_updates(&board.0, &first, None, &titles).unwrap();
    let result = delete_whiteboard_canvas_page(&board.0, first.to_str().unwrap(), first.to_str()).unwrap();
    assert!(!first.exists());
    assert_ne!(result.page_path, path_string(&first));
    assert_eq!(result.pages.len(), 1);
    assert_ne!(result.pages[0].title, "Old page");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&result.data_json).unwrap()["elements"], serde_json::json!([]));
    let reloaded = load_whiteboard_canvas(&board.0, first.to_str()).unwrap();
    assert_eq!(reloaded.page_path, result.page_path);
    assert!(!first.exists(), "an old selection must not recreate the deleted page");
}

#[test]
fn delete_rejects_non_pages_and_keeps_files_untouched() {
    let board = Board::new();
    let outside = board.0.join("source.excalidraw");
    let snapshot = project_memory_dir(&board.0).join("canvas/snapshots/drawing.excalidraw");
    let directory = board.page(2);
    fs::write(&outside, b"keep outside").unwrap();
    fs::write(&snapshot, b"keep snapshot").unwrap();
    fs::create_dir(&directory).unwrap();
    for path in [&outside, &snapshot, &directory, &project_memory_dir(&board.0).join("canvas/manifest.json")] {
        assert!(delete_whiteboard_canvas_page(&board.0, path.to_str().unwrap(), None).is_err());
        assert!(path.exists());
    }
    assert_eq!(fs::read(&outside).unwrap(), b"keep outside");
    assert_eq!(fs::read(&snapshot).unwrap(), b"keep snapshot");
    assert!(board.page(1).exists());
}

#[cfg(unix)]
#[test]
fn delete_rejects_symlinks_even_when_the_destination_is_another_page() {
    let board = Board::new();
    let first = board.page(1);
    let link = board.page(2);
    std::os::unix::fs::symlink(&first, &link).unwrap();
    assert!(delete_whiteboard_canvas_page(&board.0, link.to_str().unwrap(), None).is_err());
    assert!(first.exists());
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}

#[test]
fn delete_keeps_original_if_replacement_page_cannot_be_opened() {
    let board = Board::new();
    let first = board.page(1);
    let second = board.add(2);
    fs::write(&second, "invalid json").unwrap();
    assert!(delete_whiteboard_canvas_page(&board.0, first.to_str().unwrap(), second.to_str()).is_err());
    assert!(first.exists());
}
