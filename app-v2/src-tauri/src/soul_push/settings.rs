use crate::{message_origin::MessageOrigin, local_rpc::{error::{LocalError, LocalResult}, jev, owned::{FileLock, OwnedDirectory}}};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::{Path, PathBuf}};
use tauri::{AppHandle, Emitter};

/// Global Puppeteer policy, not an agent's SHELL or project Dance configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Settings {
    pub schema: u8,
    pub enabled: bool,
    pub mark_threshold: f64,
    pub dream_threshold: f64,
    pub max_marks: usize,
    pub max_dreams: usize,
    pub cooldown_replies: u32,
    pub minimum_message_length: usize,
}

impl Default for Settings {
    fn default() -> Self { Self::initial(true) }
}

impl Settings {
    /// No key means effectively off, not a write of `enabled=false`.
    pub(crate) fn initial(enabled: bool) -> Self {
        Self { schema: 1, enabled, mark_threshold: 0.70, dream_threshold: 0.80,
            max_marks: 3, max_dreams: 2, cooldown_replies: 10, minimum_message_length: 60 }
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let threshold = |value: f64| value.is_finite() && (0.05..=0.95).contains(&value)
            && (value * 20.0 - (value * 20.0).round()).abs() <= 1e-8;
        if self.schema != 1 || !threshold(self.mark_threshold) || !threshold(self.dream_threshold)
            || !(1..=5).contains(&self.max_marks) || self.max_dreams > 5
            || self.cooldown_replies > 50 || self.minimum_message_length > 500
            || self.minimum_message_length % 10 != 0 {
            return Err("soul_settings_invalid");
        }
        Ok(())
    }

    /// The caller supplies the length of producer-known, unwrapped user text,
    /// excluding resolved mentions and whitespace. Never count host envelopes.
    pub(crate) fn permits(&self, origin: MessageOrigin, meaningful_chars: usize) -> bool {
        self.enabled && origin.allows_user_context()
            && meaningful_chars >= self.minimum_message_length
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub settings: Settings,
    pub source_revision: String,
    pub jev_ready: bool,
}

fn path(kota_home: &Path) -> PathBuf {
    kota_home.join("heroes/system-puppeteer/puppeteer-soul.json")
}

fn read_source(kota_home: &Path) -> LocalResult<Option<Vec<u8>>> {
    let path = path(kota_home);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LocalError::new("soul_settings_read_failed")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024 {
        return Err(LocalError::new("soul_settings_invalid"));
    }
    let mut options = fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options.open(path).and_then(|file| file.take(1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|_| LocalError::new("soul_settings_read_failed"))?;
    if bytes.len() > 1024 * 1024 { return Err(LocalError::new("soul_settings_invalid")); }
    Ok(Some(bytes))
}

/// Reading defaults never creates a directory/file or rewrites old settings.
/// The backend default is enabled=true; readiness gates its effective state.
pub(crate) fn read_snapshot(kota_home: &Path, jev_ready: bool) -> LocalResult<Snapshot> {
    let source = read_source(kota_home)?;
    let settings = match source.as_deref() {
        None => Settings::default(),
        Some(bytes) => serde_json::from_slice::<Settings>(bytes)
            .map_err(|_| LocalError::new("soul_settings_invalid"))?,
    };
    settings.validate().map_err(LocalError::new)?;
    Ok(Snapshot { settings, source_revision: crate::puppeteer_setup::revision(source.as_deref()), jev_ready })
}

/// `jev_state` is a local readiness read, never provider verification. It is
/// injected so tests cannot accidentally touch credentials or make HTTP calls.
pub(crate) fn save_snapshot(
    kota_home: &Path,
    settings: &Settings,
    source_revision: &str,
    jev_state: impl FnOnce() -> LocalResult<bool>,
) -> LocalResult<Snapshot> {
    settings.validate().map_err(LocalError::new)?;
    let path = path(kota_home);
    let directory = path.parent().ok_or_else(|| LocalError::new("soul_settings_write_failed"))?;
    fs::create_dir_all(directory).map_err(|_| LocalError::new("soul_settings_write_failed"))?;
    // Match other global config writers' cross-App file lock; never hold it
    // over HTTP. A busy/stale editor fails explicitly and doesn't auto-retry.
    let _lock = FileLock::acquire(&directory.join(".puppeteer-soul.lock"), "soul_settings_write_failed")
        .map_err(|_| LocalError::new("soul_settings_write_failed"))?;
    let before = read_source(kota_home)?;
    if crate::puppeteer_setup::revision(before.as_deref()) != source_revision {
        return Err(LocalError::new("soul_settings_conflict"));
    }
    let readiness = jev_state();
    // A rejected/missing/corrupt Jev store must never prevent switching off.
    let jev_ready = if settings.enabled { readiness? } else { readiness.unwrap_or(false) };
    if settings.enabled && !jev_ready { return Err(LocalError::new("jev_key_required")); }
    let bytes = format!("{}\n", serde_json::to_string_pretty(settings)
        .map_err(|_| LocalError::new("soul_settings_invalid"))?).into_bytes();
    if read_source(kota_home)? != before { return Err(LocalError::new("soul_settings_conflict")); }
    crate::adapter_sync::atomic_replace(&path, &bytes)
        .map_err(|_| LocalError::new("soul_settings_write_failed"))?;
    Ok(Snapshot { settings: settings.clone(), source_revision: crate::puppeteer_setup::revision(Some(&bytes)), jev_ready })
}

pub(crate) fn read(kota_home: &Path) -> LocalResult<Snapshot> {
    let ready = jev::read(&OwnedDirectory::new(kota_home))?.ready;
    read_snapshot(kota_home, ready)
}

#[tauri::command]
pub(crate) async fn puppeteer_soul_read() -> LocalResult<Snapshot> {
    tauri::async_runtime::spawn_blocking(|| read(&crate::kota_home_dir()))
        .await.map_err(|_| LocalError::new("soul_settings_read_failed"))?
}

#[tauri::command]
pub(crate) async fn puppeteer_soul_save(app: AppHandle, settings: Settings, source_revision: String) -> LocalResult<Snapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let kota_home = crate::kota_home_dir();
        let snapshot = save_snapshot(&kota_home, &settings, &source_revision, || {
            jev::gate(&OwnedDirectory::new(&kota_home))?;
            Ok(true)
        })?;
        let _ = app.emit("puppeteer-soul://changed", &snapshot);
        Ok(snapshot)
    }).await.map_err(|_| LocalError::new("soul_settings_write_failed"))?
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
