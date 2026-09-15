//! An authenticated complete cache proof. File operations recheck its stamp
//! and membership, without parsing/hash-checking the whole roster per image.
use super::{context, PeerRoster, Project, StateStore};
use crate::bbs_sync::{reconcile, transport::Resource, FileStamp, PostVersion};
use anyhow::{bail, Result};
use std::{fs, sync::Arc};

#[derive(Clone)]
pub(crate) struct Peer {
    pub(crate) roster: Arc<PeerRoster>,
    own_incarnation: String,
    stamp: FileStamp,
}
impl std::ops::Deref for Peer {
    type Target = PeerRoster;
    fn deref(&self) -> &Self::Target {
        &self.roster
    }
}
impl Peer {
    #[cfg(test)]
    pub(crate) fn load(store: &StateStore, device: &str) -> Result<Option<Self>> {
        Self::load_inner(store, device, None)
    }
    pub(crate) fn load_on_file_thread(
        store: &StateStore,
        device: &str,
        io: &mut crate::bbs_sync::transport::LocalIo,
    ) -> Result<Option<Self>> {
        Self::load_inner(store, device, Some(io))
    }
    fn load_inner(
        store: &StateStore,
        device: &str,
        io: Option<&mut crate::bbs_sync::transport::LocalIo>,
    ) -> Result<Option<Self>> {
        let ctx = context(store)?;
        let Some(current) = &ctx.membership else {
            return Ok(None);
        };
        let path = store.root.join("rosters").join(format!("{device}.json"));
        crate::bbs_sync::valid_hash(device).map_err(anyhow::Error::msg)?;
        let before = match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() => FileStamp::of(&m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => bail!("invalid_peer_roster"),
        };
        let Some(roster) = super::read_peer_with_io(store, &ctx, device, io)? else {
            return Ok(None);
        };
        if before != FileStamp::of(&fs::symlink_metadata(&path)?) {
            bail!("roster_changed");
        }
        let proof = Self {
            roster: Arc::new(roster),
            own_incarnation: current.membership_id.clone(),
            stamp: before,
        };
        proof.check(store)?;
        Ok(Some(proof))
    }
    pub(crate) fn check(&self, store: &StateStore) -> Result<()> {
        let ctx = context(store)?;
        if !ctx.membership.is_some_and(|m| {
            m.group_id == self.roster.group_id && m.membership_id == self.own_incarnation
        }) || !ctx.members.iter().any(|m| {
            m.device_id == self.roster.device_id && m.membership_id == self.roster.membership_id
        }) || FileStamp::of(&fs::symlink_metadata(
            store
                .root
                .join("rosters")
                .join(format!("{}.json", self.roster.device_id)),
        )?) != self.stamp
        {
            bail!("roster_changed");
        }
        Ok(())
    }
}

/// The same reference test serves sender and receiver. Callers provide only
/// current own/peer roster data and post offers whose content fence is valid.
pub(crate) fn avatar_referenced<'a>(
    resource: &Resource,
    roster: Option<&[Project]>,
    mut posts: impl Iterator<Item = &'a PostVersion>,
) -> bool {
    roster.is_some_and(|projects| {
        projects
            .iter()
            .flat_map(|p| &p.agents)
            .any(|a| a.avatar.resource().as_ref() == Some(resource))
    }) || posts.any(|post| {
        post.avatar
            .as_ref()
            .and_then(reconcile::avatar_resource)
            .as_ref()
            == Some(resource)
    })
}
