use super::{credentials::*, owned::*};
use serde_json::json;
use std::{fs, path::PathBuf};

struct Directory(PathBuf, OwnedDirectory);
impl Directory {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("kota-owned-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root.clone(), OwnedDirectory::new(&root))
    }
    fn store(&self, value: serde_json::Value) -> AuthStore {
        self.1.initialize().unwrap();
        fs::write(self.1.auth_path(), serde_json::to_vec(&value).unwrap()).unwrap();
        AuthStore::load(&self.1).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_cold_auth_read_has_no_initialization_or_spawn_side_effect() {
    let d = Directory::new();
    assert_eq!(AuthStore::load(&d.1).unwrap().shape("fable").unwrap_err().code, "provider_not_signed_in");
    assert!(!d.1.root.exists());
}

#[test]
#[cfg(unix)]
fn existing_shared_provider_parent_keeps_its_permissions_and_other_files() {
    use std::os::unix::fs::PermissionsExt;
    let d = Directory::new();
    let parent = d.1.root.parent().unwrap();
    fs::create_dir(parent).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    let other = parent.join("other-provider.json");
    fs::write(&other, "keep unchanged").unwrap();
    d.1.initialize().unwrap();
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(fs::read_to_string(other).unwrap(), "keep unchanged");
    assert_eq!(
        fs::metadata(&d.1.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn initialization_preserves_unknown_settings_and_the_installation_identity() {
    let d = Directory::new();
    let id = d.1.initialize().unwrap();
    let path = d.1.agent.join("settings.json");
    fs::write(
        &path,
        r#"{"custom":{"keep":true},"compaction":{"keepRecentTokens":678,"enabled":false}}"#,
    )
    .unwrap();
    assert_eq!(d.1.initialize().unwrap(), id);
    let value = read_json(&path, "fixture").unwrap().unwrap();
    assert_eq!(value["custom"]["keep"], true);
    assert_eq!(
        value["compaction"],
        json!({"keepRecentTokens":678,"enabled":true})
    );
    assert_eq!(value["steeringMode"], "all");
    assert_eq!(value["followUpMode"], "all");
    assert!(!d.0.join(".pi").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&d.1.agent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(d.1.auth_path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn damaged_owned_settings_and_symlinked_directories_are_left_untouched() {
    let d = Directory::new();
    d.1.initialize().unwrap();
    let path = d.1.agent.join("settings.json");
    fs::write(&path, "{broken").unwrap();
    assert!(d.1.initialize().is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "{broken");
    #[cfg(unix)]
    {
        let other = Directory::new();
        fs::remove_dir_all(&d.1.agent).unwrap();
        std::os::unix::fs::symlink(&other.0, &d.1.agent).unwrap();
        assert!(d.1.initialize().is_err());
        assert!(AuthStore::load(&d.1).is_err());
        assert!(!other.0.join("auth.json").exists());
    }
}

#[test]
fn provider_locks_are_per_provider_and_live_owners_remain_exclusive() {
    let d = Directory::new();
    d.1.initialize().unwrap();
    let a = d.1.provider_lock("anthropic").unwrap();
    assert!(d.1.provider_lock("anthropic").is_err());
    let b = d.1.provider_lock("openai-codex").unwrap();
    drop(a);
    drop(d.1.provider_lock("anthropic").unwrap());
    drop(b);
    assert!(d.1.provider_lock("../not-a-provider").is_err());
}

#[test]
fn parallel_cold_queries_share_one_installation_and_do_not_rewrite_initialized_settings() {
    let d = Directory::new();
    let directory = d.1.clone();
    let ids = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                let directory = directory.clone();
                scope.spawn(move || directory.initialize().unwrap())
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(ids.iter().all(|id| id == &ids[0]));
    let path = d.1.agent.join("settings.json");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    d.1.initialize().unwrap();
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
}

#[test]
fn a_retired_defaults_file_is_neither_read_nor_replaced() {
    let d = Directory::new();
    d.1.initialize().unwrap();
    let path = d.1.root.join("defaults.json");
    fs::write(&path, "{broken").unwrap();
    d.1.initialize().unwrap();
    assert_eq!(AuthStore::load(&d.1).unwrap().shape("fable").unwrap_err().code, "provider_not_signed_in");
    assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
}

#[test]
fn key_classifier_matches_pi_template_escapes_and_does_not_trim() {
    for literal in [
        "plain",
        " !not-trimmed",
        "$",
        "$$VAR",
        "$!VAR",
        "${9BAD}",
        "${bad-name}",
        "${$INNER}",
        "$9",
        "中文💭",
        "!".trim_start_matches('!'),
    ] {
        assert_eq!(key_syntax(literal), KeySyntax::Literal);
    }
    for env in [
        "$VAR",
        "prefix-${VAR}-tail",
        "$_",
        "${_A9}",
        "${unfinished-$VALID",
        "$$$VAR",
    ] {
        assert_eq!(key_syntax(env), KeySyntax::Environment);
    }
    for command in ["!printf not-executed", "!", "!$VAR"] {
        assert_eq!(key_syntax(command), KeySyntax::Command);
    }
}

#[test]
fn key_references_are_rejected_before_pi_and_errors_never_contain_values() {
    let d = Directory::new();
    for dangerous in [
        "!touch /tmp/never-executed-fixture",
        "prefix-${FIXTURE_KEY}-tail",
    ] {
        let store = d.store(json!({"anthropic":{"type":"api_key","key":dangerous}}));
        let error = store.preflight().unwrap_err();
        let text = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code, "credential_invalid");
        assert!(!text.contains("never-executed") && !text.contains("FIXTURE_KEY"));
    }
    let store = d.store(json!({"openrouter":{"type":"oauth","access":"!not-a-key-$TOKEN","refresh":"","expires":9007199254740991u64}}));
    store.preflight().unwrap();
    assert_eq!(store.shape("openrouter").unwrap(), AuthType::Oauth);
}

#[test]
fn stored_shape_and_handler_are_both_required_without_borrowing_environment() {
    let d = Directory::new();
    let store = d.store(json!({"fable":{"type":"api_key","key":"fixture-literal"},"empty":{"type":"api_key","key":""},"missing":{"type":"api_key"}}));
    let caps = vec![Capabilities {
        provider_id: "fable".into(),
        provider_label: "Fixture".into(),
        can_api_key: true,
        can_o_auth: false,
    }];
    assert_eq!(store.gate("fable", &caps).unwrap(), AuthType::ApiKey);
    assert_eq!(
        store.gate("absent", &caps).unwrap_err().code,
        "provider_not_signed_in"
    );
    assert!(store.shape("empty").is_err());
    assert!(store.shape("missing").is_err());
    assert_eq!(
        store.gate("fable", &[]).unwrap_err().code,
        "auth_handler_unavailable"
    );
}

#[test]
fn cloud_native_keyless_records_are_valid_but_azure_needs_an_owned_address() {
    let d = Directory::new();
    let store = d.store(json!({
        "amazon-bedrock":{"type":"api_key"},
        "google-vertex":{"type":"api_key","env":{"GOOGLE_CLOUD_PROJECT":"fixture-project","GOOGLE_CLOUD_LOCATION":"fixture-location"}},
        "azure":{"type":"api_key","key":"fixture-key"}
    }));
    assert_eq!(store.shape("amazon-bedrock").unwrap(), AuthType::ApiKey);
    assert_eq!(store.shape("google-vertex").unwrap(), AuthType::ApiKey);
    assert_eq!(
        store.shape("azure").unwrap_err().code,
        "azure_resource_missing"
    );
    for env in [
        json!({"AZURE_OPENAI_BASE_URL":"http://127.0.0.1:1/v1"}),
        json!({"AZURE_OPENAI_RESOURCE_NAME":"fixture-resource"}),
    ] {
        assert_eq!(
            d.store(
                json!({"azure":{"type":"api_key","key":"fixture-key","env":env}})
            )
            .shape("azure")
            .unwrap(),
            AuthType::ApiKey
        );
    }
    assert!(d
        .store(json!({"google-vertex":{"type":"api_key"}}))
        .shape("google-vertex")
        .is_err());
}

#[test]
fn local_key_filter_preserves_sdk_overrides_and_cli_credentials() {
    use std::process::Command;
    let mut command = Command::new("fixture-not-executed");
    for key in super::environment::MODEL_KEY_NAMES {
        command.env(key, "fixture-model-key");
    }
    for key in [
        "HOME",
        "PATH",
        "AWS_PROFILE",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "ANTHROPIC_CUSTOM_HEADERS",
        "GOOGLE_GENAI_USE_VERTEXAI",
    ] {
        command.env(key, "fixture-kept");
    }
    command
        .env("KOTA_LOCAL_AUTH_HELPER", "1")
        .env("KOTA_LOCAL_INSTALLATION_UUID", "fixture");
    super::environment::remove_model_keys(&mut command);
    let values: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    for key in super::environment::MODEL_KEY_NAMES {
        assert!(values
            .get(std::ffi::OsStr::new(key))
            .is_some_and(|v| v.is_none()));
    }
    for key in [
        "HOME",
        "PATH",
        "AWS_PROFILE",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "ANTHROPIC_CUSTOM_HEADERS",
        "GOOGLE_GENAI_USE_VERTEXAI",
    ] {
        assert!(values
            .get(std::ffi::OsStr::new(key))
            .is_some_and(|v| *v == Some(std::ffi::OsStr::new("fixture-kept"))));
    }
}
