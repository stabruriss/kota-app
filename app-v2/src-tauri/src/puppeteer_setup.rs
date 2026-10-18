//! Prepare human-editable rule files. No credentials, provider process or model requests.
use crate::local_rpc::error::{LocalError, LocalResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

pub(crate) const STRING_COMMENTS: &str = include_str!("puppeteer_setup/string-comments.txt");
pub(crate) const DANCE_COMMENTS: &str = include_str!("puppeteer_setup/dance-comments.txt");

pub(crate) fn revision(bytes: Option<&[u8]>) -> String {
    bytes.map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)))
        .unwrap_or_else(|| "missing".into())
}

/// Replace comments with spaces, retaining newlines, strings and JSON token boundaries.
/// serde_json remains responsible for every other syntax/shape check.
pub(crate) fn json_without_comments(bytes: &[u8]) -> LocalResult<Vec<u8>> {
    let mut clean = bytes.to_vec();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        if in_string {
            match bytes[i] {
                b'\\' => { i += 2; continue; }
                b'"' => in_string = false,
                _ => {}
            }
        } else if bytes[i] == b'"' {
            in_string = true;
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                clean[i] = b' '; i += 1;
            }
            continue;
        } else if bytes[i..].starts_with(b"/*") {
            clean[i] = b' '; clean[i + 1] = b' '; i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                if !matches!(bytes[i], b'\n' | b'\r') { clean[i] = b' '; }
                i += 1;
            }
            if i == bytes.len() { return Err(LocalError::new("dance_settings_invalid")); }
            clean[i] = b' '; clean[i + 1] = b' '; i += 2;
            continue;
        }
        i += 1;
    }
    Ok(clean)
}

pub(crate) fn with_missing_comments(text: &str, comments: &str) -> String {
    // Exact complete lines only; a title alone does not count as the whole explanation.
    let missing: String = comments.lines().filter(|line| !text.lines().any(|old| old == *line))
        .map(|line| format!("{line}\n")).collect();
    format!("{missing}{text}")
}

/// Publish a complete new file without ever replacing a file created by another writer.
pub(crate) fn create_if_absent(path: &Path, text: &[u8]) -> LocalResult<()> {
    let parent = path.parent().ok_or_else(|| LocalError::new("invalid_request"))?;
    fs::create_dir_all(parent).map_err(|_| LocalError::new("rule_file_prepare_failed"))?;
    let temporary = parent.join(format!(".kota-rule-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(text)?;
        drop(file);
        match fs::hard_link(&temporary, path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        }
    })();
    let _ = fs::remove_file(temporary);
    result.map_err(|_| LocalError::new("rule_file_prepare_failed"))
}

fn prepare_string(cwd: &Path) -> LocalResult<()> {
    let lock = crate::shell_switch::metadata_lock(cwd);
    let _guard = lock.lock().map_err(|_| LocalError::new("local_config_busy"))?;
    let path = cwd.join("SHELL.yaml");
    let stat = fs::symlink_metadata(&path).map_err(|_| LocalError::new("local_config_read_failed"))?;
    if !stat.is_file() || stat.file_type().is_symlink() { return Err(LocalError::new("local_config_invalid")); }
    let before = fs::read_to_string(&path).map_err(|_| LocalError::new("local_config_read_failed"))?;
    let shell = crate::parse_shell_yaml(&before).map_err(|_| LocalError::new("local_config_invalid"))?;
    if shell.provider.as_deref().or(shell.command.as_deref()) != Some("kota-local") {
        return Err(LocalError::new("local_provider_changed"));
    }
    let mut next = with_missing_comments(&before, STRING_COMMENTS);
    if !serde_yaml::from_str::<serde_yaml::Mapping>(&before).map_err(|_| LocalError::new("local_config_invalid"))?.contains_key(serde_yaml::Value::String("auto-route".into())) {
        // Empty is deliberately not an enabled group; never select kota/auto here.
        next.push_str("\nauto-route:\n  threshold: 0.5\n  default-model: model-1\n  models: []\n");
    }
    if next != before {
        if fs::read_to_string(&path).ok().as_deref() != Some(&before) {
            return Err(LocalError::new("local_config_changed"));
        }
        crate::adapter_sync::atomic_replace(&path, next.as_bytes())
            .map_err(|_| LocalError::new("local_config_write_failed"))?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RuleKind { String, Dance }
#[derive(Serialize)]
pub(crate) struct PreparedRuleFile { path: String }

#[tauri::command]
pub(crate) async fn puppeteer_prepare_setup(
    app: tauri::AppHandle, project_root: String, kind: RuleKind, agent_id: Option<String>,
) -> LocalResult<PreparedRuleFile> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = crate::puppeteer::project_root(&project_root)?;
        let path = match kind {
            RuleKind::String => {
                let id = agent_id.ok_or_else(|| LocalError::new("invalid_request"))?;
                let cwd = crate::local_rpc::configuration::agent_cwd(&app, &root, &id)?;
                prepare_string(&cwd)?;
                cwd.join("SHELL.yaml")
            }
            RuleKind::Dance => crate::puppeteer::prepare(&root)?,
        };
        let path = path.canonicalize().map_err(|_| LocalError::new("rule_file_prepare_failed"))?;
        Ok(PreparedRuleFile { path: path.to_string_lossy().into_owned() })
    }).await.map_err(|_| LocalError::new("rule_file_prepare_failed"))?
}

#[cfg(test)]
#[path = "puppeteer_setup/tests.rs"]
mod tests;
