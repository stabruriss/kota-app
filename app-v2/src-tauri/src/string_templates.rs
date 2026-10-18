//! Account-local, copy-only String templates. Reads never initialize the store.
use crate::local_rpc::{auto_route::Group, owned::FileLock};
use unicase::UniCase;
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, fs, io::Read, path::{Path, PathBuf}};
use unicode_normalization::UnicodeNormalization;

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Template {
    pub id: String,
    pub name: String,
    pub saved_at: String,
    pub group: Group,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Library {
    pub schema: u8,
    /// Content revision (excluding this field), recomputed on read so a manual
    /// edit cannot keep an old CAS token. Formatting alone is not a change.
    pub revision: String,
    pub templates: Vec<Template>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Error {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    duplicate_id: Option<String>,
}
type Result<T> = std::result::Result<T, Error>;
fn error(code: &'static str) -> Error { Error { code, duplicate_id: None } }
fn invalid() -> Error { error("string_templates_invalid") }

fn path(home: &Path) -> PathBuf { home.join("heroes/system-puppeteer/string-templates.json") }
fn name_key(name: &str) -> UniCase<String> { UniCase::new(name.to_owned()) }

fn validate(library: &mut Library) -> Result<()> {
    if library.schema != 1 { return Err(invalid()); }
    let mut ids = HashSet::new();
    let mut names = HashMap::new();
    for item in &mut library.templates {
        item.name = item.name.nfc().collect::<String>().trim().to_owned();
        if item.name.is_empty() || item.name.chars().count() > 40 || item.name.chars().any(char::is_control)
            || uuid::Uuid::parse_str(&item.id).is_err() || !ids.insert(item.id.clone())
            || chrono::DateTime::parse_from_rfc3339(&item.saved_at).is_err() {
            return Err(invalid());
        }
        // Use the existing group validator; templates must also be complete
        // editor drafts, even though the runtime accepts empty legacy intents.
        item.group.validate().map_err(|_| invalid())?;
        if item.group.models.iter().any(|model| model.intent.trim().is_empty()) { return Err(invalid()); }
        if let Some(id) = names.insert(name_key(&item.name), item.id.clone()) {
            return Err(Error { code: "string_template_name_exists", duplicate_id: Some(id) });
        }
    }
    // Check collisions first so overwriting a name still works at capacity.
    if library.templates.len() > 50 { return Err(error("string_templates_full")); }
    Ok(())
}

fn revision(library: &Library) -> String {
    let bytes = serde_json::to_vec(&(library.schema, &library.templates)).expect("validated JSON types");
    crate::puppeteer_setup::revision(Some(&bytes))
}

fn read_source(home: &Path) -> Result<Option<Vec<u8>>> {
    let path = path(home);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(error("string_templates_read_failed")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES { return Err(invalid()); }
    let mut options = fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options.open(path).and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| error("string_templates_read_failed"))?;
    if bytes.len() as u64 > MAX_BYTES { return Err(invalid()); }
    Ok(Some(bytes))
}

fn decode(source: Option<&[u8]>) -> Result<Library> {
    let Some(bytes) = source else { return Ok(Library { schema: 1, revision: "missing".into(), templates: vec![] }); };
    let mut library: Library = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    validate(&mut library).map_err(|_| invalid())?;
    library.revision = revision(&library);
    Ok(library)
}
fn read(home: &Path) -> Result<Library> { decode(read_source(home)?.as_deref()) }

fn save(home: &Path, mut library: Library) -> Result<Library> {
    let path = path(home);
    let directory = path.parent().ok_or_else(|| error("string_templates_write_failed"))?;
    // Reject symlinked config directories as well as the file itself.
    for dir in [home.join("heroes"), directory.to_owned()] {
        match fs::symlink_metadata(&dir) {
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err(invalid()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(error("string_templates_write_failed")),
            _ => {}
        }
    }
    fs::create_dir_all(directory).map_err(|_| error("string_templates_write_failed"))?;
    let _lock = FileLock::acquire(&directory.join(".string-templates.lock"), "string_templates_write_failed")
        .map_err(|_| error("string_templates_write_failed"))?;
    let before = read_source(home)?;
    if decode(before.as_deref())?.revision != library.revision { return Err(error("string_templates_conflict")); }
    validate(&mut library)?;
    library.revision = revision(&library);
    let bytes = format!("{}\n", serde_json::to_string_pretty(&library).map_err(|_| invalid())?).into_bytes();
    if bytes.len() as u64 > MAX_BYTES { return Err(invalid()); }
    if read_source(home)? != before { return Err(error("string_templates_conflict")); }
    crate::adapter_sync::atomic_replace(&path, &bytes).map_err(|_| error("string_templates_write_failed"))?;
    Ok(library)
}

#[tauri::command]
pub(crate) async fn puppeteer_string_templates_read() -> Result<Library> {
    tauri::async_runtime::spawn_blocking(|| read(&crate::kota_home_dir())).await
        .map_err(|_| error("string_templates_read_failed"))?
}
#[tauri::command]
pub(crate) async fn puppeteer_string_templates_save(library: Library) -> Result<Library> {
    tauri::async_runtime::spawn_blocking(move || save(&crate::kota_home_dir(), library)).await
        .map_err(|_| error("string_templates_write_failed"))?
}

#[cfg(test)]
#[path = "string_templates_tests.rs"]
mod tests;
