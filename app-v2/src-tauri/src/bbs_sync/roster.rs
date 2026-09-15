//! Public directory models and lease-free reads. Network exchange and the UI
//! cache use these exact types; neither CLI queries nor refs initialize sync.
use super::{control, raw_sha256, safe_id, valid_hash, StateStore};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

mod images;
pub(crate) mod public;
pub(crate) mod reference;
pub(crate) mod runtime;
pub(crate) mod wire;

pub(crate) const MAX_AVATAR_BYTES: u64 = 600_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Avatar {
    Builtin {
        id: String,
    },
    Image {
        sha256: String,
        ext: String,
        size_bytes: u64,
    },
    None,
}
impl Avatar {
    pub(crate) fn resource(&self) -> Option<super::transport::Resource> {
        let Self::Image {
            sha256,
            ext,
            size_bytes,
        } = self
        else {
            return None;
        };
        Some(super::transport::Resource {
            identity: super::transport::ResourceKind::Avatar {
                sha256: sha256.clone(),
                ext: ext.clone(),
            },
            sha256: sha256.clone(),
            size_bytes: *size_bytes,
        })
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let mut sidecar = super::AvatarSidecar::none();
        match self {
            // Future builtins remain safe display IDs in rosters, while old
            // immutable post origins retain their existing strict whitelist.
            Self::Builtin { id } if !super::BUILTIN_AVATARS.contains(&id.as_str()) => {
                return safe_id(id).map_err(anyhow::Error::msg);
            }
            Self::Builtin { id } => {
                sidecar.kind = "builtin".into();
                sidecar.id = Some(id.clone());
            }
            Self::Image {
                sha256,
                ext,
                size_bytes,
            } => {
                sidecar.kind = "image".into();
                sidecar.sha256 = Some(sha256.clone());
                sidecar.ext = Some(ext.clone());
                sidecar.size_bytes = Some(*size_bytes);
            }
            Self::None => {}
        }
        sidecar.validate().map_err(anyhow::Error::msg)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Agent {
    pub(crate) agent_id: String,
    pub(crate) name: String,
    pub(crate) avatar: Avatar,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Project {
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) agents: Vec<Agent>,
}

pub(crate) fn validate_projects(projects: &[Project]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for project in projects {
        safe_id(&project.project_id).map_err(anyhow::Error::msg)?;
        if project.name.trim().is_empty() || !ids.insert(&project.project_id) {
            bail!("invalid_roster_project");
        }
        let mut agents = BTreeSet::new();
        for agent in &project.agents {
            safe_id(&agent.agent_id).map_err(anyhow::Error::msg)?;
            if agent.name.trim().is_empty() || !agents.insert(&agent.agent_id) {
                bail!("invalid_roster_agent");
            }
            agent.avatar.validate()?;
        }
    }
    Ok(())
}

/// Stable content version: no timestamps, presence, paths, sessions or image
/// availability. JSON goes straight into SHA rather than a second large buffer.
pub(crate) fn version(projects: &[Project]) -> Result<String> {
    json_hash(projects)
}
fn json_hash(value: &(impl Serialize + ?Sized)) -> Result<String> {
    use sha2::{Digest, Sha256};
    struct Hash(Sha256);
    impl std::io::Write for Hash {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Hash(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(format!("{:x}", writer.0.finalize()))
}

pub(crate) fn collect_local(account: &Path) -> Result<Vec<Project>> {
    let directory = crate::agent_directory::collect(account)?.complete()?;
    let avatars = crate::read_user_avatar_index(&account.join("avatars"));
    let mut resolved = BTreeMap::new();
    let mut result = Vec::new();
    for project in directory {
        let mut agents = Vec::new();
        for agent in project.agents {
            let avatar = agent
                .avatar_id
                .as_deref()
                .map(|id| {
                    resolved
                        .entry(id.to_string())
                        .or_insert_with(|| {
                            registered_avatar(account, avatars.as_deref().unwrap_or(&[]), id)
                                .unwrap_or(Avatar::None)
                        })
                        .clone()
                })
                .unwrap_or(Avatar::None);
            agents.push(Agent {
                agent_id: agent.id,
                name: agent.name,
                avatar,
            });
        }
        result.push(Project {
            project_id: project.id,
            name: project.name,
            agents,
        });
    }
    validate_projects(&result)?;
    Ok(result)
}

fn registered_avatar(
    account: &Path,
    index: &[crate::StoredUserHeroAvatar],
    id: &str,
) -> Result<Avatar> {
    if !id.starts_with("user:") && safe_id(id).is_ok() {
        return Ok(Avatar::Builtin { id: id.into() });
    }
    let item = index
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| anyhow!("avatar_unavailable"))?;
    if Path::new(&item.file_name).components().count() != 1 || item.file_name.starts_with('.') {
        bail!("invalid_avatar_reference");
    }
    let ext = match item.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => bail!("invalid_avatar_type"),
    };
    let path = account.join("avatars").join(&item.file_name);
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() == 0 || before.len() > MAX_AVATAR_BYTES {
        bail!("invalid_avatar_size");
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_AVATAR_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let stamp = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if bytes.len() as u64 != before.len()
        || stamp(&before) != stamp(&file.metadata()?)
        || stamp(&before) != stamp(&fs::symlink_metadata(&path)?)
    {
        bail!("avatar_changed");
    }
    Ok(Avatar::Image {
        sha256: raw_sha256(&bytes),
        ext: ext.into(),
        size_bytes: before.len(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Member {
    pub(crate) device_id: String,
    pub(crate) membership_id: String,
    pub(crate) name: String,
    pub(crate) online: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Members {
    pub(crate) schema_version: u32,
    pub(crate) group_id: String,
    pub(crate) membership_id: String,
    pub(crate) members: Vec<Member>,
}
impl Members {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported_roster_schema");
        }
        if self.members.len() > 32 {
            bail!("invalid_roster_members");
        }
        safe_id(&self.group_id).map_err(anyhow::Error::msg)?;
        safe_id(&self.membership_id).map_err(anyhow::Error::msg)?;
        let mut ids = BTreeSet::new();
        for member in &self.members {
            valid_hash(&member.device_id).map_err(anyhow::Error::msg)?;
            safe_id(&member.membership_id).map_err(anyhow::Error::msg)?;
            if member.name.trim().is_empty() || !ids.insert(&member.device_id) {
                bail!("invalid_roster_member");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PeerRoster {
    pub(crate) schema_version: u32,
    pub(crate) group_id: String,
    pub(crate) device_id: String,
    pub(crate) membership_id: String,
    pub(crate) version: String,
    pub(crate) received_at: String,
    pub(crate) projects: Vec<Project>,
}
impl PeerRoster {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported_roster_schema");
        }
        safe_id(&self.group_id).map_err(anyhow::Error::msg)?;
        safe_id(&self.membership_id).map_err(anyhow::Error::msg)?;
        valid_hash(&self.device_id).map_err(anyhow::Error::msg)?;
        valid_hash(&self.version).map_err(anyhow::Error::msg)?;
        chrono::DateTime::parse_from_rfc3339(&self.received_at)?;
        validate_projects(&self.projects)?;
        if version(&self.projects)? != self.version {
            bail!("roster_hash_mismatch");
        }
        Ok(())
    }
}

/// Private read projection; no ControlClient / lease / initialization. Identity
/// corruption is not interpreted as a new, identity-free account.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) device_id: Option<String>,
    pub(crate) device_name: String,
    pub(crate) membership: Option<control::Membership>,
    pub(crate) members: Vec<Member>,
}
pub(crate) fn context(store: &StateStore) -> Result<Context> {
    let existing = control::inspect_existing(store).map_err(anyhow::Error::msg)?;
    let (device_id, device_name, membership) = match existing {
        Some((identity, name, member, _)) => (
            Some(identity.device_id().map_err(anyhow::Error::msg)?),
            name,
            member,
        ),
        None => (None, "This device".into(), None),
    };
    let mut members = Vec::new();
    if let Some(current) = &membership {
        if let Some(cached) = super::load(&store.root.join("rosters/members.json"), |v: Members| {
            v.validate()
                .map_err(|_| "invalid_roster_members".to_string())?;
            Ok(v)
        })
        .map_err(anyhow::Error::msg)?
        {
            if cached.group_id == current.group_id && cached.membership_id == current.membership_id
            {
                members = cached.members;
            }
        }
    }
    Ok(Context {
        device_id,
        device_name,
        membership,
        members,
    })
}

pub(crate) fn read_peer(store: &StateStore, ctx: &Context, id: &str) -> Result<Option<PeerRoster>> {
    read_peer_with_io(store, ctx, id, None)
}
fn read_peer_with_io(
    store: &StateStore,
    ctx: &Context,
    id: &str,
    io: Option<&mut super::transport::LocalIo>,
) -> Result<Option<PeerRoster>> {
    valid_hash(id).map_err(anyhow::Error::msg)?;
    let Some(current) = &ctx.membership else {
        return Ok(None);
    };
    let Some(member) = ctx.members.iter().find(|m| m.device_id == id) else {
        return Ok(None);
    };
    let path = store.root.join("rosters").join(format!("{id}.json"));
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("invalid_peer_roster");
    }
    let before = crate::bbs_sync::FileStamp::of(&before);
    // The complete model is cacheable, but there is no second whole-file JSON
    // buffer. The receiver has already streamed and validated bounded pages.
    struct Budgeted<'a> {
        file: &'a mut fs::File,
        io: Option<&'a mut super::transport::LocalIo>,
    }
    impl Read for Budgeted<'_> {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            if let Some(io) = &mut self.io {
                io.check().map_err(std::io::Error::other)?;
            }
            let n = self.file.read(target)?;
            if let Some(io) = &mut self.io {
                io.charge(n).map_err(std::io::Error::other)?;
            }
            Ok(n)
        }
    }
    let cached: PeerRoster = serde_json::from_reader(std::io::BufReader::new(Budgeted {
        file: &mut file,
        io,
    }))
    .map_err(|_| anyhow!("invalid_peer_roster"))?;
    cached.validate()?;
    if crate::bbs_sync::FileStamp::of(&file.metadata()?) != before
        || crate::bbs_sync::FileStamp::of(&fs::symlink_metadata(&path)?) != before
    {
        bail!("roster_changed");
    }
    let cached = Some(cached);
    Ok(cached.filter(|v| {
        v.group_id == current.group_id
            && v.device_id == id
            && v.membership_id == member.membership_id
    }))
}

/// Called by the existing control owner after it has authenticated membership.
/// This is public cache data only, saved under the same short group fence used
/// by publication. It never serializes RemoteResponse, credentials or a URL.
pub(crate) fn persist_members(
    store: &StateStore,
    current: Option<&control::Membership>,
    members: &[control::Member],
) -> Result<()> {
    let next = current.map(|current| Members {
        schema_version: 1,
        group_id: current.group_id.clone(),
        membership_id: current.membership_id.clone(),
        members: members
            .iter()
            .map(|m| Member {
                device_id: m.device_id.clone(),
                membership_id: m.membership_id.clone(),
                name: m.name.clone(),
                online: m.online,
            })
            .collect(),
    });
    if let Some(next) = &next {
        next.validate()?;
    }
    let path = store.root.join("rosters/members.json");
    if next.is_none() && !path.parent().unwrap().exists() {
        return Ok(());
    }
    wire::directory(store)?;
    let lock = store.content_lock().map_err(anyhow::Error::msg)?;
    if control::read_membership(store)
        .map_err(anyhow::Error::msg)?
        .as_ref()
        != current
    {
        bail!("group_context_changed");
    }
    let previous = super::load(&path, |v: Members| {
        v.validate()
            .map_err(|_| "invalid_roster_members".to_string())?;
        Ok(v)
    })
    .map_err(anyhow::Error::msg)?;
    let same_scope = matches!((&previous, &next), (Some(a), Some(b))
        if a.group_id == b.group_id && a.membership_id == b.membership_id);
    let incarnations = |value: &Members| {
        value
            .members
            .iter()
            .map(|m| (m.device_id.clone(), m.membership_id.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let before = previous.as_ref().map(incarnations).unwrap_or_default();
    let after = next.as_ref().map(incarnations).unwrap_or_default();
    // Only membership changes enumerate cache files. Name/presence heartbeats
    // update the small members file without walking the roster directory.
    if !same_scope || before != after {
        let entries = match fs::read_dir(path.parent().unwrap()) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut removed = false;
        if let Some(entries) = entries {
            for entry in entries {
                let entry = entry?;
                let filename = entry.file_name();
                let Some(id) = filename
                    .to_str()
                    .and_then(|s| s.strip_suffix(".json"))
                    .filter(|id| valid_hash(id).is_ok())
                else {
                    continue;
                };
                let retained =
                    same_scope && after.get(id).is_some_and(|v| before.get(id) == Some(v));
                if !retained {
                    fs::remove_file(entry.path())?;
                    removed = true;
                }
            }
        }
        // Retire previous incarnations before publishing the new authority.
        if removed {
            fs::File::open(path.parent().unwrap())?.sync_all()?;
        }
    }
    if let Some(next) = next {
        let same = previous.is_some_and(|v| {
            v.group_id == next.group_id
                && v.membership_id == next.membership_id
                && v.members == next.members
        });
        if !same {
            super::write(&path, &next).map_err(anyhow::Error::msg)?;
        }
    } else {
        match fs::remove_file(&path) {
            Ok(()) => fs::File::open(path.parent().unwrap())?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    drop(lock);
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PublicAvatar {
    Builtin {
        id: String,
    },
    Image {
        sha256: String,
        ext: String,
        size_bytes: u64,
        available: bool,
    },
    None,
}
impl PublicAvatar {
    pub(crate) fn new(avatar: &Avatar, available: bool) -> Self {
        match avatar {
            Avatar::Builtin { id } => Self::Builtin { id: id.clone() },
            Avatar::Image {
                sha256,
                ext,
                size_bytes,
            } => Self::Image {
                sha256: sha256.clone(),
                ext: ext.clone(),
                size_bytes: *size_bytes,
                available,
            },
            Avatar::None => Self::None,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub agent_id: String,
    pub name: String,
    pub target_ref: String,
    pub avatar: PublicAvatar,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub project_id: String,
    pub name: String,
    pub agents: Vec<AgentView>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub device_id: Option<String>,
    pub name: String,
    pub local: bool,
    pub online: bool,
    pub roster_status: &'static str,
    pub received_at: Option<String>,
    pub projects: Option<Vec<ProjectView>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DirectoryView {
    pub devices: Vec<DeviceView>,
}
fn project_views(projects: &[Project], device: &str, local: bool) -> Vec<ProjectView> {
    projects
        .iter()
        .map(|p| ProjectView {
            project_id: p.project_id.clone(),
            name: p.name.clone(),
            agents: p
                .agents
                .iter()
                .map(|a| AgentView {
                    agent_id: a.agent_id.clone(),
                    name: a.name.clone(),
                    target_ref: format!("{device}/{}/{}", p.project_id, a.agent_id),
                    avatar: PublicAvatar::new(&a.avatar, local),
                })
                .collect(),
        })
        .collect()
}
pub(crate) fn directory(account: &Path, store: &StateStore) -> Result<DirectoryView> {
    let ctx = context(store)?;
    let local = collect_local(account)?;
    let mut devices = vec![DeviceView {
        device_id: ctx.device_id.clone(),
        name: ctx.device_name.clone(),
        local: true,
        online: true,
        roster_status: "synced",
        received_at: None,
        projects: Some(project_views(
            &local,
            ctx.device_id.as_deref().unwrap_or("local"),
            true,
        )),
    }];
    let content =
        crate::bbs::sync::ContentStore::at(store.content_root.clone(), account.to_path_buf());
    let mut images = BTreeMap::new();
    for member in &ctx.members {
        if ctx.device_id.as_deref() == Some(&member.device_id) {
            continue;
        }
        let roster = read_peer(store, &ctx, &member.device_id)?;
        devices.push(DeviceView {
            device_id: Some(member.device_id.clone()),
            name: member.name.clone(),
            local: false,
            online: member.online,
            roster_status: if roster.is_some() {
                "synced"
            } else {
                "not_synced"
            },
            received_at: roster.as_ref().map(|r| r.received_at.clone()),
            projects: roster.map(|r| {
                let mut projects = project_views(&r.projects, &member.device_id, false);
                for (source, project) in r.projects.iter().zip(&mut projects) {
                    for (agent, view) in source.agents.iter().zip(&mut project.agents) {
                        if let (Some(resource), PublicAvatar::Image { available, .. }) =
                            (agent.avatar.resource(), &mut view.avatar)
                        {
                            // CLI is an explicit one-shot read. App uses its
                            // stamp cache; neither path acquires ControlLease.
                            let key = serde_json::to_string(&resource).unwrap();
                            *available = *images
                                .entry(key)
                                .or_insert_with(|| content.read_roster_avatar(&resource).is_ok());
                        }
                    }
                }
                projects
            }),
        });
    }
    Ok(DirectoryView { devices })
}

#[cfg(test)]
pub(crate) mod tests;
