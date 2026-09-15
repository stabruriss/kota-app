use super::inventory::{verify_path, PostLocation};
use super::*;

struct ObservedPost {
    location: PostLocation,
    meta: BbsPostMeta,
    version: String,
    stamp: FileStamp,
}
impl ContentStore {
    fn observed_posts(
        &self,
        thread: &str,
        mut file_io: Option<&mut LocalIo>,
    ) -> Result<Vec<ObservedPost>> {
        let mut result = Vec::new();
        for location in self.locations(thread)? {
            let Some(before) = current_stamp(&location.path)? else {
                continue;
            };
            let parsed = if let Some(io) = file_io.as_deref_mut() {
                read_bounded(&location.path, reconcile::MAX_BODY_BYTES, io).and_then(|raw| {
                    Ok((parse_post(&raw.bytes, thread, &location.post)?, raw.sha256))
                })
            } else {
                read_post(&location.path).map(|p| (p.meta, p.version_id))
            };
            let Ok((meta, version)) = parsed else {
                continue;
            };
            if meta.thread_id != thread
                || meta.post_id != location.post
                || !matches!(meta.kind.as_str(), "topic" | "reply")
            {
                continue;
            }
            if current_stamp(&location.path)?.as_ref() != Some(&before) {
                bail!("source_changed");
            }
            result.push(ObservedPost {
                location,
                meta,
                version,
                stamp: before,
            });
        }
        Ok(result)
    }
    /// Local explicit deletion. Tombstone is durable before any destructive
    /// rename. Slow recursive removal is after releasing the BBS short lock.
    pub(crate) fn delete_local(
        &self,
        thread: &str,
        post: Option<&str>,
        version: Option<&str>,
    ) -> Result<()> {
        bbs_sync::safe_id(thread).map_err(anyhow::Error::msg)?;
        if let Some(p) = post {
            bbs_sync::safe_id(p).map_err(anyhow::Error::msg)?;
        }
        if let Some(v) = version {
            bbs_sync::valid_hash(v).map_err(anyhow::Error::msg)?;
            if post.is_none() {
                bail!("version_requires_post_id");
            }
        }
        let observed = if post.is_none() {
            vec![]
        } else {
            self.observed_posts(thread, None)?
        };
        let target = if let Some(post) = post {
            Some(
                observed
                    .iter()
                    .find(|p| p.meta.post_id == post && version.is_none_or(|v| p.version == v))
                    .ok_or_else(|| anyhow!("BBS version or post not found"))?,
            )
        } else {
            None
        };
        let deletion = Tombstone {
            thread_id: thread.into(),
            post_id: if version.is_some() || target.is_some_and(|p| p.meta.kind != "topic") {
                post.map(str::to_owned)
            } else {
                None
            },
            version_id: version.map(str::to_owned),
            deleted_at: now_iso(),
        };
        let mut garbage = Vec::new();
        {
            let lock = acquire_write_lock(&self.root)?;
            if let Some(target) = target {
                if current_stamp(&target.location.path)?.as_ref() != Some(&target.stamp) {
                    bail!("BBS target changed; retry deletion");
                }
            } else if !self
                .managed(&Self::thread_relative(thread)?, false)?
                .is_dir()
            {
                bail!("BBS thread not found");
            }
            let mut state = self.state_locked(&lock)?;
            if let Some(post) = post.filter(|_| version.is_some()) {
                let mut surviving = BTreeSet::new();
                for item in &observed {
                    if item.meta.post_id == post
                        && !reconcile::suppressed(&state, thread, post, &item.version)
                        && current_stamp(&item.location.path)?.as_ref() == Some(&item.stamp)
                    {
                        surviving.insert(&item.version);
                    }
                }
                if surviving.len() <= 1 {
                    bail!("last_version_use_logical_delete");
                }
            }
            state
                .tombstones
                .entry(deletion.key().map_err(anyhow::Error::msg)?)
                .or_insert(deletion);
            self.save_locked(&lock, &state)?;
            self.detach_deleted(
                &lock,
                &mut state,
                thread,
                &observed,
                &BTreeMap::new(),
                &mut garbage,
            )?;
            self.save_locked(&lock, &state)?;
        }
        self.state.mark_content_changed();
        for path in garbage {
            let result = if path.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
            if result.is_err() {
                crate::kota_debug_log("[bbs-sync] deleted_content_cleanup_pending");
            }
        }
        Ok(())
    }
    pub(super) fn recover_deletions(
        &self,
        fence: Option<&GroupFence>,
        roots: &BTreeSet<String>,
        io: &mut LocalIo,
    ) -> Result<()> {
        for thread in roots {
            io.check()?;
            let state = {
                let lock = acquire_write_lock(&self.root)?;
                if let Some(f) = fence {
                    self.check_fence(&lock, f)?;
                }
                self.state_locked(&lock)?
            };
            let whole = state.tombstones.contains_key(&format!("thread:{thread}"));
            // Full-thread removal doesn't parse a possibly damaged body.
            let has_deletions = state.tombstones.values().any(|d| d.thread_id == *thread);
            if !has_deletions {
                continue;
            }
            let observed = if whole {
                Vec::new()
            } else {
                self.observed_posts(thread, Some(io))?
            };
            let mut moved_proofs = BTreeMap::new();
            for p in &observed {
                if p.location.extra.is_none()
                    || reconcile::suppressed(&state, thread, &p.meta.post_id, &p.version)
                {
                    continue;
                }
                let src = p.location.path.parent().unwrap().join("attachments");
                let target = self.managed(
                    &Self::thread_relative(thread)?
                        .join("attachments")
                        .join(&p.meta.post_id),
                    false,
                )?;
                // A crash may have moved attachments but not post.md. Validate
                // every existing destination file; absence alone is no proof.
                if !src.exists() && target.exists() {
                    let declared = attachment_refs(&p.meta)?;
                    let mut proof = BTreeMap::new();
                    let mut valid = true;
                    for entry in fs::read_dir(&target)? {
                        let entry = entry?;
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let Some(a) = declared.iter().find(|a| {
                            name == if a.ext.is_empty() {
                                a.id.clone()
                            } else {
                                format!("{}.{}", a.id, a.ext)
                            }
                        }) else {
                            valid = false;
                            break;
                        };
                        match verify_path(&entry.path(), &a.sha256, a.size_bytes, None, io)? {
                            Some(stamp) => {
                                proof.insert(entry.path(), stamp);
                            }
                            None => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if valid {
                        moved_proofs.insert(p.version.clone(), proof);
                    }
                }
            }
            let mut garbage = Vec::new();
            {
                let lock = acquire_write_lock(&self.root)?;
                if let Some(f) = fence {
                    self.check_fence(&lock, f)?;
                }
                let mut fresh = self.state_locked(&lock)?;
                self.detach_deleted(
                    &lock,
                    &mut fresh,
                    thread,
                    &observed,
                    &moved_proofs,
                    &mut garbage,
                )?;
                self.save_locked(&lock, &fresh)?;
            }
            for path in garbage {
                remove_budgeted(&path, io)?;
            }
        }
        Ok(())
    }
    fn retire(&self, path: &Path, garbage: &mut Vec<PathBuf>) -> Result<()> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        let directory = self.directory(Path::new(".sync-staging"))?;
        let destination = directory.join(format!(".removed-{}", Uuid::new_v4()));
        fs::rename(path, &destination)?;
        garbage.push(destination);
        Ok(())
    }
    fn detach_deleted(
        &self,
        _lock: &BbsWriteLock,
        state: &mut SyncState,
        thread: &str,
        posts: &[ObservedPost],
        moved: &BTreeMap<String, BTreeMap<PathBuf, FileStamp>>,
        garbage: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let relative = Self::thread_relative(thread)?;
        if state.tombstones.contains_key(&format!("thread:{thread}")) {
            self.retire(&self.managed(&relative, false)?, garbage)?;
            state.ledger.retain(|_, row| row.thread_id != thread);
            state.thread_records.remove(thread);
            state
                .local_unavailable
                .retain(|_, row| row.thread_id != thread);
            warn_index_failure(remove_thread_from_board(&self.root, thread));
            return Ok(());
        }
        let logical: BTreeSet<_> = state
            .tombstones
            .values()
            .filter(|d| d.thread_id == thread && d.version_id.is_none())
            .filter_map(|d| d.post_id.clone())
            .collect();
        for post in &logical {
            for path in [
                Self::post_relative(thread, post, None)?,
                relative.join("attachments").join(post),
                relative.join("versions").join(post),
                relative.join("sidecars").join(post),
                relative.join("placeholders").join(format!("{post}.json")),
            ] {
                self.retire(&self.managed(&path, false)?, garbage)?;
            }
        }
        for p in posts {
            if logical.contains(&p.meta.post_id)
                || !reconcile::suppressed(state, thread, &p.meta.post_id, &p.version)
            {
                continue;
            }
            if current_stamp(&p.location.path)?.as_ref() != Some(&p.stamp) {
                continue;
            }
            if p.location.extra.is_some() {
                self.retire(p.location.path.parent().unwrap(), garbage)?;
            } else {
                self.retire(&p.location.path, garbage)?;
                self.retire(
                    &self.managed(&relative.join("attachments").join(&p.meta.post_id), false)?,
                    garbage,
                )?;
            }
            self.retire(
                &self.managed(
                    &Self::sidecar_relative(thread, &p.meta.post_id, &p.version)?,
                    false,
                )?,
                garbage,
            )?;
        }
        let mut promoted = BTreeSet::new();
        // No journal: surviving version files are the recovery intent. Move
        // their attachment directory first, then their exact raw post bytes.
        for p in posts {
            if p.location.extra.is_none()
                || promoted.contains(&p.meta.post_id)
                || reconcile::suppressed(state, thread, &p.meta.post_id, &p.version)
            {
                continue;
            }
            let resident =
                self.managed(&Self::post_relative(thread, &p.meta.post_id, None)?, true)?;
            if current_stamp(&resident)?.is_some()
                || current_stamp(&p.location.path)?.as_ref() != Some(&p.stamp)
            {
                continue;
            }
            let source = p.location.path.parent().unwrap().join("attachments");
            let destination =
                self.managed(&relative.join("attachments").join(&p.meta.post_id), true)?;
            if source.exists() {
                if !fs::symlink_metadata(&source)?.is_dir()
                    || fs::symlink_metadata(&source)?.file_type().is_symlink()
                {
                    bail!("unsafe_sync_path");
                }
                self.retire(&destination, garbage)?;
                fs::rename(&source, &destination)?;
            } else if destination.exists() {
                let valid = moved.get(&p.version).is_some_and(|proof| {
                    proof.iter().all(|(path, stamp)| {
                        current_stamp(path).ok().flatten().as_ref() == Some(stamp)
                    })
                });
                if !valid {
                    self.retire(&destination, garbage)?;
                }
            }
            fs::rename(&p.location.path, &resident)?;
            File::open(resident.parent().unwrap())?.sync_all()?;
            if let Some(row) = state.ledger.get_mut(
                &LedgerEntry::version_key(&p.meta.post_id, &p.version)
                    .map_err(anyhow::Error::msg)?,
            ) {
                row.body_stamp = current_stamp(&resident)?;
                // Parent renames preserve child file stamps; mismatches are
                // checked by projection/source and repaired by inventory.
            }
            let _ = fs::remove_dir(p.location.path.parent().unwrap());
            promoted.insert(p.meta.post_id.clone());
        }
        state.ledger.retain(|_, row| {
            !state
                .tombstones
                .values()
                .any(|d| reconcile::covers(d, &row.thread_id, &row.post_id, &row.version_id))
        });
        state
            .local_unavailable
            .retain(|_, row| !(row.thread_id == thread && logical.contains(&row.post_id)));
        Ok(())
    }
    /// Run only while holding the account file permit (LocalIo). Consequently
    /// no live receive/copy can own one of these same staging paths.
    pub(crate) fn recover_staging(&self, io: &mut LocalIo) -> Result<()> {
        let directory = self.prepare_layout()?;
        for entry in fs::read_dir(directory)? {
            io.check()?;
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let own = name
                .strip_prefix(".receive-")
                .and_then(|s| s.strip_suffix(".partial"))
                .or_else(|| name.strip_prefix(".removed-"));
            if own.is_some_and(|s| Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s)) {
                remove_budgeted(&entry.path(), io)?;
            }
        }
        Ok(())
    }
}
fn remove_budgeted(path: &Path, io: &mut LocalIo) -> Result<()> {
    io.check()?;
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if meta.is_dir() && !meta.file_type().is_symlink() {
        for entry in fs::read_dir(path)? {
            remove_budgeted(&entry?.path(), io)?;
        }
        fs::remove_dir(path)?;
    } else {
        fs::remove_file(path)?;
    }
    // Bound metadata write bursts too, including a large removed attachment
    // directory. This shares the same 8 MiB/s scheduled allowance as file work.
    io.charge(16 * 1024)?;
    Ok(())
}
