//! Ineligible-file facts are neither posts nor versions. Received markers stay
//! local; only the separately rebuilt source inventory can advertise a fact.
use super::*;

pub(crate) const PLACEHOLDER_TEXT: &str = "File size too large to sync.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnavailableView {
    pub post_id: String,
    pub reason: UnavailableReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema_version: u32,
    post: UnavailableView,
}
impl ContentStore {
    fn marker_path(&self, thread: &str, post: &str, parents: bool) -> Result<PathBuf> {
        bbs_sync::safe_id(post).map_err(anyhow::Error::msg)?;
        self.managed(
            &Self::thread_relative(thread)?
                .join("placeholders")
                .join(format!("{post}.json")),
            parents,
        )
    }
    fn read_marker(&self, thread: &str, post: &str) -> Result<Option<UnavailableView>> {
        let path = self.marker_path(thread, post, false)?;
        let Some(before) = current_stamp(&path)? else {
            return Ok(None);
        };
        if before.len > reconcile::MAX_THREAD_BYTES as u64 {
            bail!("invalid_unavailable_marker");
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        if !file.metadata()?.is_file() || stamp(&file.metadata()?) != before {
            bail!("invalid_unavailable_marker");
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(reconcile::MAX_THREAD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != before.len
            || stamp(&file.metadata()?) != before
            || current_stamp(&path)?.as_ref() != Some(&before)
        {
            bail!("unavailable_marker_changed");
        }
        let marker: Marker = serde_json::from_slice(&bytes)?;
        if marker.schema_version != 1 || marker.post.post_id != post {
            bail!("invalid_unavailable_marker");
        }
        UnavailablePost {
            thread_id: thread.into(),
            post_id: post.into(),
            reason: marker.post.reason,
            kind: marker.post.kind.clone(),
        }
        .key()
        .map_err(anyhow::Error::msg)?;
        Ok(Some(marker.post))
    }
    pub(super) fn clear_unavailable_locked(
        &self,
        _lock: &BbsWriteLock,
        thread: &str,
        post: &str,
    ) -> Result<()> {
        let path = self.marker_path(thread, post, false)?;
        match fs::remove_file(&path) {
            Ok(()) => {
                File::open(path.parent().unwrap())?.sync_all()?;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    /// Returns a fixed diagnostic for a conflicting known kind. No body
    /// identity is inferred, and unknown can only be refined, never downgraded.
    pub(super) fn mark_unavailable_locked(
        &self,
        lock: &BbsWriteLock,
        state: &SyncState,
        post: &UnavailablePost,
    ) -> Result<Option<reconcile::Issue>> {
        reconcile::validate_unavailable(post).map_err(anyhow::Error::msg)?;
        if !state.thread_records.contains_key(&post.thread_id)
            || reconcile::logical_deleted(state, &post.thread_id, &post.post_id)
        {
            return Ok(None);
        }
        for row in state.ledger.values().filter(|r| {
            r.thread_id == post.thread_id
                && r.post_id == post.post_id
                && !reconcile::suppressed(state, &r.thread_id, &r.post_id, &r.version_id)
        }) {
            if self.locate_row(row)?.is_some() {
                self.clear_unavailable_locked(lock, &post.thread_id, &post.post_id)?;
                return Ok(None);
            }
        }
        let old = self.read_marker(&post.thread_id, &post.post_id)?;
        if let Some(old) = &old {
            if old.kind.is_some() && post.kind.is_some() && old.kind != post.kind {
                return Ok(Some(reconcile::Issue {
                    code: "unavailable_kind_mismatch".into(),
                    thread_id: Some(post.thread_id.clone()),
                    post_id: Some(post.post_id.clone()),
                }));
            }
            if old.kind.is_some() || post.kind.is_none() {
                return Ok(None);
            }
        }
        let marker = Marker {
            schema_version: 1,
            post: UnavailableView {
                post_id: post.post_id.clone(),
                reason: post.reason,
                kind: post.kind.clone(),
            },
        };
        let bytes = serde_json::to_vec(&marker)?;
        if bytes.len() > reconcile::MAX_THREAD_BYTES {
            bail!("invalid_unavailable_marker");
        }
        write_small(
            &self.marker_path(&post.thread_id, &post.post_id, true)?,
            &bytes,
        )?;
        Ok(None)
    }
    /// Snapshot/show projection is independent of membership or peer liveness.
    /// A verified/local parsed body takes precedence even after a cleanup crash.
    pub(crate) fn unavailable_views(
        &self,
        thread: &str,
        state: &SyncState,
        real_posts: &BTreeSet<String>,
    ) -> Result<Vec<UnavailableView>> {
        if state.tombstones.contains_key(&format!("thread:{thread}")) {
            return Ok(vec![]);
        }
        let dir = self.managed(&Self::thread_relative(thread)?.join("placeholders"), false)?;
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut views = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
                continue;
            };
            if bbs_sync::safe_id(id).is_err()
                || real_posts.contains(id)
                || reconcile::logical_deleted(state, thread, id)
            {
                continue;
            }
            // A broken sidecar cannot hide neighboring real posts or markers.
            if let Ok(Some(view)) = self.read_marker(thread, id) {
                views.push(view)
            }
        }
        views.sort_by(|a, b| a.post_id.cmp(&b.post_id));
        Ok(views)
    }
}

/// Inspect eligibility before hashing an uncached body. At most 64 KiB+9 bytes
/// are read even for an enormous/unterminated frontmatter. Invalid small headers
/// remain errors; oversized headers carry no invented kind or author.
pub(super) fn ineligible_file(
    path: &Path,
    thread: &str,
    post: &str,
    known_kind: Option<&str>,
    io: &mut LocalIo,
) -> Result<Option<(UnavailablePost, FileStamp)>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("unsafe_sync_file")
    }
    let before = stamp(&metadata);
    let limit = reconcile::MAX_FRONTMATTER_BYTES + 9;
    let mut prefix = Vec::new();
    let mut buffer = io.buffer()?;
    let mut parsed_kind = None;
    let mut header_too_large = false;
    loop {
        io.check()?;
        let capacity = buffer.bytes.len().min(limit - prefix.len());
        let n = file.read(&mut buffer.bytes[..capacity])?;
        io.charge(n)?;
        prefix.extend_from_slice(&buffer.bytes[..n]);
        if prefix.len() >= 4 && !prefix.starts_with(b"---\n") {
            bail!("invalid_sync_frontmatter");
        }
        if prefix.len() >= 4 {
            if let Some(end) = prefix[4..].windows(5).position(|s| s == b"\n---\n") {
                let meta: BbsPostMeta = serde_yaml::from_slice(&prefix[4..4 + end])?;
                validate_post_meta(&meta, thread, post)?;
                parsed_kind = Some(meta.kind);
                break;
            }
        }
        if prefix.len() == limit {
            header_too_large = true;
            break;
        }
        if n == 0 {
            bail!("invalid_sync_frontmatter")
        }
    }
    if stamp(&file.metadata()?) != before || current_stamp(path)?.as_ref() != Some(&before) {
        bail!("source_changed");
    }
    if before.len <= reconcile::MAX_BODY_BYTES && !header_too_large {
        return Ok(None);
    }
    let kind = parsed_kind.or_else(|| known_kind.map(str::to_owned));
    let item = UnavailablePost {
        thread_id: thread.into(),
        post_id: post.into(),
        reason: UnavailableReason::TooLargeToSync,
        kind,
    };
    reconcile::validate_unavailable(&item).map_err(anyhow::Error::msg)?;
    Ok(Some((item, before)))
}
