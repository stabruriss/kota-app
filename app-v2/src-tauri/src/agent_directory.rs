//! Read-only account directory shared by LM and BBS. Never prepares a workspace,
//! reads launch configuration, starts a terminal, or creates account state.
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Agent {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) avatar_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Project {
    pub(crate) id: String,
    pub(crate) name: String,
    // Local routing only. These types deliberately do not implement Serialize.
    pub(crate) root: PathBuf,
    pub(crate) agents: Vec<Agent>,
}
#[derive(Default)]
pub(crate) struct Directory {
    pub(crate) projects: Vec<Project>,
    pub(crate) incomplete: bool,
}
impl Directory {
    pub(crate) fn complete(self) -> Result<Vec<Project>> {
        if self.incomplete {
            return Err(anyhow!("agent_directory_unavailable"));
        }
        Ok(self.projects)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    project_id: String,
    repo_full_name: String,
    #[serde(default)]
    archived: bool,
}

pub(crate) fn visible(status: &str) -> bool {
    !matches!(
        status.trim().to_lowercase().as_str(),
        "archived" | "deleted" | "dismissed" | "removed"
    )
}

pub(crate) fn agent_fields(id: &str, mapping: &serde_yaml::Mapping) -> Option<Agent> {
    let string = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            mapping
                .get(serde_yaml::Value::from(*key))
                .and_then(serde_yaml::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
    };
    if !visible(string(&["status"]).unwrap_or("active")) {
        return None;
    }
    Some(Agent {
        id: id.into(),
        name: string(&["display-name", "displayName"])
            .unwrap_or(id)
            .into(),
        avatar_id: string(&["avatar-id", "avatarId"]).map(str::to_string),
    })
}

fn directories(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            paths.push(entry.path());
        }
        if paths.len() % 64 == 0 {
            std::thread::yield_now();
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn collect(account: &Path) -> Result<Directory> {
    let mut result = Directory::default();
    for root in directories(&account.join("Workspaces"))? {
        match collect_project(&root) {
            Ok(Some(project)) => result.projects.push(project),
            Ok(None) => {}
            Err(_) => result.incomplete = true,
        }
    }
    result.projects.sort_by(|a, b| a.id.cmp(&b.id));
    if result.projects.windows(2).any(|p| p[0].id == p[1].id) {
        result.incomplete = true;
    }
    Ok(result)
}

fn registration(root: &Path) -> Result<Option<Project>> {
    let file = match fs::File::open(root.join("workspace.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Ignored private fields stream past the deserializer, not into a public DTO.
    let registration: Registration = serde_json::from_reader(std::io::BufReader::new(file))?;
    if registration.archived {
        return Ok(None);
    }
    crate::bbs_sync::safe_id(&registration.project_id).map_err(anyhow::Error::msg)?;
    let label = registration
        .repo_full_name
        .trim()
        .rsplit(['/', '\\'])
        .next();
    let project = Project {
        name: crate::bbs::display_project_name_with_fallback(&registration.project_id, label),
        id: registration.project_id,
        root: root.to_path_buf(),
        agents: Vec::new(),
    };
    Ok(Some(project))
}

fn collect_project(root: &Path) -> Result<Option<Project>> {
    let Some(mut project) = registration(root)? else { return Ok(None) };
    // Same registered-agent discovery as sync_workspace_agent_specs_from_disk:
    // actual agent.yaml directories, including valid newly recruited agents.
    for cwd in directories(&root.join(".agent-workspaces"))? {
        let Some(id) = cwd.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = match fs::read_to_string(cwd.join("agent.yaml")) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        crate::bbs_sync::safe_id(id).map_err(anyhow::Error::msg)?;
        let yaml: serde_yaml::Value = serde_yaml::from_str(&text)?;
        let mapping = yaml
            .as_mapping()
            .filter(|m| !m.is_empty())
            .ok_or_else(|| anyhow!("invalid_agent_metadata"))?;
        if let Some(agent) = agent_fields(id, mapping) {
            project.agents.push(agent);
        }
    }
    project.agents.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Some(project))
}

/// The destination is a registered ID, never an author-provided path/name.
/// Reading only the target metadata lets a missing/bad agent produce the
/// existing notice in its still-valid room without preparing that project.
pub(crate) fn notification_target(account: &Path, project_id: &str, agent_id: &str)
    -> Result<Option<(Project, Option<Agent>)>>
{
    crate::bbs_sync::safe_id(project_id).map_err(anyhow::Error::msg)?;
    crate::bbs_sync::safe_id(agent_id).map_err(anyhow::Error::msg)?;
    let mut matched = None;
    for root in directories(&account.join("Workspaces"))? {
        let project = match registration(&root) {
            Ok(Some(project)) if project.id == project_id => project,
            _ => continue,
        };
        if matched.is_some() { return Err(anyhow!("ambiguous_project_registration")); }
        let agent = fs::read_to_string(root.join(".agent-workspaces").join(agent_id).join("agent.yaml"))
            .ok().and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(&s).ok())
            .and_then(|v| v.as_mapping().filter(|m| !m.is_empty()).and_then(|m| agent_fields(agent_id, m)));
        matched = Some((project, agent));
    }
    Ok(matched)
}

#[cfg(test)]
pub(crate) mod tests;
