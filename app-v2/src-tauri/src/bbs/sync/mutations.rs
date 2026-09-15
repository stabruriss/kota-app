use super::*;
use crate::bbs_sync::roster::{self, reference::Peer as PeerRoster};

impl ContentStore {
    /// Admission before DataChannel receive: a valid offer, current sharing
    /// fence, bounded declared resource and enough available staging space.
    pub(crate) fn prepare_receive(
        &self,
        fence: &GroupFence,
        resource: &Resource,
        offered: Option<&PostVersion>,
    ) -> Result<PathBuf> {
        self.prepare_receive_with_roster(fence, resource, offered, None)
    }
    pub(crate) fn prepare_receive_with_roster(
        &self,
        fence: &GroupFence,
        resource: &Resource,
        offered: Option<&PostVersion>,
        roster: Option<&PeerRoster>,
    ) -> Result<PathBuf> {
        resource.validate()?;
        if let Some(p) = offered {
            reconcile::validate_post(p).map_err(anyhow::Error::msg)?;
        }
        let lock = acquire_write_lock(&self.root)?;
        self.check_fence(&lock, fence)?;
        let state = self.state_locked(&lock)?;
        match &resource.identity {
            ResourceKind::Post {
                thread_id,
                post_id,
                version_id,
            } => {
                self.require_shared(&state, fence, thread_id, post_id, version_id)?;
                if offered.map(reconcile::post_resource).as_ref() != Some(resource) {
                    bail!("unrequested_resource");
                }
            }
            ResourceKind::Attachment {
                thread_id,
                post_id,
                version_id,
                attachment_id,
            } => {
                self.require_shared(&state, fence, thread_id, post_id, version_id)?;
                let p = state
                    .ledger
                    .get(
                        &LedgerEntry::version_key(post_id, version_id)
                            .map_err(anyhow::Error::msg)?,
                    )
                    .and_then(|r| r.descriptor.as_ref())
                    .ok_or_else(|| anyhow!("missing_post_body"))?;
                if !p.attachments.iter().any(|a| {
                    &a.id == attachment_id && reconcile::attachment_resource(p, a) == *resource
                }) {
                    bail!("unrequested_resource");
                }
            }
            ResourceKind::Avatar { .. } => {
                if !self.avatar_allowed(&state, fence, resource, roster)? {
                    bail!("unrequested_resource");
                }
            }
        }
        drop(lock);
        let directory = self.prepare_layout()?;
        ensure_space(&directory, resource.size_bytes)?;
        Ok(directory)
    }
    /// Called after authenticated, strictly ordered phase validation. The
    /// coordinator completes all tombstone pages before requesting body data.
    pub(crate) fn process_page(
        &self,
        fence: &GroupFence,
        page: &Manifest,
        reader: &mut reconcile::ManifestReader,
        io: &mut LocalIo,
    ) -> Result<reconcile::Plan> {
        let mut next_reader = reader.clone();
        let parsed = next_reader.read(page).map_err(anyhow::Error::msg)?;
        if page.phase == ManifestPhase::Unavailable {
            // Throttle before taking the content flock, never sleep under it.
            io.charge(serde_json::to_vec(page)?.len())?;
        }
        let roots: BTreeSet<_> = parsed
            .deletions
            .iter()
            .map(|d| d.thread_id.clone())
            .chain(parsed.threads.iter().map(|r| r.record.thread_id.clone()))
            .chain(parsed.posts.iter().map(|p| p.thread_id.clone()))
            .chain(parsed.unavailable_posts.iter().map(|p| p.thread_id.clone()))
            .collect();
        {
            let lock = acquire_write_lock(&self.root)?;
            self.check_fence(&lock, fence)?;
            let mut state = self.state_locked(&lock)?;
            state
                .groups
                .entry(fence.group_id.clone())
                .or_default()
                .shared_threads
                // A parsed version is not yet a valid relationship. It may
                // name a private root using another post's identity. Register
                // versions only from the final validated plan below.
                .extend(
                    parsed
                        .deletions
                        .iter()
                        .map(|d| d.thread_id.clone())
                        .chain(parsed.threads.iter().map(|t| t.record.thread_id.clone())),
                );
            for deletion in &parsed.deletions {
                state
                    .tombstones
                    .entry(deletion.key().map_err(anyhow::Error::msg)?)
                    .or_insert_with(|| deletion.clone());
            }
            self.save_locked(&lock, &state)?;
        }
        // The completed tombstone phase already repaired content. Notices have
        // no body to move/promote; the final locked write checks logical markers
        // again. Avoid re-reading a large thread's bodies for every notice page.
        if page.phase != ManifestPhase::Unavailable {
            self.recover_deletions(Some(fence), &roots, io)?;
        }
        let mut index_issues = Vec::new();
        if matches!(page.phase, ManifestPhase::Threads | ManifestPhase::Versions) {
            for root in &roots {
                io.check()?;
                match self.refresh_root(root, Some(fence), io) {
                    Ok(items) => index_issues.extend(items),
                    Err(_) => index_issues.push(reconcile::Issue {
                        code: "local_thread_unavailable".into(),
                        thread_id: Some(root.clone()),
                        post_id: None,
                    }),
                }
            }
        }
        let lock = acquire_write_lock(&self.root)?;
        self.check_fence(&lock, fence)?;
        let mut state = self.state_locked(&lock)?;
        let mut plan = reconcile::plan(&state, parsed).map_err(anyhow::Error::msg)?;
        state
            .groups
            .entry(fence.group_id.clone())
            .or_default()
            .shared_threads
            .extend(plan.shared_roots.iter().cloned());
        plan.issues.extend(index_issues);
        for item in &plan.threads {
            if state
                .tombstones
                .contains_key(&format!("thread:{}", item.record.thread_id))
            {
                continue;
            }
            let path = self.managed(
                &Self::thread_relative(&item.record.thread_id)?.join("thread.yaml"),
                true,
            )?;
            if current_stamp(&path)?.is_none() {
                write_small(
                    &path,
                    serde_yaml::to_string(&meta_from_record(&item.record))?.as_bytes(),
                )?;
                state
                    .thread_records
                    .insert(item.record.thread_id.clone(), item.clone());
            } else {
                // A local publisher may have appeared since the preflight scan.
                let local = match current_stamp(&path)?
                    .filter(|m| m.len <= 64 * 1024)
                    .ok_or_else(|| anyhow!("invalid_local_thread_record"))
                    .and_then(|_| read_thread_meta(&path))
                    .and_then(|meta| record_from_meta(&meta))
                {
                    Ok(record) => record,
                    Err(_) => {
                        plan.issues.push(reconcile::Issue {
                            code: "invalid_local_thread_record".into(),
                            thread_id: Some(item.record.thread_id.clone()),
                            post_id: None,
                        });
                        continue;
                    }
                };
                if local.sha256 != item.sha256 {
                    plan.warnings.push(reconcile::ThreadMismatch {
                        thread_id: item.record.thread_id.clone(),
                        local_sha256: local.sha256.clone(),
                        remote_sha256: item.sha256.clone(),
                    });
                }
                state
                    .thread_records
                    .insert(local.record.thread_id.clone(), local);
            }
        }
        for item in &plan.unavailable_posts {
            if !state
                .groups
                .get(&fence.group_id)
                .is_some_and(|g| g.shared_threads.contains(&item.thread_id))
            {
                plan.issues.push(reconcile::Issue {
                    code: "unshared_unavailable_post".into(),
                    thread_id: Some(item.thread_id.clone()),
                    post_id: Some(item.post_id.clone()),
                });
                continue;
            }
            io.check()?;
            if let Some(issue) = self.mark_unavailable_locked(&lock, &state, item)? {
                plan.issues.push(issue);
            }
        }
        self.save_locked(&lock, &state)?;
        *reader = next_reader;
        Ok(plan)
    }
    pub(crate) fn catalog(&self, fence: &GroupFence) -> Result<reconcile::Catalog> {
        let lock = acquire_write_lock(&self.root)?;
        self.check_fence(&lock, fence)?;
        reconcile::Catalog::from_state(&self.state_locked(&lock)?, &fence.group_id)
            .map_err(anyhow::Error::msg)
    }
    /// Exact registered resource only. No pathname from the network reaches this
    /// resolver. The transport rechecks regular-file/length/hash while streaming.
    pub(crate) fn source(&self, fence: &GroupFence, resource: &Resource) -> Result<PathBuf> {
        resource.validate()?;
        let lock = acquire_write_lock(&self.root)?;
        self.check_fence(&lock, fence)?;
        let state = self.state_locked(&lock)?;
        self.source_in(&state, Some(&fence.group_id), resource)
    }
    /// Serving an avatar accepts exactly the current local roster or this
    /// authenticated round's post offers. Transport still hashes every byte.
    pub(crate) fn source_for_exchange(
        &self,
        fence: &GroupFence,
        resource: &Resource,
        catalog: &reconcile::Catalog,
        local: Option<&roster::wire::Local>,
    ) -> Result<PathBuf> {
        let ResourceKind::Avatar { sha256, ext } = &resource.identity else {
            return self.source(fence, resource);
        };
        resource.validate()?;
        let lock = acquire_write_lock(&self.root)?;
        self.check_fence(&lock, fence)?;
        let state = self.state_locked(&lock)?;
        let path = self.roster_avatar_path(sha256, ext)?;
        let proof = current_stamp(&path)?.ok_or_else(|| anyhow!("source_unavailable"))?;
        let offered = catalog.post_versions().filter(|p| {
            self.require_shared(&state, fence, &p.thread_id, &p.post_id, &p.version_id)
                .is_ok()
                && state
                    .ledger
                    .get(&format!("{}/{}", p.post_id, p.version_id))
                    .is_some_and(|r| r.avatar_stamp.as_ref() == Some(&proof))
        });
        if !roster::reference::avatar_referenced(
            resource,
            local.map(|r| r.projects.as_slice()),
            offered,
        ) {
            bail!("unrequested_avatar");
        }
        Ok(path)
    }
    fn avatar_allowed(
        &self,
        state: &SyncState,
        fence: &GroupFence,
        resource: &Resource,
        peer: Option<&PeerRoster>,
    ) -> Result<bool> {
        if let Some(peer) = peer {
            peer.check(&self.state)?;
        }
        let posts = state
            .ledger
            .values()
            .filter(|r| {
                r.body_stamp.is_some()
                    && self
                        .require_shared(state, fence, &r.thread_id, &r.post_id, &r.version_id)
                        .is_ok()
            })
            .filter_map(|r| r.descriptor.as_ref());
        Ok(roster::reference::avatar_referenced(
            resource,
            peer.map(|p| p.roster.projects.as_slice()),
            posts,
        ))
    }
    fn source_in(
        &self,
        state: &SyncState,
        group: Option<&str>,
        resource: &Resource,
    ) -> Result<PathBuf> {
        let shared = |thread: &str| {
            group.is_none_or(|g| {
                state
                    .groups
                    .get(g)
                    .is_some_and(|g| g.shared_threads.contains(thread))
            })
        };
        match &resource.identity {
            ResourceKind::Post {
                thread_id,
                post_id,
                version_id,
            }
            | ResourceKind::Attachment {
                thread_id,
                post_id,
                version_id,
                ..
            } => {
                if !shared(thread_id)
                    || reconcile::suppressed(state, thread_id, post_id, version_id)
                {
                    bail!("resource_not_shared");
                }
                let row = state
                    .ledger
                    .get(
                        &LedgerEntry::version_key(post_id, version_id)
                            .map_err(anyhow::Error::msg)?,
                    )
                    .filter(|r| &r.thread_id == thread_id)
                    .ok_or_else(|| anyhow!("resource_not_registered"))?;
                let p = row
                    .descriptor
                    .as_ref()
                    .ok_or_else(|| anyhow!("resource_not_registered"))?;
                let location = self
                    .locate_row(row)?
                    .ok_or_else(|| anyhow!("resource_changed"))?;
                match &resource.identity {
                    ResourceKind::Post { .. } if reconcile::post_resource(p) == *resource => {
                        Ok(location.path)
                    }
                    ResourceKind::Attachment { attachment_id, .. } => {
                        let a = p
                            .attachments
                            .iter()
                            .find(|a| {
                                &a.id == attachment_id
                                    && reconcile::attachment_resource(p, a) == *resource
                            })
                            .ok_or_else(|| anyhow!("resource_not_registered"))?;
                        let verified = row
                            .attachments
                            .get(attachment_id)
                            .filter(|s| s.verified && s.stamp.is_some())
                            .ok_or_else(|| anyhow!("resource_unavailable"))?;
                        let path = self.attachment_path(thread_id, &location, a, false)?;
                        if current_stamp(&path)? != verified.stamp {
                            bail!("resource_changed");
                        }
                        Ok(path)
                    }
                    _ => bail!("resource_not_registered"),
                }
            }
            ResourceKind::Avatar { sha256, ext } => {
                let path =
                    self.managed(&Path::new("avatars").join(format!("{sha256}.{ext}")), false)?;
                let observed = current_stamp(&path)?;
                if observed.is_none() {
                    bail!("resource_unavailable");
                }
                let known = state.ledger.values().any(|r| {
                    shared(&r.thread_id)
                        && !reconcile::suppressed(state, &r.thread_id, &r.post_id, &r.version_id)
                        && r.avatar_stamp == observed
                        && r.descriptor
                            .as_ref()
                            .and_then(|p| p.avatar.as_ref())
                            .and_then(reconcile::avatar_resource)
                            .as_ref()
                            == Some(resource)
                });
                if !known {
                    bail!("resource_not_registered");
                }
                Ok(path)
            }
        }
    }
    pub(crate) async fn install_received(
        &self,
        verified: VerifiedFile,
        fence: GroupFence,
        offered: Option<PostVersion>,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.install_received_with_roster(verified, fence, offered, None, cancel)
            .await
    }
    pub(crate) async fn install_received_with_roster(
        &self,
        verified: VerifiedFile,
        fence: GroupFence,
        offered: Option<PostVersion>,
        roster: Option<PeerRoster>,
        cancel: &Cancellation,
    ) -> Result<()> {
        let store = self.clone();
        verified
            .install(cancel, move |path, resource, io| {
                store.install_verified_with_roster(
                    path,
                    resource,
                    offered.as_ref(),
                    &fence,
                    roster.as_ref(),
                    io,
                )
            })
            .await??;
        Ok(())
    }
    fn install_verified(
        &self,
        partial: &Path,
        resource: &Resource,
        offered: Option<&PostVersion>,
        fence: &GroupFence,
        io: &mut LocalIo,
    ) -> Result<()> {
        self.install_verified_with_roster(partial, resource, offered, fence, None, io)
    }
    fn install_verified_with_roster(
        &self,
        partial: &Path,
        resource: &Resource,
        offered: Option<&PostVersion>,
        fence: &GroupFence,
        roster: Option<&PeerRoster>,
        io: &mut LocalIo,
    ) -> Result<()> {
        io.check()?;
        resource.validate()?;
        let body = match &resource.identity {
            ResourceKind::Post {
                thread_id, post_id, ..
            } => {
                let p = offered.ok_or_else(|| anyhow!("missing_version_offer"))?;
                reconcile::validate_post(p).map_err(anyhow::Error::msg)?;
                if reconcile::post_resource(p) != *resource {
                    bail!("unrequested_resource");
                }
                let raw = read_bounded(partial, reconcile::MAX_BODY_BYTES, io)?;
                let meta = parse_post(&raw.bytes, thread_id, post_id)?;
                if raw.sha256 != resource.sha256
                    || raw.bytes.len() as u64 != resource.size_bytes
                    || meta.kind != p.kind
                {
                    bail!("invalid_post_relationship");
                }
                let mut declared = attachment_refs(&meta)?;
                let mut advertised = p.attachments.clone();
                for a in &mut advertised {
                    a.available = false;
                }
                declared.sort_by(|a, b| a.id.cmp(&b.id));
                advertised.sort_by(|a, b| a.id.cmp(&b.id));
                if declared != advertised {
                    bail!("attachment_manifest_mismatch");
                }
                self.validate_existing_post_relationship(p, io)?;
                Some((p.clone(), raw, meta))
            }
            _ => None,
        };
        let lock = acquire_write_lock(&self.root)?;
        io.check()?;
        self.check_fence(&lock, fence)?;
        let mut state = self.state_locked(&lock)?;
        let observed = current_stamp(partial)?.ok_or_else(|| anyhow!("partial_missing"))?;
        if observed.len != resource.size_bytes {
            bail!("partial_changed");
        }
        let destination = match &resource.identity {
            ResourceKind::Post {
                thread_id,
                post_id,
                version_id,
            } => {
                self.require_shared(&state, fence, thread_id, post_id, version_id)?;
                let (p, raw, meta) = body.unwrap();
                if raw.stamp != observed {
                    bail!("partial_changed");
                }
                for row in state.ledger.values() {
                    if let Some(old) = &row.descriptor {
                        if (old.post_id == p.post_id
                            && (old.thread_id != p.thread_id || old.kind != p.kind))
                            || (p.kind == "topic"
                                && old.thread_id == p.thread_id
                                && old.kind == "topic"
                                && old.post_id != p.post_id)
                        {
                            bail!("invalid_post_relationship");
                        }
                    }
                }
                if !state.thread_records.contains_key(thread_id) {
                    bail!("missing_thread_record");
                }
                let key =
                    LedgerEntry::version_key(post_id, version_id).map_err(anyhow::Error::msg)?;
                if let Some(old) = state.ledger.get(&key) {
                    if self.locate_row(old)?.is_some() {
                        fs::remove_file(partial)?;
                        self.clear_unavailable_locked(&lock, thread_id, post_id)?;
                        return Ok(());
                    }
                }
                let resident =
                    self.managed(&Self::post_relative(thread_id, post_id, None)?, true)?;
                let extra = current_stamp(&resident)?
                    .is_some()
                    .then_some(version_id.as_str());
                let destination =
                    self.managed(&Self::post_relative(thread_id, post_id, extra)?, true)?;
                if current_stamp(&destination)?.is_some() {
                    bail!("version_requires_reindex");
                }
                let origin = self.write_origin_locked(
                    &lock,
                    thread_id,
                    post_id,
                    version_id,
                    &OriginSnapshot {
                        schema_version: 1,
                        origin: "received".into(),
                        avatar: p.avatar.clone().unwrap(),
                    },
                )?;
                let mut p = p;
                p.avatar = Some(origin.avatar);
                for a in &mut p.attachments {
                    a.available = false;
                }
                let attachments = p
                    .attachments
                    .iter()
                    .map(|a| {
                        (
                            a.id.clone(),
                            AttachmentState {
                                sha256: a.sha256.clone(),
                                size_bytes: a.size_bytes,
                                verified: false,
                                stamp: None,
                            },
                        )
                    })
                    .collect();
                let notifications = super::super::notify::prepare(
                    self, &lock, &meta, &raw.bytes, version_id, false, &state,
                )?;
                fs::rename(partial, &destination)?;
                File::open(destination.parent().unwrap())?.sync_all()?;
                state.ledger.insert(
                    key,
                    LedgerEntry {
                        thread_id: thread_id.clone(),
                        post_id: post_id.clone(),
                        version_id: version_id.clone(),
                        updated_at: now_iso(),
                        attachments,
                        body_stamp: current_stamp(&destination)?,
                        descriptor: Some(p),
                        avatar_stamp: None,
                    },
                );
                self.save_locked(&lock, &state)?;
                super::super::notify::published(self, &lock, &notifications, &destination);
                self.clear_unavailable_locked(&lock, thread_id, post_id)?;
                warn_index_failure(add_thread_to_board(&self.root, thread_id));
                return Ok(());
            }
            ResourceKind::Attachment {
                thread_id,
                post_id,
                version_id,
                attachment_id,
            } => {
                self.require_shared(&state, fence, thread_id, post_id, version_id)?;
                let key =
                    LedgerEntry::version_key(post_id, version_id).map_err(anyhow::Error::msg)?;
                let row = state
                    .ledger
                    .get(&key)
                    .ok_or_else(|| anyhow!("missing_post_body"))?;
                let location = self
                    .locate_row(row)?
                    .ok_or_else(|| anyhow!("post_body_changed"))?;
                let p = row
                    .descriptor
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing_post_body"))?;
                let a = p
                    .attachments
                    .iter()
                    .find(|a| {
                        &a.id == attachment_id && reconcile::attachment_resource(p, a) == *resource
                    })
                    .ok_or_else(|| anyhow!("unrequested_resource"))?;
                let destination = self.attachment_path(thread_id, &location, a, true)?;
                // An exact already verified resource is reused without inode churn.
                if row.attachments.get(attachment_id).is_some_and(|s| {
                    s.verified
                        && s.stamp.is_some()
                        && current_stamp(&destination).ok().flatten() == s.stamp
                }) {
                    fs::remove_file(partial)?;
                    super::super::notify::attachments_saved(self, thread_id, &state);
                    return Ok(());
                }
                fs::rename(partial, &destination)?;
                let row = state.ledger.get_mut(&key).unwrap();
                row.attachments.insert(
                    attachment_id.clone(),
                    AttachmentState {
                        sha256: resource.sha256.clone(),
                        size_bytes: resource.size_bytes,
                        verified: true,
                        stamp: current_stamp(&destination)?,
                    },
                );
                if let Some(a) = row
                    .descriptor
                    .as_mut()
                    .unwrap()
                    .attachments
                    .iter_mut()
                    .find(|a| &a.id == attachment_id)
                {
                    a.available = true;
                }
                destination
            }
            ResourceKind::Avatar { sha256, ext } => {
                let allowed = self.avatar_allowed(&state, fence, resource, roster)?;
                if !allowed {
                    bail!("unrequested_avatar");
                }
                let destination =
                    self.managed(&Path::new("avatars").join(format!("{sha256}.{ext}")), true)?;
                fs::rename(partial, &destination)?;
                let proof = current_stamp(&destination)?;
                for row in state.ledger.values_mut() {
                    if row
                        .descriptor
                        .as_ref()
                        .and_then(|p| p.avatar.as_ref())
                        .and_then(reconcile::avatar_resource)
                        .as_ref()
                        == Some(resource)
                    {
                        row.avatar_stamp = proof.clone();
                    }
                }
                destination
            }
        };
        File::open(destination.parent().unwrap())?.sync_all()?;
        self.save_locked(&lock, &state)?;
        if let ResourceKind::Attachment { thread_id, .. } = &resource.identity {
            super::super::notify::attachments_saved(self, thread_id, &state);
        }
        Ok(())
    }
    fn require_shared(
        &self,
        state: &SyncState,
        fence: &GroupFence,
        thread: &str,
        post: &str,
        version: &str,
    ) -> Result<()> {
        if !state
            .groups
            .get(&fence.group_id)
            .is_some_and(|g| g.shared_threads.contains(thread))
        {
            bail!("resource_not_shared");
        }
        if reconcile::suppressed(state, thread, post, version) {
            bail!("resource_deleted");
        }
        Ok(())
    }
    /// Reuse bytes already present in a *registered BBS resource*, including a
    /// Fork's attachments. Still copies into a new inode, hashes while copying,
    /// uses the account worker/permit/pacer, and rechecks deletion on install.
    pub(crate) fn copy_available(
        &self,
        fence: &GroupFence,
        target: &Resource,
        offered: Option<&PostVersion>,
        io: &mut LocalIo,
    ) -> Result<bool> {
        self.copy_available_with_roster(fence, target, offered, None, io)
    }
    pub(crate) fn copy_available_with_roster(
        &self,
        fence: &GroupFence,
        target: &Resource,
        offered: Option<&PostVersion>,
        roster: Option<&PeerRoster>,
        io: &mut LocalIo,
    ) -> Result<bool> {
        if matches!(target.identity, ResourceKind::Avatar { .. }) {
            // One shared content-addressed image, even if it is referenced by
            // both rosters and immutable post origins. Verify outside the lock.
            target.validate()?;
            let Some(proof) = self.verify_roster_avatar(target, io).ok().flatten() else {
                return Ok(false);
            };
            let lock = acquire_write_lock(&self.root)?;
            self.check_fence(&lock, fence)?;
            let mut state = self.state_locked(&lock)?;
            if !self.avatar_allowed(&state, fence, target, roster)? {
                bail!("unrequested_avatar");
            }
            let ResourceKind::Avatar { sha256, ext } = &target.identity else {
                unreachable!()
            };
            if current_stamp(&self.roster_avatar_path(sha256, ext)?)?.as_ref() != Some(&proof) {
                bail!("avatar_changed");
            }
            let mut changed = false;
            for row in state.ledger.values_mut() {
                if row
                    .descriptor
                    .as_ref()
                    .and_then(|p| p.avatar.as_ref())
                    .and_then(reconcile::avatar_resource)
                    .as_ref()
                    == Some(target)
                    && row.avatar_stamp.as_ref() != Some(&proof)
                {
                    row.avatar_stamp = Some(proof.clone());
                    changed = true;
                }
            }
            if changed {
                self.save_locked(&lock, &state)?;
            }
            return Ok(true);
        }
        let source = {
            let lock = acquire_write_lock(&self.root)?;
            self.check_fence(&lock, fence)?;
            let state = self.state_locked(&lock)?;
            if self
                .source_in(&state, Some(&fence.group_id), target)
                .is_ok()
            {
                return Ok(true);
            }
            let mut source = None;
            for row in state.ledger.values() {
                let Some(p) = &row.descriptor else {
                    continue;
                };
                let resources: Vec<_> = match &target.identity {
                    ResourceKind::Post { .. } => vec![reconcile::post_resource(p)],
                    ResourceKind::Attachment { .. } => p
                        .attachments
                        .iter()
                        .map(|a| reconcile::attachment_resource(p, a))
                        .collect(),
                    ResourceKind::Avatar { .. } => p
                        .avatar
                        .as_ref()
                        .and_then(reconcile::avatar_resource)
                        .into_iter()
                        .collect(),
                };
                for resource in resources {
                    let same_content = match (&resource.identity, &target.identity) {
                        (
                            ResourceKind::Attachment {
                                thread_id: a,
                                post_id: p,
                                ..
                            },
                            ResourceKind::Attachment {
                                thread_id: b,
                                post_id: q,
                                ..
                            },
                        ) => a == b && p == q,
                        (ResourceKind::Post { .. }, ResourceKind::Post { .. }) => {
                            resource.identity == target.identity
                        }
                        (ResourceKind::Avatar { .. }, ResourceKind::Avatar { .. }) => {
                            resource.identity == target.identity
                        }
                        _ => false,
                    };
                    if same_content
                        && resource.sha256 == target.sha256
                        && resource.size_bytes == target.size_bytes
                    {
                        if let Ok(path) = self.source_in(&state, None, &resource) {
                            source = Some(path);
                            break;
                        }
                    }
                }
                if source.is_some() {
                    break;
                }
            }
            source
        };
        let Some(source) = source else {
            return Ok(false);
        };
        let directory = self.prepare_layout()?;
        ensure_space(&directory, target.size_bytes)?;
        let partial = directory.join(format!(".receive-{}.partial", Uuid::new_v4()));
        let result = (|| {
            copy_verified(&source, &partial, target, io)?;
            self.install_verified(&partial, target, offered, fence, io)
        })();
        if partial.exists() {
            let _ = fs::remove_file(&partial);
        }
        result.map(|_| true)
    }
}

