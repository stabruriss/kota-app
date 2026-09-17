//! Pure paginated manifests and delete-first planning. Files, credentials,
//! timers and sockets remain the repository/coordinator's responsibilities.
use super::{
    raw_sha256, safe_id,
    transport::{Resource, ResourceKind},
    valid_hash, AvatarSidecar, Manifest, ManifestPhase, PostVersion, SyncState, ThreadRecord,
    ThreadRecordItem, Tombstone, UnavailablePost,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_PAGE_BYTES: usize = 16_375;
pub(crate) const MAX_PAGE_ITEMS: usize = 64;
pub(crate) const MAX_THREAD_BYTES: usize = 2048;
pub(crate) const MAX_VERSION_BYTES: usize = 4096;
// User-approved per-post sync eligibility; local publishing is not limited.
// MAX_BODY_BYTES includes the entire original Markdown, including frontmatter.
pub(crate) const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;

pub(crate) fn timestamp(value: &str) -> Result<(), String> {
    if value.len() > 64 {
        return Err("invalid_manifest_time".into());
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| "invalid_manifest_time".into())
}
pub(crate) fn valid_extension(value: &str) -> bool {
    value.len() <= 16 && value.bytes().all(|v| v.is_ascii_alphanumeric())
}
/// Fixed lexicographic JSON field order, independent of serde_json's optional
/// preserve_order feature. Arrays retain order. Derived thread fields excluded.
pub(crate) fn thread_bytes(record: &ThreadRecord) -> Result<Vec<u8>, String> {
    let values = BTreeMap::from([
        ("schema", serde_json::json!(record.schema)),
        ("threadId", serde_json::json!(record.thread_id)),
        ("status", serde_json::json!(record.status)),
        ("visibility", serde_json::json!(record.visibility)),
        ("projectTags", serde_json::json!(record.project_tags)),
        (
            "createdByProject",
            serde_json::json!(record.created_by_project),
        ),
        ("createdByAgent", serde_json::json!(record.created_by_agent)),
        ("createdAt", serde_json::json!(record.created_at)),
    ]);
    serde_json::to_vec(&values).map_err(|_| "invalid_thread_record".into())
}
pub(crate) fn thread_item(record: ThreadRecord) -> Result<ThreadRecordItem, String> {
    let sha256 = raw_sha256(&thread_bytes(&record)?);
    let item = ThreadRecordItem { record, sha256 };
    validate_thread(&item)?;
    Ok(item)
}
pub(crate) fn validate_thread(item: &ThreadRecordItem) -> Result<(), String> {
    let r = &item.record;
    if r.schema != "kota.bbs.thread.v1"
        || !matches!(r.visibility.as_str(), "targeted" | "broadcast")
        || r.project_tags.len() > 32
        || r.status.is_empty()
        || r.status.len() > 64
        || !r
            .status
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid_thread_record".into());
    }
    safe_id(&r.thread_id)?;
    safe_id(&r.created_by_project)?;
    safe_id(&r.created_by_agent)?;
    for id in &r.project_tags {
        if id.len() > 64 {
            return Err("invalid_thread_record".into());
        }
        safe_id(id)?;
    }
    timestamp(&r.created_at)?;
    valid_hash(&item.sha256)?;
    if raw_sha256(&thread_bytes(r)?) != item.sha256 {
        return Err("thread_record_hash_mismatch".into());
    }
    if encoded_len(item)? > MAX_THREAD_BYTES {
        return Err("thread_record_too_large".into());
    }
    Ok(())
}
pub(crate) fn validate_post(post: &PostVersion) -> Result<(), String> {
    safe_id(&post.thread_id)?;
    safe_id(&post.post_id)?;
    valid_hash(&post.version_id)?;
    if post.size_bytes == 0 || post.size_bytes > MAX_BODY_BYTES {
        return Err("sync_post_too_large_or_empty".into());
    }
    if !matches!(post.kind.as_str(), "topic" | "reply") {
        return Err("invalid_post_relationship".into());
    }
    if post.attachments.len() > crate::bbs::MAX_ATTACHMENTS {
        return Err("too_many_sync_attachments".into());
    }
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    for a in &post.attachments {
        safe_id(&a.id)?;
        valid_hash(&a.sha256)?;
        if !ids.insert(&a.id)
            || !valid_extension(&a.ext)
            || a.size_bytes > crate::bbs::MAX_ATTACHMENT_BYTES
        {
            return Err("invalid_sync_attachment".into());
        }
        total = total
            .checked_add(a.size_bytes)
            .ok_or("attachment_total_overflow")?;
        if total > crate::bbs::MAX_POST_ATTACHMENT_BYTES {
            return Err("sync_attachments_too_large".into());
        }
    }
    post.avatar
        .as_ref()
        .ok_or("missing_origin_avatar")?
        .validate()?;
    if encoded_len(post)? > MAX_VERSION_BYTES {
        return Err("manifest_version_too_large".into());
    }
    Ok(())
}
fn encoded_len(value: &impl Serialize) -> Result<usize, String> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|_| "invalid_manifest".into())
}
pub(crate) fn validate_unavailable(post: &UnavailablePost) -> Result<(), String> {
    post.key()?;
    if encoded_len(post)? > MAX_THREAD_BYTES {
        return Err("unavailable_item_too_large".into());
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct VersionCursor(pub String, pub String, pub String);
impl VersionCursor {
    fn of(post: &PostVersion) -> Self {
        Self(
            post.thread_id.clone(),
            post.post_id.clone(),
            post.version_id.clone(),
        )
    }
    fn validate(&self) -> Result<(), String> {
        safe_id(&self.0)?;
        safe_id(&self.1)?;
        valid_hash(&self.2)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cursor {
    Delete(String),
    Thread(String),
    Version(VersionCursor),
    Unavailable((String, String)),
}
fn cursor(phase: ManifestPhase, value: &Value) -> Result<Cursor, String> {
    match phase {
        ManifestPhase::Tombstones => {
            let key = value.as_str().ok_or("invalid_manifest_cursor")?;
            Tombstone::from_key(key, String::new())?;
            Ok(Cursor::Delete(key.into()))
        }
        ManifestPhase::Threads => {
            let id = value.as_str().ok_or("invalid_manifest_cursor")?;
            safe_id(id)?;
            Ok(Cursor::Thread(id.into()))
        }
        ManifestPhase::Versions => {
            let key: VersionCursor =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_manifest_cursor")?;
            key.validate()?;
            Ok(Cursor::Version(key))
        }
        ManifestPhase::Unavailable => {
            let key: (String, String) =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_manifest_cursor")?;
            safe_id(&key.0)?;
            safe_id(&key.1)?;
            Ok(Cursor::Unavailable(key))
        }
    }
}
fn cursor_value(value: &Cursor) -> Value {
    match value {
        Cursor::Delete(s) | Cursor::Thread(s) => Value::String(s.clone()),
        Cursor::Version(key) => serde_json::to_value(key).expect("string tuple"),
        Cursor::Unavailable(key) => serde_json::to_value(key).expect("string pair"),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Issue {
    pub(crate) code: String,
    pub(crate) thread_id: Option<String>,
    pub(crate) post_id: Option<String>,
}
impl Issue {
    fn item(code: impl Into<String>, value: &Value) -> Self {
        let safe = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|s| safe_id(s).is_ok())
                .map(str::to_string)
        };
        Self {
            code: code.into(),
            thread_id: safe("thread_id").or_else(|| {
                value
                    .get("record")
                    .and_then(|r| r.get("threadId"))
                    .and_then(Value::as_str)
                    .filter(|s| safe_id(s).is_ok())
                    .map(str::to_string)
            }),
            post_id: safe("post_id"),
        }
    }
}
#[derive(Default, Debug)]
pub(crate) struct ParsedPage {
    pub(crate) deletions: Vec<Tombstone>,
    pub(crate) threads: Vec<ThreadRecordItem>,
    pub(crate) posts: Vec<PostVersion>,
    pub(crate) unavailable_posts: Vec<UnavailablePost>,
    pub(crate) issues: Vec<Issue>,
}

/// One peer/round, four sequential phases. The repository advances a cloned
/// reader only after the page's durable mutations succeed; an interrupted page
/// can be replayed without skipping deletion or content records.
#[derive(Clone, Debug)]
pub(crate) struct ManifestReader {
    phase: ManifestPhase,
    after: Option<Value>,
    complete: bool,
}
impl Default for ManifestReader {
    fn default() -> Self {
        Self {
            phase: ManifestPhase::Tombstones,
            after: None,
            complete: false,
        }
    }
}
impl ManifestReader {
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn read(&mut self, page: &Manifest) -> Result<ParsedPage, String> {
        if self.complete {
            return Err("manifest_already_complete".into());
        }
        let parsed = parse_page(page, self.phase, self.after.as_ref())?;
        self.after = page.next.clone();
        if self.after.is_none() {
            match self.phase {
                ManifestPhase::Tombstones => self.phase = ManifestPhase::Threads,
                ManifestPhase::Threads => self.phase = ManifestPhase::Versions,
                ManifestPhase::Versions => self.phase = ManifestPhase::Unavailable,
                ManifestPhase::Unavailable => self.complete = true,
            }
        }
        Ok(parsed)
    }
}
fn item_cursor(phase: ManifestPhase, value: &Value) -> Result<Cursor, String> {
    match phase {
        ManifestPhase::Tombstones => {
            let d: Tombstone =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_manifest_deletion")?;
            Ok(Cursor::Delete(d.key()?))
        }
        ManifestPhase::Threads => {
            let t: ThreadRecordItem =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_thread_record")?;
            safe_id(&t.record.thread_id)?;
            Ok(Cursor::Thread(t.record.thread_id))
        }
        ManifestPhase::Versions => {
            let p: PostVersion =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_manifest_version")?;
            let c = VersionCursor::of(&p);
            c.validate()?;
            Ok(Cursor::Version(c))
        }
        ManifestPhase::Unavailable => {
            let p: UnavailablePost =
                serde_json::from_value(value.clone()).map_err(|_| "invalid_unavailable_post")?;
            p.key()?;
            Ok(Cursor::Unavailable((p.thread_id, p.post_id)))
        }
    }
}
/// Reject a bad envelope/order as a protocol error. Invalid individual records
/// are reported and skipped; valid neighbors in the same page still progress.
pub(crate) fn parse_page(
    page: &Manifest,
    expected: ManifestPhase,
    after: Option<&Value>,
) -> Result<ParsedPage, String> {
    if page.phase != expected
        || page.items.len() > MAX_PAGE_ITEMS
        || encoded_len(page)? > MAX_PAGE_BYTES
    {
        return Err("invalid_manifest_page".into());
    }
    let previous = after.map(|v| cursor(expected, v)).transpose()?;
    let next = page
        .next
        .as_ref()
        .map(|v| cursor(expected, v))
        .transpose()?;
    if next
        .as_ref()
        .is_some_and(|n| previous.as_ref().is_some_and(|p| n <= p))
    {
        return Err("manifest_cursor_did_not_advance".into());
    }
    let mut last = previous;
    let mut parsed = ParsedPage::default();
    for value in &page.items {
        let key = match item_cursor(expected, value) {
            Ok(v) => v,
            Err(e) => {
                parsed.issues.push(Issue::item(e, value));
                continue;
            }
        };
        if last.as_ref().is_some_and(|p| &key <= p) || next.as_ref().is_some_and(|n| &key > n) {
            return Err("manifest_items_out_of_order".into());
        }
        last = Some(key);
        let result: Result<(), String> = (|| {
            match expected {
                ManifestPhase::Tombstones => {
                    let d: Tombstone = serde_json::from_value(value.clone())
                        .map_err(|_| "invalid_manifest_deletion")?;
                    d.validate()?;
                    timestamp(&d.deleted_at)?;
                    if encoded_len(&d)? > MAX_THREAD_BYTES {
                        return Err("manifest_deletion_too_large".into());
                    }
                    parsed.deletions.push(d);
                }
                ManifestPhase::Threads => {
                    let t = serde_json::from_value(value.clone())
                        .map_err(|_| "invalid_thread_record")?;
                    validate_thread(&t)?;
                    parsed.threads.push(t);
                }
                ManifestPhase::Versions => {
                    let p = serde_json::from_value(value.clone())
                        .map_err(|_| "invalid_manifest_version")?;
                    validate_post(&p)?;
                    parsed.posts.push(p);
                }
                ManifestPhase::Unavailable => {
                    let p = serde_json::from_value(value.clone())
                        .map_err(|_| "invalid_unavailable_post")?;
                    validate_unavailable(&p)?;
                    parsed.unavailable_posts.push(p);
                }
            }
            Ok(())
        })();
        if let Err(code) = result {
            parsed.issues.push(Issue::item(code, value));
        }
    }
    Ok(parsed)
}
/// Immutable metadata-only round catalog, shared across peers by the account
/// coordinator. Construct once per known round, not once per page or heartbeat.
#[derive(Clone, Default)]
pub(crate) struct Catalog {
    deletions: BTreeMap<String, Tombstone>,
    threads: BTreeMap<String, ThreadRecordItem>,
    versions: BTreeMap<VersionCursor, PostVersion>,
    unavailable_posts: BTreeMap<(String, String), UnavailablePost>,
}
impl Serialize for Catalog {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeSeq, SerializeStruct};
        struct Values<'a, K, V>(&'a BTreeMap<K, V>);
        impl<K, V: Serialize> Serialize for Values<'_, K, V> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for value in self.0.values() { seq.serialize_element(value)?; }
                seq.end()
            }
        }
        let mut out = serializer.serialize_struct("Catalog", 4)?;
        out.serialize_field("tombstones", &Values(&self.deletions))?;
        out.serialize_field("threads", &Values(&self.threads))?;
        out.serialize_field("versions", &Values(&self.versions))?;
        out.serialize_field("unavailable", &Values(&self.unavailable_posts))?;
        out.end()
    }
}
impl Catalog {
    /// A delta only omits equal facts. Absence in either snapshot is never a
    /// deletion; tombstones remain the sole deletion authority. Include thread
    /// records needed by changed versions/notices even when their record is old.
    pub(crate) fn since(&self, previous: &Self) -> Self {
        let mut delta = Self {
            deletions: self.deletions.iter().filter(|(k, v)| previous.deletions.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone())).collect(),
            threads: self.threads.iter().filter(|(k, v)| previous.threads.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone())).collect(),
            versions: self.versions.iter().filter(|(k, v)| previous.versions.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone())).collect(),
            unavailable_posts: self.unavailable_posts.iter()
                .filter(|(k, v)| previous.unavailable_posts.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        for thread in delta.versions.values().map(|p| &p.thread_id)
            .chain(delta.unavailable_posts.values().map(|p| &p.thread_id))
        {
            if let Some(record) = self.threads.get(thread) {
                delta.threads.insert(thread.clone(), record.clone());
            }
        }
        delta
    }

    /// Stable public revision for a catalog snapshot.  It is derived only from
    /// ordered content facts; activity timestamps and online state are absent.
    pub(crate) fn revision(&self) -> String {
        self.revision_with_roster(None)
    }

    pub(crate) fn revision_with_roster(&self, roster_version: Option<&str>) -> String {
        let value = serde_json::json!({
            "tombstones": self.deletions.values().collect::<Vec<_>>(),
            "threads": self.threads.values().collect::<Vec<_>>(),
            "versions": self.versions.values().collect::<Vec<_>>(),
            "unavailable": self.unavailable_posts.values().collect::<Vec<_>>(),
            "rosterVersion": roster_version,
        });
        raw_sha256(&serde_json::to_vec(&value).expect("catalog revision is serializable"))
    }

    pub(crate) fn post_versions(&self) -> impl Iterator<Item = &PostVersion> {
        self.versions.values()
    }

    pub(crate) fn from_state(state: &SyncState, group: &str) -> Result<Self, String> {
        safe_id(group)?;
        let Some(group) = state.groups.get(group) else {
            return Ok(Self::default());
        };
        let shared = &group.shared_threads;
        let mut catalog = Self::default();
        for (key, d) in &state.tombstones {
            if shared.contains(&d.thread_id) {
                catalog.deletions.insert(key.clone(), d.clone());
            }
        }
        for (id, t) in &state.thread_records {
            if shared.contains(id) && !state.tombstones.contains_key(&format!("thread:{id}")) {
                catalog.threads.insert(id.clone(), t.clone());
            }
        }
        for row in state.ledger.values() {
            if let Some(post) = row.descriptor.as_ref().filter(|_| {
                row.body_stamp.is_some()
                    && shared.contains(&row.thread_id)
                    && !suppressed(state, &row.thread_id, &row.post_id, &row.version_id)
            }) {
                catalog
                    .versions
                    .insert(VersionCursor::of(post), post.clone());
            }
        }
        for p in state.local_unavailable.values() {
            if shared.contains(&p.thread_id)
                && catalog.threads.contains_key(&p.thread_id)
                && !logical_deleted(state, &p.thread_id, &p.post_id)
            {
                catalog
                    .unavailable_posts
                    .insert((p.thread_id.clone(), p.post_id.clone()), p.clone());
            }
        }
        Ok(catalog)
    }
    pub(crate) fn page(
        &self,
        phase: ManifestPhase,
        after: Option<&Value>,
    ) -> Result<(Manifest, Vec<Issue>), String> {
        self.page_with_budget(phase, after, MAX_PAGE_BYTES)
    }
    pub(crate) fn page_with_budget(
        &self,
        phase: ManifestPhase,
        after: Option<&Value>,
        budget: usize,
    ) -> Result<(Manifest, Vec<Issue>), String> {
        if !(8192..=MAX_PAGE_BYTES).contains(&budget) {
            return Err("invalid_page_budget".into());
        }
        let after = after.map(|v| cursor(phase, v)).transpose()?;
        let mut page = Manifest {
            phase,
            items: Vec::new(),
            next: None,
        };
        let mut issues = Vec::new();
        let mut last = None;
        let mut visited = 0;
        let iter: Box<dyn Iterator<Item = (Cursor, Result<Value, serde_json::Error>)> + '_> =
            match phase {
                ManifestPhase::Tombstones => Box::new(
                    self.deletions
                        .iter()
                        .map(|(k, v)| (Cursor::Delete(k.clone()), serde_json::to_value(v))),
                ),
                ManifestPhase::Threads => Box::new(
                    self.threads
                        .iter()
                        .map(|(k, v)| (Cursor::Thread(k.clone()), serde_json::to_value(v))),
                ),
                ManifestPhase::Versions => Box::new(
                    self.versions
                        .iter()
                        .map(|(k, v)| (Cursor::Version(k.clone()), serde_json::to_value(v))),
                ),
                ManifestPhase::Unavailable => Box::new(
                    self.unavailable_posts
                        .iter()
                        .map(|(k, v)| (Cursor::Unavailable(k.clone()), serde_json::to_value(v))),
                ),
            };
        for (key, value) in iter {
            if after.as_ref().is_some_and(|a| &key <= a) {
                continue;
            }
            if visited == MAX_PAGE_ITEMS {
                page.next = last.as_ref().map(cursor_value);
                return Ok((page, issues));
            }
            let value = value.map_err(|_| "invalid_manifest")?;
            // Reuse incoming validation; an oversized local item is an explicit
            // local round failure, never truncated or allowed to stall pagination.
            let one = Manifest {
                phase,
                items: vec![value.clone()],
                next: None,
            };
            let limit = if phase == ManifestPhase::Versions {
                MAX_VERSION_BYTES
            } else {
                MAX_THREAD_BYTES
            };
            let item_error = if encoded_len(&value)? > limit {
                Some("manifest_item_too_large".into())
            } else {
                parse_page(&one, phase, None)?
                    .issues
                    .into_iter()
                    .next()
                    .map(|i| i.code)
            };
            if let Some(code) = item_error {
                issues.push(Issue::item(code, &value));
                last = Some(key);
                visited += 1;
                continue;
            }
            let mut candidate = page.clone();
            candidate.items.push(value);
            candidate.next = Some(cursor_value(&key));
            if encoded_len(&candidate)? > budget {
                page.next = Some(cursor_value(
                    last.as_ref().ok_or("manifest_item_too_large")?,
                ));
                return Ok((page, issues));
            }
            page = candidate;
            last = Some(key);
            visited += 1;
        }
        page.next = None;
        Ok((page, issues))
    }
}
pub(crate) fn suppressed(state: &SyncState, thread: &str, post: &str, version: &str) -> bool {
    logical_deleted(state, thread, post)
        || state
            .tombstones
            .contains_key(&format!("version:{thread}/{post}/{version}"))
}
pub(crate) fn logical_deleted(state: &SyncState, thread: &str, post: &str) -> bool {
    state.tombstones.contains_key(&format!("thread:{thread}"))
        || state
            .tombstones
            .contains_key(&format!("post:{thread}/{post}"))
}
pub(crate) fn covers(d: &Tombstone, thread: &str, post: &str, version: &str) -> bool {
    d.thread_id == thread
        && d.post_id.as_deref().is_none_or(|p| p == post)
        && d.version_id.as_deref().is_none_or(|v| v == version)
}
#[derive(Debug, Default)]
pub(crate) struct Plan {
    pub(crate) shared_roots: BTreeSet<String>,
    pub(crate) deletions: Vec<Tombstone>,
    pub(crate) threads: Vec<ThreadRecordItem>,
    pub(crate) fetch: Vec<Resource>,
    pub(crate) versions: Vec<PostVersion>,
    pub(crate) unavailable_posts: Vec<UnavailablePost>,
    pub(crate) unavailable: Vec<Resource>,
    pub(crate) issues: Vec<Issue>,
    pub(crate) warnings: Vec<ThreadMismatch>,
}
#[derive(Debug, Clone)]
pub(crate) struct ThreadMismatch {
    pub(crate) thread_id: String,
    pub(crate) local_sha256: String,
    pub(crate) remote_sha256: String,
}
pub(crate) fn post_resource(p: &PostVersion) -> Resource {
    Resource {
        identity: ResourceKind::Post {
            thread_id: p.thread_id.clone(),
            post_id: p.post_id.clone(),
            version_id: p.version_id.clone(),
        },
        sha256: p.version_id.clone(),
        size_bytes: p.size_bytes,
    }
}
pub(crate) fn attachment_resource(p: &PostVersion, a: &super::AttachmentRef) -> Resource {
    Resource {
        identity: ResourceKind::Attachment {
            thread_id: p.thread_id.clone(),
            post_id: p.post_id.clone(),
            version_id: p.version_id.clone(),
            attachment_id: a.id.clone(),
        },
        sha256: a.sha256.clone(),
        size_bytes: a.size_bytes,
    }
}
pub(crate) fn avatar_resource(a: &AvatarSidecar) -> Option<Resource> {
    if a.kind != "image" {
        return None;
    }
    Some(Resource {
        identity: ResourceKind::Avatar {
            sha256: a.sha256.clone()?,
            ext: a.ext.clone()?,
        },
        sha256: a.sha256.clone()?,
        size_bytes: a.size_bytes?,
    })
}
/// Membership and phase progression are supplied by the authenticated session.
/// A repository first indexes any newly announced local roots; no wall clock or
/// disk-presence heuristic grants shared membership.
pub(crate) fn plan(local: &SyncState, parsed: ParsedPage) -> Result<Plan, String> {
    let mut plan = Plan {
        issues: parsed.issues,
        ..Default::default()
    };
    for d in parsed.deletions {
        plan.shared_roots.insert(d.thread_id.clone());
        if !local.tombstones.contains_key(&d.key()?) {
            plan.deletions.push(d);
        }
    }
    for t in parsed.threads {
        plan.shared_roots.insert(t.record.thread_id.clone());
        match local.thread_records.get(&t.record.thread_id) {
            None => plan.threads.push(t),
            Some(old) if old.sha256 != t.sha256 => plan.warnings.push(ThreadMismatch {
                thread_id: t.record.thread_id,
                local_sha256: old.sha256.clone(),
                remote_sha256: t.sha256,
            }),
            _ => {}
        }
    }
    for p in parsed.unavailable_posts {
        if logical_deleted(local, &p.thread_id, &p.post_id) {
            continue;
        }
        if !local.thread_records.contains_key(&p.thread_id) {
            plan.issues.push(Issue {
                code: "missing_thread_record".into(),
                thread_id: Some(p.thread_id),
                post_id: Some(p.post_id),
            });
            continue;
        }
        // A notice neither enrolls a root nor establishes a body relationship.
        plan.unavailable_posts.push(p);
    }
    let mut known = BTreeMap::new();
    let mut roots = BTreeMap::new();
    for row in local.ledger.values() {
        if let Some(p) = &row.descriptor {
            known.insert(p.post_id.clone(), (p.thread_id.clone(), p.kind.clone()));
            if p.kind == "topic" {
                roots.insert(p.thread_id.clone(), p.post_id.clone());
            }
        }
    }
    let mut avatar_ids = BTreeSet::new();
    let mut valid_posts = Vec::new();
    for p in parsed.posts {
        let relation = (p.thread_id.clone(), p.kind.clone());
        if known.get(&p.post_id).is_some_and(|old| old != &relation)
            || (p.kind == "topic" && roots.get(&p.thread_id).is_some_and(|id| id != &p.post_id))
        {
            plan.issues.push(Issue {
                code: "invalid_post_relationship".into(),
                thread_id: Some(p.thread_id),
                post_id: Some(p.post_id),
            });
            continue;
        }
        known.insert(p.post_id.clone(), relation);
        if p.kind == "topic" {
            roots.insert(p.thread_id.clone(), p.post_id.clone());
        }
        plan.shared_roots.insert(p.thread_id.clone());
        if suppressed(local, &p.thread_id, &p.post_id, &p.version_id)
            || plan
                .deletions
                .iter()
                .any(|d| covers(d, &p.thread_id, &p.post_id, &p.version_id))
        {
            continue;
        }
        let existing = local
            .ledger
            .get(&super::LedgerEntry::version_key(&p.post_id, &p.version_id)?)
            .filter(|row| row.body_stamp.is_some());
        if !existing.is_some_and(|row| row.body_stamp.is_some()) {
            plan.fetch.push(post_resource(&p));
        }
        for a in &p.attachments {
            let verified = existing
                .and_then(|row| row.attachments.get(&a.id))
                .is_some_and(|s| {
                    s.verified
                        && s.stamp.is_some()
                        && s.sha256 == a.sha256
                        && s.size_bytes == a.size_bytes
                });
            if !verified {
                let resource = attachment_resource(&p, a);
                if a.available {
                    plan.fetch.push(resource);
                } else {
                    plan.unavailable.push(resource);
                }
            }
        }
        if let Some(r) = p.avatar.as_ref().and_then(avatar_resource) {
            let verified = local.ledger.values().any(|row| {
                row.avatar_stamp.is_some()
                    && row
                        .descriptor
                        .as_ref()
                        .and_then(|p| p.avatar.as_ref())
                        .and_then(avatar_resource)
                        .as_ref()
                        == Some(&r)
            });
            if !verified && avatar_ids.insert((r.sha256.clone(), r.size_bytes)) {
                plan.fetch.push(r);
            }
        }
        valid_posts.push(p);
    }
    plan.fetch.sort_by_key(|r| match &r.identity {
        ResourceKind::Post { post_id, .. } => (
            if valid_posts
                .iter()
                .any(|p| &p.post_id == post_id && p.kind == "topic")
            {
                0
            } else {
                1
            },
            post_id.clone(),
        ),
        ResourceKind::Attachment { attachment_id, .. } => (2, attachment_id.clone()),
        ResourceKind::Avatar { sha256, .. } => (3, sha256.clone()),
    });
    plan.versions = valid_posts;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbs_sync::{AttachmentRef, FileStamp, GroupState, LedgerEntry};
    const NOW: &str = "2026-09-12T12:00:00Z";
    #[test]
    fn reader_requires_complete_deletions_before_threads_and_versions() {
        let mut reader = ManifestReader::default();
        let page = |phase, next| Manifest {
            phase,
            items: vec![],
            next,
        };
        assert!(reader.read(&page(ManifestPhase::Versions, None)).is_err());
        reader
            .read(&page(
                ManifestPhase::Tombstones,
                Some(Value::String("thread:one".into())),
            ))
            .unwrap();
        assert!(reader.read(&page(ManifestPhase::Threads, None)).is_err());
        reader.read(&page(ManifestPhase::Tombstones, None)).unwrap();
        reader.read(&page(ManifestPhase::Threads, None)).unwrap();
        assert!(!reader.is_complete());
        reader.read(&page(ManifestPhase::Versions, None)).unwrap();
        assert!(!reader.is_complete());
        reader
            .read(&page(ManifestPhase::Unavailable, None))
            .unwrap();
        assert!(reader.is_complete());
        assert!(reader.read(&page(ManifestPhase::Versions, None)).is_err());
    }
    fn post(id: &str, raw: &[u8]) -> PostVersion {
        PostVersion {
            thread_id: "thread-one".into(),
            post_id: id.into(),
            version_id: raw_sha256(raw),
            size_bytes: raw.len() as u64,
            kind: if id == "root" { "topic" } else { "reply" }.into(),
            attachments: vec![],
            avatar: Some(AvatarSidecar::none()),
        }
    }
    fn record() -> ThreadRecordItem {
        thread_item(ThreadRecord {
            schema: "kota.bbs.thread.v1".into(),
            thread_id: "thread-one".into(),
            status: "open".into(),
            visibility: "targeted".into(),
            project_tags: vec!["project".into()],
            created_by_project: "project".into(),
            created_by_agent: "author".into(),
            created_at: NOW.into(),
        })
        .unwrap()
    }
    fn remember(state: &mut SyncState, p: PostVersion) {
        let row = LedgerEntry {
            thread_id: p.thread_id.clone(),
            post_id: p.post_id.clone(),
            version_id: p.version_id.clone(),
            body_stamp: Some(FileStamp {
                len: p.size_bytes,
                modified_ns: 1,
                changed_ns: 1,
                dev: 1,
                ino: 1,
            }),
            descriptor: Some(p),
            ..Default::default()
        };
        state.ledger.insert(row.key().unwrap(), row);
    }
    fn shared(state: &mut SyncState, group: &str, share: bool) {
        state.groups.insert(
            group.into(),
            GroupState {
                shared_threads: if share {
                    BTreeSet::from(["thread-one".into()])
                } else {
                    BTreeSet::new()
                },
                ..Default::default()
            },
        );
    }
    fn parsed_posts(mut posts: Vec<PostVersion>) -> ParsedPage {
        posts.sort_by_key(VersionCursor::of);
        parse_page(
            &Manifest {
                phase: ManifestPhase::Versions,
                items: posts
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap())
                    .collect(),
                next: None,
            },
            ManifestPhase::Versions,
            None,
        )
        .unwrap()
    }
    #[test]
    #[ignore = "explicit existing-history manifest budget measurement"]
    fn history_manifest_bytes_and_page_counts() {
        let mut rows = Vec::new();
        for count in [0, 1000, 10000] {
            let mut state = SyncState::new();
            shared(&mut state, "group", true);
            state.thread_records.insert("thread-one".into(), record());
            for i in 0..count {
                let id = format!("post-{i:08}");
                remember(&mut state, post(&id, id.as_bytes()));
            }
            let catalog = Catalog::from_state(&state, "group").unwrap();
            let mut pages = 0;
            let mut bytes = 0;
            let mut reader = ManifestReader::default();
            for phase in [
                ManifestPhase::Tombstones,
                ManifestPhase::Threads,
                ManifestPhase::Versions,
                ManifestPhase::Unavailable,
            ] {
                let mut after = None;
                loop {
                    // Same 12,000-byte payload allowance as exchange::PAGE_BUDGET.
                    let (page, issues) = catalog
                        .page_with_budget(phase, after.as_ref(), 12_000)
                        .unwrap();
                    assert!(issues.is_empty());
                    reader.read(&page).unwrap();
                    pages += 1;
                    bytes += serde_json::to_vec(&page).unwrap().len();
                    after = page.next;
                    if after.is_none() {
                        break;
                    }
                }
            }
            assert!(reader.is_complete());
            rows.push(serde_json::json!({
                "existingVersions": count,
                "oneDirectionPages": pages,
                "oneDirectionManifestBytes": bytes,
            }));
        }
        println!(
            "BBS_RELAY_HISTORY_COUNTS {}",
            serde_json::to_string(&rows).unwrap()
        );
    }
    #[test]
    fn delta_retains_explicit_deletions_dependencies_and_changed_resource_availability() {
        let mut state = SyncState::new();
        shared(&mut state, "group", true);
        state.thread_records.insert("thread-one".into(), record());
        let unchanged = post("unchanged", b"old");
        let mut changed = post("changed", b"body");
        changed.attachments.push(super::super::AttachmentRef { id: "file".into(),
            sha256: raw_sha256(b"file"), size_bytes: 4, ext: "txt".into(), available: false });
        remember(&mut state, unchanged.clone()); remember(&mut state, changed.clone());
        let notice = UnavailablePost { thread_id: "thread-one".into(), post_id: "oversized".into(),
            reason: super::super::UnavailableReason::TooLargeToSync, kind: None };
        state.local_unavailable.insert(notice.key().unwrap(), notice.clone());
        let before = Catalog::from_state(&state, "group").unwrap();
        changed.attachments[0].available = true; remember(&mut state, changed.clone());
        let new = post("added", b"new"); remember(&mut state, new.clone());
        let mut notice = notice; notice.kind = Some("reply".into());
        state.local_unavailable.insert(notice.key().unwrap(), notice.clone());
        let deletion = Tombstone { thread_id: "thread-one".into(), post_id: Some("removed".into()),
            version_id: None, deleted_at: "2026-09-16T00:00:00Z".into() };
        state.tombstones.insert(deletion.key().unwrap(), deletion.clone());
        let delta = Catalog::from_state(&state, "group").unwrap().since(&before);
        assert_eq!(delta.versions.len(),2);
        assert_eq!(delta.versions[&VersionCursor::of(&changed)],changed);
        assert_eq!(delta.versions[&VersionCursor::of(&new)],new);
        assert_eq!(delta.threads.len(),1); // relation available before version/notice pages
        assert_eq!(delta.unavailable_posts.values().next(),Some(&notice));
        assert_eq!(delta.deletions.values().next(),Some(&deletion));
        let absent = Catalog::default().since(&before);
        assert!(absent.deletions.is_empty()); // physical unlink is never synthesized into a tombstone
        assert!(absent.versions.is_empty());
        assert!(before.since(&before).versions.is_empty());
    }

    #[test]
    fn fifty_dispersed_deltas_do_not_repeat_existing_history_pages() {
        fn measure(catalog: &Catalog) -> (usize, usize, usize) {
            let mut result=(0,0,0); let mut reader=ManifestReader::default();
            for phase in [ManifestPhase::Tombstones, ManifestPhase::Threads,
                ManifestPhase::Versions, ManifestPhase::Unavailable]
            {
                let mut after=None;
                loop {
                    let (page,issues)=catalog.page_with_budget(phase,after.as_ref(),12_000).unwrap();
                    assert!(issues.is_empty());reader.read(&page).unwrap();
                    result.0+=1;result.1+=serde_json::to_vec(&page).unwrap().len();
                    if phase==ManifestPhase::Versions { result.2+=page.items.len(); }
                    after=page.next;if after.is_none(){break;}
                }
            }
            assert!(reader.is_complete());result
        }
        let mut rows=Vec::new();
        for history in [1000,10_000] {
            let mut state=SyncState::new();shared(&mut state,"group",true);
            state.thread_records.insert("thread-one".into(),record());
            for i in 0..history {let id=format!("post-{i:08}");remember(&mut state,post(&id,id.as_bytes()));}
            let mut confirmed=Catalog::from_state(&state,"group").unwrap();
            let full=measure(&confirmed);let mut delta_pages=0;let mut delta_bytes=0;
            for i in 0..50 {
                let id=format!("new-{i:08}");remember(&mut state,post(&id,id.as_bytes()));
                let current=Catalog::from_state(&state,"group").unwrap();
                let delta=current.since(&confirmed);let counts=measure(&delta);
                assert_eq!(counts.0,4);assert_eq!(counts.2,1);
                assert_eq!(delta.threads.len(),1);
                delta_pages+=counts.0;delta_bytes+=counts.1;confirmed=current;
            }
            rows.push(serde_json::json!({"history":history,"fullPages":full.0,"fullBytes":full.1,
                "dispersedChanges":50,"deltaPages":delta_pages,"deltaBytes":delta_bytes,
                "dailyFullPagesPlusFiftyDeltas":full.0+delta_pages}));
        }
        println!("BBS_DELTA_HISTORY_COUNTS {}",serde_json::to_string(&rows).unwrap());
    }
    #[test]
    fn historical_scope_is_not_backfilled_and_newly_announced_local_delete_only_sends_marker() {
        let mut state = SyncState::new();
        shared(&mut state, "a", true);
        shared(&mut state, "b", false);
        let p = post("root", b"history");
        remember(&mut state, p.clone());
        let d = Tombstone {
            thread_id: p.thread_id.clone(),
            deleted_at: NOW.into(),
            ..Default::default()
        };
        state.tombstones.insert(d.key().unwrap(), d);
        let c = Catalog::from_state(&state, "b").unwrap();
        for phase in [
            ManifestPhase::Tombstones,
            ManifestPhase::Threads,
            ManifestPhase::Versions,
        ] {
            assert!(c.page(phase, None).unwrap().0.items.is_empty());
        }
        let plan = plan(&state, parsed_posts(vec![p])).unwrap();
        assert!(plan.fetch.is_empty());
        assert_eq!(plan.shared_roots, BTreeSet::from(["thread-one".into()]));
        state.groups.get_mut("b").unwrap().shared_threads = plan.shared_roots;
        let c = Catalog::from_state(&state, "b").unwrap();
        assert_eq!(
            c.page(ManifestPhase::Tombstones, None)
                .unwrap()
                .0
                .items
                .len(),
            1
        );
        assert!(c
            .page(ManifestPhase::Versions, None)
            .unwrap()
            .0
            .items
            .is_empty());
    }
    #[test]
    fn same_bytes_reuse_changed_bytes_fork_and_different_reply_ids_remain_distinct() {
        let mut local = SyncState::new();
        let root = post("root", b"old");
        remember(&mut local, root.clone());
        let fork = post("root", b"new");
        let reply = post("reply-one", b"reply");
        let result = plan(
            &local,
            parsed_posts(vec![root, fork.clone(), reply.clone()]),
        )
        .unwrap();
        assert_eq!(
            result.fetch,
            vec![post_resource(&fork), post_resource(&reply)]
        );
        assert!(result.issues.is_empty());
        let mut bad = post("root", b"changed role");
        bad.kind = "reply".into();
        let result = plan(&local, parsed_posts(vec![bad])).unwrap();
        assert!(result.fetch.is_empty());
        assert_eq!(result.issues[0].code, "invalid_post_relationship");
        let mut second = post("another-root", b"second root");
        second.kind = "topic".into();
        let result = plan(&local, parsed_posts(vec![second])).unwrap();
        assert!(result.fetch.is_empty());
        assert_eq!(result.issues[0].code, "invalid_post_relationship");
    }
    #[test]
    fn exact_version_deletion_does_not_suppress_another_fork_but_parent_does() {
        let p = post("root", b"one");
        let mut fork = post("root", b"two");
        fork.attachments.push(AttachmentRef {
            id: "file".into(),
            sha256: raw_sha256(b"a"),
            size_bytes: 1,
            ext: "bin".into(),
            available: true,
        });
        let mut state = SyncState::new();
        let d = Tombstone {
            thread_id: p.thread_id.clone(),
            post_id: Some(p.post_id.clone()),
            version_id: Some(p.version_id.clone()),
            deleted_at: NOW.into(),
        };
        state.tombstones.insert(d.key().unwrap(), d);
        let result = plan(&state, parsed_posts(vec![p.clone(), fork.clone()])).unwrap();
        assert_eq!(result.fetch.len(), 2);
        assert_eq!(result.fetch[0], post_resource(&fork));
        let d = Tombstone {
            thread_id: p.thread_id.clone(),
            deleted_at: NOW.into(),
            ..Default::default()
        };
        state.tombstones.insert(d.key().unwrap(), d);
        assert!(plan(&state, parsed_posts(vec![p, fork]))
            .unwrap()
            .fetch
            .is_empty());
    }
    #[test]
    fn thread_creation_mismatch_is_warning_and_does_not_swallow_a_body_fork() {
        let mut state = SyncState::new();
        let old = record();
        state
            .thread_records
            .insert(old.record.thread_id.clone(), old.clone());
        remember(&mut state, post("root", b"old"));
        let mut changed = old.record.clone();
        changed.visibility = "broadcast".into();
        let changed = thread_item(changed).unwrap();
        let plan = plan(
            &state,
            ParsedPage {
                threads: vec![changed.clone()],
                posts: vec![post("root", b"new")],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].local_sha256, old.sha256);
        assert_eq!(plan.warnings[0].remote_sha256, changed.sha256);
        assert!(plan.threads.is_empty());
        assert_eq!(plan.fetch.len(), 1);
        assert!(plan.issues.is_empty());
        let mut broken = changed;
        broken.sha256 = raw_sha256(b"wrong");
        assert!(validate_thread(&broken).is_err());
    }
    #[test]
    fn all_four_phase_cursors_cover_boundary_pages_empty_phases_and_tombstones_only() {
        let mut state = SyncState::new();
        shared(&mut state, "g", true);
        state.thread_records.insert("thread-one".into(), record());
        for n in 0..128 {
            remember(&mut state, post(&format!("reply-{n:03}"), b"body"));
            let d = Tombstone {
                thread_id: "thread-one".into(),
                post_id: Some(format!("deleted-{n:03}")),
                deleted_at: NOW.into(),
                ..Default::default()
            };
            state.tombstones.insert(d.key().unwrap(), d);
            let unavailable = UnavailablePost {
                thread_id: "thread-one".into(),
                post_id: format!("missing-{n:03}"),
                reason: super::super::UnavailableReason::TooLargeToSync,
                kind: None,
            };
            state
                .local_unavailable
                .insert(unavailable.key().unwrap(), unavailable);
        }
        let catalog = Catalog::from_state(&state, "g").unwrap();
        for (phase, count) in [
            (ManifestPhase::Tombstones, 128),
            (ManifestPhase::Threads, 1),
            (ManifestPhase::Versions, 128),
            (ManifestPhase::Unavailable, 128),
        ] {
            let mut after = None;
            let mut ids = BTreeSet::new();
            let mut rounds = 0;
            loop {
                let (page, issues) = catalog.page(phase, after.as_ref()).unwrap();
                assert!(issues.is_empty());
                assert!(encoded_len(&page).unwrap() <= MAX_PAGE_BYTES);
                parse_page(&page, phase, after.as_ref()).unwrap();
                for item in &page.items {
                    assert!(ids.insert(format!("{:?}", item_cursor(phase, item).unwrap())));
                }
                after = page.next;
                rounds += 1;
                assert!(rounds < 30);
                if after.is_none() {
                    break;
                }
            }
            assert_eq!(ids.len(), count);
        }
        state.ledger.clear();
        state.thread_records.clear();
        let catalog = Catalog::from_state(&state, "g").unwrap();
        for phase in [
            ManifestPhase::Threads,
            ManifestPhase::Versions,
            ManifestPhase::Unavailable,
        ] {
            let (p, _) = catalog.page(phase, None).unwrap();
            assert!(p.items.is_empty());
            assert!(p.next.is_none());
        }
        let bad = Manifest {
            phase: ManifestPhase::Threads,
            items: vec![],
            next: Some(Value::String("same".into())),
        };
        assert!(parse_page(
            &bad,
            ManifestPhase::Threads,
            Some(&Value::String("same".into()))
        )
        .is_err());
    }
    #[test]
    fn unavailable_wire_rejects_bad_fields_and_cursor_replays_without_hiding_neighbors() {
        let item = |id: &str| {
            serde_json::json!({
                "thread_id":"thread-one", "post_id":id, "reason":"too_large_to_sync"
            })
        };
        let mut bad = item("b");
        bad["kind"] = Value::String("unknown".into());
        let mut invented = item("c");
        invented["version_id"] = Value::String("a".repeat(64));
        let mut traversal = item("../outside");
        traversal["reason"] = Value::String("arbitrary_text".into());
        let page = Manifest {
            phase: ManifestPhase::Unavailable,
            items: vec![item("a"), bad, invented, traversal, item("z")],
            next: Some(serde_json::json!(["thread-one", "z"])),
        };
        let parsed = parse_page(&page, ManifestPhase::Unavailable, None).unwrap();
        assert_eq!(parsed.issues.len(), 3);
        assert_eq!(
            parsed
                .unavailable_posts
                .iter()
                .map(|p| p.post_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert!(parsed.posts.is_empty());
        assert!(parse_page(&page, ManifestPhase::Unavailable, page.next.as_ref()).is_err());
        let mut reversed = page.clone();
        reversed.items = vec![item("z"), item("a")];
        assert!(parse_page(&reversed, ManifestPhase::Unavailable, None).is_err());
        let mut fake_hash_cursor = page;
        fake_hash_cursor.next = Some(serde_json::json!(["thread-one", "z", "a".repeat(64)]));
        assert!(parse_page(&fake_hash_cursor, ManifestPhase::Unavailable, None).is_err());
    }
    #[test]
    fn oversized_item_is_reported_without_hiding_valid_neighbors_or_looping() {
        let good = post("a", b"body");
        let mut bad = post("b", b"body");
        bad.avatar = Some(AvatarSidecar {
            kind: "builtin".into(),
            id: Some("x".repeat(5000)),
            ..AvatarSidecar::none()
        });
        let last = post("c", b"body");
        let p = Manifest {
            phase: ManifestPhase::Versions,
            items: [good.clone(), bad.clone(), last.clone()]
                .iter()
                .map(|v| serde_json::to_value(v).unwrap())
                .collect(),
            next: None,
        };
        let parsed = parse_page(&p, ManifestPhase::Versions, None).unwrap();
        assert_eq!(parsed.posts, vec![good.clone(), last.clone()]);
        assert_eq!(parsed.issues.len(), 1);
        let mut state = SyncState::new();
        shared(&mut state, "g", true);
        for p in [good, bad, last] {
            remember(&mut state, p);
        }
        let c = Catalog::from_state(&state, "g").unwrap();
        let (p, issues) = c.page(ManifestPhase::Versions, None).unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(issues.len(), 1);
        assert!(p.next.is_none());
        let mut large = record();
        large.record.project_tags = (0..32)
            .map(|n| format!("tag-{n:02}-{}", "x".repeat(54)))
            .collect();
        large.sha256 = raw_sha256(&thread_bytes(&large.record).unwrap());
        assert!(validate_thread(&large).is_err());
    }

    #[test]
    fn catalog_revision_is_stable_and_changes_only_for_content_facts() {
        let mut state = SyncState::new();
        shared(&mut state, "g", true);
        let first = Catalog::from_state(&state, "g").unwrap().revision();
        assert!(valid_hash(&first).is_ok());
        let same = Catalog::from_state(&state, "g").unwrap().revision();
        assert_eq!(first, same);
        let roster_changed = Catalog::from_state(&state, "g")
            .unwrap()
            .revision_with_roster(Some(&"b".repeat(64)));
        assert_ne!(first, roster_changed);
        assert!(valid_hash(&roster_changed).is_ok());

        remember(&mut state, post("a", b"body"));
        let changed = Catalog::from_state(&state, "g").unwrap().revision();
        assert_ne!(first, changed);
        assert!(valid_hash(&changed).is_ok());
    }
}
