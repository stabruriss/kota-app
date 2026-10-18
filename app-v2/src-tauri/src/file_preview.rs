use std::fs::Metadata;

/// A cheap file revision sampled during reference resolution. Keep it as a
/// string across IPC: inode numbers and nanosecond timestamps exceed JS's
/// exact integer range. No image bytes are read or hashed here.
pub(crate) fn metadata_version(metadata: &Metadata) -> Option<String> {
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!(
            "v1:{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some(format!(
            "v1:{}:{}:{}",
            metadata.len(),
            modified.as_secs(),
            modified.subsec_nanos()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("image-preview-fixture-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let path = root.join("preview.png");
            fs::write(&path, b"old-image").unwrap();
            Self { root, path }
        }

        fn resolve(&self) -> crate::VioletFileRefResolveResult {
            crate::resolve_violet_file_ref(&crate::VioletFileRefRequest {
                project_root: Some(self.root.to_string_lossy().into_owned()),
                path: "preview.png".into(),
            })
            .unwrap()
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn resolution_returns_a_stable_version_without_changing_the_path() {
        let fixture = Fixture::new();
        let first = fixture.resolve();
        let second = fixture.resolve();
        assert_eq!(
            first.path,
            fs::canonicalize(&fixture.path).unwrap().to_string_lossy()
        );
        assert!(!first.is_dir);
        assert!(first.version.is_some());
        assert_eq!(first.version, second.version);
        let json = serde_json::to_value(first).unwrap();
        assert!(json["version"].is_string());
    }

    #[test]
    fn changed_size_invalidates_the_same_path() {
        let fixture = Fixture::new();
        let before = fixture.resolve();
        fs::write(&fixture.path, b"a larger replacement image").unwrap();
        let after = fixture.resolve();
        assert_eq!(before.path, after.path);
        assert_ne!(before.version, after.version);
    }

    #[test]
    fn subsecond_edits_are_not_rounded_to_seconds() {
        let fixture = Fixture::new();
        let file = File::open(&fixture.path).unwrap();
        let time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_times(FileTimes::new().set_modified(time + Duration::from_millis(100)))
            .unwrap();
        let before = fixture.resolve();
        file.set_times(FileTimes::new().set_modified(time + Duration::from_millis(200)))
            .unwrap();
        assert_ne!(before.version, fixture.resolve().version);
    }

    #[cfg(unix)]
    #[test]
    fn same_size_overwrite_with_restored_mtime_still_changes_version() {
        let fixture = Fixture::new();
        let modified = fs::metadata(&fixture.path).unwrap().modified().unwrap();
        let before = fixture.resolve();
        fs::write(&fixture.path, b"new-image").unwrap();
        File::open(&fixture.path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            fs::metadata(&fixture.path).unwrap().modified().unwrap(),
            modified
        );
        assert_ne!(before.version, fixture.resolve().version);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_with_same_size_and_mtime_gets_a_new_version() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        let metadata = fs::metadata(&fixture.path).unwrap();
        let before = fixture.resolve();
        let replacement = fixture.root.join("replacement.png");
        fs::write(&replacement, b"new-image").unwrap();
        File::open(&replacement)
            .unwrap()
            .set_times(FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        fs::rename(&replacement, &fixture.path).unwrap();
        let after = fs::metadata(&fixture.path).unwrap();
        assert_eq!(metadata.len(), after.len());
        assert_eq!(metadata.modified().unwrap(), after.modified().unwrap());
        assert_ne!(metadata.ino(), after.ino());
        assert_ne!(before.version, fixture.resolve().version);
    }

    #[test]
    fn directories_have_no_image_version() {
        let fixture = Fixture::new();
        assert_eq!(
            metadata_version(&fs::metadata(&fixture.root).unwrap()),
            None
        );
    }
}
