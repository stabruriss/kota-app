//! A local stamp cache, not an authority. Only the registered source or a
//! current roster reference authorizes an image; hash verification supplies its
//! availability proof. Names/presence changes do not rehash unchanged pictures.
use super::{Agent, Avatar, Project};
use crate::{
    bbs::sync::ContentStore,
    bbs_sync::{
        transport::{LocalIo, Resource},
        FileStamp,
    },
};
use anyhow::{bail, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn observed(path: &Path) -> Option<FileStamp> {
    let m = fs::symlink_metadata(path).ok()?;
    (m.is_file() && !m.file_type().is_symlink()).then(|| FileStamp::of(&m))
}
fn key(resource: &Resource) -> String {
    // ResourceKind is validated before reaching the cache.
    serde_json::to_string(resource).unwrap_or_default()
}
struct Source {
    path: PathBuf,
    mime: String,
    stamp: FileStamp,
    avatar: Avatar,
}
struct Check {
    observed: Option<FileStamp>,
    valid: bool,
}
#[derive(Default)]
pub(super) struct Images {
    sources: BTreeMap<String, Source>,
    checked: BTreeMap<String, Check>,
    paths: BTreeSet<PathBuf>,
    #[cfg(test)]
    source_reads: usize,
    #[cfg(test)]
    blob_hashes: usize,
}
impl Images {
    pub(super) fn collect(
        &mut self,
        store: &ContentStore,
        io: &mut LocalIo,
    ) -> Result<Vec<Project>> {
        let dir = crate::agent_directory::collect(store.account_dir())?.complete()?;
        let index =
            crate::read_user_avatar_index(&store.account_dir().join("avatars")).unwrap_or_default();
        self.paths.clear();
        let mut used = BTreeSet::new();
        let mut resolved = BTreeMap::new();
        let mut projects = Vec::new();
        for p in dir {
            let mut agents = Vec::new();
            for a in p.agents {
                io.check()?;
                let avatar = if let Some(id) = a.avatar_id {
                    used.insert(id.clone());
                    resolved
                        .entry(id.clone())
                        .or_insert_with(|| {
                            if !id.starts_with("user:") && super::safe_id(&id).is_ok() {
                                Avatar::Builtin { id }
                            } else {
                                self.registered(store, &id, &index, io)
                                    .unwrap_or(Avatar::None)
                            }
                        })
                        .clone()
                } else {
                    Avatar::None
                };
                agents.push(Agent {
                    agent_id: a.id,
                    name: a.name,
                    avatar,
                });
            }
            projects.push(Project {
                project_id: p.id,
                name: p.name,
                agents,
            });
        }
        self.sources.retain(|id, _| used.contains(id));
        super::validate_projects(&projects)?;
        Ok(projects)
    }
    fn registered(
        &mut self,
        store: &ContentStore,
        id: &str,
        index: &[crate::StoredUserHeroAvatar],
        io: &mut LocalIo,
    ) -> Result<Avatar> {
        let item = index
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| anyhow::anyhow!("avatar_unavailable"))?;
        if Path::new(&item.file_name).components().count() != 1 || item.file_name.starts_with('.') {
            bail!("invalid_avatar_reference");
        }
        let path = store.account_dir().join("avatars").join(&item.file_name);
        self.paths.insert(path.clone());
        let stamp = observed(&path).ok_or_else(|| anyhow::anyhow!("avatar_unavailable"))?;
        if let Some(old) = self.sources.get(id) {
            if old.path == path && old.mime == item.mime && old.stamp == stamp {
                if let Some(resource) = old.avatar.resource() {
                    let crate::bbs_sync::transport::ResourceKind::Avatar { ext, .. } =
                        &resource.identity
                    else {
                        unreachable!()
                    };
                    let blob = store.roster_avatar_path(&resource.sha256, ext)?;
                    if self
                        .checked
                        .get(&key(&resource))
                        .is_some_and(|c| c.valid && c.observed == observed(&blob))
                    {
                        return Ok(old.avatar.clone());
                    }
                }
            }
        }
        #[cfg(test)]
        {
            self.source_reads += 1;
        }
        let (sidecar, proof) = store.capture_registered_avatar(item, io)?;
        let avatar = Avatar::Image {
            sha256: sidecar.sha256.unwrap(),
            ext: sidecar.ext.unwrap(),
            size_bytes: sidecar.size_bytes.unwrap(),
        };
        // Don't cache a captured blob under an unrelated source stamp.
        if observed(&path).as_ref() != Some(&stamp) {
            bail!("avatar_changed");
        }
        self.checked.insert(
            key(&avatar.resource().unwrap()),
            Check {
                observed: proof,
                valid: true,
            },
        );
        self.sources.insert(
            id.into(),
            Source {
                path,
                mime: item.mime.clone(),
                stamp,
                avatar: avatar.clone(),
            },
        );
        Ok(avatar)
    }
    pub(super) fn available(
        &mut self,
        store: &ContentStore,
        avatar: &Avatar,
        io: &mut LocalIo,
    ) -> bool {
        let Some(resource) = avatar.resource() else {
            return false;
        };
        let ext = match &resource.identity {
            crate::bbs_sync::transport::ResourceKind::Avatar { ext, .. } => ext,
            _ => return false,
        };
        let Ok(path) = store.roster_avatar_path(&resource.sha256, ext) else {
            return false;
        };
        let now = observed(&path);
        let k = key(&resource);
        if let Some(old) = self.checked.get(&k) {
            if old.observed == now {
                return old.valid;
            }
        }
        #[cfg(test)]
        {
            self.blob_hashes += 1;
        }
        let valid = store
            .verify_roster_avatar(&resource, io)
            .is_ok_and(|p| p.is_some() && p == now);
        self.checked.insert(
            k,
            Check {
                observed: now,
                valid,
            },
        );
        valid
    }
    pub(super) fn source_paths(&self) -> Vec<PathBuf> {
        self.paths.iter().cloned().collect()
    }
    pub(super) fn retain(&mut self, avatars: impl Iterator<Item = Avatar>) {
        let keys = avatars
            .filter_map(|a| a.resource())
            .map(|r| key(&r))
            .collect::<BTreeSet<_>>();
        self.checked.retain(|k, _| keys.contains(k));
    }
}

#[cfg(test)]
mod tests;
