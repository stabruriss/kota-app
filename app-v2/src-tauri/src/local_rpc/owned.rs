use super::error::{LocalError, LocalResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    io::AsRawFd,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_CONFIG: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct OwnedDirectory {
    pub root: PathBuf,
    pub agent: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    schema: u8,
    pi_version: String,
    installation_uuid: String,
}

impl OwnedDirectory {
    pub fn new(kota_home: &Path) -> Self {
        let root = kota_home.join("providers/kota-local");
        Self {
            agent: root.join("pi-agent"),
            root,
        }
    }
    pub fn auth_path(&self) -> PathBuf {
        self.agent.join("auth.json")
    }

    pub fn validate_existing(&self) -> LocalResult<()> {
        for path in [
            self.root.parent().unwrap(),
            self.root.as_path(),
            self.agent.as_path(),
        ] {
            match fs::symlink_metadata(path) {
                Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                    return Err(LocalError::new("owned_directory_invalid"))
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(LocalError::new("owned_directory_invalid"))
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Called only for an explicit Local action. Read-defaults never calls it.
    pub fn initialize(&self) -> LocalResult<String> {
        self.initialize_observed(&super::query::Query::new(super::auth::QUERY_TIMEOUT))
    }
    pub fn initialize_observed(&self, query: &super::query::Query) -> LocalResult<String> {
        query.check("configuration_lock")?;
        self.validate_existing()?;
        // This parent can also contain other providers. Create it privately
        // when absent, but never change permissions on an existing parent.
        let parent = self.root.parent().unwrap();
        if !parent.exists() {
            directory_private(parent)?;
        }
        for directory in [self.root.as_path(), self.agent.as_path()] {
            directory_private(directory)?;
        }
        let _lock = self.configuration_lock_observed(query)?;
        let metadata_path = self.agent.join("kota-runtime.json");
        let existing_metadata = read_json(&metadata_path, "owned_config_invalid")?;
        let mut metadata = match existing_metadata.clone() {
            Some(value) => serde_json::from_value::<Metadata>(value)
                .map_err(|_| LocalError::new("owned_config_invalid"))?,
            None => Metadata {
                schema: 1,
                pi_version: super::bundle::PI_VERSION.into(),
                installation_uuid: uuid::Uuid::new_v4().to_string(),
            },
        };
        if metadata.schema != 1 || uuid::Uuid::parse_str(&metadata.installation_uuid).is_err() {
            return Err(LocalError::new("owned_config_invalid"));
        }
        // Version changes preserve the installation identity and all auth. No
        // import, migration, or removal of the user's personal Pi directory.
        metadata.pi_version = super::bundle::PI_VERSION.into();
        let metadata_value = serde_json::to_value(&metadata).unwrap();
        if existing_metadata.as_ref() != Some(&metadata_value) {
            write_json(&metadata_path, &metadata_value, "owned_config_invalid")?;
        }
        let settings_path = self.agent.join("settings.json");
        let existing_settings = read_json(&settings_path, "owned_settings_invalid")?;
        let mut settings = existing_settings.clone().unwrap_or_else(|| json!({}));
        let object = settings
            .as_object_mut()
            .ok_or_else(|| LocalError::new("owned_settings_invalid"))?;
        object.insert("steeringMode".into(), json!("all"));
        object.insert("followUpMode".into(), json!("all"));
        let compaction = object.entry("compaction").or_insert_with(|| json!({}));
        compaction
            .as_object_mut()
            .ok_or_else(|| LocalError::new("owned_settings_invalid"))?
            .insert("enabled".into(), json!(true));
        if existing_settings.as_ref() != Some(&settings) {
            write_json(&settings_path, &settings, "owned_settings_invalid")?;
        }
        if read_json(&self.auth_path(), "auth_store_invalid")?.is_none() {
            write_json(&self.auth_path(), &json!({}), "auth_store_invalid")?;
        }
        #[cfg(unix)]
        for path in [&metadata_path, &settings_path, &self.auth_path()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|_| LocalError::new("owned_config_invalid"))?;
        }
        directory_private(&self.agent.join(".kota-auth"))?;
        Ok(metadata.installation_uuid)
    }
    pub fn provider_lock(&self, provider: &str) -> LocalResult<FileLock> {
        // Identity is checked before using it as a filename; native provider
        // IDs, rather than a UI label, determine cross-App ownership.
        if provider.is_empty()
            || provider.len() > 128
            || !provider
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(LocalError::new("invalid_request"));
        }
        FileLock::acquire(
            &self
                .agent
                .join(".kota-auth")
                .join(format!("{provider}.lock")),
            "busy-auth",
        )
    }
    pub(super) fn configuration_lock_observed(&self, query: &super::query::Query) -> LocalResult<FileLock> {
        // Catalog and availability are read in parallel. Serialize only the
        // short configuration mutation; auth owners keep their separate lock.
        loop {
            query.check("configuration_lock").map_err(|error| if error.code == "auth_timeout" {
                LocalError::new("local_config_busy").at("configuration_lock")
            } else { error })?;
            match FileLock::acquire(&self.root.join(".config.lock"), "local_config_busy") {
                Err(error) if error.code == "local_config_busy" => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                result => return result,
            }
        }
    }
}

pub(super) fn directory_private(path: &Path) -> LocalResult<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
        return Err(LocalError::new("owned_directory_invalid"));
    }
    fs::create_dir_all(path).map_err(|_| LocalError::new("owned_directory_invalid"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| LocalError::new("owned_directory_invalid"))?;
    Ok(())
}

/// None means absent, not unreadable. Symlinks and broken JSON fail closed.
pub(super) fn read_json(path: &Path, code: &str) -> LocalResult<Option<Value>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LocalError::new(code)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_CONFIG {
        return Err(LocalError::new(code));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|_| LocalError::new(code))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalError::new(code))?;
    if bytes.len() as u64 > MAX_CONFIG {
        return Err(LocalError::new(code));
    }
    let invalid_code = match code {
        "auth_store_read_failed" => "auth_store_invalid",
        _ => code,
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| LocalError::new(invalid_code))
}

pub(super) fn write_json(path: &Path, value: &Value, code: &str) -> LocalResult<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        return Err(LocalError::new(code));
    }
    let directory = path.parent().ok_or_else(|| LocalError::new(code))?;
    let temporary = directory.join(format!(".kota-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| LocalError::new(code))?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|_| LocalError::new(code))?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|_| LocalError::new(code))?;
        fs::rename(&temporary, path).map_err(|_| LocalError::new(code))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) struct FileLock(File);
impl FileLock {
    pub fn acquire(path: &Path, busy_code: &str) -> LocalResult<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|_| LocalError::new("owned_lock_failed"))?;
        #[cfg(unix)]
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(LocalError::new(
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
                    busy_code
                } else {
                    "owned_lock_failed"
                },
            ));
        }
        Ok(Self(file))
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
