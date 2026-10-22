//! Working-directory-first identity resolution for Kota's agent CLIs.
//!
//! When a CLI runs inside `.agent-workspaces/<agent-id>/` (including `project-files/`
//! beneath it), `agent.yaml` and the ancestor `workspace.json` are the source of truth;
//! `KOTA_AGENT_ID`, `KOTA_PROJECT_ID` and `KOTA_AGENT_CWD` act as corroborating checks
//! and fail closed on any mismatch. Outside an agent workspace (such as `/tmp` or a
//! standalone source checkout), resolution returns `Ok(None)` so each CLI falls back to
//! its existing environment-variable behavior.

use anyhow::{bail, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkdirIdentity {
    pub agent_cwd: PathBuf,
    pub agent_id: String,
    pub agent_display_name: Option<String>,
    pub agent_avatar: Option<String>,
    pub project_root: PathBuf,
    pub project_id: String,
    pub project_display_name: String,
}

impl WorkdirIdentity {
    pub(crate) fn display_name_or_id(&self) -> String {
        self.agent_display_name
            .clone()
            .unwrap_or_else(|| self.agent_id.clone())
    }
}

pub(crate) fn resolve(
    cwd: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<WorkdirIdentity>> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let Some(dir) = discover(cwd) else {
        return Ok(None);
    };
    let env_agent_id = non_empty(env("KOTA_AGENT_ID"));
    let env_project_id = non_empty(env("KOTA_PROJECT_ID"));
    let env_agent_cwd = non_empty(env("KOTA_AGENT_CWD"));

    let agent_mismatch = env_agent_id
        .as_deref()
        .is_some_and(|id| id != dir.agent_id);
    let project_mismatch = env_project_id
        .as_deref()
        .is_some_and(|id| id != dir.project_id);
    let cwd_mismatch = env_agent_cwd
        .as_deref()
        .is_some_and(|path| !paths_same(Path::new(path), &dir.agent_cwd));

    if agent_mismatch || project_mismatch || cwd_mismatch {
        let env_cwd_identity = env_agent_cwd
            .as_deref()
            .and_then(|path| discover(Path::new(path)));
        let env_agent = if agent_mismatch {
            env_agent_id.as_deref()
        } else if cwd_mismatch {
            env_cwd_identity
                .as_ref()
                .map(|d| d.agent_id.as_str())
                .or_else(|| {
                    env_agent_cwd
                        .as_deref()
                        .and_then(|p| Path::new(p).file_name())
                        .and_then(|n| n.to_str())
                })
                .or(env_agent_id.as_deref())
        } else {
            env_agent_id.as_deref()
        }
        .unwrap_or("unknown-agent");

        let env_project = if project_mismatch {
            env_project_id.as_deref()
        } else if cwd_mismatch {
            env_cwd_identity
                .as_ref()
                .map(|d| d.project_id.as_str())
                .or(env_project_id.as_deref())
        } else {
            env_project_id.as_deref()
        }
        .unwrap_or("unknown-project");

        bail!(
            "identity mismatch: environment says {env_agent}@{env_project}, working directory says {}@{}",
            dir.agent_id,
            dir.project_id
        );
    }

    Ok(Some(dir))
}

pub(crate) fn discover(cwd: &Path) -> Option<WorkdirIdentity> {
    discover_from_ancestors(cwd).or_else(|| {
        let canonical = cwd.canonicalize().ok()?;
        discover_from_ancestors(&canonical)
    })
}

fn discover_from_ancestors(start: &Path) -> Option<WorkdirIdentity> {
    let agent_cwd = start.ancestors().find(|dir| {
        dir.parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(".agent-workspaces")
            && dir.join("agent.yaml").is_file()
    })?;
    let yaml_path = agent_cwd.join("agent.yaml");
    let agent_id = agent_yaml_string(&yaml_path, &["id", "agent-id", "agentId"]).or_else(|| {
        agent_cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })?;
    let agent_display_name =
        agent_yaml_string(&yaml_path, &["display-name", "displayName"]);
    let agent_avatar = agent_yaml_string(&yaml_path, &["avatar-id", "avatarId"]);

    let project_root = agent_cwd
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("workspace.json").is_file())?;
    let workspace_bytes = fs::read(project_root.join("workspace.json")).ok()?;
    let workspace = serde_json::from_slice::<serde_json::Value>(&workspace_bytes).ok()?;
    let project_id = json_string(&workspace, &["projectId", "project_id"])
        .or_else(|| crate::soul::project_slug(None, Some(project_root)))?;
    let project_display_name = json_string(&workspace, &["displayName", "display_name"])
        .unwrap_or_else(|| crate::bbs::display_project_name_with_fallback(&project_id, None));

    Some(WorkdirIdentity {
        agent_cwd: agent_cwd.to_path_buf(),
        agent_id,
        agent_display_name,
        agent_avatar,
        project_root: project_root.to_path_buf(),
        project_id,
        project_display_name,
    })
}

