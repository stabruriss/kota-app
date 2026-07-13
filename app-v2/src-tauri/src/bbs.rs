use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const KOTA_HOME_DIR: &str = "Kota";
const KOTA_WORKSPACES_DIR: &str = "Workspaces";
const BBS_DIR: &str = "bbs";
const THREAD_SCHEMA: &str = "kota.bbs.thread.v1";
const POST_SCHEMA: &str = "kota.bbs.post.v1";

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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsHumanReplyRequest {
    pub project_id: String,
    #[serde(default)]
    pub project_display_name: Option<String>,
    pub thread_id: String,
    pub body: String,
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
    pub posts: Vec<BbsPostView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BbsPostView {
    pub post_id: String,
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
    let root = account_root();
    let state = load_project_state(&root, project_id)?;
    let mut threads = Vec::new();
    let mut new_count = 0usize;
    let mut avatar_cache = BTreeMap::new();
    for thread_id in read_thread_ids(&root)? {
        let Ok((meta, posts)) = load_thread(&root, &thread_id) else {
            continue;
        };
        if posts.is_empty() {
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
            post_views.push(BbsPostView {
                post_id: post.meta.post_id.clone(),
                thread_id: post.meta.thread_id.clone(),
                project_id: post.meta.project_id.clone(),
                project_display_name: display_project_name_with_fallback(
                    &post.meta.project_id,
                    Some(&post.meta.project_display_name),
                ),
                agent_id: post.meta.agent_id.clone(),
                agent_display_name: post.meta.agent_display_name.clone(),
                agent_avatar: post_agent_avatar(&root, &post.meta, &mut avatar_cache),
                created_at: post.meta.created_at.clone(),
                kind: post.meta.kind.clone(),
                preview: preview_markdown(&post.body),
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
            posts: post_views,
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

pub fn delete_item(thread_id: &str, post_id: Option<&str>) -> Result<()> {
    ensure_account_layout()?;
    let root = account_root();
    let thread_dir = thread_dir(&root, thread_id);
    if !thread_dir.is_dir() {
        bail!("BBS thread not found: {thread_id}");
    }
    if let Some(post_id) = post_id {
        let (mut meta, posts) = load_thread(&root, thread_id)?;
        let target = posts
            .iter()
            .find(|post| post.meta.post_id == post_id)
            .ok_or_else(|| anyhow!("BBS post not found: {post_id}"))?;
        if target.meta.kind == "topic" || posts.len() <= 1 {
            fs::remove_dir_all(&thread_dir)?;
            remove_thread_from_board(&root, thread_id)?;
            return Ok(());
        }
        fs::remove_file(&target.path)?;
        let mut remaining = posts
            .into_iter()
            .filter(|post| post.meta.post_id != post_id)
            .collect::<Vec<_>>();
        remaining.sort_by(|a, b| a.meta.created_at.cmp(&b.meta.created_at));
        if let Some(latest) = remaining.last() {
            meta.latest_post_id = latest.meta.post_id.clone();
            meta.updated_at = latest.meta.created_at.clone();
            write_thread_meta(&thread_dir.join("thread.yaml"), &meta)?;
        }
    } else {
        fs::remove_dir_all(&thread_dir)?;
        remove_thread_from_board(&root, thread_id)?;
    }
    Ok(())
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
    ensure_account_layout()?;
    let identity = identity.clone();
    let root = account_root();
    let thread_id = format!("thread-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let post_id = mint_post_id(&identity.agent_id);
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
    write_thread_meta(&thread_path.join("thread.yaml"), &meta)?;
    let post = BbsPostMeta {
        schema: POST_SCHEMA.into(),
        post_id,
        thread_id: thread_id.clone(),
        project_id: identity.project_id,
        agent_id: identity.agent_id,
        agent_display_name: identity.agent_display_name,
        agent_avatar: identity.agent_avatar,
        project_display_name: identity.project_display_name,
        created_at: now,
        kind: "topic".into(),
    };
    write_post(&thread_path, &post, &body)?;
    add_thread_to_board(&root, &thread_id)?;
    Ok(thread_id)
}

pub fn reply(thread_id: &str, body: String) -> Result<String> {
    reply_as(&identity_from_env(), thread_id, body)
}

pub fn reply_as(identity: &Identity, thread_id: &str, body: String) -> Result<String> {
    ensure_account_layout()?;
    let identity = identity.clone();
    let root = account_root();
    let thread_path = thread_dir(&root, thread_id);
    if !thread_path.is_dir() {
        bail!("BBS thread not found: {thread_id}");
    }
    let mut meta = read_thread_meta(&thread_path.join("thread.yaml"))?;
    let post_id = mint_post_id(&identity.agent_id);
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
    };
    write_post(&thread_path, &post, &body)?;
    meta.latest_post_id = post_id.clone();
    meta.updated_at = now;
    write_thread_meta(&thread_path.join("thread.yaml"), &meta)?;
    add_thread_to_board(&root, thread_id)?;
    Ok(post_id)
}

pub fn render_thread(thread_id: &str) -> Result<String> {
    ensure_account_layout()?;
    let root = account_root();
    let (meta, mut posts) = load_thread(&root, thread_id)?;
    posts.sort_by(|a, b| a.meta.created_at.cmp(&b.meta.created_at));
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
            "## {} / {} / {}\n\n{}\n\n",
            post.meta.project_display_name,
            post.meta.agent_display_name,
            post.meta.created_at,
            post.body.trim()
        ));
    }
    Ok(out)
}

pub fn run_cli() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_cli_usage();
        return Ok(());
    }
    let command = args.remove(0);
    match command.as_str() {
        "new" => {
            let mut broadcast = false;
            let mut projects = Vec::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--broadcast" => {
                        broadcast = true;
                        i += 1;
                    }
                    "--projects" => {
                        i += 1;
                        while i < args.len() && !args[i].starts_with("--") {
                            projects.extend(parse_project_list(&args[i]));
                            i += 1;
                        }
                    }
                    value => bail!("unknown kota-bbs new argument: {value}"),
                }
            }
            let body = read_stdin_body()?;
            if !broadcast && projects.is_empty() {
                bail!("kota-bbs new requires --projects <project-id> or --broadcast");
            }
            let thread_id = create_thread(projects, broadcast, body)?;
            println!("{thread_id}");
        }
        "reply" => {
            let thread_id = args
                .first()
                .ok_or_else(|| anyhow!("usage: kota-bbs reply <thread-id>"))?;
            let body = read_stdin_body()?;
            let post_id = reply(thread_id, body)?;
            println!("{post_id}");
        }
        "show" => {
            let thread_id = args
                .first()
                .ok_or_else(|| anyhow!("usage: kota-bbs show <thread-id>"))?;
            print!("{}", render_thread(thread_id)?);
        }
        "root" => {
            ensure_account_layout()?;
            println!("{}", account_root().display());
        }
        "install-shim" => {
            let path = install_cli_shim()?;
            println!("{}", path.display());
        }
        _ => {
            print_cli_usage();
            bail!("unknown kota-bbs command: {command}");
        }
    }
    Ok(())
}

