use super::*;

#[derive(Clone)]
pub(crate) struct PostLocation {
    pub(crate) path: PathBuf,
    pub(crate) post: String,
    pub(crate) extra: Option<String>,
}
impl ContentStore {
    /// Legacy/unshared posts may not have a sync ledger yet. Probe only this
    /// exact logical post ID in each local thread (and its version directory),
    /// reading at most frontmatter. This grants no sharing membership and never
    /// samples a private post's avatar/attachments or creates another index.
    pub(super) fn validate_existing_post_relationship(
        &self,
        incoming: &PostVersion,
        io: &mut LocalIo,
    ) -> Result<()> {
        for thread in read_thread_ids(&self.root)? {
            if bbs_sync::safe_id(&thread).is_err()
                || self
                    .managed(&Self::thread_relative(&thread)?, false)
                    .is_err()
            {
                continue;
            }
            let mut paths = vec![self.managed(
                &Self::post_relative(&thread, &incoming.post_id, None)?,
                false,
            )?];
            let dir = self.managed(
                &Self::thread_relative(&thread)?
                    .join("versions")
                    .join(&incoming.post_id),
                false,
            )?;
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let version = entry.file_name().to_string_lossy().into_owned();
                    if bbs_sync::valid_hash(&version).is_ok() {
                        paths.push(self.managed(
                            &Self::post_relative(&thread, &incoming.post_id, Some(&version))?,
                            false,
                        )?);
                    }
                }
            }
            for path in paths {
                let Some(observed) = current_stamp(&path)? else {
                    continue;
                };
                let mut file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&path)?;
                let mut buffer = io.buffer()?;
                let mut prefix = Vec::new();
                let metadata = loop {
                    io.check()?;
                    let n = file.read(&mut buffer.bytes)?;
                    io.charge(n)?;
                    prefix.extend_from_slice(&buffer.bytes[..n]);
                    if !prefix.starts_with(b"---\n") {
                        bail!("invalid_post_relationship");
                    }
                    if let Some(end) = prefix[4..].windows(5).position(|s| s == b"\n---\n") {
                        if end > reconcile::MAX_FRONTMATTER_BYTES {
                            bail!("invalid_post_relationship");
                        }
                        break serde_yaml::from_slice::<BbsPostMeta>(&prefix[4..4 + end])?;
                    }
                    if n == 0 || prefix.len() > reconcile::MAX_FRONTMATTER_BYTES + 9 {
                        bail!("invalid_post_relationship");
                    }
                };
                if metadata.thread_id != thread
                    || metadata.post_id != incoming.post_id
                    || metadata.thread_id != incoming.thread_id
                    || metadata.kind != incoming.kind
                    || current_stamp(&path)?.as_ref() != Some(&observed)
                {
                    bail!("invalid_post_relationship");
                }
            }
        }
        Ok(())
    }
    pub(crate) fn locations(&self, thread: &str) -> Result<Vec<PostLocation>> {
        let relative = Self::thread_relative(thread)?;
        let mut result = Vec::new();
        let resident = self.managed(&relative.join("posts"), false)?;
        if resident.is_dir() {
            for entry in fs::read_dir(resident)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name();
                let Some(post) = name.to_str().and_then(|s| s.strip_suffix(".md")) else {
                    continue;
                };
                if bbs_sync::safe_id(post).is_ok() {
                    result.push(PostLocation {
                        path: entry.path(),
                        post: post.into(),
                        extra: None,
                    });
                }
            }
        }
        let versions = self.managed(&relative.join("versions"), false)?;
        if versions.is_dir() {
            for entry in fs::read_dir(versions)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let post = entry.file_name().to_string_lossy().into_owned();
                if bbs_sync::safe_id(&post).is_err() {
                    continue;
                }
                for version in fs::read_dir(entry.path())? {
                    let version = version?;
                    if !version.file_type()?.is_dir() {
                        continue;
                    }
                    let hash = version.file_name().to_string_lossy().into_owned();
                    if bbs_sync::valid_hash(&hash).is_err() {
                        continue;
                    }
                    let path =
                        self.managed(&Self::post_relative(thread, &post, Some(&hash))?, false)?;
                    if current_stamp(&path)?.is_some() {
                        result.push(PostLocation {
                            path,
                            post: post.clone(),
                            extra: Some(hash),
                        });
                    }
                }
            }
        }
        result.sort_by(|a, b| (&a.extra, &a.post).cmp(&(&b.extra, &b.post)));
        Ok(result)
    }
    pub(super) fn attachment_path(
        &self,
        thread: &str,
        location: &PostLocation,
        a: &AttachmentRef,
        create: bool,
    ) -> Result<PathBuf> {
        bbs_sync::safe_id(&a.id).map_err(anyhow::Error::msg)?;
        if !reconcile::valid_extension(&a.ext) {
            bail!("invalid_attachment_extension");
        }
        let base = if let Some(extra) = &location.extra {
            Self::post_relative(thread, &location.post, Some(extra))?
                .parent()
                .unwrap()
                .join("attachments")
        } else {
            Self::thread_relative(thread)?
                .join("attachments")
                .join(&location.post)
        };
        self.managed(
            &base.join(if a.ext.is_empty() {
                a.id.clone()
            } else {
                format!("{}.{}", a.id, a.ext)
            }),
            create,
        )
    }
    /// Cache hits require the exact inode/size/nanosecond mtime+ctime stamp.
    /// A moved or externally edited file is re-indexed before it is served.
    pub(super) fn locate_row(&self, row: &LedgerEntry) -> Result<Option<PostLocation>> {
        let Some(expected) = &row.body_stamp else {
            return Ok(None);
        };
        for extra in [None, Some(row.version_id.clone())] {
            let path = self.managed(
                &Self::post_relative(&row.thread_id, &row.post_id, extra.as_deref())?,
                false,
            )?;
            if current_stamp(&path)?.as_ref() == Some(expected) {
                return Ok(Some(PostLocation {
                    path,
                    post: row.post_id.clone(),
                    extra,
                }));
            }
        }
        Ok(None)
    }
    pub(crate) fn refresh(
        &self,
        fence: &GroupFence,
        io: &mut LocalIo,
    ) -> Result<Vec<reconcile::Issue>> {
        let roots = {
            let lock = acquire_write_lock(&self.root)?;
            self.check_fence(&lock, fence)?;
            self.state_locked(&lock)?
                .groups
                .get(&fence.group_id)
                .map(|g| g.shared_threads.clone())
                .unwrap_or_default()
        };
        let mut issues = Vec::new();
        for root in roots {
            io.check()?;
            match self.refresh_root(&root, Some(fence), io) {
                Ok(items) => issues.extend(items),
                Err(_) => issues.push(reconcile::Issue {
                    code: "local_thread_unavailable".into(),
                    thread_id: Some(root),
                    post_id: None,
                }),
            }
        }
        io.check()?;
        self.check_fence(&acquire_write_lock(&self.root)?, fence)?;
        Ok(issues)
    }
    pub(super) fn refresh_root(
        &self,
        thread: &str,
        fence: Option<&GroupFence>,
        io: &mut LocalIo,
    ) -> Result<Vec<reconcile::Issue>> {
        let old = {
            let lock = acquire_write_lock(&self.root)?;
            if let Some(fence) = fence {
                self.check_fence(&lock, fence)?;
            }
            self.state_locked(&lock)?
        };
        let mut issues = Vec::new();
        let record_path =
            self.managed(&Self::thread_relative(thread)?.join("thread.yaml"), false)?;
        let record = match (|| -> Result<_> {
            if current_stamp(&record_path)?.is_none() {
                return Ok(None);
            }
            let raw = read_bounded(&record_path, 64 * 1024, io)?;
            let meta: BbsThreadMeta = serde_yaml::from_slice(&raw.bytes)?;
            if meta.thread_id != thread {
                bail!("invalid_thread_relationship");
            }
            Ok(Some((record_from_meta(&meta)?, raw.stamp)))
        })() {
            Ok(record) => record,
            Err(_) => {
                io.check()?;
                issues.push(reconcile::Issue {
                    code: "invalid_local_thread_record".into(),
                    thread_id: Some(thread.into()),
                    post_id: None,
                });
                None
            }
        };
        let mut rows = Vec::new();
        let mut unavailable = Vec::new();
        let mut root_id: Option<String> = None;
        // Known immutable relationships are also checked when all previous
        // bytes have been edited away. This cache is not a second floor table.
        let mut known = BTreeMap::new();
        for row in old.ledger.values() {
            if let Some(p) = &row.descriptor {
                known.insert(p.post_id.clone(), (p.thread_id.clone(), p.kind.clone()));
                if p.thread_id == thread && p.kind == "topic" {
                    root_id = Some(p.post_id.clone());
                }
            }
        }
        for mut location in self.locations(thread)? {
            io.check()?;
            let inspected = (|| -> Result<_> {
                let observed =
                    current_stamp(&location.path)?.ok_or_else(|| anyhow!("source_changed"))?;
                let cached = old.ledger.values().find(|r| {
                    r.thread_id == thread
                        && r.post_id == location.post
                        && r.body_stamp.as_ref() == Some(&observed)
                        && r.descriptor.is_some()
                });
                let (mut p, metadata, mut body_stamp) = if let Some(row) = cached {
                    (row.descriptor.clone().unwrap(), None, observed.clone())
                } else {
                    let known_kind = known
                        .get(&location.post)
                        .filter(|(t, _)| t == thread)
                        .map(|(_, k)| k.as_str());
                    if let Some((item, proof)) = super::unavailable::ineligible_file(
                        &location.path,
                        thread,
                        &location.post,
                        known_kind,
                        io,
                    )? {
                        if known.get(&item.post_id).is_some_and(|(t, k)| {
                            t != thread || item.kind.as_ref().is_some_and(|v| v != k)
                        }) || (item.kind.as_deref() == Some("topic")
                            && root_id.as_ref().is_some_and(|id| id != &item.post_id))
                        {
                            bail!("invalid_post_relationship")
                        }
                        if !reconcile::logical_deleted(&old, thread, &item.post_id) {
                            if let Some(kind) = &item.kind {
                                known.insert(item.post_id.clone(), (thread.into(), kind.clone()));
                                if kind == "topic" {
                                    root_id = Some(item.post_id.clone())
                                }
                            }
                            unavailable.push((item, proof, location.path.clone()));
                        }
                        return Ok(None);
                    }
                    let raw = read_bounded(&location.path, reconcile::MAX_BODY_BYTES, io)?;
                    let meta = parse_post(&raw.bytes, thread, &location.post)?;
                    (
                        PostVersion {
                            thread_id: thread.into(),
                            post_id: location.post.clone(),
                            version_id: raw.sha256,
                            size_bytes: raw.bytes.len() as u64,
                            kind: meta.kind.clone(),
                            attachments: attachment_refs(&meta)?,
                            avatar: None,
                        },
                        Some(meta),
                        raw.stamp,
                    )
                };
                let relation = (p.thread_id.clone(), p.kind.clone());
                if known.get(&p.post_id).is_some_and(|v| v != &relation)
                    || (p.kind == "topic" && root_id.as_ref().is_some_and(|v| v != &p.post_id))
                {
                    bail!("invalid_post_relationship");
                }
                known.insert(p.post_id.clone(), relation);
                if p.kind == "topic" {
                    root_id = Some(p.post_id.clone());
                }
                if reconcile::suppressed(&old, thread, &p.post_id, &p.version_id) {
                    return Ok(None);
                }
                // Manual edits to an extra version remain a new raw-byte
                // version. Rename its private container, never reserialize MD.
                let previous_extra = location.extra.clone();
                if location.extra.as_ref().is_some_and(|v| v != &p.version_id) {
                    let lock = acquire_write_lock(&self.root)?;
                    if let Some(fence) = fence {
                        self.check_fence(&lock, fence)?;
                    }
                    let fresh = self.state_locked(&lock)?;
                    if reconcile::suppressed(&fresh, thread, &p.post_id, &p.version_id)
                        || current_stamp(&location.path)?.as_ref() != Some(&observed)
                    {
                        bail!("source_changed");
                    }
                    let destination = self.managed(
                        &Self::post_relative(thread, &p.post_id, Some(&p.version_id))?,
                        false,
                    )?;
                    if destination.parent().unwrap().exists() {
                        bail!("version_container_already_exists");
                    }
                    fs::rename(
                        location.path.parent().unwrap(),
                        destination.parent().unwrap(),
                    )?;
                    location.path = destination;
                    location.extra = Some(p.version_id.clone());
                    body_stamp =
                        current_stamp(&location.path)?.ok_or_else(|| anyhow!("source_changed"))?;
                }
                let mut origin = self.read_origin(thread, &p.post_id, &p.version_id)?;
                if origin.is_none() {
                    // An edit of a received post inherits its source sidecar;
                    // a coincident local hero ID must never substitute for it.
                    let previous = previous_extra
                        .as_ref()
                        .and_then(|v| self.read_origin(thread, &p.post_id, v).ok().flatten())
                        .or_else(|| {
                            old.ledger
                                .values()
                                .filter(|r| r.thread_id == thread && r.post_id == p.post_id)
                                .filter_map(|r| {
                                    self.read_origin(thread, &p.post_id, &r.version_id)
                                        .ok()
                                        .flatten()
                                })
                                .find(|o| o.origin == "received")
                        });
                    origin = Some(if let Some(previous) = previous {
                        previous
                    } else {
                        let avatar = if let Some(meta) = &metadata {
                            self.capture_avatar(meta, io)
                                .map(|v| v.0)
                                .unwrap_or_else(|_| AvatarSidecar::none())
                        } else {
                            p.avatar.clone().unwrap_or_else(AvatarSidecar::none)
                        };
                        OriginSnapshot {
                            schema_version: 1,
                            origin: "local".into(),
                            avatar,
                        }
                    });
                }
                let origin = origin.unwrap();
                p.avatar = Some(origin.avatar.clone());
                let key = LedgerEntry::version_key(&p.post_id, &p.version_id)
                    .map_err(anyhow::Error::msg)?;
                let existing = old.ledger.get(&key);
                let mut attachments = BTreeMap::new();
                for a in &mut p.attachments {
                    let path = self.attachment_path(thread, &location, a, false)?;
                    let cached = existing
                        .and_then(|r| r.attachments.get(&a.id))
                        .filter(|s| {
                            s.verified && s.sha256 == a.sha256 && s.size_bytes == a.size_bytes
                        })
                        .and_then(|s| s.stamp.as_ref());
                    let verified = verify_path(&path, &a.sha256, a.size_bytes, cached, io)?;
                    a.available = verified.is_some();
                    attachments.insert(
                        a.id.clone(),
                        AttachmentState {
                            sha256: a.sha256.clone(),
                            size_bytes: a.size_bytes,
                            verified: a.available,
                            stamp: verified,
                        },
                    );
                }
                let avatar_stamp =
                    if let Some(r) = p.avatar.as_ref().and_then(reconcile::avatar_resource) {
                        let ResourceKind::Avatar { ext, .. } = &r.identity else {
                            unreachable!()
                        };
                        let path = self.managed(
                            &Path::new("avatars").join(format!("{}.{}", r.sha256, ext)),
                            false,
                        )?;
                        verify_path(
                            &path,
                            &r.sha256,
                            r.size_bytes,
                            existing.and_then(|r| r.avatar_stamp.as_ref()),
                            io,
                        )?
                    } else {
                        None
                    };
                reconcile::validate_post(&p).map_err(anyhow::Error::msg)?;
                let row = LedgerEntry {
                    thread_id: thread.into(),
                    post_id: p.post_id.clone(),
                    version_id: p.version_id.clone(),
                    updated_at: now_iso(),
                    attachments,
                    body_stamp: Some(body_stamp),
                    descriptor: Some(p),
                    avatar_stamp,
                };
                Ok(Some((row, location.clone(), origin)))
            })();
            match inspected {
                Ok(Some(value)) => rows.push(value),
                Ok(None) => {}
                Err(_) => issues.push(reconcile::Issue {
                    code: "invalid_or_changed_local_version".into(),
                    thread_id: Some(thread.into()),
                    post_id: Some(location.post),
                }),
            }
        }
        let lock = acquire_write_lock(&self.root)?;
        if let Some(fence) = fence {
            self.check_fence(&lock, fence)?;
        }
        let mut fresh = self.state_locked(&lock)?;
        // Never replace a stale full snapshot: merge only inspected resources
        // into state freshly loaded under the CLI/App/content short flock.
        for row in fresh.ledger.values_mut().filter(|r| r.thread_id == thread) {
            row.body_stamp = None;
            row.avatar_stamp = None;
            for a in row.attachments.values_mut() {
                a.verified = false;
                a.stamp = None;
            }
        }
        if let Some((record, observed)) = record {
            if current_stamp(&record_path)?.as_ref() == Some(&observed) {
                fresh.thread_records.insert(thread.into(), record);
            }
        } else {
            fresh.thread_records.remove(thread);
        }
        // This is a local source cache, not received placeholder state. A scan
        // may replace it without making absence mean deletion on another device.
        fresh.local_unavailable.retain(|_, p| p.thread_id != thread);
        for (item, observed, path) in unavailable {
            if !reconcile::logical_deleted(&fresh, thread, &item.post_id)
                && current_stamp(&path)?.as_ref() == Some(&observed)
            {
                fresh
                    .local_unavailable
                    .insert(item.key().map_err(anyhow::Error::msg)?, item);
            }
        }
        for (mut row, location, origin) in rows {
            if reconcile::suppressed(&fresh, thread, &row.post_id, &row.version_id)
                || current_stamp(&location.path)? != row.body_stamp
            {
                continue;
            }
            self.write_origin_locked(&lock, thread, &row.post_id, &row.version_id, &origin)?;
            self.clear_unavailable_locked(&lock, thread, &row.post_id)?;
            let p = row.descriptor.as_mut().unwrap();
            for a in &mut p.attachments {
                let state = row.attachments.get_mut(&a.id).unwrap();
                if current_stamp(&self.attachment_path(thread, &location, a, false)?)?
                    != state.stamp
                {
                    state.verified = false;
                    state.stamp = None;
                    a.available = false;
                }
            }
            fresh
                .ledger
                .insert(row.key().map_err(anyhow::Error::msg)?, row);
        }
        self.save_locked(&lock, &fresh)?;
        Ok(issues)
    }
}

/// Streams even a 1 GiB attachment with one budgeted chunk. No whole-file
/// buffer, no repeated hash for an unchanged verified resource.
pub(super) fn verify_path(
    path: &Path,
    hash: &str,
    size: u64,
    cached: Option<&FileStamp>,
    io: &mut LocalIo,
) -> Result<Option<FileStamp>> {
    io.check()?;
    let Some(before) = current_stamp(path)? else {
        return Ok(None);
    };
    if before.len != size {
        return Ok(None);
    }
    if cached == Some(&before) {
        return Ok(Some(before));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() || stamp(&file.metadata()?) != before {
        bail!("source_changed");
    }
    let mut buffer = io.buffer()?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        io.check()?;
        let n = file.read(&mut buffer.bytes)?;
        if n == 0 {
            break;
        }
        io.charge(n)?;
        total += n as u64;
        if total > size {
            bail!("source_changed");
        }
        hasher.update(&buffer.bytes[..n]);
    }
    if current_stamp(path)?.as_ref() != Some(&before) || stamp(&file.metadata()?) != before {
        bail!("source_changed");
    }
    Ok((total == size && format!("{:x}", hasher.finalize()) == hash).then_some(before))
}