pub(crate) fn agent_display_name_from_yaml(path: &Path) -> Option<String> {
    agent_yaml_string(path, &["display-name", "displayName"])
}

pub(crate) fn agent_yaml_string(path: &Path, keys: &[&str]) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text).ok()?;
    keys.iter().find_map(|key| {
        yaml.get(*key)
            .and_then(|value| match value {
                serde_yaml::Value::String(s) => Some(s.trim().to_string()),
                serde_yaml::Value::Number(n) => Some(n.to_string()),
                serde_yaml::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
    })
}

fn json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn resolves_from_agent_workspace_and_project_files_and_checks_corroborating_env() {
        let root = std::env::temp_dir().join(format!("kota-cli-id-{}", Uuid::new_v4().simple()));
        let cedar = root.join("cedar-project");
        let birch_cwd = cedar.join(".agent-workspaces/agent-birch");
        let deep = birch_cwd.join("project-files/app-v2/src");
        let maple = root.join("maple-project");
        let maple_cwd = maple.join(".agent-workspaces/agent-maple");
        let outside = root.join("plain-source");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(&maple_cwd).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            cedar.join("workspace.json"),
            r#"{"projectId":"cedar-project","displayName":"Cedar"}"#,
        )
        .unwrap();
        fs::write(
            birch_cwd.join("agent.yaml"),
            "id: agent-birch\ndisplay-name: Birch\navatar-id: user:birch\n",
        )
        .unwrap();
        fs::write(
            maple.join("workspace.json"),
            r#"{"projectId":"maple-project"}"#,
        )
        .unwrap();
        fs::write(
            maple_cwd.join("agent.yaml"),
            "id: agent-maple\ndisplay-name: Maple\n",
        )
        .unwrap();

        // Works with empty env and from deep inside project-files/.
        let resolved = resolve(Some(&deep), |_| None).unwrap().unwrap();
        assert_eq!(resolved.agent_cwd, birch_cwd);
        assert_eq!(resolved.agent_id, "agent-birch");
        assert_eq!(resolved.agent_display_name.as_deref(), Some("Birch"));
        assert_eq!(resolved.agent_avatar.as_deref(), Some("user:birch"));
        assert_eq!(resolved.project_root, cedar);
        assert_eq!(resolved.project_id, "cedar-project");
        assert_eq!(resolved.project_display_name, "Cedar");

        // Mismatch on KOTA_AGENT_CWD alone still names the environment's agent@project.
        let cwd_only = HashMap::from([("KOTA_AGENT_CWD", maple_cwd.display().to_string())]);
        let err = resolve(Some(&deep), |k| cwd_only.get(k).cloned())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "identity mismatch: environment says agent-maple@maple-project, working directory says agent-birch@cedar-project"
        );

        // Outside an agent workspace returns Ok(None).
        assert!(resolve(Some(&outside), |k| cwd_only.get(k).cloned())
            .unwrap()
            .is_none());

        fs::remove_dir_all(root).unwrap();
    }
}
