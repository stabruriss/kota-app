pub(crate) mod sync;
pub mod mentions;
pub(crate) mod notify;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

const KOTA_HOME_DIR: &str = "Kota";
const KOTA_WORKSPACES_DIR: &str = "Workspaces";
const BBS_DIR: &str = "bbs";
const THREAD_SCHEMA: &str = "kota.bbs.thread.v1";
const POST_SCHEMA: &str = "kota.bbs.post.v1";
pub const MAX_ATTACHMENT_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_POST_ATTACHMENT_BYTES: u64 = MAX_ATTACHMENT_BYTES;
pub const MAX_ATTACHMENTS: usize = 9;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsAttachmentSource {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BbsAttachment {
    id: String,
    name: String,
    path: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsAttachmentView {
    #[serde(flatten)]
    attachment: BbsAttachment,
    local_path: String,
    available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsAttachmentValidation {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsProjectRequest {
    pub project_id: String,
    #[serde(default)]
    pub project_display_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsHumanPostRequest {
    pub project_id: String,
    #[serde(default)]
    pub project_display_name: Option<String>,
    #[serde(default)]
    pub project_tags: Vec<String>,
    pub body: String,
    #[serde(default)]
    pub attachments: Vec<BbsAttachmentSource>,
    #[serde(default)]
    pub mentions: Vec<mentions::Mention>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsHumanReplyRequest {
    pub project_id: String,
    #[serde(default)]
    pub project_display_name: Option<String>,
    pub thread_id: String,
    pub body: String,
    #[serde(default)]
    pub attachments: Vec<BbsAttachmentSource>,
    #[serde(default)]
    pub mentions: Vec<mentions::Mention>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsPostStateRequest {
    pub project_id: String,
    pub post_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsDeleteRequest {
    pub thread_id: String,
    #[serde(default)]
    pub post_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsSnapshot {
    pub project_id: String,
    pub project_display_name: String,
    pub root: String,
    pub new_count: usize,
    pub threads: Vec<BbsThreadView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsThreadView {
    pub thread_id: String,
    pub visibility: String,
    pub project_tags: Vec<String>,
    pub project_tag_labels: Vec<String>,
    pub created_by_project: String,
    pub created_by_project_label: String,
    pub updated_at: String,
    pub latest_post_id: String,
    pub is_new: bool,
    pub relevant: bool,
    pub sharing_group_id: Option<String>,
    pub posts: Vec<BbsPostView>,
    pub unavailable_posts: Vec<sync::UnavailableView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsPostView {
    pub post_id: String,
    pub version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_avatar: Option<sync::AvatarView>,
    pub thread_id: String,
    pub project_id: String,
    pub project_display_name: String,
    pub agent_id: String,
    pub agent_display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_avatar: Option<String>,
    pub created_at: String,
    pub kind: String,
    pub body: String,
    pub attachments: Vec<BbsAttachmentView>,
    pub preview: String,
    pub state: String,
    pub external: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BbsThreadMeta {
    schema: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    status: String,
    visibility: String,
    #[serde(rename = "projectTags", default)]
    project_tags: Vec<String>,
    #[serde(rename = "createdByProject")]
    created_by_project: String,
    #[serde(rename = "createdByAgent")]
    created_by_agent: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(rename = "latestPostId")]
    latest_post_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BbsPostMeta {
    schema: String,
    #[serde(rename = "postId")]
    post_id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(rename = "projectId")]
    project_id: String,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(rename = "agentDisplayName")]
    agent_display_name: String,
    #[serde(rename = "agentAvatar", default)]
    agent_avatar: Option<String>,
    #[serde(rename = "projectDisplayName")]
    project_display_name: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<BbsAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<mentions::Mention>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BbsBoardIndex {
    schema: String,
    updated_at: String,
    threads: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BbsProjectState {
    project_id: String,
    #[serde(default)]
    processed_posts: BTreeMap<String, BbsSeenPost>,
    #[serde(default)]
    ignored_posts: BTreeMap<String, BbsSeenPost>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BbsSeenPost {
    thread_id: String,
    at: String,
}

#[derive(Clone, Debug)]
struct LoadedPost {
    meta: BbsPostMeta,
    body: String,
    path: PathBuf,
    version_id: String,
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub project_id: String,
    pub project_display_name: String,
    pub agent_id: String,
    pub agent_display_name: String,
    pub agent_avatar: Option<String>,
}

/// Author identity for posts written by the human user from the BBS panel
/// (instead of an agent CLI session that carries KOTA_* env).
pub fn human_identity(project_id: &str, name: String, avatar: Option<String>) -> Identity {
    human_identity_with_project_display_name(project_id, None, name, avatar)
}

pub fn human_identity_with_project_display_name(
    project_id: &str,
    project_display_name: Option<&str>,
    name: String,
    avatar: Option<String>,
) -> Identity {
    Identity {
        project_id: project_id.to_string(),
        project_display_name: display_project_name_with_fallback(project_id, project_display_name),
        agent_id: "human".into(),
        agent_display_name: if name.trim().is_empty() {
            "User".into()
        } else {
            name.trim().to_string()
        },
        agent_avatar: avatar.filter(|value| !value.trim().is_empty()),
    }
}

pub fn account_root() -> PathBuf {
    std::env::var("KOTA_BBS_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_account_root)
}

fn default_account_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
        .join(KOTA_WORKSPACES_DIR)
        .join(BBS_DIR)
}

pub fn ensure_account_layout() -> Result<()> {
    let root = account_root();
    ensure_layout_at(&root)
}

fn ensure_layout_at(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("threads"))?;
    fs::create_dir_all(root.join("indexes").join("projects"))?;
    fs::create_dir_all(root.join("state").join("projects"))?;
    let board = root.join("board.json");
    if !board.exists() {
        write_json_pretty(
            &board,
            &BbsBoardIndex {
                schema: "kota.bbs.board.v1".into(),
                updated_at: now_iso(),
                threads: Vec::new(),
            },
        )?;
    }
    Ok(())
}

pub fn ensure_project_projection(project_memory_dir: &Path) -> Result<()> {
    ensure_account_layout()?;
    fs::create_dir_all(project_memory_dir)?;
    let link = project_memory_dir.join(BBS_DIR);
    replace_symlink_or_empty_dir(&account_root(), &link)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
}

fn attachment_name(source: &BbsAttachmentSource) -> String {
    source
        .name
        .as_deref()
        .and_then(|name| Path::new(name).file_name().and_then(|name| name.to_str()))
        .filter(|name| !name.is_empty())
        .or_else(|| {
            Path::new(&source.path)
                .file_name()
                .and_then(|name| name.to_str())
        })
        .unwrap_or("attachment")
        .chars()
        .take(240)
        .map(|ch| if ch.is_control() { '_' } else { ch })
        .collect()
}

fn attachment_extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 16 && ext.bytes().all(|ch| ch.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase)
}

fn open_attachment(source: &BbsAttachmentSource) -> Result<(File, BbsAttachmentValidation)> {
    let path = Path::new(&source.path);
    let metadata =
        fs::metadata(path).with_context(|| format!("read attachment {}", path.display()))?;
    if !metadata.is_file() {
        bail!("attachment is not a regular file: {}", path.display());
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        bail!("attachment exceeds 1 GiB: {}", path.display());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // A source replaced by a FIFO between stat and open must not block an IPC worker.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open attachment {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ATTACHMENT_BYTES {
        bail!(
            "attachment must be a regular file of at most 1 GiB: {}",
            path.display()
        );
    }
    Ok((
        file,
        BbsAttachmentValidation {
            path: source.path.clone(),
            name: attachment_name(source),
            size_bytes: metadata.len(),
        },
    ))
}

/// BBS-only preflight. No directory creation, file copy, or shared composer mutation.
pub fn validate_attachments(
    sources: &[BbsAttachmentSource],
) -> Result<Vec<BbsAttachmentValidation>> {
    if sources.len() > MAX_ATTACHMENTS {
        bail!("a BBS post supports at most {MAX_ATTACHMENTS} attachments");
    }
    let mut total = 0;
    sources
        .iter()
        .map(|source| {
            let (_, validated) = open_attachment(source)?;
            total += validated.size_bytes;
            if total > MAX_POST_ATTACHMENT_BYTES {
                bail!("BBS attachments exceed 1 GiB in total: {}", source.path);
            }
            Ok(validated)
        })
        .collect()
}

struct PreparedAttachments {
    staging: Option<PathBuf>,
    entries: Vec<BbsAttachment>,
}

impl Drop for PreparedAttachments {
    fn drop(&mut self) {
        if let Some(path) = self.staging.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

impl PreparedAttachments {
    fn prepare(root: &Path, post_id: &str, sources: &[BbsAttachmentSource]) -> Result<Self> {
        if sources.len() > MAX_ATTACHMENTS {
            bail!("a BBS post supports at most {MAX_ATTACHMENTS} attachments");
        }
        // Validate every source before copying any. Keep the opened descriptors, not a second path lookup.
        let opened = sources
            .iter()
            .map(open_attachment)
            .collect::<Result<Vec<_>>>()?;
        if opened.iter().map(|(_, item)| item.size_bytes).sum::<u64>() > MAX_POST_ATTACHMENT_BYTES {
            bail!("BBS attachments exceed 1 GiB in total");
        }
        let mut prepared = Self {
            staging: None,
            entries: Vec::new(),
        };
        if opened.is_empty() {
            return Ok(prepared);
        }
        // Outside the thread: a slow copy cannot recreate a concurrently deleted thread.
        let staging = root
            .join(".staging")
            .join(format!("{post_id}.tmp-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&staging)?;
        prepared.staging = Some(staging.clone());
        let mut total = 0u64;
        for (file, source) in opened {
            let id = format!("att-{}", Uuid::new_v4().simple());
            let filename = match attachment_extension(&source.name) {
                Some(ext) => format!("{id}.{ext}"),
                None => id.clone(),
            };
            let result = (|| -> Result<(u64, String)> {
                let before = file.metadata()?;
                let limit = MAX_ATTACHMENT_BYTES.min(MAX_POST_ATTACHMENT_BYTES - total);
                let mut reader = file.take(limit + 1);
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staging.join(&filename))?;
                let (count, sha256) = copy_attachment_stream(&mut reader, &mut output, limit)?;
                let after = reader.get_ref().metadata()?;
                if count != source.size_bytes
                    || before.len() != after.len()
                    || before.modified().ok() != after.modified().ok()
                {
                    bail!("attachment changed while copying; retry");
                }
                output.sync_all()?;
                Ok((count, sha256))
            })()
            .with_context(|| format!("could not attach {}", source.path))?;
            total += result.0;
            prepared.entries.push(BbsAttachment {
                id,
                name: source.name,
                path: format!("attachments/{post_id}/{filename}"),
                size_bytes: result.0,
                sha256: result.1,
            });
        }
        Ok(prepared)
    }

    fn install(&mut self, thread: &Path, post_id: &str) -> Result<()> {
        if let Some(staging) = &self.staging {
            fs::create_dir_all(thread.join("attachments"))?;
            fs::rename(staging, thread.join("attachments").join(post_id))?;
            self.staging = None;
        }
        Ok(())
    }
}

fn copy_attachment_stream(
    reader: &mut impl Read,
    output: &mut impl Write,
    limit: u64,
) -> Result<(u64, String)> {
    let mut reader = reader.take(limit + 1);
    let mut hasher = Sha256::new();
    let mut count = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > limit {
            bail!("attachment or post exceeds 1 GiB");
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    Ok((count, format!("{:x}", hasher.finalize())))
}

/// The stable lock inode is never unlinked. Kernel ownership ends on close/crash.
pub(crate) struct BbsWriteLock(File);

pub(crate) fn acquire_write_lock(root: &Path) -> Result<BbsWriteLock> {
    fs::create_dir_all(root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".lock"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let started = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(BbsWriteLock(file));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock
                && error.kind() != io::ErrorKind::Interrupted
            {
                return Err(error.into());
            }
            if started.elapsed() >= Duration::from_secs(5) {
                bail!("BBS is busy; retry");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        bail!("BBS attachment writes require an OS file lock on this platform")
    }
}

impl Drop for BbsWriteLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

fn attachment_views(
    root: &Path,
    thread_id: &str,
    post_id: &str,
    entries: &[BbsAttachment],
) -> Vec<BbsAttachmentView> {
    if entries.is_empty() || !safe_component(thread_id) || !safe_component(post_id) {
        return Vec::new();
    }
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let thread = root.join("threads").join(thread_id);
    let expected_base = thread.join("attachments").join(post_id);
    entries
        .iter()
        .filter_map(|entry| {
            let prefix = format!("attachments/{post_id}/");
            let filename = entry.path.strip_prefix(&prefix)?;
            if !safe_component(&entry.id)
                || !safe_component(filename)
                || !(filename == entry.id
                    || filename
                        .strip_prefix(&format!("{}.", entry.id))
                        .is_some_and(|ext| {
                            !ext.is_empty()
                                && ext.len() <= 16
                                && ext.bytes().all(|ch| ch.is_ascii_alphanumeric())
                        }))
            {
                return None;
            }
            let expected = expected_base.join(filename);
            let canonical = expected.canonicalize().ok();
            let available = canonical
                .as_ref()
                .is_some_and(|path| path.starts_with(&expected_base) && path.is_file());
            // Missing/escaped files still have a safe expected path for the error label; never read it unless available.
            Some(BbsAttachmentView {
                attachment: entry.clone(),
                local_path: if available {
                    canonical.unwrap()
                } else {
                    expected
                }
                .display()
                .to_string(),
                available,
            })
        })
        .collect()
}

pub fn install_cli_shim() -> Result<PathBuf> {
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
        .join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join("kota-bbs");
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("kota-bbs"));
            candidates.push(parent.join("../Resources/kota-bbs"));
            if let Some(triple) = current_target_triple_guess() {
                candidates.push(parent.join(format!("kota-bbs-{triple}")));
                candidates.push(parent.join(format!("../Resources/kota-bbs-{triple}")));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/kota-bbs"));
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/kota-bbs"));

    let mut script = String::from("#!/bin/sh\nset -eu\n");
    script.push_str("if [ -n \"${KOTA_BBS_BIN:-}\" ] && [ -x \"$KOTA_BBS_BIN\" ]; then exec \"$KOTA_BBS_BIN\" \"$@\"; fi\n");
    for candidate in candidates {
        script.push_str("if [ -x ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" ]; then exec ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" \"$@\"; fi\n");
    }
    script.push_str("echo 'kota-bbs binary is not installed. Build it with: cargo build --bin kota-bbs' >&2\nexit 127\n");
    fs::write(&shim, script)?;
    make_executable(&shim)?;
    Ok(shim)
}

pub fn snapshot(project_id: &str, project_display_name: Option<&str>) -> Result<BbsSnapshot> {
    ensure_account_layout()?;
    snapshot_at(
        &sync::ContentStore::account(),
        project_id,
        project_display_name,
    )
}

fn snapshot_at(
    content: &sync::ContentStore,
    project_id: &str,
    project_display_name: Option<&str>,
) -> Result<BbsSnapshot> {
    let root = content.root().to_path_buf();
    let sync_state = content
        .state
        .load_state()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default();
    let membership =
        crate::bbs_sync::control::read_membership(&content.state).map_err(anyhow::Error::msg)?;
    let state = load_project_state(&root, project_id)?;
    let mut threads = Vec::new();
    let mut new_count = 0usize;
    let mut avatar_cache = BTreeMap::new();
    for thread_id in read_thread_ids(&root)? {
        let Ok((meta, mut posts)) = load_thread(&root, &thread_id) else {
            continue;
        };
        posts.retain(|p| {
            !crate::bbs_sync::reconcile::suppressed(
                &sync_state,
                &thread_id,
                &p.meta.post_id,
                &p.version_id,
            )
        });
        let real_posts = posts.iter().map(|p| p.meta.post_id.clone()).collect();
        let unavailable_posts = content
            .unavailable_views(&thread_id, &sync_state, &real_posts)
            .unwrap_or_default();
        if !posts.iter().any(|p| p.meta.kind == "topic") && unavailable_posts.is_empty() {
            continue;
        }
        let relevant = thread_relevant(&meta, &posts, project_id);
        let mut post_views = Vec::new();
        let mut thread_is_new = false;
        for post in posts {
            let external = post.meta.project_id != project_id;
            let state_label = if state.processed_posts.contains_key(&post.meta.post_id) {
                "processed"
            } else if state.ignored_posts.contains_key(&post.meta.post_id) {
                "ignored"
            } else if relevant && external {
                thread_is_new = true;
                "new"
            } else {
                "none"
            };
            let preview_body = if post.body.trim().is_empty() {
                post.meta
                    .attachments
                    .iter()
                    .map(|item| item.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                post.body.clone()
            };
            let sync_avatar = content.avatar_view(
                &thread_id,
                &post.meta.post_id,
                &post.version_id,
                Some(&sync_state),
            );
            let agent_avatar = if sync_avatar.is_none() {
                post_agent_avatar(&root, &post.meta, &mut avatar_cache)
            } else {
                None
            };
            post_views.push(BbsPostView {
                post_id: post.meta.post_id.clone(),
                version_id: post.version_id.clone(),
                sync_avatar,
                thread_id: post.meta.thread_id.clone(),
                project_id: post.meta.project_id.clone(),
                project_display_name: display_project_name_with_fallback(
                    &post.meta.project_id,
                    Some(&post.meta.project_display_name),
                ),
                agent_id: post.meta.agent_id.clone(),
                agent_display_name: post.meta.agent_display_name.clone(),
                agent_avatar,
                created_at: post.meta.created_at.clone(),
                kind: post.meta.kind.clone(),
                preview: preview_markdown(&preview_body),
                attachments: content.attachment_views(&post, Some(&sync_state)),
                body: post.body,
                state: state_label.into(),
                external,
            });
        }
        if thread_is_new {
            new_count += 1;
        }
        threads.push(BbsThreadView {
            thread_id: meta.thread_id.clone(),
            visibility: meta.visibility.clone(),
            project_tag_labels: meta
                .project_tags
                .iter()
                .map(|id| display_project_name_with_fallback(id, None))
                .collect(),
            project_tags: meta.project_tags.clone(),
            created_by_project_label: display_project_name_with_fallback(
                &meta.created_by_project,
                None,
            ),
            created_by_project: meta.created_by_project,
            updated_at: meta.updated_at,
            latest_post_id: meta.latest_post_id,
            is_new: thread_is_new,
            relevant,
            sharing_group_id: membership
                .as_ref()
                .filter(|m| {
                    sync_state
                        .groups
                        .get(&m.group_id)
                        .is_some_and(|g| g.shared_threads.contains(&thread_id))
                })
                .map(|m| m.group_id.clone()),
            posts: post_views,
            unavailable_posts,
        });
    }
    threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(BbsSnapshot {
        project_id: project_id.to_string(),
        project_display_name: display_project_name_with_fallback(project_id, project_display_name),
        root: root.display().to_string(),
        new_count,
        threads,
    })
}

pub fn mark_processed(project_id: &str, post_id: &str) -> Result<()> {
    mark_post_state(project_id, post_id, true)
}

pub fn ignore_post(project_id: &str, post_id: &str) -> Result<()> {
    mark_post_state(project_id, post_id, false)
}

fn mark_post_state(project_id: &str, post_id: &str, processed: bool) -> Result<()> {
    ensure_account_layout()?;
    let root = account_root();
    let (_, post) = find_post(&root, post_id)?;
    let mut state = load_project_state(&root, project_id)?;
    let item = BbsSeenPost {
        thread_id: post.meta.thread_id,
        at: now_iso(),
    };
    if processed {
        state.ignored_posts.remove(post_id);
        state.processed_posts.insert(post_id.to_string(), item);
    } else {
        state.processed_posts.remove(post_id);
        state.ignored_posts.insert(post_id.to_string(), item);
    }
    save_project_state(&root, &state)
}

pub fn delete_item(thread_id: &str, post_id: Option<&str>, version_id: Option<&str>) -> Result<()> {
    ensure_account_layout()?;
    sync::ContentStore::account().delete_local(thread_id, post_id, version_id)
}

#[cfg(test)]
fn delete_item_at(root: &Path, thread: &str, post: Option<&str>) -> Result<()> {
    sync::ContentStore::at(root.into(), root.join(".test-account")).delete_local(thread, post, None)
}

#[cfg(test)]
fn create_thread_at(
    root: &Path,
    identity: &Identity,
    tags: Vec<String>,
    broadcast: bool,
    body: String,
    sources: &[BbsAttachmentSource],
) -> Result<String> {
    create_thread_scoped(
        &sync::ContentStore::at(root.into(), root.join(".test-account")),
        identity,
        tags,
        broadcast,
        body,
        sources,
    )
}
#[cfg(test)]
fn reply_at(
    root: &Path,
    identity: &Identity,
    thread: &str,
    body: String,
    sources: &[BbsAttachmentSource],
) -> Result<String> {
    reply_scoped(
        &sync::ContentStore::at(root.into(), root.join(".test-account")),
        identity,
        thread,
        body,
        sources,
    )
}

pub fn create_thread(project_tags: Vec<String>, broadcast: bool, body: String) -> Result<String> {
    create_thread_as(&identity_from_env(), project_tags, broadcast, body)
}

pub fn create_thread_as(
    identity: &Identity,
    project_tags: Vec<String>,
    broadcast: bool,
    body: String,
) -> Result<String> {
    create_thread_with_attachments_as(identity, project_tags, broadcast, body, Vec::new())
}

pub fn create_thread_with_attachments_as(
    identity: &Identity,
    project_tags: Vec<String>,
    broadcast: bool,
    body: String,
    sources: Vec<BbsAttachmentSource>,
) -> Result<String> {
    ensure_account_layout()?;
    create_thread_scoped(
        &sync::ContentStore::account(),
        identity,
        project_tags,
        broadcast,
        body,
        &sources,
    )
}

fn create_thread_scoped(
    content: &sync::ContentStore,
    identity: &Identity,
    project_tags: Vec<String>,
    broadcast: bool,
    body: String,
    sources: &[BbsAttachmentSource],
) -> Result<String> {
    create_thread_mentions_scoped(content, identity, project_tags, broadcast, body, sources, &[])
}

pub fn create_thread_with_mentions_as(
    identity: &Identity, project_tags: Vec<String>, broadcast: bool, body: String,
    sources: Vec<BbsAttachmentSource>, targets: Vec<mentions::Mention>,
) -> Result<String> {
    ensure_account_layout()?;
    create_thread_mentions_scoped(&sync::ContentStore::account(), identity, project_tags, broadcast, body, &sources, &targets)
}

fn create_thread_mentions_scoped(
    content: &sync::ContentStore, identity: &Identity, project_tags: Vec<String>, broadcast: bool,
    body: String, sources: &[BbsAttachmentSource], targets: &[mentions::Mention],
) -> Result<String> {
    let root = content.root();
    if body.trim().is_empty() && sources.is_empty() {
        bail!("BBS post needs text or an attachment");
    }
    let identity = identity.clone();
    let thread_id = format!("thread-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let post_id = mint_post_id(&identity.agent_id);
    let mentions = mentions::Prepared::load(content, targets, true)?;
    let mut prepared = PreparedAttachments::prepare(root, &post_id, sources)?;
    let _lock = acquire_write_lock(root)?;
    let (mentions, body) = mentions.finalize(content, &_lock, &thread_id, true, body)?;
    let now = now_iso();
    let thread_path = thread_dir(&root, &thread_id);
    fs::create_dir_all(thread_path.join("posts"))?;
    fs::create_dir_all(thread_path.join("attachments"))?;
    let tags = normalize_project_tags(project_tags);
    let meta = BbsThreadMeta {
        schema: THREAD_SCHEMA.into(),
        thread_id: thread_id.clone(),
        status: "open".into(),
        visibility: if broadcast { "broadcast" } else { "targeted" }.into(),
        project_tags: if broadcast { Vec::new() } else { tags },
        created_by_project: identity.project_id.clone(),
        created_by_agent: identity.agent_id.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
        latest_post_id: post_id.clone(),
    };
    let post = BbsPostMeta {
        schema: POST_SCHEMA.into(),
        post_id: post_id.clone(),
        thread_id: thread_id.clone(),
        project_id: identity.project_id,
        agent_id: identity.agent_id,
        agent_display_name: identity.agent_display_name,
        agent_avatar: identity.agent_avatar,
        project_display_name: identity.project_display_name,
        created_at: now,
        kind: "topic".into(),
        attachments: prepared.entries.clone(),
        mentions,
    };
    let publish = (|| -> Result<()> {
        let notifications = content.before_local_publish(
            &_lock,
            &post,
            serialized_post(&post, &body)?.as_bytes(),
            true,
        )?;
        write_thread_meta(&thread_path.join("thread.yaml"), &meta)?;
        prepared.install(&thread_path, &post_id)?;
        write_post(&thread_path, &post, &body)?;
        notify::published(
            content, &_lock, &notifications,
            &thread_path.join("posts").join(format!("{post_id}.md")),
        );
        Ok(())
    })();
    if let Err(error) = publish {
        let _ = fs::remove_dir_all(&thread_path);
        return Err(error);
    }
    // The post is the publication point. Index failures must not invite a duplicate submission.
    warn_index_failure(add_thread_to_board(&root, &thread_id));
    content.state.mark_content_changed();
    Ok(thread_id)
}

pub fn reply(thread_id: &str, body: String) -> Result<String> {
    reply_as(&identity_from_env(), thread_id, body)
}

pub fn reply_as(identity: &Identity, thread_id: &str, body: String) -> Result<String> {
    reply_with_attachments_as(identity, thread_id, body, Vec::new())
}

pub fn reply_with_attachments_as(
    identity: &Identity,
    thread_id: &str,
    body: String,
    sources: Vec<BbsAttachmentSource>,
) -> Result<String> {
    ensure_account_layout()?;
    reply_scoped(
        &sync::ContentStore::account(),
        identity,
        thread_id,
        body,
        &sources,
    )
}

fn reply_scoped(
    content: &sync::ContentStore,
    identity: &Identity,
    thread_id: &str,
    body: String,
    sources: &[BbsAttachmentSource],
) -> Result<String> {
    reply_mentions_scoped(content, identity, thread_id, body, sources, &[])
}

pub fn reply_with_mentions_as(
    identity: &Identity, thread_id: &str, body: String, sources: Vec<BbsAttachmentSource>, targets: Vec<mentions::Mention>,
) -> Result<String> {
    ensure_account_layout()?;
    reply_mentions_scoped(&sync::ContentStore::account(), identity, thread_id, body, &sources, &targets)
}

fn reply_mentions_scoped(
    content: &sync::ContentStore, identity: &Identity, thread_id: &str, body: String,
    sources: &[BbsAttachmentSource], targets: &[mentions::Mention],
) -> Result<String> {
    let root = content.root();
    if !safe_component(thread_id) {
        bail!("invalid BBS thread id");
    }
    if body.trim().is_empty() && sources.is_empty() {
        bail!("BBS reply needs text or an attachment");
    }
    let identity = identity.clone();
    let thread_path = thread_dir(&root, thread_id);
    if !thread_path.is_dir() {
        bail!("BBS thread not found: {thread_id}");
    }
    let post_id = mint_post_id(&identity.agent_id);
    let mentions = mentions::Prepared::load(content, targets, false)?;
    let mut prepared = PreparedAttachments::prepare(root, &post_id, sources)?;
    let _lock = acquire_write_lock(root)?;
    let (mentions, body) = mentions.finalize(content, &_lock, thread_id, false, body)?;
    // Re-read under the same lock as delete, after the potentially slow copies.
    let mut meta = read_thread_meta(&thread_path.join("thread.yaml"))
        .with_context(|| format!("BBS thread is unavailable or was deleted: {thread_id}"))?;
    if meta.thread_id != thread_id {
        bail!("BBS thread metadata ID mismatch");
    }
    let now = now_iso();
    let post = BbsPostMeta {
        schema: POST_SCHEMA.into(),
        post_id: post_id.clone(),
        thread_id: thread_id.to_string(),
        project_id: identity.project_id,
        agent_id: identity.agent_id,
        agent_display_name: identity.agent_display_name,
        agent_avatar: identity.agent_avatar,
        project_display_name: identity.project_display_name,
        created_at: now.clone(),
        kind: "reply".into(),
        attachments: prepared.entries.clone(),
        mentions,
    };
    let notifications = content.before_local_publish(
        &_lock,
        &post,
        serialized_post(&post, &body)?.as_bytes(),
        false,
    )?;
    prepared.install(&thread_path, &post_id)?;
    write_post(&thread_path, &post, &body)?;
    meta.latest_post_id = post_id.clone();
    meta.updated_at = now;
    warn_index_failure(write_thread_meta(&thread_path.join("thread.yaml"), &meta));
    warn_index_failure(add_thread_to_board(&root, thread_id));
    notify::published(
        content, &_lock, &notifications,
        &thread_path.join("posts").join(format!("{post_id}.md")),
    );
    content.state.mark_content_changed();
    Ok(post_id)
}

pub(crate) fn publish_human_post(
    identity: &Identity,
    request: BbsHumanPostRequest,
) -> std::result::Result<String, mentions::PublishError> {
    ensure_account_layout().map_err(mentions::PublishError::from)?;
    human_post_scoped(&sync::ContentStore::account(), identity, request)
}
fn human_post_scoped(
    content: &sync::ContentStore,
    identity: &Identity,
    request: BbsHumanPostRequest,
) -> std::result::Result<String, mentions::PublishError> {
    create_thread_mentions_scoped(
        content, identity, request.project_tags, false,
        request.body, &request.attachments, &request.mentions,
    ).map_err(Into::into)
}
pub(crate) fn publish_human_reply(
    identity: &Identity,
    request: BbsHumanReplyRequest,
) -> std::result::Result<String, mentions::PublishError> {
    ensure_account_layout().map_err(mentions::PublishError::from)?;
    human_reply_scoped(&sync::ContentStore::account(), identity, request)
}
fn human_reply_scoped(
    content: &sync::ContentStore,
    identity: &Identity,
    request: BbsHumanReplyRequest,
) -> std::result::Result<String, mentions::PublishError> {
    reply_mentions_scoped(
        content, identity, &request.thread_id, request.body,
        &request.attachments, &request.mentions,
    ).map_err(Into::into)
}

fn warn_index_failure(result: Result<()>) {
    if let Err(error) = result {
        eprintln!(
            "BBS change published; index refresh failed (reads recover from posts): {error:#}"
        );
    }
}

pub fn render_thread(thread_id: &str) -> Result<String> {
    ensure_account_layout()?;
    render_thread_scoped(&sync::ContentStore::account(), thread_id)
}

#[cfg(test)]
fn render_thread_at(root: &Path, thread_id: &str) -> Result<String> {
    render_thread_scoped(
        &sync::ContentStore::at(root.into(), root.join(".test-account")),
        thread_id,
    )
}
fn render_thread_scoped(content: &sync::ContentStore, thread_id: &str) -> Result<String> {
    let root = content.root();
    let state = content
        .state
        .load_state()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default();
    let (meta, mut posts) = load_thread(&root, thread_id)?;
    posts.retain(|p| {
        !crate::bbs_sync::reconcile::suppressed(&state, thread_id, &p.meta.post_id, &p.version_id)
    });
    posts.sort_by(|a, b| a.meta.created_at.cmp(&b.meta.created_at));
    let real_posts = posts.iter().map(|p| p.meta.post_id.clone()).collect();
    let unavailable = content.unavailable_views(thread_id, &state, &real_posts)?;
    let mut out = String::new();
    out.push_str(&format!("Thread: {}\n", meta.thread_id));
    out.push_str(&format!("Visibility: {}\n", meta.visibility));
    if meta.visibility == "broadcast" {
        out.push_str("To: Broadcast\n");
    } else {
        out.push_str(&format!("To: {}\n", meta.project_tags.join(", ")));
    }
    out.push('\n');
    for post in posts {
        out.push_str(&format!(
            "## {} / {} / {}\npost: {}\nversion: {}\n\n{}\n\n",
            post.meta.project_display_name,
            post.meta.agent_display_name,
            post.meta.created_at,
            post.meta.post_id,
            post.version_id,
            post.body.trim()
        ));
        for attachment in content.attachment_views(&post, Some(&state)) {
            out.push_str(&format!(
                "Attachment: {}\n  id: {}\n  size: {} bytes\n  sha256: {}\n  path: {}{}\n\n",
                attachment.attachment.name,
                attachment.attachment.id,
                attachment.attachment.size_bytes,
                attachment.attachment.sha256,
                attachment.local_path,
                if attachment.available {
                    ""
                } else {
                    " [missing]"
                },
            ));
        }
    }
    for item in unavailable {
        out.push_str(&format!(
            "## Unavailable post\npost: {}\n\n{}\n\n",
            item.post_id,
            sync::PLACEHOLDER_TEXT
        ));
    }
    Ok(out)
}

pub fn run_cli() -> Result<()> {
    match parse_cli_args(&std::env::args().skip(1).collect::<Vec<_>>())? {
        BbsCliCommand::Help => print!("{CLI_HELP}"),
        BbsCliCommand::Agents => {
            let account = crate::kota_home_dir();
            let view = crate::bbs_sync::roster::directory(&account, &crate::bbs_sync::StateStore::at(&account))?;
            serde_json::to_writer(io::stdout().lock(), &view)?;
            println!();
        }
        BbsCliCommand::New {
            projects,
            broadcast,
            attachments,
            at,
        } => {
            let body = read_stdin_body(!attachments.is_empty())?;
            let thread_id = create_thread_with_mentions_as(
                &identity_from_env(),
                projects,
                broadcast,
                body,
                attachments,
                at.into_iter().collect(),
            )?;
            println!("{thread_id}");
        }
        BbsCliCommand::Reply {
            thread_id,
            attachments,
            at,
        } => {
            let body = read_stdin_body(!attachments.is_empty())?;
            let post_id =
                reply_with_mentions_as(&identity_from_env(), &thread_id, body, attachments, at.into_iter().collect())?;
            println!("{post_id}");
        }
        BbsCliCommand::Show(thread_id) => {
            print!("{}", render_thread(&thread_id)?);
        }
        BbsCliCommand::Root => {
            ensure_account_layout()?;
            println!("{}", account_root().display());
        }
        BbsCliCommand::InstallShim => {
            let path = install_cli_shim()?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

const CLI_HELP: &str = "Kota BBS — cross-project posts and replies\n\n\
Usage:\n\
  kota-bbs agents\n\
  kota-bbs new --projects <project-id> [project-id...] [--at <targetRef>] [--attach <file> ...]\n\
  kota-bbs new --broadcast [--at <targetRef>] [--attach <file> ...]\n\
  kota-bbs reply <thread-id> [--at <targetRef>] [--attach <file> ...]\n\
  kota-bbs show <thread-id>\n\
  kota-bbs root\n\
  kota-bbs install-shim\n\
  kota-bbs help | --help | -h (also available after a subcommand)\n\n\
new/reply read Markdown from stdin; they print the new thread/post ID.\n\
agents prints the full public JSON directory; it does not join a group or start agents.\n\
Copy targetRef from agents for --at (at most one per post/reply).\n\
Refs are <deviceId|local>/<projectId>/<agentId>; names in text do not route notices.\n\
The App delivers explicit mentions after publication. Reply targets are never inherited.\n\
Local mentions work without a sync identity; remote replies require a shared thread.\n\
Posts over 8 MiB or with frontmatter over 64 KiB cannot trigger remote notices.\n\
Publication success does not mean a remote notice has been delivered.\n\
Repeat --attach for images or other regular files (not directories).\n\
Limits: 1 GiB per file, 1 GiB total per post, at most 9 attachments.\n\
Example:\n\
  kota-bbs reply thread-example --attach ./screenshot.png --attach ./report.pdf <<'EOF'\n\
  Findings are attached.\n\
  EOF\n\
For an attachment-only post, use --attach ./file </dev/null.\n\n\
Files are copied as a snapshot at publication; originals are never moved or edited.\n\
Attachments live with their post, outside the Markdown body. Images appear below\n\
the body; other files show their name and full local path. Use show to find each\n\
post ID and its attachments, local paths, size and SHA-256 (or missing status).\n\
Only --attach files get a BBS copy. Paths written in the body are NOT collected,\n\
rewritten or guaranteed to work on another device. Old posts are not migrated.\n\
Attachment failure aborts publication; fix/remove the attachment and retry.\n";

#[derive(Debug)]
enum BbsCliCommand {
    Help,
    Agents,
    New {
        projects: Vec<String>,
        broadcast: bool,
        attachments: Vec<BbsAttachmentSource>,
        at: Option<mentions::Mention>,
    },
    Reply {
        thread_id: String,
        attachments: Vec<BbsAttachmentSource>,
        at: Option<mentions::Mention>,
    },
    Show(String),
    Root,
    InstallShim,
}

fn parse_cli_args(args: &[String]) -> Result<BbsCliCommand> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(BbsCliCommand::Help);
    };
    let known = ["new", "reply", "show", "root", "install-shim", "agents"];
    if matches!(command, "help" | "--help" | "-h") {
        if args.len() > 2
            || args
                .get(1)
                .is_some_and(|value| !known.contains(&value.as_str()))
        {
            bail!("usage: kota-bbs help [new|reply|show|root|install-shim|agents]");
        }
        return Ok(BbsCliCommand::Help);
    }
    if !known.contains(&command) {
        bail!("unknown kota-bbs command: {command}");
    }
    if (args.len() == 2 && args[1] == "help")
        || args[1..]
            .iter()
            .any(|value| matches!(value.as_str(), "--help" | "-h"))
    {
        return Ok(BbsCliCommand::Help);
    }
    let mut projects = Vec::new();
    let mut broadcast = false;
    let mut attachments = Vec::new();
    let mut thread_id = None;
    let mut at = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--at" if matches!(command, "new" | "reply") && at.is_none() => {
                i += 1;
                let value = args.get(i).ok_or_else(|| anyhow!("--at requires a targetRef"))?;
                at = Some(mentions::Mention::parse(value)?);
            }
            "--attach" if matches!(command, "new" | "reply") => {
                i += 1;
                let path = args
                    .get(i)
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .ok_or_else(|| anyhow!("--attach requires a file path"))?;
                attachments.push(BbsAttachmentSource {
                    path: path.clone(),
                    name: None,
                });
            }
            "--broadcast" if command == "new" && !broadcast => broadcast = true,
            "--projects" if command == "new" => {
                i += 1;
                let start = i;
                while i < args.len() && !args[i].starts_with('-') {
                    projects.extend(parse_project_list(&args[i]));
                    i += 1;
                }
                if i == start {
                    bail!("--projects requires a project ID");
                }
                continue;
            }
            value
                if matches!(command, "reply" | "show")
                    && thread_id.is_none()
                    && !value.starts_with('-')
                    && safe_component(value) =>
            {
                thread_id = Some(value.to_string());
            }
            value => bail!("unknown or extra kota-bbs {command} argument: {value}"),
        }
        i += 1;
    }
    if attachments.len() > MAX_ATTACHMENTS {
        bail!("at most {MAX_ATTACHMENTS} attachments are allowed");
    }
    Ok(match command {
        "new" => {
            if broadcast == !projects.is_empty() {
                bail!("new requires either --projects <project-id> or --broadcast");
            }
            BbsCliCommand::New {
                projects,
                broadcast,
                attachments,
                at,
            }
        }
        "reply" => BbsCliCommand::Reply {
            thread_id: thread_id.ok_or_else(|| anyhow!("reply requires a thread ID"))?,
            attachments,
            at,
        },
        "show" => {
            BbsCliCommand::Show(thread_id.ok_or_else(|| anyhow!("show requires a thread ID"))?)
        }
        "root" => BbsCliCommand::Root,
        "agents" => BbsCliCommand::Agents,
        _ => BbsCliCommand::InstallShim,
    })
}

fn read_stdin_body(has_attachments: bool) -> Result<String> {
    let mut body = String::new();
    io::stdin().read_to_string(&mut body)?;
    let body = body.trim().to_string();
    if body.is_empty() && !has_attachments {
        bail!("kota-bbs requires text on stdin or --attach <file>");
    }
    Ok(body)
}

fn read_thread_ids(root: &Path) -> Result<Vec<String>> {
    let board_path = root.join("board.json");
    let mut ids = fs::read(&board_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BbsBoardIndex>(&bytes).ok())
        .map(|board| board.threads)
        .unwrap_or_default();
    let threads_dir = root.join("threads");
    if threads_dir.is_dir() {
        for entry in fs::read_dir(threads_dir)? {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                ids.push(name.to_string());
            }
        }
    }
    let mut seen = BTreeSet::new();
    ids.retain(|id| safe_component(id) && seen.insert(id.clone()));
    Ok(ids)
}

fn load_thread(root: &Path, thread_id: &str) -> Result<(BbsThreadMeta, Vec<LoadedPost>)> {
    if !safe_component(thread_id) {
        bail!("invalid BBS thread id");
    }
    let dir = thread_dir(root, thread_id);
    let mut meta = read_thread_meta(&dir.join("thread.yaml"))?;
    if meta.thread_id != thread_id {
        bail!("BBS thread metadata ID mismatch");
    }
    // The path adapter is read-only here; no credentials, scan worker or
    // account state are constructed by the snapshot loader.
    let paths = sync::ContentStore::at(root.into(), root.join(".unused-view-state"));
    let mut posts = Vec::new();
    for location in paths.locations(thread_id)? {
        if let Ok(post) = read_post(&location.path) {
            if post.meta.thread_id == thread_id
                && post.meta.post_id == location.post
                && matches!(post.meta.kind.as_str(), "topic" | "reply")
            {
                posts.push(post);
            }
        }
    }
    posts.sort_by(|a, b| {
        (&a.meta.created_at, &a.meta.post_id, &a.version_id).cmp(&(
            &b.meta.created_at,
            &b.meta.post_id,
            &b.version_id,
        ))
    });
    let root_post = posts
        .iter()
        .find(|p| p.meta.kind == "topic")
        .map(|p| p.meta.post_id.clone());
    let mut seen = BTreeSet::new();
    posts.retain(|p| {
        (p.meta.kind != "topic" || Some(&p.meta.post_id) == root_post.as_ref())
            && seen.insert((p.meta.post_id.clone(), p.version_id.clone()))
    });
    posts.sort_by(|a, b| a.meta.created_at.cmp(&b.meta.created_at));
    if let Some(latest) = posts.last() {
        // Existing timestamps have second precision. Do not replace a valid same-second
        // publication pointer with read_dir's arbitrary order while repairing indexes.
        let latest = posts
            .iter()
            .find(|post| {
                post.meta.post_id == meta.latest_post_id
                    && post.meta.created_at == latest.meta.created_at
            })
            .unwrap_or(latest);
        meta.latest_post_id = latest.meta.post_id.clone();
        meta.updated_at = latest.meta.created_at.clone();
    }
    Ok((meta, posts))
}

fn read_thread_meta(path: &Path) -> Result<BbsThreadMeta> {
    let meta = serde_yaml::from_slice::<BbsThreadMeta>(&fs::read(path)?)?;
    if meta.schema != THREAD_SCHEMA {
        bail!("unsupported BBS thread schema in {}", path.display());
    }
    Ok(meta)
}

fn write_thread_meta(path: &Path, meta: &BbsThreadMeta) -> Result<()> {
    write_bytes_atomic(path, serde_yaml::to_string(meta)?.as_bytes())
}

fn read_post(path: &Path) -> Result<LoadedPost> {
    let raw = fs::read_to_string(path)?;
    let rest = raw
        .strip_prefix("---\n")
        .ok_or_else(|| anyhow!("missing frontmatter start in {}", path.display()))?;
    let Some((yaml, body)) = rest.split_once("\n---\n") else {
        bail!("missing frontmatter end in {}", path.display());
    };
    let meta = serde_yaml::from_str::<BbsPostMeta>(yaml)?;
    mentions::validate(&meta.mentions)?;
    if meta.schema != POST_SCHEMA {
        bail!("unsupported BBS post schema in {}", path.display());
    }
    Ok(LoadedPost {
        version_id: format!("{:x}", Sha256::digest(raw.as_bytes())),
        meta,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

fn write_post(thread_path: &Path, meta: &BbsPostMeta, body: &str) -> Result<()> {
    let path = thread_path
        .join("posts")
        .join(format!("{}.md", meta.post_id));
    write_bytes_atomic(&path, serialized_post(meta, body)?.as_bytes())
}

fn serialized_post(meta: &BbsPostMeta, body: &str) -> Result<String> {
    let mut raw = String::new();
    raw.push_str("---\n");
    raw.push_str(&serde_yaml::to_string(meta)?);
    raw.push_str("---\n\n");
    raw.push_str(body.trim());
    raw.push('\n');
    Ok(raw)
}

fn add_thread_to_board(root: &Path, thread_id: &str) -> Result<()> {
    let path = root.join("board.json");
    let mut board = if path.is_file() {
        serde_json::from_slice::<BbsBoardIndex>(&fs::read(&path)?)?
    } else {
        BbsBoardIndex {
            schema: "kota.bbs.board.v1".into(),
            updated_at: now_iso(),
            threads: Vec::new(),
        }
    };
    board.threads.retain(|id| id != thread_id);
    board.threads.insert(0, thread_id.to_string());
    board.updated_at = now_iso();
    write_json_pretty(&path, &board)
}

fn remove_thread_from_board(root: &Path, thread_id: &str) -> Result<()> {
    let path = root.join("board.json");
    if !path.is_file() {
        return Ok(());
    }
    let mut board = serde_json::from_slice::<BbsBoardIndex>(&fs::read(&path)?)?;
    board.threads.retain(|id| id != thread_id);
    board.updated_at = now_iso();
    write_json_pretty(&path, &board)
}

fn find_post(root: &Path, post_id: &str) -> Result<(BbsThreadMeta, LoadedPost)> {
    for thread_id in read_thread_ids(root)? {
        let Ok((thread, posts)) = load_thread(root, &thread_id) else {
            continue;
        };
        if let Some(post) = posts.into_iter().find(|post| post.meta.post_id == post_id) {
            return Ok((thread, post));
        }
    }
    bail!("BBS post not found: {post_id}");
}

fn thread_relevant(meta: &BbsThreadMeta, posts: &[LoadedPost], project_id: &str) -> bool {
    meta.visibility == "broadcast"
        || meta.project_tags.iter().any(|tag| tag == project_id)
        || meta.created_by_project == project_id
        || posts.iter().any(|post| post.meta.project_id == project_id)
}

fn load_project_state(root: &Path, project_id: &str) -> Result<BbsProjectState> {
    let path = project_state_path(root, project_id);
    if !path.is_file() {
        return Ok(BbsProjectState {
            project_id: project_id.to_string(),
            ..Default::default()
        });
    }
    let mut state = serde_json::from_slice::<BbsProjectState>(&fs::read(&path)?)?;
    if state.project_id.trim().is_empty() {
        state.project_id = project_id.to_string();
    }
    Ok(state)
}

fn save_project_state(root: &Path, state: &BbsProjectState) -> Result<()> {
    write_json_pretty(&project_state_path(root, &state.project_id), state)
}

fn project_state_path(root: &Path, project_id: &str) -> PathBuf {
    root.join("state")
        .join("projects")
        .join(format!("{}.json", sanitize_id(project_id)))
}

fn thread_dir(root: &Path, thread_id: &str) -> PathBuf {
    root.join("threads").join(sanitize_id(thread_id))
}

fn normalize_project_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        for item in parse_project_list(tag) {
            if seen.insert(item.clone()) {
                out.push(item);
            }
        }
    }
    out
}

fn parse_project_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn identity_from_env() -> Identity {
    let project_id = std::env::var("KOTA_PROJECT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown-project".into());
    let agent_id = std::env::var("KOTA_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown-agent".into());
    let agent_yaml = std::env::var("KOTA_AGENT_CWD")
        .ok()
        .map(PathBuf::from)
        .map(|cwd| cwd.join("agent.yaml"));
    let agent_display_name = std::env::var("KOTA_AGENT_DISPLAY_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| agent_yaml.as_deref().and_then(agent_display_name_from_yaml))
        .unwrap_or_else(|| agent_id.clone());
    let project_display_name = std::env::var("KOTA_PROJECT_DISPLAY_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| display_project_name_with_fallback(&project_id, None));
    let agent_avatar = std::env::var("KOTA_AGENT_AVATAR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| agent_yaml.as_deref().and_then(agent_avatar_from_yaml));
    Identity {
        project_id,
        project_display_name,
        agent_id,
        agent_display_name,
        agent_avatar,
    }
}

pub fn agent_display_name_from_yaml(path: &Path) -> Option<String> {
    agent_yaml_string(path, &["display-name", "displayName"])
}

pub fn agent_avatar_from_yaml(path: &Path) -> Option<String> {
    agent_yaml_string(path, &["avatar-id", "avatarId"])
}

fn agent_yaml_string(path: &Path, keys: &[&str]) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text).ok()?;
    keys.iter().find_map(|key| {
        yaml.get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn post_agent_avatar(
    root: &Path,
    meta: &BbsPostMeta,
    cache: &mut BTreeMap<(String, String), Option<String>>,
) -> Option<String> {
    if meta.agent_avatar.is_some() || meta.agent_id == "human" {
        return meta.agent_avatar.clone();
    }

    let key = (meta.project_id.clone(), meta.agent_id.clone());
    if let Some(avatar) = cache.get(&key) {
        return avatar.clone();
    }

    let avatar = project_agent_avatar_from_yaml(root, &meta.project_id, &meta.agent_id);
    cache.insert(key, avatar.clone());
    avatar
}

fn project_agent_avatar_from_yaml(root: &Path, project_id: &str, agent_id: &str) -> Option<String> {
    let workspaces_root = root.parent()?;
    let project_id = project_id.trim();
    let agent_id = agent_id.trim();
    if project_id.is_empty() || agent_id.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    candidates.push(
        workspaces_root
            .join(project_id)
            .join(".agent-workspaces")
            .join(agent_id)
            .join("agent.yaml"),
    );

    let sanitized_project_id = sanitize_id(project_id);
    let sanitized_agent_id = sanitize_id(agent_id);
    if sanitized_project_id != project_id || sanitized_agent_id != agent_id {
        candidates.push(
            workspaces_root
                .join(sanitized_project_id)
                .join(".agent-workspaces")
                .join(sanitized_agent_id)
                .join("agent.yaml"),
        );
    }

    candidates
        .into_iter()
        .find_map(|path| agent_avatar_from_yaml(&path))
}

pub fn display_project_name_with_fallback(project_id: &str, fallback: Option<&str>) -> String {
    if let Some(fallback) = fallback.map(str::trim).filter(|value| !value.is_empty()) {
        return humanize_project_label(fallback);
    }
    let trimmed = project_id.trim();
    if trimmed.is_empty() {
        return "Project".into();
    }
    let display_slug = trimmed
        .split_once('-')
        .map(|(_, rest)| rest)
        .filter(|rest| !rest.trim().is_empty())
        .unwrap_or(trimmed);
    humanize_project_label(display_slug)
}

fn humanize_project_label(value: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for ch in value.chars() {
        if matches!(ch, '-' | '_' | '/') {
            out.push(' ');
            capitalize = true;
        } else if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    if out.trim().is_empty() {
        "Project".into()
    } else {
        out
    }
}

fn preview_markdown(body: &str) -> String {
    let text = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= 220 {
        return text;
    }
    let mut preview = text.chars().take(217).collect::<String>();
    preview.push_str("...");
    preview
}

fn mint_post_id(agent_id: &str) -> String {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    format!(
        "post-{}-{}-{}",
        timestamp,
        sanitize_id(agent_id),
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes_atomic(path, &serde_json::to_vec_pretty(value)?)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

fn replace_symlink_or_empty_dir(target: &Path, link: &Path) -> Result<()> {
    if let Ok(existing) = fs::read_link(link) {
        if existing == target {
            return Ok(());
        }
        fs::remove_file(link)?;
    } else if link.exists() {
        if link.is_dir() && fs::read_dir(link)?.next().is_none() {
            fs::remove_dir(link)?;
        } else {
            return Ok(());
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link)?;
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn current_target_triple_guess() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc.exe"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kota-bbs-{name}-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct TestBoard(PathBuf);
    impl TestBoard {
        fn new() -> Self {
            let board = Self(temp_dir("attachments"));
            ensure_layout_at(&board.0).unwrap();
            board
        }
        fn source(&self, name: &str, bytes: &[u8]) -> BbsAttachmentSource {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            BbsAttachmentSource {
                path: path.display().to_string(),
                name: None,
            }
        }
        fn post(&self, body: &str, sources: &[BbsAttachmentSource]) -> Result<String> {
            create_thread_at(
                &self.0,
                &human_identity("project", "User".into(), None),
                Vec::new(),
                true,
                body.into(),
                sources,
            )
        }
        fn reply(
            &self,
            thread: &str,
            body: &str,
            sources: &[BbsAttachmentSource],
        ) -> Result<String> {
            reply_at(
                &self.0,
                &human_identity("project", "User".into(), None),
                thread,
                body.into(),
                sources,
            )
        }
    }
    impl Drop for TestBoard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn attachment_posts_are_self_contained_and_each_reply_has_its_own_directory() {
        let board = TestBoard::new();
        let source = board.source("same name.PNG", b"picture");
        let body = format!("Handwritten reference remains: {}", source.path);
        let thread = board
            .post(&body, &[source.clone(), source.clone()])
            .unwrap();
        let reply = board.reply(&thread, "", &[source.clone()]).unwrap();
        let (_, posts) = load_thread(&board.0, &thread).unwrap();
        let topic = posts.iter().find(|post| post.meta.kind == "topic").unwrap();
        let response = posts
            .iter()
            .find(|post| post.meta.post_id == reply)
            .unwrap();
        assert_eq!(topic.body, body);
        assert!(response.body.is_empty());
        assert_ne!(topic.meta.attachments[0].id, topic.meta.attachments[1].id);
        assert_ne!(topic.meta.post_id, response.meta.post_id);
        for post in &posts {
            for item in &post.meta.attachments {
                assert!(item
                    .path
                    .starts_with(&format!("attachments/{}/", post.meta.post_id)));
                assert!(item.path.ends_with(".png"));
                assert_eq!(item.size_bytes, 7);
                assert_eq!(item.sha256, format!("{:x}", Sha256::digest(b"picture")));
                assert_eq!(
                    fs::read(thread_dir(&board.0, &thread).join(&item.path)).unwrap(),
                    b"picture"
                );
                assert!(!fs::read_to_string(&post.path)
                    .unwrap()
                    .contains("localPath:"));
            }
        }
        assert_eq!(fs::read(&source.path).unwrap(), b"picture");
        let show = render_thread_at(&board.0, &thread).unwrap();
        assert!(show.contains(&format!("post: {reply}")));
        assert!(show.contains("same name.PNG"));
        assert!(show.contains("sha256:"));
        assert!(show.contains(&board.0.canonicalize().unwrap().display().to_string()));
        let reply_dir = thread_dir(&board.0, &thread)
            .join("attachments")
            .join(&reply);
        delete_item_at(&board.0, &thread, Some(&reply)).unwrap();
        assert!(!reply_dir.exists());
        assert!(thread_dir(&board.0, &thread)
            .join(&topic.meta.attachments[0].path)
            .exists());
        delete_item_at(&board.0, &thread, None).unwrap();
        assert!(!thread_dir(&board.0, &thread).exists());
        assert!(Path::new(&source.path).exists());
    }

    #[test]
    fn attachment_validation_rejects_invalid_batches_before_publishing() {
        let board = TestBoard::new();
        let source = board.source("small.md", b"hello");
        let dir = BbsAttachmentSource {
            path: board.0.display().to_string(),
            name: None,
        };
        assert!(board.post("body", &[source.clone(), dir]).is_err());
        let missing = BbsAttachmentSource {
            path: board.0.join("missing").display().to_string(),
            name: None,
        };
        assert!(board.post("body", &[source.clone(), missing]).is_err());
        assert!(board.post("body", &vec![source.clone(); 10]).is_err());
        assert!(board.post("", &[]).is_err());
        assert!(read_thread_ids(&board.0).unwrap().is_empty());
        assert!(!board.0.join(".staging").exists());
        let huge = board.source("huge.bin", b"");
        File::options()
            .write(true)
            .open(&huge.path)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        assert!(board
            .post("body", &[huge.clone()])
            .unwrap_err()
            .to_string()
            .contains("1 GiB"));
        File::options()
            .write(true)
            .open(&huge.path)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES)
            .unwrap();
        assert!(validate_attachments(&[huge.clone()]).is_ok());
        assert!(board
            .post("body", &[huge, source])
            .unwrap_err()
            .to_string()
            .contains("total"));
        assert!(read_thread_ids(&board.0).unwrap().is_empty());
    }

    #[test]
    fn attachment_stream_is_bounded_even_if_source_outgrows_preflight() {
        let mut output = Vec::new();
        assert!(
            copy_attachment_stream(&mut io::Cursor::new(b"123456789"), &mut output, 8).is_err()
        );
        assert!(output.len() <= 8);
        let (count, hash) =
            copy_attachment_stream(&mut io::Cursor::new(b"12345678"), &mut Vec::new(), 8).unwrap();
        assert_eq!(count, 8);
        assert_eq!(hash, format!("{:x}", Sha256::digest(b"12345678")));
    }

    #[test]
    fn attachment_missing_paths_and_orphans_are_not_silently_loaded() {
        let board = TestBoard::new();
        let thread = board
            .post("", &[board.source("file.pdf", b"document")])
            .unwrap();
        let (_, posts) = load_thread(&board.0, &thread).unwrap();
        let post = &posts[0];
        let file = thread_dir(&board.0, &thread).join(&post.meta.attachments[0].path);
        fs::remove_file(&file).unwrap();
        let views = attachment_views(
            &board.0,
            &thread,
            &post.meta.post_id,
            &post.meta.attachments,
        );
        assert!(!views[0].available);
        assert!(render_thread_at(&board.0, &thread)
            .unwrap()
            .contains("[missing]"));
        let orphan = thread_dir(&board.0, &thread).join("attachments/orphan.tmp-test");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("hidden.png"), b"hidden").unwrap();
        assert!(!render_thread_at(&board.0, &thread)
            .unwrap()
            .contains("hidden"));
        let mut invalid = post.meta.attachments[0].clone();
        for path in [
            "../../private.png",
            "/tmp/private.png",
            "attachments/other/att.png",
        ] {
            invalid.path = path.into();
            assert!(
                attachment_views(&board.0, &thread, &post.meta.post_id, &[invalid.clone()])
                    .is_empty()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn attachment_sources_follow_only_regular_files_and_views_reject_symlink_escape() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;
        let board = TestBoard::new();
        let source = board.source("source.png", b"image");
        let file_link = board.0.join("link.png");
        symlink(&source.path, &file_link).unwrap();
        let thread = board
            .post(
                "",
                &[BbsAttachmentSource {
                    path: file_link.display().to_string(),
                    name: None,
                }],
            )
            .unwrap();
        let (_, posts) = load_thread(&board.0, &thread).unwrap();
        let post = &posts[0];
        let stored = thread_dir(&board.0, &thread).join(&post.meta.attachments[0].path);
        fs::remove_file(&stored).unwrap();
        symlink(&source.path, &stored).unwrap();
        assert!(
            !attachment_views(
                &board.0,
                &thread,
                &post.meta.post_id,
                &post.meta.attachments
            )[0]
            .available
        );
        let dir_link = board.0.join("directory-link");
        symlink(&board.0, &dir_link).unwrap();
        // macOS sockaddr_un cannot hold the long per-user temporary directory path.
        let socket = PathBuf::from("/tmp").join(format!(
            "kota-bbs-socket-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        let _listener = UnixListener::bind(&socket).unwrap();
        for path in [dir_link, socket.clone()] {
            assert!(validate_attachments(&[BbsAttachmentSource {
                path: path.display().to_string(),
                name: None
            }])
            .is_err());
        }
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn publication_survives_index_failure_and_legacy_posts_stay_unchanged() {
        let board = TestBoard::new();
        fs::remove_file(board.0.join("board.json")).unwrap();
        fs::create_dir(board.0.join("board.json")).unwrap();
        let thread = board.post("legacy plain text", &[]).unwrap();
        let (_, posts) = load_thread(&board.0, &thread).unwrap();
        assert!(posts[0].meta.attachments.is_empty());
        assert!(!fs::read_to_string(&posts[0].path)
            .unwrap()
            .contains("attachments:"));
        let reply = board.reply(&thread, "still published", &[]).unwrap();
        assert_eq!(read_thread_ids(&board.0).unwrap(), vec![thread.clone()]);
        let (mut meta, _) = load_thread(&board.0, &thread).unwrap();
        meta.latest_post_id = "missing".into();
        meta.updated_at = "1900-01-01".into();
        write_thread_meta(&thread_dir(&board.0, &thread).join("thread.yaml"), &meta).unwrap();
        let (meta, posts) = load_thread(&board.0, &thread).unwrap();
        assert_eq!(meta.latest_post_id, posts.last().unwrap().meta.post_id);
        assert_ne!(meta.updated_at, "1900-01-01");
        assert!(posts.iter().any(|post| post.meta.post_id == reply));
    }

    #[test]
    fn failed_publication_does_not_expose_staged_or_orphan_attachments() {
        let board = TestBoard::new();
        let source = board.source("source.pdf", b"document");
        let prepared =
            PreparedAttachments::prepare(&board.0, "post-aborted", &[source.clone()]).unwrap();
        let staging = prepared.staging.clone().unwrap();
        assert!(staging.is_dir());
        drop(prepared);
        assert!(!staging.exists());

        let thread = board.post("Original topic", &[]).unwrap();
        let thread_path = thread_dir(&board.0, &thread);
        // Simulate failure at the post publication point, after the attachment rename.
        fs::rename(thread_path.join("posts"), thread_path.join("saved-posts")).unwrap();
        fs::write(thread_path.join("posts"), b"not a directory").unwrap();
        assert!(board
            .reply(&thread, "Unpublished reply", &[source.clone()])
            .is_err());
        fs::remove_file(thread_path.join("posts")).unwrap();
        fs::rename(thread_path.join("saved-posts"), thread_path.join("posts")).unwrap();
        assert_eq!(load_thread(&board.0, &thread).unwrap().1.len(), 1);
        let shown = render_thread_at(&board.0, &thread).unwrap();
        assert!(!shown.contains("Unpublished reply"));
        assert!(!shown.contains("Attachment:"));
        assert_eq!(
            fs::read_dir(thread_path.join("attachments"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(fs::read_dir(board.0.join(".staging")).unwrap().count(), 0);
        assert_eq!(fs::read(&source.path).unwrap(), b"document");
    }

    #[test]
    fn index_repair_preserves_a_valid_latest_pointer_within_the_same_second() {
        let board = TestBoard::new();
        let thread = board.post("topic", &[]).unwrap();
        board.reply(&thread, "reply", &[]).unwrap();
        let (mut meta, posts) = load_thread(&board.0, &thread).unwrap();
        for mut post in posts {
            post.meta.created_at = "2026-09-10T01:00:00Z".into();
            write_post(&thread_dir(&board.0, &thread), &post.meta, &post.body).unwrap();
        }
        let (_, posts) = load_thread(&board.0, &thread).unwrap();
        meta.latest_post_id = posts.first().unwrap().meta.post_id.clone();
        write_thread_meta(&thread_dir(&board.0, &thread).join("thread.yaml"), &meta).unwrap();
        let (repaired, _) = load_thread(&board.0, &thread).unwrap();
        assert_eq!(repaired.latest_post_id, meta.latest_post_id);
        assert_eq!(repaired.updated_at, "2026-09-10T01:00:00Z");
    }

    #[test]
    fn cli_help_and_arguments_are_parsed_without_io() {
        for args in [
            vec![],
            vec!["help"],
            vec!["--help"],
            vec!["-h"],
            vec!["new", "--help"],
            vec!["reply", "help"],
            vec!["show", "-h"],
        ] {
            assert!(matches!(
                parse_cli_args(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).unwrap(),
                BbsCliCommand::Help
            ));
        }
        assert!(CLI_HELP.contains("1 GiB total"));
        assert!(CLI_HELP.contains("9 attachments"));
        for args in [
            vec!["new", "--broadcast", "extra"],
            vec!["reply", "thread-1", "extra"],
            vec!["show", "thread-1", "extra"],
            vec!["root", "extra"],
            vec!["new", "--projects"],
            vec!["new", "--broadcast", "--attach"],
            vec!["show", "../private"],
        ] {
            assert!(
                parse_cli_args(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err()
            );
        }
        let args = [
            "reply",
            "thread-1",
            "--attach",
            "./one.png",
            "--attach",
            "./two.pdf",
        ]
        .map(str::to_string);
        match parse_cli_args(&args).unwrap() {
            BbsCliCommand::Reply { attachments, .. } => assert_eq!(attachments.len(), 2),
            _ => panic!("expected reply"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_reply_and_delete_cannot_resurrect_a_thread() {
        for _ in 0..5 {
            let board = TestBoard::new();
            let thread = board.post("topic", &[]).unwrap();
            let root = board.0.clone();
            let reply_thread = thread.clone();
            let task = std::thread::spawn(move || {
                reply_at(
                    &root,
                    &human_identity("project", "User".into(), None),
                    &reply_thread,
                    "reply".into(),
                    &[],
                )
            });
            delete_item_at(&board.0, &thread, None).unwrap();
            let _ = task.join().unwrap();
            assert!(!thread_dir(&board.0, &thread).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_lock_is_released_by_process_exit_without_drop() {
        if let Some(root) = std::env::var_os("KOTA_BBS_LOCK_TEST_ROOT") {
            let _lock = acquire_write_lock(Path::new(&root)).unwrap();
            // exit does not run Rust Drop; the OS must release this descriptor's lock.
            std::process::exit(0);
        }
        let board = TestBoard::new();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bbs::tests::file_lock_is_released_by_process_exit_without_drop",
            ])
            .env("KOTA_BBS_LOCK_TEST_ROOT", &board.0)
            .status()
            .unwrap();
        assert!(status.success());
        let _lock = acquire_write_lock(&board.0).unwrap();
    }

    #[test]
    fn project_display_fallback_keeps_repo_dash_segments() {
        assert_eq!(
            display_project_name_with_fallback("stabruriss-kota-app", None),
            "Kota App"
        );
        assert_eq!(
            display_project_name_with_fallback("example-cedar-lantern", None),
            "Cedar Lantern"
        );
        assert_eq!(
            display_project_name_with_fallback("stabruriss-kota-app", Some("kota-app")),
            "Kota App"
        );
        assert_eq!(
            display_project_name_with_fallback("stabruriss-kota-app", Some("Kota App")),
            "Kota App"
        );
    }

    #[test]
    fn agent_avatar_from_yaml_reads_avatar_id() {
        let dir = temp_dir("agent-avatar");
        let path = dir.join("agent.yaml");
        fs::write(
            &path,
            "id: agent-one\ndisplay-name: Agent One\navatar-id: user:avatar-one\n",
        )
        .unwrap();

        assert_eq!(
            agent_avatar_from_yaml(&path).as_deref(),
            Some("user:avatar-one")
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_agent_avatar_from_yaml_reads_source_project_agent() {
        let workspaces = temp_dir("workspaces");
        let root = workspaces.join("bbs");
        let agent_dir = workspaces
            .join("source-project")
            .join(".agent-workspaces")
            .join("agent-one");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "id: agent-one\ndisplay-name: Agent One\navatar-id: user:avatar-one\n",
        )
        .unwrap();

        assert_eq!(
            project_agent_avatar_from_yaml(&root, "source-project", "agent-one").as_deref(),
            Some("user:avatar-one")
        );

        fs::remove_dir_all(workspaces).unwrap();
    }
}