pub(super) fn ensure_space(directory: &Path, size: u64) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes())?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    let available = (info.f_bavail as u128) * (info.f_frsize as u128);
    if available < size as u128 {
        bail!("insufficient_space");
    }
    Ok(())
}
fn copy_verified(
    source: &Path,
    partial: &Path,
    resource: &Resource,
    io: &mut LocalIo,
) -> Result<()> {
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)?;
    let observed = input.metadata()?;
    if !observed.is_file() || observed.len() != resource.size_bytes {
        bail!("copy_source_changed");
    }
    let observed = stamp(&observed);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(partial)?;
    let mut buffer = io.buffer()?;
    let mut total = 0u64;
    let mut hash = Sha256::new();
    loop {
        io.check()?;
        let n = input.read(&mut buffer.bytes)?;
        if n == 0 {
            break;
        }
        io.charge(n)?;
        total += n as u64;
        if total > resource.size_bytes {
            bail!("copy_source_changed");
        }
        output.write_all(&buffer.bytes[..n])?;
        hash.update(&buffer.bytes[..n]);
    }
    if total != resource.size_bytes
        || format!("{:x}", hash.finalize()) != resource.sha256
        || current_stamp(source)?.as_ref() != Some(&observed)
        || stamp(&input.metadata()?) != observed
    {
        bail!("copy_source_changed");
    }
    output.sync_all()?;
    Ok(())
}
