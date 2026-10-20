//! Dream-time export of new room messages into each active incarnation's episodes.
//! Ember calls it when the countdown starts; failures stay local to each incarnation.
use super::{
    episodes::{self, Episode},
    lock, now_ts, project_slug, read_jsonl, write_json_atomic, SoulPaths,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

/// Shorter messages (after envelopes, @mentions and whitespace) are neighbours only.
pub(crate) const MIN_INDEXED_CHARS: usize = 16;
pub(crate) const MESSAGE_W0: f64 = 0.5;
const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ExportReport {
    pub agents: Vec<AgentExport>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgentExport {
    pub agent_id: String,
    pub messages: usize,
    pub old_dreams: usize,
    pub error: Option<String>,
}

/// `episodes/state.json`; legacy old_dreams fields are ignored on read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct State {
    pub version: u32,
    #[serde(default)]
    pub current_project: Option<String>,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
}

impl Default for State {
    fn default() -> Self {
        Self { version: STATE_VERSION, current_project: None, projects: BTreeMap::new() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ProjectState {
    pub name: String,
    pub root: String,
    #[serde(default)]
    pub stints: Vec<Stint>,
    #[serde(default)]
    pub cursor: Cursor,
    #[serde(default)]
    pub last_export: Option<String>,
    #[serde(default)]
    pub episodes: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Stint {
    pub from: String,
    pub to: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Cursor {
    #[serde(default)]
    pub segment: Option<String>,
    #[serde(default)]
    pub violet_seq: u64,
}

pub(crate) fn read_state(paths: &SoulPaths) -> Result<State, String> {
    let path = paths.episodes_state();
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| format!("parse {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(err) => Err(format!("read {}: {err}", path.display())),
    }
}

pub(crate) fn write_state(paths: &SoulPaths, state: &State) -> Result<(), String> {
    write_json_atomic(&paths.episodes_state(), state)
}

/// The fields of one chathistory event this export reads; everything else streams past.
#[derive(Clone, Debug, Deserialize)]
struct RoomEvent {
    id: String,
    ts: String,
    #[serde(default)]
    violet_seq: Option<u64>,
    role: String,
    agent_id: String,
    #[serde(default)]
    shell: String,
    kind: String,
    #[serde(default = "shown")]
    display: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    overflow_path: Option<String>,
    #[serde(default)]
    target_agent_ids: Vec<String>,
    #[serde(default)]
    message_origin: Option<String>,
}

fn shown() -> bool {
    true
}

struct Segment {
    name: String,
    events: Vec<RoomEvent>,
}

/// Every day file of the project's chathistory, oldest first. No directory is no messages.
fn read_segments(project_root: &Path) -> Result<Vec<Segment>, String> {
    let dir = project_root.join("project-memory").join("chathistory").join("events");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read {}: {err}", dir.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|err| format!("read {}: {err}", dir.display()))?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut segments = Vec::new();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        segments.push(Segment { name, events: read_jsonl(&path)? });
    }
    Ok(segments)
}

enum Side {
    User(String),
    Own,
}

/// The whitelist (晴雯 10-15): a message whose origin allows user context and that names
/// this incarnation, by whichever role Violet recorded it under; or this incarnation's own
/// end-turn reply (assistant, message, its own session shell, no origin tag). Bus records
/// (`shell: system`), tagged system traffic and untagged legacy input stay out.
fn whitelisted(event: &RoomEvent, agent_id: &str) -> Option<Side> {
    if event.kind != "message" || !event.display {
        return None;
    }
    let targeted = event.agent_id == agent_id || event.target_agent_ids.iter().any(|target| target == agent_id);
    if let Some(origin) = event.message_origin.as_deref() {
        return (targeted && crate::message_origin::allows_user_context(Some(origin))).then(|| Side::User(origin.to_string()));
    }
    (event.role == "assistant" && event.agent_id == agent_id && event.shell != "system").then_some(Side::Own)
}

/// The whole reply when Violet kept it beside the preview.
fn full_text(project_root: &Path, event: &RoomEvent) -> String {
    if let Some(relative) = event.overflow_path.as_deref() {
        if let Ok(text) = fs::read_to_string(project_root.join(relative)) {
            return text;
        }
    }
    event.text.clone()
}

/// Host envelopes that may lead a message, in any order and number, before the text the
/// author wrote: `<NAME …>…</NAME>`. A `<KOTA_…>` anywhere else is the author quoting one
/// and stays as written (晴雯 10-15).
const HOST_ENVELOPES: [&str; 4] = ["KOTA_TEMPORAL_GAP", "KOTA_HANDOFF", "KOTA_MARKS", "KOTA_MESSAGE"];

/// Peels the host envelopes off the front of a message, keeping what the author wrote.
pub(crate) fn strip_envelopes(text: &str) -> String {
    let mut rest = text;
    loop {
        let lead = rest.trim_start();
        let Some(tag) = lead.strip_prefix('<') else { break };
        let Some(name) = HOST_ENVELOPES.iter().find(|name| {
            tag.strip_prefix(**name)
                .is_some_and(|after| after.starts_with(|ch: char| ch == '>' || ch.is_whitespace()))
        }) else {
            break;
        };
        let close = format!("</{name}>");
        let Some(close_at) = lead.find(&close) else { break };
        rest = &lead[close_at + close.len()..];
    }
    rest.trim().to_string()
}

/// What a whitelisted message contributes to the pool, if anything (座山雕 10-15). Leading
/// provider attachment markers (`[Image #3]`, Violet's rule) and host envelopes are peeled;
/// a message with nothing left is no episode. Nor is this incarnation's own Dream entry,
/// which never enters the pool. Neither occupies `prev`.
fn pooled_text(side: &Side, raw: &str) -> Option<String> {
    let peeled = strip_envelopes(crate::violet::strip_leading_provider_attachment_markers(raw));
    let text = crate::violet::strip_leading_provider_attachment_markers(&peeled);
    if text.is_empty() {
        return None;
    }
    let own_dream = matches!(side, Side::Own)
        && text.starts_with("<KOTA_DREAM_ENTRY")
        && text.ends_with("</KOTA_DREAM_ENTRY>");
    (!own_dream).then(|| text.to_string())
}

/// Characters that count toward the 16-character floor: no @mentions, no whitespace.
pub(crate) fn countable_chars(text: &str) -> usize {
    text.split_whitespace().filter(|word| !word.starts_with('@')).map(|word| word.chars().count()).sum()
}

/// Second precision, UTC, as every episode timestamp is written.
fn normalize_ts(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts.trim())
        .map(|t| t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|_| ts.trim().to_string())
}

fn message_episode(event: &RoomEvent, side: Side, project: &str, human: &str, prev: Option<String>, text: String) -> Episode {
    let indexed = countable_chars(&text) >= MIN_INDEXED_CHARS;
    let (role, origin, from) = match side {
        Side::User(origin) => ("user", origin, human.to_string()),
        Side::Own => ("assistant", "end_turn".to_string(), "self".to_string()),
    };
    Episode {
        id: event.id.clone(),
        kind: "message".into(),
        role: Some(role.into()),
        origin: Some(origin),
        project: Some(project.into()),
        ts: normalize_ts(&event.ts),
        from: Some(from),
        prev,
        text,
        indexed,
        w0: indexed.then_some(MESSAGE_W0),
        replaced_by: None,
    }
}

#[derive(Deserialize)]
struct AgentYaml {
    #[serde(default)]
    status: Option<String>,
}

/// The project's incarnations with `status: active`, by id.
pub(crate) fn active_incarnations(project_root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let dir = project_root.join(".agent-workspaces");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read {}: {err}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let cwd = entry.map_err(|err| format!("read {}: {err}", dir.display()))?.path();
        let Ok(text) = fs::read_to_string(cwd.join("agent.yaml")) else { continue };
        let Ok(agent) = serde_yaml::from_str::<AgentYaml>(&text) else { continue };
        if agent.status.as_deref() != Some("active") {
            continue;
        }
        if let Some(id) = cwd.file_name().map(|name| name.to_string_lossy().to_string()) {
            out.push((id, cwd));
        }
    }
    out.sort();
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    #[serde(default)]
    project_id: Option<String>,
}

/// The registered project id from `workspace.json`, when the project has one.
fn registered_project_id(project_root: &Path) -> Option<String> {
    let bytes = fs::read(project_root.join("workspace.json")).ok()?;
    serde_json::from_slice::<Registration>(&bytes).ok()?.project_id.filter(|id| !id.trim().is_empty())
}

/// Runs the export for one project with the configured human name and the clock.
pub(crate) fn export_after_dream(project_root: &Path, kota_home: &Path) -> Result<ExportReport, String> {
    let human = crate::ember::configured_account_user_display_name().unwrap_or_else(|| "user".into());
    export_with(project_root, kota_home, &human, &now_ts())
}

/// `Err` only when the project itself cannot be read; each incarnation's outcome is a
/// row of the report. Missing chathistory means no messages, never no export.
pub(crate) fn export_with(project_root: &Path, _kota_home: &Path, human: &str, now: &str) -> Result<ExportReport, String> {
    let root = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    let slug = project_slug(registered_project_id(&root).as_deref(), Some(&root)).unwrap_or_else(|| "project".into());
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| slug.clone());
    let segments = read_segments(&root)?;
    let mut report = ExportReport::default();
    for (agent_id, cwd) in active_incarnations(&root)? {
        let paths = SoulPaths::new(&cwd);
        let row = match export_agent(&paths, &agent_id, &root, &slug, &name, &segments, human, now) {
            Ok(messages) => AgentExport { agent_id, messages, old_dreams: 0, error: None },
            Err(error) => AgentExport { agent_id, messages: 0, old_dreams: 0, error: Some(error) },
        };
        report.agents.push(row);
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn export_agent(
    paths: &SoulPaths,
    agent_id: &str,
    root: &Path,
    slug: &str,
    name: &str,
    segments: &[Segment],
    human: &str,
    now: &str,
) -> Result<usize, String> {
    // Each run reads the cursors and the pool before it appends, so two at once would
    // pool the same messages twice.
    let _lock = lock(&paths.export_lock())?;
    let mut state = read_state(paths)?;
    let mut existing = read_jsonl::<Episode>(&paths.episodes())?.into_iter().map(|episode| episode.id).collect::<HashSet<_>>();
    state.current_project = Some(slug.to_string());
    let (first, since) = {
        let project = state.projects.entry(slug.to_string()).or_insert_with(|| ProjectState {
            name: name.to_string(),
            root: root.display().to_string(),
            stints: vec![Stint { from: now.to_string(), to: None }],
            ..ProjectState::default()
        });
        (project.cursor.segment.is_none(), project.cursor.violet_seq)
    };
    let mut batch = Vec::new();
    let mut max_seq = since;
    let mut last_segment = None;
    for segment in segments {
        // The neighbour is the previous message of the same day file that is in the pool,
        // exported before or now; one that contributed nothing does not count.
        let mut prev: Option<String> = None;
        for event in &segment.events {
            let Some(side) = whitelisted(event, agent_id) else { continue };
            let seq = event.violet_seq.unwrap_or(0);
            let due = first || seq > since;
            if due {
                max_seq = max_seq.max(seq);
                last_segment = Some(segment.name.clone());
            }
            let pooled = if due {
                match pooled_text(&side, &full_text(root, event)) {
                    Some(text) => {
                        if existing.insert(event.id.clone()) {
                            batch.push(message_episode(event, side, slug, human, prev.clone(), text));
                        }
                        true
                    }
                    None => false,
                }
            } else {
                existing.contains(&event.id)
            };
            if pooled {
                prev = Some(event.id.clone());
            }
        }
    }
    let messages = batch.len();
    if !batch.is_empty() {
        episodes::append(paths, &batch)?;
    }
    // The cursors move only after the pool took the lines.
    let project = state.projects.get_mut(slug).expect("project entry was inserted above");
    if let Some(segment) = last_segment {
        project.cursor = Cursor { segment: Some(segment), violet_seq: max_seq };
    }
    project.last_export = Some(now.to_string());
    project.episodes += messages as u64;
    write_state(paths, &state)?;
    Ok(messages)
}
