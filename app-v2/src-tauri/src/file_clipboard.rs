const MAX_PATH_BYTES: usize = 64 * 1024;

pub(crate) fn validate_path(path: &str) -> Result<(), String> {
    // The menu already resolved the path. Do not require the file to still
    // exist, and never interpret its contents as shell input.
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return Err("The file path is empty, invalid, or too long to copy.".into());
    }
    Ok(())
}

/// Called on the App's main thread, only after validating the input.
#[cfg(target_os = "macos")]
pub(crate) fn copy_file_path(path: &str) -> Result<(), String> {
    write_path(&objc2_app_kit::NSPasteboard::generalPasteboard(), path)
}

#[cfg(target_os = "macos")]
fn write_path(pasteboard: &objc2_app_kit::NSPasteboard, path: &str) -> Result<(), String> {
    use objc2_app_kit::NSPasteboardTypeString;
    use objc2_foundation::NSString;

    let text = NSString::from_str(path);
    pasteboard.clearContents();
    // AppKit exports this immutable pasteboard type for the life of the process.
    if pasteboard.setString_forType(&text, unsafe { NSPasteboardTypeString }) {
        Ok(())
    } else {
        Err("Could not copy the file path to the clipboard.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_copy_input_before_accessing_the_clipboard() {
        for path in [
            "".into(),
            "/tmp/file\0.md".into(),
            "x".repeat(MAX_PATH_BYTES + 1),
            "中".repeat(MAX_PATH_BYTES / 3 + 1),
        ] {
            assert!(validate_path(&path).is_err());
        }
        assert!(validate_path(&"x".repeat(MAX_PATH_BYTES)).is_ok());
    }

    #[test]
    fn copying_does_not_depend_on_the_file_still_existing() {
        assert!(validate_path("/fixture/no-longer-exists/file.md").is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn writes_full_paths_verbatim_to_an_isolated_native_pasteboard() {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

        // A private named pasteboard: never read, clear, or write the user's
        // general pasteboard, even while running the full suite.
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let paths = [
            "/fixture/project/first.md",
            "/fixture/project/中文 space/\"quoted\" $file;$(literal).md",
            "/fixture/project/project-memory/chathistory/overflow/violet-fixture.md",
        ];
        for path in paths {
            write_path(&pasteboard, path).unwrap();
            let actual = pasteboard.stringForType(unsafe { NSPasteboardTypeString });
            assert_eq!(actual.map(|text| text.to_string()).as_deref(), Some(path));
        }
        // objc2 omits this SDK method; release this test-only unique board.
        unsafe { objc2::msg_send![&*pasteboard, releaseGlobally] }
    }
}
