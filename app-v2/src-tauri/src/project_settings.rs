//! Local, per-project preferences. Kept outside the source repository and
//! separate from workspace snapshots so background refreshes cannot undo a save.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "project-settings.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    #[serde(default)]
    pub commit_email: Option<String>,
}

pub fn read(project_root: &Path) -> Result<ProjectSettings> {
    let bytes = match fs::read(project_root.join(FILE_NAME)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(ProjectSettings::default()),
        Err(error) => return Err(error).context("Read project settings"),
    };
    let mut settings: ProjectSettings =
        serde_json::from_slice(&bytes).context("Read project settings JSON")?;
    settings.commit_email = settings.commit_email.filter(|value| !value.is_empty());
    Ok(settings)
}

/// Optional preferences must not prevent normal work. A valid unset value
/// preserves the caller's legacy behavior; a read failure explicitly selects
/// its default email. Settings IPC continues to use the strict reader above.
pub fn runtime_commit_email(project_root: &Path, default_email: &str) -> Option<String> {
    // A local project keeps its saved email for a later connection but commits as before.
    if crate::integrations::project_is_local(project_root) {
        return None;
    }
    match read(project_root) {
        Ok(settings) => settings.commit_email,
        Err(error) => {
            crate::kota_debug_log(&format!(
                "[project-settings] warning: cannot read commit email for {}; using default Git identity: {error}",
                project_root.display(),
            ));
            Some(default_email.to_owned())
        }
    }
}

pub fn save_commit_email(project_root: &Path, email: Option<String>) -> Result<ProjectSettings> {
    // No email format/ownership validation or trimming. An empty input removes
    // the override; every other string is passed as data, never shell source.
    let mut settings = read(project_root)?;
    settings.commit_email = email.filter(|value| !value.is_empty());
    let temporary = project_root.join(format!(".project-settings-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .context("Create project settings file")?;
        file.write_all(&serde_json::to_vec_pretty(&settings)?)?;
        file.sync_all()?;
        fs::rename(&temporary, project_root.join(FILE_NAME)).context("Save project settings")?;
        Ok(settings)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_email_persists_without_format_validation_and_is_project_scoped() {
        let root = std::env::temp_dir().join(format!("settings-{}", uuid::Uuid::new_v4()));
        let other = root.join("other");
        fs::create_dir_all(&other).unwrap();
        assert_eq!(read(&root).unwrap(), ProjectSettings::default());
        for email in [
            "mail@fable.example",
            "任意字符串",
            "  arbitrary text  ",
            "$(not-a-command)",
        ] {
            save_commit_email(&root, Some(email.into())).unwrap();
            assert_eq!(read(&root).unwrap().commit_email.as_deref(), Some(email));
            assert_eq!(read(&other).unwrap().commit_email, None);
        }
        save_commit_email(&root, None).unwrap();
        assert_eq!(read(&root).unwrap().commit_email, None);
        save_commit_email(&root, Some(String::new())).unwrap();
        assert_eq!(read(&root).unwrap().commit_email, None);
        fs::write(root.join(FILE_NAME), "{}").unwrap();
        assert_eq!(read(&root).unwrap(), ProjectSettings::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_settings_are_reported_not_silently_replaced() {
        let root = std::env::temp_dir().join(format!("settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(FILE_NAME), "broken").unwrap();
        assert!(read(&root).is_err());
        assert!(save_commit_email(&root, Some("any text".into())).is_err());
        assert_eq!(fs::read_to_string(root.join(FILE_NAME)).unwrap(), "broken");
        fs::remove_dir_all(root).unwrap();
    }
}
