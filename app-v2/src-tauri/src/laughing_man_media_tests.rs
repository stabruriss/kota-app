use super::{detect_media_paths, MAX_MEDIA_BYTES};
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-lm-media-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("project")).unwrap();
        let root = root.canonicalize().unwrap();
        Self {
            project: root.join("project"),
            root,
        }
    }

    fn file(&self, relative: &str) -> PathBuf {
        let path = self.project.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Detection only: no image decoding, Telegram calls or real account data.
        fs::write(&path, b"synthetic media fixture").unwrap();
        path.canonicalize().unwrap()
    }

    fn detect(&self, text: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
        detect_media_paths(text, &self.project, &[self.project.clone()])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn markdown_images_and_image_links_include_space_and_unicode_paths() {
    let fixture = Fixture::new();
    let plain = fixture.file("shot.png");
    let spaced = fixture.file("设计图/Screen Shot (1).PNG");
    for text in [
        format!("![截图]({})", plain.display()),
        format!("[截图]({})", plain.display()),
        format!("[截图](<{}>)", plain.display()),
    ] {
        assert_eq!(fixture.detect(&text), (vec![plain.clone()], vec![]));
    }
    assert_eq!(
        fixture.detect(&format!("![截图](<{}>)", spaced.display())),
        (vec![spaced], vec![])
    );
    let unwrapped = fixture.file("Screen Shot.webp");
    assert_eq!(
        fixture.detect(&format!("前文 [截图]({}) 后文", unwrapped.display())),
        (vec![unwrapped], vec![])
    );
}

#[test]
fn relative_images_use_the_rooms_three_resolution_locations() {
    let fixture = Fixture::new();
    let root_image = fixture.file("root.png");
    let source_image = fixture.file("project-files/assets/source.jpg");
    let memory_image = fixture.file("project-memory/scratch/memory.gif");
    assert_eq!(
        fixture
            .detect("[root](root.png) ![source](assets/source.jpg) [memory](scratch/memory.gif)"),
        (vec![root_image, source_image, memory_image], vec![])
    );
}

#[test]
fn canonical_image_deduplication_keeps_the_first_occurrence() {
    let fixture = Fixture::new();
    let image = fixture.file("shots/a.png");
    let text = format!(
        "[图](shots/a.png)\n{}\n`./shots/a.png`\n[带行号](file://{}:12)\n",
        image.display(),
        image.display()
    );
    assert_eq!(fixture.detect(&text), (vec![image.clone()], vec![]));
    #[cfg(unix)]
    {
        let alias = fixture.project.join("alias.png");
        std::os::unix::fs::symlink(&image, &alias).unwrap();
        assert_eq!(
            fixture.detect(&format!("{text}[alias]({})", alias.display())),
            (vec![image], vec![])
        );
    }
}

#[test]
fn ambiguous_relative_images_are_rejected_like_the_room() {
    let fixture = Fixture::new();
    let first = fixture.file("same.png");
    fixture.file("project-files/same.png");
    assert!(fixture.detect("![ambiguous](same.png)").0.is_empty());
    assert_eq!(
        fixture
            .detect(&format!("[explicit]({})", first.display()))
            .0,
        vec![first]
    );
}

#[test]
fn quoted_syntax_escaped_links_and_remote_links_do_not_upload_local_images() {
    let fixture = Fixture::new();
    let image = fixture.file("shot.png");
    for text in [
        format!("`![example]({})`", image.display()),
        format!(r"\![example]({})", image.display()),
        format!(r"\[example]({})", image.display()),
        format!("[{}](https://example.invalid/photo.png)", image.display()),
        format!("![remote](https://example.invalid{})", image.display()),
        format!("https://example.invalid{}", image.display()),
        format!(
            "```md\n![example]({})\n{}\n```",
            image.display(),
            image.display()
        ),
        format!("~~~\n{}\n~~~", image.display()),
        "The filename is shot.png".into(),
    ] {
        assert!(fixture.detect(&text).0.is_empty(), "{text}");
    }
    // Two backslashes escape each other; the room renders this link normally.
    assert_eq!(
        fixture
            .detect(&format!(r"\\![real]({})", image.display()))
            .0,
        vec![image]
    );
}

#[test]
fn image_links_preserve_upload_directory_type_and_size_limits() {
    let fixture = Fixture::new();
    let outside = fixture.file("../outside.png");
    let oversized = fixture.file("oversized.png");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_MEDIA_BYTES + 1)
        .unwrap();
    let at_limit = fixture.file("at-limit.png");
    fs::File::create(&at_limit)
        .unwrap()
        .set_len(MAX_MEDIA_BYTES)
        .unwrap();
    let unsupported = fixture.file("vector.svg");
    let directory = fixture.project.join("directory.png");
    fs::create_dir_all(&directory).unwrap();
    for path in [&outside, &oversized, &unsupported, &directory] {
        assert!(fixture
            .detect(&format!("![image]({})", path.display()))
            .0
            .is_empty());
    }
    assert!(fixture.detect("![escape](../outside.png)").0.is_empty());
    assert!(fixture.detect("![gone](missing.png)").0.is_empty());
    assert_eq!(fixture.detect("[limit](at-limit.png)").0, vec![at_limit]);
    #[cfg(unix)]
    {
        let alias = fixture.project.join("outside-link.png");
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        assert!(fixture.detect("![escape](outside-link.png)").0.is_empty());
    }
}

#[test]
fn mixed_image_references_remain_ordered_and_capped_at_ten() {
    let fixture = Fixture::new();
    let images = (0..12)
        .map(|i| fixture.file(&format!("image-{i}.png")))
        .collect::<Vec<_>>();
    let text = images
        .iter()
        .enumerate()
        .map(|(i, path)| {
            if i % 2 == 0 {
                format!("![图]({})", path.display())
            } else {
                path.display().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(fixture.detect(&text).0, images[..10]);
}

#[test]
fn document_detection_keeps_its_existing_standalone_absolute_path_rule() {
    let fixture = Fixture::new();
    let doc = fixture.file("note.md");
    assert!(fixture
        .detect("[note](note.md)\nproject-files/note.md")
        .1
        .is_empty());
    assert_eq!(fixture.detect(&doc.display().to_string()).1, vec![doc]);
    let docs = (0..6)
        .map(|i| fixture.file(&format!("doc-{i}.txt")))
        .collect::<Vec<_>>();
    let text = docs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(fixture.detect(&text).1, docs[..4]);
    assert!(detect_media_paths(
        &text,
        &fixture.project,
        &[Path::new("/nonexistent-lm-root").into()]
    )
    .1
    .is_empty());
}
