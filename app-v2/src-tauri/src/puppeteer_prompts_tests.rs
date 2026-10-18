use super::*;
use serde_json::json;
const USER: &str = include_str!("../../kota-ext/test/fixtures/puppeteer-user-prompts.json");
fn root() -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("kota-puppeteer-prompts-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn all_four_templates_validate_and_invalid_schema_is_never_partially_used() {
    assert!(valid(builtin()));
    let user: Value = serde_json::from_str(USER).unwrap();
    assert!(valid(&user));
    let root = root();
    let path = root.join(FILE);
    let mut bad = Vec::new();
    for (part, question) in [
        ("stringFirst", "model"),
        ("stringLater", "model"),
        ("danceContext", "target"),
        ("danceRule", "target"),
    ] {
        for instruction in ["😀".repeat(513), "bad\0instruction".into(), "".into()] {
            let mut value = user.clone();
            value[part]["questions"][question]["instructions"] = json!(instruction);
            bad.push(value);
        }
        let mut value = user.clone();
        value.as_object_mut().unwrap().remove(part);
        bad.push(value);
        let mut value = user.clone();
        value[part]["questions"][question]["criteria"] = json!({});
        bad.push(value);
    }
    let mut extra = user.clone();
    extra["namedRoute"] = json!("not supported");
    bad.push(extra);
    for value in bad {
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&path, &bytes).unwrap();
        let loaded = load(path.clone());
        assert_eq!(loaded.warning, Some(INVALID));
        assert_eq!(loaded.value, *builtin());
        assert_eq!(loaded.content, BUNDLED.trim_end());
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "invalid user edits are not repaired"
        );
    }
    fs::write(&path, "not json").unwrap();
    assert_eq!(load(path.clone()).warning, Some(INVALID));
    fs::write(&path, [0xff]).unwrap();
    assert_eq!(load(path.clone()).warning, Some(UNREADABLE));
    fs::remove_file(&path).unwrap();
    assert_eq!(load(path.clone()).warning, Some(UNREADABLE));
    assert!(!path.exists(), "loader itself never creates a file");
    fs::create_dir(&path).unwrap();
    assert_eq!(load(path.clone()).warning, Some(INVALID));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn an_effective_user_snapshot_is_frozen_until_the_next_load() {
    let root = root();
    let path = root.join(FILE);
    fs::write(&path, USER).unwrap();
    let snapshot = load(path.clone());
    assert_eq!(snapshot.warning, None);
    assert_eq!(snapshot.content, USER.trim_end());
    fs::write(&path, "not json").unwrap();
    assert_eq!(
        snapshot.value["stringFirst"]["questions"]["model"]["instructions"],
        "Fixture user file: first classification 😀."
    );
    assert_eq!(load(path).warning, Some(INVALID));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn account_file_ensure_preview_and_reset_use_existing_system_prompt_protocol() {
    // A child test binary has a throwaway HOME, without changing process-wide environment
    // under the full gate's other parallel tests or touching the user's account directory.
    if std::env::var_os("KOTA_PUPPETEER_PROMPT_TEST_CHILD").is_none() {
        let root = root();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "puppeteer_prompts::tests::account_file_ensure_preview_and_reset_use_existing_system_prompt_protocol", "--nocapture", "--test-threads=1"])
            .env("HOME", &root).env("KOTA_HOME", root.join("Kota"))
            .env("KOTA_PUPPETEER_PROMPT_TEST_CHILD", "1").output().unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let template = crate::system_prompt_template_by_file_name(FILE).unwrap();
    let path = crate::system_prompt_account_path(template);
    assert_eq!(
        path,
        crate::kota_home_dir()
            .join("heroes/system-puppeteer")
            .join(FILE)
    );
    assert!(!path.exists());
    let first = crate::system_prompt_read_template(FILE, false).unwrap();
    assert_eq!(first.path, crate::path_string(&path));
    assert_eq!(first.content, BUNDLED.trim_end());
    assert!(serde_json::to_value(&first)
        .unwrap()
        .get("warning")
        .is_none());
    assert!(crate::read_system_prompt_defaults_manifest()
        .prompts
        .contains_key("system-puppeteer/puppeteer-prompts.json"));
    // The previous shipped default upgrades at startup only when it still matches
    // the account's recorded default hash. No real account files are involved.
    let previous = include_str!("../../kota-ext/test/fixtures/puppeteer-hotfix3-prompts.json");
    assert_ne!(previous, BUNDLED);
    fs::write(&path, previous).unwrap();
    let mut manifest = crate::read_system_prompt_defaults_manifest();
    manifest.prompts.get_mut("system-puppeteer/puppeteer-prompts.json").unwrap().content_sha256 = crate::system_prompt_content_hash(previous);
    crate::persist_system_prompt_defaults_manifest(&manifest);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(crate::system_prompt_read_template(FILE, false).unwrap().content, previous.trim_end());
    assert_eq!(read().content, previous.trim_end());
    assert_eq!(fs::read_to_string(&path).unwrap(), previous);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before, "ordinary reads must not rewrite an existing default");
    crate::ensure_default_system_prompt_templates(false).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), BUNDLED);
    fs::write(&path, USER).unwrap();
    crate::ensure_default_system_prompt_templates(false).unwrap();
    assert_eq!(
        crate::system_prompt_read_template(FILE, false)
            .unwrap()
            .content,
        USER.trim_end()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), USER);
    let questions = crate::puppeteer::puppeteer_jev_prompts();
    assert_eq!(questions.as_object().unwrap().len(), 4);
    assert_eq!(questions["stringFirst"], read().value["stringFirst"]);
    let magi = crate::system_prompt_read_template("magi-nl-translate.md", false).unwrap();
    fs::write(&magi.path, "Unrelated user-edited Magi template").unwrap();
    fs::write(&path, "broken JSON").unwrap();
    crate::ensure_default_system_prompt_templates(false).unwrap();
    let bad = crate::system_prompt_read_template(FILE, false).unwrap();
    assert_eq!(bad.warning.as_deref(), Some(INVALID));
    assert_eq!(bad.content, BUNDLED.trim_end());
    assert_eq!(bad.path, first.path);
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken JSON");
    assert_eq!(
        crate::system_prompt_read_template("magi-nl-translate.md", false)
            .unwrap()
            .content,
        "Unrelated user-edited Magi template"
    );
    assert!(serde_json::to_value(magi).unwrap().get("warning").is_none());
    let reset = crate::system_prompt_read_template(FILE, true).unwrap();
    assert!(reset.warning.is_none());
    assert_eq!(fs::read_to_string(&path).unwrap(), BUNDLED);
    #[cfg(unix)]
    {
        let outside = path.with_file_name("not-the-prompt.txt");
        fs::write(&outside, "preserve unrelated target").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert_eq!(
            crate::system_prompt_read_template(FILE, false)
                .unwrap()
                .warning
                .as_deref(),
            Some(INVALID)
        );
        crate::system_prompt_read_template(FILE, true).unwrap();
        assert!(!fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(outside).unwrap(),
            "preserve unrelated target"
        );
    }
}
