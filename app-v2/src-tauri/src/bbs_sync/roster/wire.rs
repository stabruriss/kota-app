//! Complete direct-peer roster replacement. Pages are bounded; an inbound
//! directory is streamed into a private file rather than accumulated in RAM.
use super::{context, public, validate_projects, version, Agent, Project, StateStore};
use crate::bbs_sync::{
    safe_id,
    transport::{FileIo, LocalIo},
    valid_hash, FileStamp,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};

pub(crate) const PAGE_BUDGET: usize = 12_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Item {
    Project { project_id: String, name: String },
    Agent { project_id: String, agent: Agent },
}
impl Item {
    fn validate(&self) -> Result<()> {
        let (project_id, name) = match self {
            Self::Project { project_id, name } => (project_id, name),
            Self::Agent { project_id, agent } => {
                safe_id(&agent.agent_id).map_err(anyhow::Error::msg)?;
                agent.avatar.validate()?;
                (project_id, &agent.name)
            }
        };
        safe_id(project_id).map_err(anyhow::Error::msg)?;
        if name.trim().is_empty() {
            bail!("invalid_roster_name");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub version: String,
    pub items: Vec<Item>,
    pub next: Option<String>,
}

pub(crate) struct Local {
    pub(crate) version: String,
    pub(crate) projects: Arc<Vec<Project>>,
    items: Vec<Item>,
    sizes: Vec<usize>,
}
impl Local {
    pub(crate) fn new(projects: Vec<Project>) -> Result<Self> {
        validate_projects(&projects)?;
        let mut items = Vec::new();
        let mut previous = None;
        for p in &projects {
            if previous
                .as_ref()
                .is_some_and(|old: &&str| *old >= p.project_id.as_str())
            {
                bail!("unordered_roster");
            }
            previous = Some(p.project_id.as_str());
            items.push(Item::Project {
                project_id: p.project_id.clone(),
                name: p.name.clone(),
            });
            let mut last = None;
            for a in &p.agents {
                if last
                    .as_ref()
                    .is_some_and(|old: &&str| *old >= a.agent_id.as_str())
                {
                    bail!("unordered_roster");
                }
                last = Some(a.agent_id.as_str());
                items.push(Item::Agent {
                    project_id: p.project_id.clone(),
                    agent: a.clone(),
                });
            }
        }
        let sizes = items
            .iter()
            .map(|item| serde_json::to_vec(item).map(|b| b.len()))
            .collect::<Result<Vec<_>, _>>()?;
        let version = version(&projects)?;
        let overhead = serde_json::to_vec(&Page {
            version: version.clone(),
            items: vec![],
            next: Some(usize::MAX.to_string()),
        })?
        .len();
        if sizes.iter().any(|n| n + overhead > PAGE_BUDGET) {
            bail!("roster_item_too_large");
        }
        Ok(Self {
            version,
            projects: Arc::new(projects),
            items,
            sizes,
        })
    }
    pub(crate) fn page(&self, after: Option<&str>) -> Result<Page> {
        let start = match after {
            None => 0,
            Some(after) => public::offset(after)
                .filter(|n| *n < self.items.len())
                .ok_or_else(|| anyhow!("invalid_roster_cursor"))?,
        };
        let mut page = Page {
            version: self.version.clone(),
            items: vec![],
            next: None,
        };
        let mut used = serde_json::to_vec(&Page {
            version: self.version.clone(),
            items: vec![],
            next: Some(usize::MAX.to_string()),
        })?
        .len();
        let mut end = start;
        while end < self.items.len() && page.items.len() < public::PAGE_ITEMS {
            let extra = self.sizes[end] + usize::from(!page.items.is_empty());
            if used + extra > PAGE_BUDGET {
                break;
            }
            used += extra;
            page.items.push(self.items[end].clone());
            end += 1;
        }
        if end < self.items.len() {
            page.next = Some(end.to_string());
        }
        Ok(page)
    }
}

pub(crate) fn directory(store: &StateStore) -> Result<PathBuf> {
    for path in [store.root.clone(), store.root.join("rosters")] {
        match fs::symlink_metadata(&path) {
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => bail!("unsafe_roster_directory"),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(&path)?
            }
            Err(e) => return Err(e.into()),
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(store.root.join("rosters"))
}

pub(crate) struct Receiver {
    path: PathBuf,
    file_io: FileIo,
    stamp: FileStamp,
    hash: Sha256,
    version: String,
    group: String,
    incarnation: String,
    peer: String,
    peer_incarnation: String,
    offset: usize,
    project: Option<String>,
    last_agent: Option<String>,
    committed: bool,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        if !self.committed {
            self.file_io.remove_later(self.path.clone());
        }
    }
}
impl Receiver {
    pub(crate) fn begin(
        store: &StateStore,
        peer: &str,
        expected_version: &str,
        file_io: FileIo,
    ) -> Result<Self> {
        valid_hash(peer).map_err(anyhow::Error::msg)?;
        valid_hash(expected_version).map_err(anyhow::Error::msg)?;
        let ctx = context(store)?;
        let current = ctx
            .membership
            .ok_or_else(|| anyhow!("group_context_changed"))?;
        if ctx.device_id.as_deref() == Some(peer) {
            bail!("invalid_roster_peer");
        }
        let member = ctx
            .members
            .iter()
            .find(|m| m.device_id == peer)
            .ok_or_else(|| anyhow!("invalid_roster_peer"))?;
        let path = directory(store)?.join(format!(".roster-{}.partial", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let header = format!("{{\"schemaVersion\":1,\"groupId\":{},\"deviceId\":{},\"membershipId\":{},\"version\":{},\"receivedAt\":{},\"projects\":",
            serde_json::to_string(&current.group_id)?,serde_json::to_string(peer)?,serde_json::to_string(&member.membership_id)?,
            serde_json::to_string(expected_version)?,serde_json::to_string(&chrono::Utc::now().to_rfc3339())?);
        if let Err(e) = file
            .write_all(header.as_bytes())
            .and_then(|_| file.write_all(b"["))
        {
            let _ = fs::remove_file(&path);
            return Err(e.into());
        }
        let stamp = FileStamp::of(&file.metadata()?);
        let mut hash = Sha256::new();
        hash.update(b"[");
        Ok(Self {
            path,
            file_io,
            stamp,
            hash,
            version: expected_version.into(),
            group: current.group_id,
            incarnation: current.membership_id,
            peer: peer.into(),
            peer_incarnation: member.membership_id.clone(),
            offset: 0,
            project: None,
            last_agent: None,
            committed: false,
        })
    }
    fn check_member(&self, store: &StateStore) -> Result<()> {
        let ctx = context(store)?;
        if !ctx
            .membership
            .is_some_and(|m| m.group_id == self.group && m.membership_id == self.incarnation)
            || !ctx
                .members
                .iter()
                .any(|m| m.device_id == self.peer && m.membership_id == self.peer_incarnation)
        {
            bail!("group_context_changed");
        }
        Ok(())
    }
    pub(crate) fn append(
        &mut self,
        store: &StateStore,
        page: &Page,
        io: &mut LocalIo,
        authenticated: impl Fn() -> bool,
    ) -> Result<bool> {
        io.check()?;
        if !authenticated() {
            bail!("invalid_roster_peer");
        }
        self.check_member(store)?;
        let bytes = serde_json::to_vec(page)?;
        if bytes.len() > PAGE_BUDGET
            || page.items.len() > public::PAGE_ITEMS
            || page.version != self.version
        {
            bail!("invalid_roster_page");
        }
        let next_offset = self
            .offset
            .checked_add(page.items.len())
            .ok_or_else(|| anyhow!("invalid_roster_cursor"))?;
        if let Some(next) = &page.next {
            if page.items.is_empty() || public::offset(next) != Some(next_offset) {
                bail!("invalid_roster_cursor");
            }
        }
        if page.items.is_empty() && self.offset != 0 {
            bail!("empty_roster_tail");
        }
        io.charge(bytes.len())?;
        let mut file = OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)?;
        if FileStamp::of(&file.metadata()?) != self.stamp
            || FileStamp::of(&fs::symlink_metadata(&self.path)?) != self.stamp
        {
            bail!("roster_stage_changed");
        }
        for item in &page.items {
            item.validate()?;
            let chunk = match item {
                Item::Project { project_id, name } => {
                    if self.project.as_ref().is_some_and(|old| old >= project_id) {
                        bail!("unordered_roster");
                    }
                    let prefix = if self.project.is_some() { "]}," } else { "" };
                    let text = format!(
                        "{prefix}{{\"projectId\":{},\"name\":{},\"agents\":[",
                        serde_json::to_string(project_id)?,
                        serde_json::to_string(name)?
                    );
                    self.project = Some(project_id.clone());
                    self.last_agent = None;
                    text.into_bytes()
                }
                Item::Agent { project_id, agent } => {
                    if self.project.as_ref() != Some(project_id)
                        || self
                            .last_agent
                            .as_ref()
                            .is_some_and(|old| old >= &agent.agent_id)
                    {
                        bail!("unordered_roster");
                    }
                    let mut chunk = Vec::new();
                    if self.last_agent.is_some() {
                        chunk.push(b',');
                    }
                    serde_json::to_writer(&mut chunk, agent)?;
                    self.last_agent = Some(agent.agent_id.clone());
                    chunk
                }
            };
            file.write_all(&chunk)?;
            self.hash.update(&chunk);
        }
        self.offset = next_offset;
        if page.next.is_none() {
            let tail = if self.project.is_some() { "]}]" } else { "]" };
            file.write_all(tail.as_bytes())?;
            self.hash.update(tail.as_bytes());
            if format!("{:x}", self.hash.clone().finalize()) != self.version {
                bail!("roster_hash_mismatch");
            }
            file.write_all(b"}")?;
            file.sync_all()?;
            let proof = FileStamp::of(&file.metadata()?);
            let _lock = store.content_lock().map_err(anyhow::Error::msg)?;
            self.check_member(store)?;
            io.check()?;
            if !authenticated() || FileStamp::of(&fs::symlink_metadata(&self.path)?) != proof {
                bail!("roster_stage_changed");
            }
            let dir = directory(store)?;
            fs::rename(&self.path, dir.join(format!("{}.json", self.peer)))?;
            File::open(dir)?.sync_all()?;
            self.committed = true;
            return Ok(true);
        }
        self.stamp = FileStamp::of(&file.metadata()?);
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