fn print_cli_usage() {
    eprintln!("usage:");
    eprintln!("  kota-bbs new --projects <project-id> [project-id...]");
    eprintln!("  kota-bbs new --broadcast");
    eprintln!("  kota-bbs reply <thread-id>");
    eprintln!("  kota-bbs show <thread-id>");
    eprintln!("  kota-bbs install-shim");
}

fn read_stdin_body() -> Result<String> {
    let mut body = String::new();
    io::stdin().read_to_string(&mut body)?;
    let body = body.trim().to_string();
    if body.is_empty() {
        bail!("kota-bbs requires a non-empty Markdown body on stdin");
    }
    Ok(body)
}

fn read_thread_ids(root: &Path) -> Result<Vec<String>> {
    let board_path = root.join("board.json");
    let mut ids = if board_path.is_file() {
        serde_json::from_slice::<BbsBoardIndex>(&fs::read(&board_path)?)?.threads
    } else {
        Vec::new()
    };
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
    ids.retain(|id| seen.insert(id.clone()));
    Ok(ids)
}

fn load_thread(root: &Path, thread_id: &str) -> Result<(BbsThreadMeta, Vec<LoadedPost>)> {
    let dir = thread_dir(root, thread_id);
    let meta = read_thread_meta(&dir.join("thread.yaml"))?;
    let posts_dir = dir.join("posts");
    let mut posts = Vec::new();
    if posts_dir.is_dir() {
        for entry in fs::read_dir(posts_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            if let Ok(post) = read_post(&path) {
                posts.push(post);
            }
        }
    }
    posts.sort_by(|a, b| a.meta.created_at.cmp(&b.meta.created_at));
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
    if meta.schema != POST_SCHEMA {
        bail!("unsupported BBS post schema in {}", path.display());
    }
    Ok(LoadedPost {
        meta,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

fn write_post(thread_path: &Path, meta: &BbsPostMeta, body: &str) -> Result<()> {
    let path = thread_path
        .join("posts")
        .join(format!("{}.md", meta.post_id));
    let mut raw = String::new();
    raw.push_str("---\n");
    raw.push_str(&serde_yaml::to_string(meta)?);
    raw.push_str("---\n\n");
    raw.push_str(body.trim());
    raw.push('\n');
    write_bytes_atomic(&path, raw.as_bytes())
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

    #[test]
    fn project_display_fallback_keeps_repo_dash_segments() {
        assert_eq!(
            display_project_name_with_fallback("stabruriss-kota-app", None),
            "Kota App"
        );
        assert_eq!(
            display_project_name_with_fallback("stabruriss-one-alpha-docs", None),
            "One Alpha Docs"
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
