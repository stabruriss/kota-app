//! An invitation is a snapshot of one project incarnation, not a GHOST hash.
use super::{
    sha256_hex, tavern_display_name_key, tavern_profile_reserves_display_name,
    unique_name_with_roman_suffix, ProjectAgentInviteEligibility, ProjectAgentInviteResult,
    TavernHeroProfileDraft, TavernHeroSource,
};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(super) fn hero_id(project_id: Option<&str>, project_root: &Path, agent_id: &str) -> String {
    // Registered project IDs survive directory moves. Legacy projects use their resolved root.
    let (kind, project) = match project_id {
        Some(id) => ("project", id.to_owned()),
        None => ("root", project_root.to_string_lossy().into_owned()),
    };
    let identity = serde_json::to_vec(&(kind, project, agent_id)).expect("string tuple serializes");
    format!("hero-invited-{}", sha256_hex(&identity))
}

fn existing(root: &Path, id: &str) -> Result<Option<TavernHeroProfileDraft>, String> {
    let path = root.join(id).join("hero.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let profile: TavernHeroProfileDraft = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if profile.hero_id != id {
        return Err("invited hero identity does not match its directory".into());
    }
    Ok(Some(profile))
}

pub(super) fn eligibility(
    root: &Path,
    id: String,
    name: &str,
    ghost: &str,
) -> Result<ProjectAgentInviteEligibility, String> {
    let saved = existing(root, &id)?;
    let reason = if saved.is_some() {
        Some("This incarnation is already in Tavern.".into())
    } else if ghost.trim().is_empty() {
        Some("GHOST/persona section is empty".into())
    } else {
        None
    };
    Ok(ProjectAgentInviteEligibility {
        eligible: reason.is_none(),
        reason,
        duplicate_hero_id: saved.as_ref().map(|profile| profile.hero_id.clone()),
        proposed_hero_id: id,
        proposed_display_name: saved
            .map(|profile| profile.name)
            .unwrap_or_else(|| name.into()),
    })
}

pub(super) fn snapshot(
    detail: &super::ProjectAgentDetail,
    requested_name: Option<&str>,
) -> Result<TavernHeroProfileDraft, String> {
    let requested_display_name = requested_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&detail.display_name)
        .to_string();
    let shell = fs::read_to_string(&detail.shell_path)
        .map_err(|err| format!("read {}: {err}", detail.shell_path))?;
    Ok(TavernHeroProfileDraft {
        hero_id: detail.invite_eligibility.proposed_hero_id.clone(),
        name_fields: (requested_display_name == detail.display_name)
            .then(|| detail.name_fields.clone())
            .flatten(),
        name: requested_display_name,
        provider: detail.provider.clone(),
        model: detail.model.clone(),
        effort: detail.effort.clone(), auto_route: detail.auto_route.clone(),
        avatar_id: detail.avatar_id.clone(),
        skills: detail.skills.clone(),
        ghost: detail.ghost.clone(),
        shell,
        archived: false,
        dismissed: false,
        kind: Some("invited".into()),
        // The incarnation this hero follows (SOUL-PLAN v4 §17): read live while it exists,
        // kept as a relic when it is dismissed.
        source: Some(TavernHeroSource {
            project_id: Some(detail.project_id.clone()).filter(|id| !id.trim().is_empty()),
            agent_id: detail.agent_id.clone(),
            agent_cwd: Path::new(&detail.agent_yaml_path)
                .parent()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
            dismissed_at: None,
        }),
    })
}

pub(super) fn save(
    root: &Path,
    mut profile: TavernHeroProfileDraft,
) -> Result<ProjectAgentInviteResult, String> {
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    // Serialize check + publish across IPC workers and App processes. File::lock is released
    // on drop/process exit; it is only taken on the blocking invitation path, never status reads.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(".invite.lock"))
        .map_err(|error| format!("open Tavern invitation lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("lock Tavern invitations: {error}"))?;
    if let Some(saved) = existing(root, &profile.hero_id)? {
        return Ok(result(root, &saved, true));
    }
    if profile.name.trim().is_empty() || profile.ghost.trim().is_empty() {
        return Err("An invitation needs a name and a non-empty GHOST/persona section.".into());
    }

    let mut names = std::collections::HashSet::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        // System directories have no hero.json; a missing GHOST never terminates the scan.
        if let Some(other) = existing(root, &entry.file_name().to_string_lossy())? {
            if tavern_profile_reserves_display_name(&other) {
                names.insert(tavern_display_name_key(&other.name));
            }
        }
    }
    let original = profile.name.trim().to_owned();
    profile.name = unique_name_with_roman_suffix(&original, |name| {
        names.contains(&tavern_display_name_key(name))
    });
    if let Some(fields) = profile.name_fields.as_mut() {
        // Keep structured fields consistent with the existing name-collision suffix policy.
        let suffix = &profile.name[original.len()..];
        let last = if !fields.surname.is_empty() {
            &mut fields.surname
        } else if !fields.middle.is_empty() {
            &mut fields.middle
        } else {
            &mut fields.given
        };
        last.push_str(suffix);
    }

    // The staging container is not a hero. Publish all three files with one directory rename;
    // a failed/interrupted invitation leaves no visible half-profile and is safe to retry.
    let staging = root.join(".invite-staging");
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let draft = staging.join(&profile.hero_id);
    fs::create_dir_all(&draft).map_err(|error| error.to_string())?;
    let publish = (|| -> Result<(), String> {
        let metadata = serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?;
        for (name, content) in [
            ("GHOST.md", profile.ghost.as_bytes()),
            ("SHELL.yaml", profile.shell.as_bytes()),
            ("hero.json", metadata.as_slice()),
        ] {
            let mut file = File::create(draft.join(name)).map_err(|error| error.to_string())?;
            file.write_all(content)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
        }
        File::open(&draft)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        fs::rename(&draft, root.join(&profile.hero_id)).map_err(|error| error.to_string())?;
        File::open(root)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if publish.is_err() {
        let _ = fs::remove_dir_all(&draft);
    }
    publish?;
    Ok(result(root, &profile, false))
}

fn result(
    root: &Path,
    profile: &TavernHeroProfileDraft,
    duplicate: bool,
) -> ProjectAgentInviteResult {
    ProjectAgentInviteResult {
        hero_id: profile.hero_id.clone(),
        display_name: profile.name.clone(),
        path: root.join(&profile.hero_id).to_string_lossy().into_owned(),
        duplicate_hero_id: duplicate.then(|| profile.hero_id.clone()),
    }
}

#[cfg(test)]
mod tests;
