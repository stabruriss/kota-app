//! Violet — native-log normalizer + room chat reader.
//!
//! This MVP deliberately uses provider-native logs as the only content
//! source. `project-memory/raw_logs/` is normally an output cache; actor bus
//! raw logs are replayed as a repair source because those messages originate in
//! Kota rather than in provider-native transcripts.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chrono::{DateTime, Duration, TimeZone, Utc};
use notify::{RecursiveMode, Watcher};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(all(not(test), unix))]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(test))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration as StdDuration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

const ROOM_LIMIT_DEFAULT: usize = 200;
const SOURCE_EVENT_LIMIT: usize = 120;
const JSONL_TAIL_LINES: usize = 1_200;
const JSONL_TAIL_BYTES: u64 = 2 * 1024 * 1024;
const JSON_MESSAGE_TAIL: usize = 1_200;
const OPENCODE_LOG_TAIL_FILES: usize = 6;
const PARSED_EVENT_CACHE_LIMIT: usize = 2_000;
// Bump when parser changes must replay old sources. Keep stable for deliberately
// forward-only fixes that must not rematerialize frozen room history.
const PARSED_EVENT_CACHE_VERSION: &str = "8";
const ACTOR_RAW_REPLAY_CURSOR_VERSION: &str = "1";
const FIRST_VIOLET_SEQ: u64 = 1;
const MAX_SAFE_VIOLET_SEQ: u64 = (1u64 << 53) - 1;
const MAX_EVENT_TEXT_CHARS: usize = 10_000;
const CHATHISTORY_LATEST_LIMIT: usize = 500;
const PRIVATE_START_BUFFER_SECS: i64 = 2;
const PRIVATE_END_BUFFER_SECS: i64 = 30;
const VIOLET_ROOM_CHANGED_EVENT: &str = "violet://room/changed";
const VIOLET_WATCH_THROTTLE_MS: u64 = 500;
const OPENCODE_SQLITE_MONITOR_POLL_SECS: u64 = 2;
const OPENCODE_SQLITE_MONITOR_STOP_SLICE_MS: u64 = 250;
const CLAUDE_HOOK_SOURCE_KIND: &str = "claude-hook-jsonl";
const PI_SOURCE_KIND: &str = "pi-jsonl";
const KIMI_SOURCE_KIND: &str = "kimi-jsonl";
const ROOM_EXCEPTION_CONFIG: &str = include_str!("room-exceptions.json");
const ROOM_EXCEPTION_MAX_BASE64_CHARS: usize = 32 * 1024 * 1024;
const ROOM_EXCEPTION_MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;
const ROOM_EXCEPTION_MAX_IMAGE_DIMENSION: u32 = 8_192;
const ROOM_EXCEPTION_MAX_IMAGE_PIXELS: u64 = 32 * 1024 * 1024;
const ROOM_EXCEPTION_ARTIFACT_STORE_MAX_BYTES: u64 = 256 * 1024 * 1024;
const CODEX_ROOM_EXCEPTION_JSONL_TAIL_BYTES: u64 =
    ROOM_EXCEPTION_MAX_BASE64_CHARS as u64 + 2 * 1024 * 1024;
const VIOLET_SUMMARY_PROMPT_FILE: &str = "violet-summary.md";
const VIOLET_SUMMARY_PROMPT_TEMPLATE: &str = include_str!("../../prompts/violet-summary.md");
const VIOLET_SUMMARY_LOG_PATH: &str = "project-memory/chathistory/summaries/recent.json";
const VIOLET_SUMMARY_HISTORY_LIMIT: usize = 40;
const EMBER_DREAM_CONSOLIDATE_PROMPT_FILE: &str = "ember-dream-consolidate.md";
const EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE: &str =
    include_str!("../../prompts/ember-dream-consolidate.md");
const EMBER_DREAM_ENTRY_START: &str = "<KOTA_DREAM_ENTRY>";
const EMBER_DREAM_ENTRY_END: &str = "</KOTA_DREAM_ENTRY>";
const EMBER_DREAM_EMPTY_MARKER: &str = "__KOTA_DREAM_NONE__";
const TURN_ABORTED_ROOM_TEXT: &str = "interrupted the previous turn per human request";
const CODEX_INTERNAL_PROGRESS_FORMAT_WARNING: &str =
    "Codex emitted an unrecognized internal progress format. Raw event retained in native logs.";
const KIMI_UNKNOWN_EVENT_WARNING: &str =
    "Kimi Code emitted an unrecognized native event. Raw event retained in native logs.";
const EMBER_DREAM_MAX_ACTIVE_ENTRIES: usize = 15;
#[cfg(not(test))]
const VIOLET_SUMMARY_CLI_TIMEOUT_SECS: u64 = 225;
const VIOLET_SUMMARY_FAILURE_COOLDOWN_SECS: i64 = 15 * 60;
static WRITE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static ROOM_EXCEPTION_SKIPS: AtomicU64 = AtomicU64::new(0);

#[cfg(all(not(test), unix))]
const SIGKILL: i32 = 9;

#[cfg(all(not(test), unix))]
unsafe extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

static CODEX_SOURCE_CACHE: OnceLock<Mutex<HashMap<String, NativeSource>>> = OnceLock::new();
static ROOM_EXCEPTION_REGISTRY: OnceLock<Option<RoomExceptionRegistry>> = OnceLock::new();
static VIOLET_SUMMARY_RUNS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
static EMBER_DREAM_RUNS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
// Chathistory files are read-modify-written from Violet sync and actor bus paths.
// Serialize by project so one writer cannot overwrite another writer's fresh event.
static CHATHISTORY_WRITE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

#[cfg(unix)]
const CHATHISTORY_LOCK_EX: i32 = 2;
#[cfg(unix)]
const CHATHISTORY_LOCK_UN: i32 = 8;

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletRoomRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub agent_ids: Option<Vec<String>>,
    #[serde(default)]
    pub watch_agent_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletPrivacyRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    pub agent_id: String,
    pub private: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSummaryRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub config: Option<VioletSummaryConfig>,
    #[serde(default)]
    pub auto_run: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberDreamConsolidateRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub project_roots: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSummaryConfig {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub trigger_a_messages: Option<usize>,
    #[serde(default)]
    pub trigger_b_hours: Option<u64>,
    #[serde(default)]
    pub trigger_b_min_outstanding: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSummaryState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<VioletSummaryEntry>,
    pub history: Vec<VioletSummaryEntry>,
    pub outstanding: VioletSummaryOutstanding,
    pub log_path: String,
    pub prompt_path: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSummaryOutstanding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ts: Option<String>,
    pub message_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSummaryEntry {
    pub id: String,
    pub updated_at: String,
    pub trigger: String,
    pub provider: String,
    pub summary_start_ts: String,
    pub summary_end_ts: String,
    pub message_count: usize,
    pub completed: Vec<String>,
    pub last_event_id: String,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberDreamConsolidateState {
    pub account_dreams_path: String,
    pub entries_dir: String,
    pub old_dreams_path: String,
    pub prompt_path: String,
    pub processed_entry_count: usize,
    pub active_entry_count: usize,
    pub archived_entry_count: usize,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VioletSummaryLog {
    version: u32,
    updated_at: String,
    entries: Vec<VioletSummaryEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct VioletSummaryModelOutput {
    completed: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct EmberDreamConsolidationModelOutput {
    decisions: Vec<EmberDreamDecision>,
}

#[derive(Clone, Debug, Deserialize)]
struct EmberDreamDecision {
    id: String,
    op: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct EmberDreamPromptItem {
    id: String,
    kind: String,
    text: String,
}

#[derive(Clone, Debug)]
struct EmberDreamCandidateItem {
    id: String,
    project_index: usize,
    text: String,
}

#[derive(Clone, Debug)]
struct EmberDreamCandidateProposal {
    project_index: usize,
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EmberDreamDecisionAction {
    Keep,
    Drop,
    Rewrite(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EmberDreamApplyResult {
    active: Vec<String>,
    archived: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmberDreamEntryRecord {
    event_id: String,
    ts: String,
    agent_id: String,
    agent_display_name: Option<String>,
    text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletRoomState {
    pub messages: Vec<VioletChatMessage>,
    pub sources: Vec<VioletSourceStatus>,
    #[serde(default)]
    pub work_events: Vec<AgentWorkEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_bus_receipts: Vec<AgentBusReceipt>,
    pub raw_log_dir: String,
    pub chathistory_dir: String,
    pub synced_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletChatMessage {
    pub id: String,
    pub session_id: String,
    pub agent_id: String,
    pub shell: String,
    pub role: String,
    pub kind: String,
    pub timestamp: String,
    pub text: String,
    pub source_path: Option<String>,
    pub native_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub violet_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_agent_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_avatar_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletSourceStatus {
    pub agent_id: String,
    pub shell: String,
    pub session_id: Option<String>,
    pub source_kind: String,
    pub source_path: Option<String>,
    pub status: String,
    pub parsed: usize,
    pub written: usize,
    pub skipped_private: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkEvent {
    pub agent_id: String,
    pub state: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_event_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusReceipt {
    pub event_id: String,
    pub agent_id: String,
    pub timestamp: String,
}

#[derive(Clone, Debug)]
struct ProjectAgent {
    agent_id: String,
    shell: String,
    cwd: PathBuf,
    session_id: Option<String>,
}

#[derive(Clone, Debug)]
struct NativeSource {
    kind: String,
    session_id: String,
    path: PathBuf,
    aux_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoomExceptionRegistry {
    schema_version: u32,
    exceptions: Vec<RoomExceptionRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoomExceptionRule {
    id: String,
    provider: String,
    record_type: String,
    payload_type: String,
    source: RoomExceptionSource,
    action: RoomExceptionAction,
    reshape: RoomExceptionReshape,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoomExceptionSource {
    shape: RoomExceptionSourceShape,
    field: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RoomExceptionSourceShape {
    ScalarBase64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RoomExceptionAction {
    Reshape,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RoomExceptionReshape {
    Base64ImageToArtifact,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NativeEvent {
    session_id: String,
    agent_id: String,
    shell: String,
    role: String,
    kind: String,
    timestamp: String,
    text: String,
    source_path: PathBuf,
    native_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct AgentIdentitySnapshot {
    display_name: Option<String>,
    avatar_id: Option<String>,
    provider: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChathistoryEvent {
    id: String,
    ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    violet_seq: Option<u64>,
    role: String,
    agent_id: String,
    shell: String,
    kind: String,
    display: bool,
    agent_visible: bool,
    text: String,
    source: ChathistorySource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    target_agent_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_avatar_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChathistorySource {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_end: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    byte_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    byte_end: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VioletRoomChangedEvent {
    pub project_root: String,
    pub changed_at: String,
    pub reason: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ActorMessageRecord {
    pub actor_id: String,
    pub actor_name: String,
    pub text: String,
    pub target_agent_ids: Vec<String>,
    pub event_id: String,
    pub actor_intent: Option<String>,
}

#[derive(Clone, Default)]
pub struct VioletWatchManager {
    projects: Arc<Mutex<HashMap<PathBuf, VioletProjectWatch>>>,
}

struct VioletProjectWatch {
    _watcher: notify::RecommendedWatcher,
    watched_paths: HashSet<PathBuf>,
    opencode_monitor_specs: HashSet<OpencodeSqliteMonitorSpec>,
    _opencode_monitors: Vec<OpencodeSqliteMonitor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VioletWatchPlan {
    watched_paths: HashSet<PathBuf>,
    opencode_monitor_specs: HashSet<OpencodeSqliteMonitorSpec>,
}

impl VioletWatchPlan {
    fn is_empty(&self) -> bool {
        self.watched_paths.is_empty() && self.opencode_monitor_specs.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OpencodeSqliteMonitorSpec {
    db_path: PathBuf,
}

struct OpencodeSqliteMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for OpencodeSqliteMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SourceCursor {
    path: String,
    offset: u64,
    line_index: usize,
    updated_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActorRawReplayCursor {
    #[serde(default)]
    version: String,
    #[serde(default)]
    files: BTreeMap<String, ActorRawFileCursor>,
    #[serde(default)]
    updated_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActorRawFileCursor {
    offset: u64,
    modified_millis: u64,
}

struct ActorRawReplayBatch {
    messages: Vec<VioletChatMessage>,
    cursor_path: PathBuf,
    cursor: ActorRawReplayCursor,
    cursor_changed: bool,
}

struct JsonlReadResult {
    lines: Vec<(usize, String)>,
    next_offset: u64,
    next_line_index: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrivacySpan {
    agent_id: String,
    started_at: String,
    ended_at: Option<String>,
}

pub fn sync_project(
    project_root: &Path,
    request: VioletRoomRequest,
) -> Result<VioletRoomState, String> {
    let raw_log_dir = project_root.join("project-memory").join("raw_logs");
    let chathistory_dir = chathistory_dir(project_root);
    fs::create_dir_all(&raw_log_dir)
        .map_err(|err| format!("create {}: {err}", raw_log_dir.display()))?;
    ensure_chathistory_dirs(project_root)?;

    let requested_agent_ids = request
        .agent_ids
        .as_ref()
        .map(|ids| ids.iter().cloned().collect::<HashSet<_>>());
    let agents = discover_project_agents(project_root)?
        .into_iter()
        .filter(|agent| {
            requested_agent_ids
                .as_ref()
                .map_or(true, |ids| ids.contains(&agent.agent_id))
        })
        .collect::<Vec<_>>();
    let privacy_spans = load_privacy_spans(project_root)?;
    let mut fresh_room_messages = Vec::new();
    let mut sources = Vec::new();
    let mut work_events = Vec::new();
    let mut agent_bus_receipts = Vec::new();

    for agent in agents {
        let mut status = VioletSourceStatus {
            agent_id: agent.agent_id.clone(),
            shell: agent.shell.clone(),
            session_id: agent.session_id.clone(),
            source_kind: agent.shell.clone(),
            source_path: None,
            status: "missing".into(),
            parsed: 0,
            written: 0,
            skipped_private: 0,
            error: None,
        };

        let mut parsed = Vec::new();
        let mut output_session_id = None;
        let mut parse_error = None;
        let native_source = match locate_native_source_for_project(project_root, &agent) {
            Ok(source) => source,
            Err(err) => {
                parse_error = Some(err);
                None
            }
        };

        if let Some(source) = native_source.as_ref() {
            output_session_id = Some(source.session_id.clone());
            status.session_id = Some(source.session_id.clone());
            status.source_kind = source.kind.clone();
            status.source_path = Some(path_string(&source.path));
            match parse_source(project_root, &agent, source, &privacy_spans) {
                Ok(mut events) => parsed.append(&mut events),
                Err(err) => parse_error = Some(err),
            }
        }

        let hook_source = native_source
            .as_ref()
            .and_then(|source| locate_claude_hook_source(project_root, &agent, &source.session_id))
            .or_else(|| {
                agent.session_id.as_deref().and_then(|session_id| {
                    locate_claude_hook_source(project_root, &agent, session_id)
                })
            });
        if parse_error.is_none() {
            if let Some(source) = hook_source.as_ref() {
                if output_session_id.is_none() {
                    output_session_id = Some(source.session_id.clone());
                    status.session_id = Some(source.session_id.clone());
                    status.source_kind = source.kind.clone();
                    status.source_path = Some(path_string(&source.path));
                }
                match parse_claude_hook_source(project_root, &agent, source) {
                    Ok(mut events) => parsed.append(&mut events),
                    Err(err) => parse_error = Some(err),
                }
            }
        }

        if let Some(err) = parse_error {
            status.status = "error".into();
            status.error = Some(err);
        } else if let Some(session_id) = output_session_id {
            status.parsed = parsed.len();
            agent_bus_receipts.extend(parsed.iter().filter_map(agent_bus_receipt_from_event));
            let parsed = filter_internal_agent_bus_envelopes(filter_bootstrap_noise(parsed));
            work_events.extend(parsed.iter().filter_map(native_work_event));
            let (visible, skipped) = partition_private(parsed, &privacy_spans);
            let visible = dedupe_native_events(visible);
            let visible = tail_events(visible, SOURCE_EVENT_LIMIT);
            let (room_events, shared_events) = split_for_violet_outputs(visible, project_root);
            status.skipped_private = skipped;
            if room_events.is_empty() && shared_events.is_empty() {
                remove_raw_session_output(project_root, &session_id)?;
                status.status = "empty".into();
            } else {
                if shared_events.is_empty() {
                    remove_raw_session_output(project_root, &session_id)?;
                } else {
                    write_session_outputs(project_root, &session_id, &shared_events)?;
                }
                status.written = shared_events.len();
                status.status = "synced".into();
                fresh_room_messages.extend(room_events.into_iter().map(event_to_message));
            }
        }

        sources.push(status);
    }

    if !fresh_room_messages.is_empty() {
        write_chathistory_messages(project_root, &fresh_room_messages)?;
        if let Err(err) = write_turn_credit_events_for_messages(project_root, &fresh_room_messages)
        {
            eprintln!(
                "Kota Violet turn credit sync failed for {}: {err}",
                project_root.display()
            );
        }
    }
    sync_actor_raw_messages_to_chathistory(project_root)?;
    let messages = read_room_messages(project_root, &request)?;
    dedupe_agent_bus_receipts(&mut agent_bus_receipts);

    Ok(VioletRoomState {
        messages,
        sources,
        work_events,
        agent_bus_receipts,
        raw_log_dir: path_string(&raw_log_dir),
        chathistory_dir: path_string(&chathistory_dir),
        synced_at: Utc::now().to_rfc3339(),
    })
}

pub fn read_cache(
    project_root: &Path,
    request: VioletRoomRequest,
) -> Result<VioletRoomState, String> {
    let raw_log_dir = project_root.join("project-memory").join("raw_logs");
    let chathistory_dir = chathistory_dir(project_root);
    let messages = read_room_messages(project_root, &request)?;

    Ok(VioletRoomState {
        messages,
        sources: Vec::new(),
        work_events: Vec::new(),
        agent_bus_receipts: Vec::new(),
        raw_log_dir: path_string(&raw_log_dir),
        chathistory_dir: path_string(&chathistory_dir),
        synced_at: Utc::now().to_rfc3339(),
    })
}

fn agent_bus_receipt_from_event(event: &NativeEvent) -> Option<AgentBusReceipt> {
    if event.role != "user" || event.kind != "message" {
        return None;
    }
    let event_id = agent_bus_envelope_event_id(&event.text)?;
    if !event_id.starts_with("agentbus-") {
        return None;
    }
    Some(AgentBusReceipt {
        event_id,
        agent_id: event.agent_id.clone(),
        timestamp: event.timestamp.clone(),
    })
}

fn dedupe_agent_bus_receipts(receipts: &mut Vec<AgentBusReceipt>) {
    receipts.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then(a.agent_id.cmp(&b.agent_id))
            .then(a.event_id.cmp(&b.event_id))
    });
    receipts.dedup_by(|a, b| a.event_id == b.event_id && a.agent_id == b.agent_id);
}

fn agent_bus_envelope_event_id(text: &str) -> Option<String> {
    let trimmed = trim_terminal_envelope_padding(strip_leading_provider_attachment_markers(text));
    if !is_agent_bus_envelope_text(trimmed) {
        return None;
    }
    let tag_end = trimmed.find('>')?;
    let tag = &trimmed[..tag_end];
    let id_start = tag.find("id=")? + 3;
    let rest = tag[id_start..].trim_start();
    let (quote, rest) = match rest.as_bytes().first().copied() {
        Some(b'"') => ('"', &rest[1..]),
        Some(b'\'') => ('\'', &rest[1..]),
        _ => (' ', rest),
    };
    let value = if quote == ' ' {
        rest.split_whitespace().next().unwrap_or_default()
    } else {
        rest.split(quote).next().unwrap_or_default()
    };
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn summary_status(
    project_root: &Path,
    request: VioletSummaryRequest,
) -> Result<VioletSummaryState, String> {
    ensure_chathistory_dirs(project_root)?;
    // Backward compatibility only: status is intentionally a pure read.
    let _ = request.auto_run;
    build_summary_state(project_root, None)
}

pub fn summarize_now(
    project_root: &Path,
    request: VioletSummaryRequest,
) -> Result<VioletSummaryState, String> {
    ensure_chathistory_dirs(project_root)?;
    let config = normalized_summary_config(request.config.as_ref());
    let Some(_guard) = try_acquire_summary_run(project_root)? else {
        return build_summary_state(
            project_root,
            Some("Violet summary is already running.".into()),
        );
    };
    match run_summary_for_outstanding(project_root, &config, "manual") {
        Ok(_) => build_summary_state(project_root, None),
        Err(err) => build_summary_state(project_root, Some(err)),
    }
}

pub fn summarize_auto(
    project_root: &Path,
    request: VioletSummaryRequest,
) -> Result<VioletSummaryState, String> {
    ensure_chathistory_dirs(project_root)?;
    let config = normalized_summary_config(request.config.as_ref());
    let state = build_summary_state(project_root, None)?;
    if let Some(error) = recent_summary_failure_cooldown(project_root, &state, &config)? {
        return build_summary_state(project_root, Some(error));
    }
    let Some(trigger) = automatic_summary_trigger(&state, &config) else {
        return Ok(state);
    };
    let Some(_guard) = try_acquire_summary_run(project_root)? else {
        return Ok(state);
    };
    match run_summary_for_outstanding(project_root, &config, trigger) {
        Ok(_) => build_summary_state(project_root, None),
        Err(err) => build_summary_state(project_root, Some(err)),
    }
}

pub fn consolidate_ember_dreams(
    project_roots: &[PathBuf],
    request: EmberDreamConsolidateRequest,
) -> Result<EmberDreamConsolidateState, String> {
    if project_roots.is_empty() {
        return Err("Ember dream consolidation requires at least one project root.".into());
    }
    for project_root in project_roots {
        ensure_chathistory_dirs(project_root)?;
    }
    let Some(_guard) = try_acquire_ember_dream_run()? else {
        return build_ember_dream_state(
            0,
            0,
            Some("Ember dream consolidation is already running.".into()),
        );
    };
    for project_root in project_roots {
        let _ = sync_project(
            project_root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(1),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        );
    }
    match run_ember_dream_consolidation(project_roots, &request) {
        Ok((processed, archived)) => build_ember_dream_state(processed, archived, None),
        Err(err) => build_ember_dream_state(0, 0, Some(err)),
    }
}

fn read_room_messages(
    project_root: &Path,
    request: &VioletRoomRequest,
) -> Result<Vec<VioletChatMessage>, String> {
    read_chathistory_messages(project_root, request)
}

fn apply_room_request_window(
    messages: Vec<VioletChatMessage>,
    request: &VioletRoomRequest,
) -> Vec<VioletChatMessage> {
    let mut messages = dedupe_room_messages(messages);
    messages.sort_by(compare_room_message_order);
    if let Some(before) = request
        .before
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        messages.retain(|message| message_timestamp_before(&message.timestamp, before));
    }
    let limit = request.limit.unwrap_or(ROOM_LIMIT_DEFAULT).max(1);
    if messages.len() > limit {
        messages.split_off(messages.len() - limit)
    } else {
        messages
    }
}

fn message_timestamp_before(timestamp: &str, before: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(timestamp),
        DateTime::parse_from_rfc3339(before),
    ) {
        (Ok(timestamp), Ok(before)) => timestamp < before,
        _ => timestamp < before,
    }
}

pub fn set_privacy(project_root: &Path, request: VioletPrivacyRequest) -> Result<(), String> {
    let violet_dir = project_root.join("project-memory").join(".violet");
    fs::create_dir_all(&violet_dir)
        .map_err(|err| format!("create {}: {err}", violet_dir.display()))?;
    let path = violet_dir.join("privacy-spans.jsonl");
    let mut spans = load_privacy_spans(project_root)?;
    let has_active = spans
        .iter()
        .any(|span| span.agent_id == request.agent_id && span.ended_at.is_none());
    if request.private {
        if !has_active {
            spans.push(PrivacySpan {
                agent_id: request.agent_id,
                started_at: Utc::now().to_rfc3339(),
                ended_at: None,
            });
        }
    } else {
        let ended_at = (Utc::now() + Duration::seconds(PRIVATE_END_BUFFER_SECS)).to_rfc3339();
        for span in spans
            .iter_mut()
            .filter(|span| span.agent_id == request.agent_id && span.ended_at.is_none())
        {
            span.ended_at = Some(ended_at.clone());
        }
    }
    let mut out = String::new();
    for span in spans {
        out.push_str(
            &serde_json::to_string(&span)
                .map_err(|err| format!("serialize privacy span: {err}"))?,
        );
        out.push('\n');
    }
    fs::write(&path, out).map_err(|err| format!("write {}: {err}", path.display()))
}

pub fn record_actor_message(
    project_root: &Path,
    record: &ActorMessageRecord,
) -> Result<Vec<PathBuf>, String> {
    let timestamp = Utc::now().to_rfc3339();
    let session_id = format!("actor-{}", sanitize_path_segment(&record.actor_id));
    let raw_dir = project_root.join("project-memory").join("raw_logs");
    fs::create_dir_all(&raw_dir).map_err(|err| format!("create {}: {err}", raw_dir.display()))?;
    let source_path = project_root
        .join("project-memory")
        .join(".violet")
        .join("actor-messages");
    let target_line = if record.target_agent_ids.is_empty() {
        String::new()
    } else {
        format!(
            "- target_agent_ids: {}\n",
            record.target_agent_ids.join(",")
        )
    };
    let actor_intent_line = record
        .actor_intent
        .as_deref()
        .map(|intent| format!("- actor_intent: {intent}\n"))
        .unwrap_or_default();
    let raw_block = format!(
        "## {timestamp} · {} · system · session {session_id}\n\nAssistant:\n{}\n\nMetadata:\n- agent_id: {}\n- actor_name: {}\n- shell: system\n- native_log: {}\n- kind: message\n- native_event_id: {}\n{}{}",
        record.actor_id,
        record.text.trim(),
        record.actor_id,
        record.actor_name,
        source_path.display(),
        record.event_id,
        actor_intent_line,
        target_line,
    );
    let raw_block = format!("{raw_block}\n");
    let raw_path = raw_dir.join(format!("{session_id}.md"));
    append_text(&raw_path, &raw_block)?;
    let message = VioletChatMessage {
        id: stable_message_id(&session_id, &record.actor_id, &timestamp, &record.text),
        session_id,
        agent_id: record.actor_id.clone(),
        shell: "system".into(),
        role: "assistant".into(),
        kind: "message".into(),
        timestamp,
        text: record.text.trim().to_string(),
        source_path: Some(path_string(&source_path)),
        native_event_id: Some(record.event_id.clone()),
        violet_seq: None,
        actor_intent: record.actor_intent.clone(),
        target_agent_ids: record.target_agent_ids.clone(),
        agent_display_name: Some(record.actor_name.clone()),
        agent_avatar_id: Some(record.actor_id.clone()),
        agent_provider: Some("system".into()),
        agent_status: None,
    };
    let mut changed_paths = vec![raw_path];
    changed_paths.extend(write_chathistory_messages(project_root, &[message])?);
    Ok(changed_paths)
}

pub fn actor_event_exists(project_root: &Path, event_id: &str) -> Result<bool, String> {
    if event_id.trim().is_empty() {
        return Ok(false);
    }
    let raw_dir = project_root.join("project-memory").join("raw_logs");
    if !raw_dir.is_dir() {
        return Ok(false);
    }
    let needle = format!("- native_event_id: {event_id}");
    let mut paths = Vec::new();
    collect_matching_files(&raw_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("md")
    })?;
    for path in paths {
        let text =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        if text.lines().any(|line| line.trim() == needle) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sync_actor_raw_messages_to_chathistory(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut batch = read_actor_raw_log_messages(project_root)?;
    let changed_paths = if batch.messages.is_empty() {
        Vec::new()
    } else {
        write_chathistory_messages(project_root, &batch.messages)?
    };
    if batch.cursor_changed {
        batch.cursor.updated_at = now_iso();
        write_actor_raw_replay_cursor(&batch.cursor_path, &batch.cursor)?;
    }
    Ok(changed_paths)
}

pub fn emit_room_changed(app: &AppHandle, project_root: &Path, reason: &str, paths: Vec<PathBuf>) {
    let payload = VioletRoomChangedEvent {
        project_root: path_string(project_root),
        changed_at: now_iso(),
        reason: reason.to_string(),
        paths: paths.iter().map(|path| path_string(path)).collect(),
    };
    let _ = app.emit(VIOLET_ROOM_CHANGED_EVENT, payload);
}

fn append_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|err| format!("write {}: {err}", path.display()))
}

struct VioletWatchThrottle {
    last_emit: Instant,
    pending_paths: HashSet<PathBuf>,
    trailing_scheduled: bool,
}

impl VioletWatchThrottle {
    fn new() -> Self {
        Self {
            last_emit: Instant::now()
                .checked_sub(StdDuration::from_secs(60))
                .unwrap_or_else(Instant::now),
            pending_paths: HashSet::new(),
            trailing_scheduled: false,
        }
    }
}

fn emit_violet_room_changed(
    app: &AppHandle,
    project_root: &str,
    paths: Vec<PathBuf>,
    source: &str,
) {
    log_violet_watch_debug(&format!(
        "[violet-watch-debug] emit source={} project={} paths={}",
        source,
        project_root,
        paths.len()
    ));
    let payload = VioletRoomChangedEvent {
        project_root: project_root.to_string(),
        changed_at: now_iso(),
        reason: "native-log-or-agent-manifest".into(),
        paths: paths.iter().map(|path| path_string(path)).collect(),
    };
    let _ = app.emit(VIOLET_ROOM_CHANGED_EVENT, payload);
}

fn schedule_violet_watch_trailing_emit(
    throttle: Arc<Mutex<VioletWatchThrottle>>,
    app: AppHandle,
    project_root: String,
    initial_delay: StdDuration,
) {
    thread::spawn(move || {
        let interval = StdDuration::from_millis(VIOLET_WATCH_THROTTLE_MS);
        let mut delay = initial_delay;
        loop {
            thread::sleep(delay);
            let mut emit_paths: Option<Vec<PathBuf>> = None;
            let mut next_delay = interval;
            let mut done = false;
            if let Ok(mut throttle) = throttle.lock() {
                if throttle.pending_paths.is_empty() {
                    throttle.trailing_scheduled = false;
                    return;
                }
                let now = Instant::now();
                let elapsed = now.duration_since(throttle.last_emit);
                if elapsed < interval {
                    next_delay = interval.saturating_sub(elapsed);
                    log_violet_watch_debug(&format!(
                        "[violet-watch-debug] trailing-wait project={} pending_paths={} next_in_ms={}",
                        project_root,
                        throttle.pending_paths.len(),
                        next_delay.as_millis()
                    ));
                } else {
                    throttle.last_emit = now;
                    emit_paths = Some(throttle.pending_paths.drain().collect());
                    throttle.trailing_scheduled = false;
                    done = true;
                }
            } else {
                return;
            }
            if let Some(paths) = emit_paths {
                emit_violet_room_changed(&app, &project_root, paths, "trailing");
            }
            if done {
                return;
            }
            delay = next_delay;
        }
    });
}

#[cfg(debug_assertions)]
fn log_violet_watch_debug(message: &str) {
    crate::kota_debug_log(message);
}

#[cfg(not(debug_assertions))]
fn log_violet_watch_debug(_message: &str) {}

impl VioletWatchManager {
    pub fn refresh_project(
        &self,
        app: &AppHandle,
        project_root: &Path,
        agent_ids: Option<&[String]>,
    ) -> Result<(), String> {
        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let watch_plan = collect_violet_watch_plan(&key, agent_ids)?;
        let mut projects = self
            .projects
            .lock()
            .map_err(|_| "violet watch manager poisoned".to_string())?;
        if projects.get(&key).map_or(false, |existing| {
            existing.watched_paths == watch_plan.watched_paths
                && existing.opencode_monitor_specs == watch_plan.opencode_monitor_specs
        }) {
            return Ok(());
        }
        if watch_plan.is_empty() {
            let previous = projects.remove(&key);
            drop(projects);
            drop(previous);
            return Ok(());
        }

        let app_handle = app.clone();
        let project_root_string = path_string(project_root);
        let watch_throttle = Arc::new(Mutex::new(VioletWatchThrottle::new()));
        let watch_throttle_for_callback = Arc::clone(&watch_throttle);
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            if event.paths.is_empty() {
                return;
            }
            let now = Instant::now();
            let paths = event.paths;
            let throttle_interval = StdDuration::from_millis(VIOLET_WATCH_THROTTLE_MS);
            let mut emit_paths: Option<Vec<PathBuf>> = None;
            let mut schedule_trailing = false;
            let mut trailing_delay = throttle_interval;
            if let Ok(mut throttle) = watch_throttle_for_callback.lock() {
                let elapsed = now.duration_since(throttle.last_emit);
                if elapsed >= throttle_interval {
                    throttle.last_emit = now;
                    let mut all_paths = std::mem::take(&mut throttle.pending_paths);
                    all_paths.extend(paths);
                    emit_paths = Some(all_paths.into_iter().collect());
                } else {
                    for path in paths {
                        throttle.pending_paths.insert(path);
                    }
                    trailing_delay = throttle_interval.saturating_sub(elapsed);
                    if !throttle.trailing_scheduled {
                        throttle.trailing_scheduled = true;
                        schedule_trailing = true;
                    }
                    log_violet_watch_debug(&format!(
                        "[violet-watch-debug] throttled project={} pending_paths={} trailing_in_ms={}",
                        project_root_string,
                        throttle.pending_paths.len(),
                        trailing_delay.as_millis()
                    ));
                }
            }
            if let Some(paths) = emit_paths {
                emit_violet_room_changed(&app_handle, &project_root_string, paths, "immediate");
            }
            if schedule_trailing {
                schedule_violet_watch_trailing_emit(
                    Arc::clone(&watch_throttle_for_callback),
                    app_handle.clone(),
                    project_root_string.clone(),
                    trailing_delay,
                );
            }
        })
        .map_err(|err| format!("create Violet watcher: {err}"))?;

        for path in &watch_plan.watched_paths {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(|err| format!("watch {}: {err}", path.display()))?;
        }
        let opencode_monitors = watch_plan
            .opencode_monitor_specs
            .iter()
            .cloned()
            .map(|spec| start_opencode_sqlite_monitor(app.clone(), key.clone(), spec))
            .collect::<Vec<_>>();
        let previous = projects.insert(
            key,
            VioletProjectWatch {
                _watcher: watcher,
                watched_paths: watch_plan.watched_paths,
                opencode_monitor_specs: watch_plan.opencode_monitor_specs,
                _opencode_monitors: opencode_monitors,
            },
        );
        drop(projects);
        drop(previous);
        Ok(())
    }
}

fn collect_violet_watch_plan(
    project_root: &Path,
    agent_ids: Option<&[String]>,
) -> Result<VioletWatchPlan, String> {
    let mut plan = VioletWatchPlan::default();
    let requested_agent_ids = agent_ids.map(|ids| ids.iter().cloned().collect::<HashSet<_>>());
    for agent in discover_project_agents(project_root)? {
        if requested_agent_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(&agent.agent_id))
        {
            continue;
        }
        if !is_supported_violet_shell(&agent.shell) {
            continue;
        }
        let source = if native_source_should_refresh_for_watch(&agent.shell) {
            locate_native_source_for_project(project_root, &agent)?
        } else {
            let source = cached_project_native_source(project_root, &agent)?;
            if source.is_none() {
                locate_native_source_for_project(project_root, &agent)?
            } else {
                source
            }
        };
        if let Some(source) = source {
            collect_violet_source_watch_paths(&agent, &source, &mut plan);
        } else {
            collect_violet_fallback_watch_paths(&agent, &mut plan)?;
        }
        if agent.shell == "claude" {
            collect_claude_hook_watch_paths(project_root, &mut plan.watched_paths);
        }
    }
    Ok(plan)
}

fn collect_violet_source_watch_paths(
    agent: &ProjectAgent,
    source: &NativeSource,
    plan: &mut VioletWatchPlan,
) {
    insert_existing_watch_path(&mut plan.watched_paths, &source.path);
    match agent.shell.as_str() {
        "claude" => collect_claude_watch_paths(&agent.cwd, &source.path, &mut plan.watched_paths),
        "codex" => collect_codex_watch_paths(&source.path, &mut plan.watched_paths),
        "antigravity" => collect_antigravity_watch_paths(&source.path, &mut plan.watched_paths),
        "opencode" => {
            collect_opencode_watch_paths(&source.path, &mut plan.watched_paths);
            if source.kind == "opencode-sqlite" {
                collect_opencode_sqlite_monitor(&source.path, &mut plan.opencode_monitor_specs);
            }
        }
        "pi" => collect_pi_watch_paths(&agent.cwd, &source.path, &mut plan.watched_paths),
        "kimi" => collect_kimi_watch_paths(&source.path, &mut plan.watched_paths),
        _ => {}
    }
}

fn native_source_should_refresh_for_watch(shell: &str) -> bool {
    matches!(shell, "claude" | "codex" | "kimi")
}

fn collect_claude_hook_watch_paths(project_root: &Path, paths: &mut HashSet<PathBuf>) {
    let dir = claude_hook_dir(project_root);
    if fs::create_dir_all(&dir).is_ok() {
        insert_existing_watch_path(paths, &dir);
    }
}

fn collect_violet_fallback_watch_paths(
    agent: &ProjectAgent,
    plan: &mut VioletWatchPlan,
) -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    match agent.shell.as_str() {
        "claude" => collect_claude_fallback_watch_paths(&home, &agent.cwd, &mut plan.watched_paths),
        "antigravity" => collect_antigravity_fallback_watch_paths(&home, &mut plan.watched_paths),
        "opencode" => {
            let db_path = opencode_db_path(&home);
            collect_opencode_watch_paths(&db_path, &mut plan.watched_paths);
            collect_opencode_sqlite_monitor(&db_path, &mut plan.opencode_monitor_specs);
        }
        "pi" => collect_pi_fallback_watch_paths(&home, &agent.cwd, &mut plan.watched_paths),
        "kimi" => collect_kimi_fallback_watch_paths(&home, &mut plan.watched_paths),
        _ => {}
    }
    Ok(())
}

fn collect_claude_watch_paths(cwd: &Path, source_path: &Path, paths: &mut HashSet<PathBuf>) {
    if let Some(parent) = source_path.parent() {
        insert_existing_watch_path(paths, parent);
    }
    if let Some(home) = dirs::home_dir() {
        collect_claude_fallback_watch_paths(&home, cwd, paths);
    }
}

fn collect_claude_fallback_watch_paths(home: &Path, cwd: &Path, paths: &mut HashSet<PathBuf>) {
    insert_existing_watch_path(paths, claude_project_dir(home, cwd));
    insert_existing_watch_path(paths, home.join(".claude").join("transcripts"));
}

fn collect_antigravity_watch_paths(path: &Path, paths: &mut HashSet<PathBuf>) {
    if let Some(parent) = path.parent() {
        insert_existing_watch_path(paths, parent);
    }
    if let Some(home) = dirs::home_dir() {
        collect_antigravity_fallback_watch_paths(&home, paths);
    }
}

fn collect_antigravity_fallback_watch_paths(home: &Path, paths: &mut HashSet<PathBuf>) {
    let app_dir = antigravity_app_dir(home);
    insert_existing_watch_path(paths, app_dir.join("cache").join("last_conversations.json"));
    insert_existing_watch_path(paths, app_dir.join("log"));
    insert_existing_watch_path(paths, app_dir.join("brain"));
}

fn collect_opencode_watch_paths(db_path: &Path, paths: &mut HashSet<PathBuf>) {
    insert_existing_watch_path(paths, db_path);
    insert_existing_watch_path(paths, db_path.with_file_name("opencode.db-wal"));
    insert_existing_watch_path(paths, db_path.with_file_name("opencode.db-shm"));
    if let Some(parent) = db_path.parent() {
        insert_existing_watch_path(paths, parent);
        insert_existing_watch_path(paths, parent.join("log"));
    }
}

fn collect_pi_watch_paths(cwd: &Path, source_path: &Path, paths: &mut HashSet<PathBuf>) {
    insert_existing_watch_path(paths, source_path);
    if let Some(parent) = source_path.parent() {
        insert_existing_watch_path(paths, parent);
    }
    if let Some(home) = dirs::home_dir() {
        collect_pi_fallback_watch_paths(&home, cwd, paths);
    }
}

fn collect_pi_fallback_watch_paths(home: &Path, cwd: &Path, paths: &mut HashSet<PathBuf>) {
    let sessions_dir = pi_sessions_dir(home);
    insert_existing_watch_path(paths, &sessions_dir);
    insert_existing_watch_path(paths, pi_project_session_dir(&sessions_dir, cwd));
}

fn collect_kimi_watch_paths(source_path: &Path, paths: &mut HashSet<PathBuf>) {
    insert_existing_watch_path(paths, source_path);
    let kimi_home = dirs::home_dir().map(|home| kimi_code_home_from(&home));
    if let Some(kimi_home) = kimi_home.as_ref() {
        insert_existing_watch_path(paths, kimi_home.join("workspaces.json"));
        insert_existing_watch_path(paths, kimi_home.join("sessions"));
    }
    let mut current = source_path.parent();
    while let Some(dir) = current {
        insert_existing_watch_path(paths, dir);
        if kimi_home
            .as_ref()
            .is_some_and(|kimi_home| dir == kimi_home || !dir.starts_with(kimi_home))
        {
            break;
        }
        current = dir.parent();
    }
}

fn collect_kimi_fallback_watch_paths(home: &Path, paths: &mut HashSet<PathBuf>) {
    let kimi_home = kimi_code_home_from(home);
    insert_existing_watch_path(paths, &kimi_home);
    insert_existing_watch_path(paths, kimi_home.join("workspaces.json"));
    insert_existing_watch_path(paths, kimi_home.join("sessions"));
}

fn collect_opencode_sqlite_monitor(
    db_path: &Path,
    monitors: &mut HashSet<OpencodeSqliteMonitorSpec>,
) {
    if !db_path.is_file() {
        return;
    }
    let db_path = db_path
        .canonicalize()
        .unwrap_or_else(|_| db_path.to_path_buf());
    monitors.insert(OpencodeSqliteMonitorSpec { db_path });
}

fn start_opencode_sqlite_monitor(
    app: AppHandle,
    project_root: PathBuf,
    spec: OpencodeSqliteMonitorSpec,
) -> OpencodeSqliteMonitor {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let db_path = spec.db_path.clone();
    let handle = thread::spawn(move || {
        run_opencode_sqlite_monitor(app, project_root, db_path, stop_for_thread);
    });
    OpencodeSqliteMonitor {
        stop,
        handle: Some(handle),
    }
}

fn run_opencode_sqlite_monitor(
    app: AppHandle,
    project_root: PathBuf,
    db_path: PathBuf,
    stop: Arc<AtomicBool>,
) {
    let mut conn = None;
    let mut last_data_version = None;
    let mut last_error = None::<String>;
    while !stop.load(Ordering::Relaxed) {
        if conn.is_none() {
            match open_opencode_monitor_db(&db_path) {
                Ok(opened) => {
                    conn = Some(opened);
                    last_data_version = None;
                    last_error = None;
                }
                Err(err) => {
                    log_opencode_monitor_error(&mut last_error, &db_path, &err);
                    sleep_opencode_sqlite_monitor(&stop, OPENCODE_SQLITE_MONITOR_POLL_SECS);
                    continue;
                }
            }
        }

        let data_version = match conn.as_ref() {
            Some(opened) => opencode_data_version(opened),
            None => continue,
        };
        match data_version {
            Ok(version) => {
                last_error = None;
                let changed = last_data_version.is_some_and(|last| last != version);
                last_data_version = Some(version);
                if changed {
                    emit_room_changed(
                        &app,
                        &project_root,
                        "opencode-sqlite-data-version",
                        vec![db_path.clone()],
                    );
                }
            }
            Err(err) => {
                log_opencode_monitor_error(&mut last_error, &db_path, &err);
                conn = None;
                last_data_version = None;
            }
        }

        sleep_opencode_sqlite_monitor(&stop, OPENCODE_SQLITE_MONITOR_POLL_SECS);
    }
}

fn sleep_opencode_sqlite_monitor(stop: &AtomicBool, seconds: u64) {
    let deadline = Instant::now() + StdDuration::from_secs(seconds);
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now);
        thread::sleep(remaining.min(StdDuration::from_millis(
            OPENCODE_SQLITE_MONITOR_STOP_SLICE_MS,
        )));
    }
}

fn log_opencode_monitor_error(last_error: &mut Option<String>, db_path: &Path, err: &str) {
    if last_error.as_deref() == Some(err) {
        return;
    }
    *last_error = Some(err.to_string());
    crate::kota_debug_log(&format!(
        "[violet] opencode sqlite monitor {}: {}",
        db_path.display(),
        err
    ));
}

fn collect_codex_watch_paths(path: &Path, paths: &mut HashSet<PathBuf>) {
    insert_existing_watch_path(paths, path);
    let Some(home) = dirs::home_dir() else {
        if let Some(parent) = path.parent() {
            insert_existing_watch_path(paths, parent);
        }
        return;
    };
    let sessions_dir = home.join(".codex").join("sessions");
    if !path.starts_with(&sessions_dir) {
        if let Some(parent) = path.parent() {
            insert_existing_watch_path(paths, parent);
        }
        return;
    }
    insert_existing_watch_path(paths, &sessions_dir);
    let mut current = path.parent();
    while let Some(dir) = current {
        insert_existing_watch_path(paths, dir);
        if paths_same(dir, &sessions_dir) {
            break;
        }
        current = dir.parent();
    }
}

fn insert_existing_watch_path(paths: &mut HashSet<PathBuf>, path: impl AsRef<Path>) {
    let path = path.as_ref();
    if path.is_file() || path.is_dir() {
        paths.insert(path.to_path_buf());
    }
}

fn is_supported_violet_shell(shell: &str) -> bool {
    matches!(
        shell,
        "claude" | "codex" | "antigravity" | "opencode" | "pi" | "kimi"
    )
}

fn discover_project_agents(project_root: &Path) -> Result<Vec<ProjectAgent>, String> {
    let dir = project_root.join(".agent-workspaces");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut agents = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
        let cwd = entry.path();
        if !cwd.is_dir() {
            continue;
        }
        let yaml_path = cwd.join("agent.yaml");
        if !yaml_path.is_file() {
            continue;
        }
        let yaml_text = fs::read_to_string(&yaml_path)
            .map_err(|err| format!("read {}: {err}", yaml_path.display()))?;
        let yaml: YamlValue = serde_yaml::from_str(&yaml_text)
            .map_err(|err| format!("parse {}: {err}", yaml_path.display()))?;
        let status = yaml_string(&yaml, "status").unwrap_or_else(|| "active".into());
        let status = status.trim().to_ascii_lowercase();
        if status == "archived" || status == "dismissed" || status == "removed" {
            continue;
        }
        let agent_id = yaml_string(&yaml, "id")
            .or_else(|| {
                cwd.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "agent".into());
        let shell_yaml = read_yaml_file(&cwd.join("SHELL.yaml"))?;
        let shell = normalize_shell(
            &yaml_string(&yaml, "shell")
                .or_else(|| yaml_string(&yaml, "provider"))
                .or_else(|| {
                    shell_yaml
                        .as_ref()
                        .and_then(|yaml| yaml_string(yaml, "provider"))
                })
                .or_else(|| {
                    shell_yaml
                        .as_ref()
                        .and_then(|yaml| yaml_string(yaml, "command"))
                })
                .unwrap_or_else(|| "codex".into()),
        );
        let session_id =
            yaml_string(&yaml, "session-id").or_else(|| yaml_string(&yaml, "sessionId"));
        agents.push(ProjectAgent {
            agent_id,
            shell,
            cwd,
            session_id,
        });
    }
    agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    Ok(agents)
}

fn locate_native_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    match agent.shell.as_str() {
        "claude" => locate_claude_source(agent),
        "codex" => locate_codex_source(agent),
        "antigravity" => locate_antigravity_source(agent),
        "opencode" => locate_opencode_source(agent),
        "pi" => locate_pi_source(agent),
        "kimi" => locate_kimi_source(agent),
        other => Err(format!("unsupported Violet shell source: {other}")),
    }
}

fn locate_native_source_for_project(
    project_root: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    if agent.shell == "codex" {
        if let Some(source) = locate_native_source(agent)? {
            if native_source_is_after_agent_session_reset(agent, &source) {
                store_project_native_source(agent, &source);
                persist_agent_session_binding(agent, &source);
                return Ok(Some(source));
            }
        }
        if let Some(source) = cached_codex_source(agent)? {
            if native_source_is_after_agent_session_reset(agent, &source) {
                persist_agent_session_binding(agent, &source);
                return Ok(Some(source));
            }
        }
        return Ok(cached_native_source_from_raw_logs(project_root, agent)?
            .filter(|source| native_source_is_after_agent_session_reset(agent, source)));
    }

    let source = locate_native_source(agent)?;
    if let Some(source) = source.as_ref() {
        if native_source_is_after_agent_session_reset(agent, source) {
            store_project_native_source(agent, source);
            persist_agent_session_binding(agent, source);
            return Ok(Some(source.clone()));
        }
    }
    Ok(cached_native_source_from_raw_logs(project_root, agent)?
        .filter(|source| native_source_is_after_agent_session_reset(agent, source)))
}

fn cached_project_native_source(
    project_root: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    if agent.shell == "codex" {
        if let Some(source) = cached_codex_source(agent)? {
            return Ok(Some(source));
        }
    }
    cached_native_source_from_raw_logs(project_root, agent)
}

fn store_project_native_source(agent: &ProjectAgent, source: &NativeSource) {
    if agent.shell == "codex" {
        store_codex_source(agent, source);
    }
}

fn persist_agent_session_binding(agent: &ProjectAgent, source: &NativeSource) {
    if agent.session_id.as_deref() == Some(source.session_id.as_str()) {
        return;
    }
    if let Err(err) = write_agent_session_binding(agent, source) {
        crate::kota_debug_log(&format!(
            "[violet] failed to persist session binding for {}: {}",
            agent.agent_id, err
        ));
    }
}

fn write_agent_session_binding(agent: &ProjectAgent, source: &NativeSource) -> Result<(), String> {
    let path = agent.cwd.join("agent.yaml");
    if !path.is_file() {
        return Ok(());
    }
    let text =
        fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let mut yaml: YamlValue =
        serde_yaml::from_str(&text).map_err(|err| format!("parse {}: {err}", path.display()))?;
    let existing_session_id =
        yaml_string(&yaml, "session-id").or_else(|| yaml_string(&yaml, "sessionId"));
    if existing_session_id.as_deref() == Some(source.session_id.as_str()) {
        return Ok(());
    }
    let YamlValue::Mapping(map) = &mut yaml else {
        return Err(format!("agent.yaml is not a mapping: {}", path.display()));
    };
    yaml_mapping_set_string(map, "session-id", &source.session_id);
    yaml_mapping_set_string(map, "session-source", "native");
    yaml_mapping_set_string(map, "session-updated-at", &now_iso());
    yaml_mapping_remove(map, "sessionId");
    yaml_mapping_remove(map, "sessionSource");
    yaml_mapping_remove(map, "sessionUpdatedAt");
    yaml_mapping_remove(map, "session-reset-at");
    yaml_mapping_remove(map, "sessionResetAt");
    let next =
        serde_yaml::to_string(&yaml).map_err(|err| format!("serialize agent.yaml: {err}"))?;
    if next != text {
        write_if_changed(&path, next.as_bytes())?;
    }
    Ok(())
}

fn native_source_is_after_agent_session_reset(agent: &ProjectAgent, source: &NativeSource) -> bool {
    let Some(cutoff) = agent_session_reset_cutoff(agent) else {
        return true;
    };
    file_modified_time(&source.path) > cutoff
}

fn agent_session_reset_cutoff(agent: &ProjectAgent) -> Option<SystemTime> {
    let path = agent.cwd.join("agent.yaml");
    let text = fs::read_to_string(&path).ok()?;
    let yaml: YamlValue = serde_yaml::from_str(&text).ok()?;
    let raw =
        yaml_string(&yaml, "session-reset-at").or_else(|| yaml_string(&yaml, "sessionResetAt"))?;
    let timestamp = DateTime::parse_from_rfc3339(&raw).ok()?;
    let timestamp = timestamp.with_timezone(&Utc);
    let secs = timestamp.timestamp();
    if secs < 0 {
        return None;
    }
    Some(SystemTime::UNIX_EPOCH + StdDuration::new(secs as u64, timestamp.timestamp_subsec_nanos()))
}

fn locate_claude_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    locate_claude_source_in(&home, agent)
}

fn locate_claude_source_in(
    home: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    let project_dir = claude_project_dir(home, &agent.cwd);
    if let Some((_, path)) = latest_file_by_mtime(&project_dir, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })? {
        let session_id = file_stem(&path).unwrap_or_else(|| source_session_id(&path));
        return Ok(Some(NativeSource {
            kind: "claude-jsonl".into(),
            session_id,
            path,
            aux_path: None,
        }));
    }
    if let Some(session_id) = agent.session_id.as_deref() {
        for path in [
            project_dir.join(format!("{session_id}.jsonl")),
            home.join(".claude")
                .join("transcripts")
                .join(format!("{session_id}.jsonl")),
        ] {
            if path.is_file() {
                return Ok(Some(NativeSource {
                    kind: "claude-jsonl".into(),
                    session_id: session_id.to_string(),
                    path,
                    aux_path: None,
                }));
            }
        }
    }
    Ok(None)
}

fn claude_project_dir(home: &Path, cwd: &Path) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(claude_project_dir_name(cwd))
}

fn locate_claude_hook_source(
    project_root: &Path,
    agent: &ProjectAgent,
    session_id: &str,
) -> Option<NativeSource> {
    if agent.shell != "claude" {
        return None;
    }
    let path = claude_hook_log_path(project_root, &agent.agent_id);
    path.is_file().then(|| NativeSource {
        kind: CLAUDE_HOOK_SOURCE_KIND.into(),
        session_id: session_id.to_string(),
        path,
        aux_path: None,
    })
}

fn claude_hook_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("claude-hooks")
}

fn claude_hook_log_path(project_root: &Path, agent_id: &str) -> PathBuf {
    claude_hook_dir(project_root).join(format!("{}.jsonl", sanitize_path_segment(agent_id)))
}

fn locate_codex_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let sessions_dir = home.join(".codex").join("sessions");
    locate_codex_source_in(&sessions_dir, agent)
}

fn locate_codex_source_in(
    sessions_dir: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    if !sessions_dir.is_dir() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    collect_codex_source_candidates(&sessions_dir, &mut candidates)?;
    candidates.sort_by(|(left_modified, left_path), (right_modified, right_path)| {
        right_modified
            .cmp(left_modified)
            .then_with(|| right_path.cmp(left_path))
    });

    let mut requested_source = None;
    for (_, path) in candidates {
        let Some(meta) = read_codex_session_meta(&path)? else {
            continue;
        };
        if meta.is_subagent {
            continue;
        }
        let source = NativeSource {
            kind: "codex-jsonl".into(),
            session_id: meta.session_id,
            path,
            aux_path: None,
        };
        if paths_same(&meta.cwd, &agent.cwd) {
            store_codex_source(agent, &source);
            return Ok(Some(source));
        }
        if agent.session_id.as_deref() == Some(source.session_id.as_str()) {
            requested_source = Some(source);
        }
    }
    if let Some(source) = requested_source {
        store_codex_source(agent, &source);
        return Ok(Some(source));
    }
    Ok(None)
}

fn cached_codex_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(cache) = CODEX_SOURCE_CACHE.get() else {
        return Ok(None);
    };
    for key in codex_source_cache_keys(agent) {
        let source = cache
            .lock()
            .map_err(|_| "Codex source cache poisoned".to_string())?
            .get(&key)
            .cloned();
        let Some(source) = source else {
            continue;
        };
        if !source.path.is_file() {
            continue;
        }
        let Some(meta) = read_codex_session_meta(&source.path)? else {
            continue;
        };
        if meta.is_subagent || meta.session_id != source.session_id {
            continue;
        }
        if !paths_same(&meta.cwd, &agent.cwd)
            && agent.session_id.as_deref() != Some(meta.session_id.as_str())
        {
            continue;
        }
        return Ok(Some(source));
    }
    Ok(None)
}

fn store_codex_source(agent: &ProjectAgent, source: &NativeSource) {
    let cache = CODEX_SOURCE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        for key in codex_source_cache_keys(agent) {
            cache.insert(key, source.clone());
        }
        cache.insert(format!("session:{}", source.session_id), source.clone());
    }
}

fn codex_source_cache_keys(agent: &ProjectAgent) -> Vec<String> {
    let mut keys = Vec::new();
    let cwd = agent
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| agent.cwd.clone());
    keys.push(format!("cwd:{}", path_string(&cwd)));
    if let Some(session_id) = agent.session_id.as_deref() {
        keys.push(format!("session:{session_id}"));
    }
    keys
}

fn cached_native_source_from_raw_logs(
    project_root: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    let raw_log_dir = project_root.join("project-memory").join("raw_logs");
    if !raw_log_dir.is_dir() {
        return Ok(None);
    }

    let mut paths = Vec::new();
    collect_matching_files(&raw_log_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("md")
    })?;

    let mut best: Option<(String, NativeSource)> = None;
    for path in paths {
        let text =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        for block in split_normalized_raw_blocks(&text) {
            let Some((timestamp, agent_id, shell, session_id, metadata)) =
                normalized_raw_block_source_parts(block)
            else {
                continue;
            };
            if agent_id != agent.agent_id || shell != agent.shell {
                continue;
            }
            let Some(native_log) = metadata_value(metadata, "native_log") else {
                continue;
            };
            let source_path = PathBuf::from(native_log);
            if !source_path.is_file() {
                continue;
            }
            if agent.shell == "codex" {
                let Some(meta) = read_codex_session_meta(&source_path)? else {
                    continue;
                };
                if meta.is_subagent
                    || meta.session_id != session_id
                    || !paths_same(&meta.cwd, &agent.cwd)
                {
                    continue;
                }
            }
            let source = NativeSource {
                kind: native_source_kind_for_shell(&shell),
                session_id,
                path: source_path,
                aux_path: None,
            };
            if best
                .as_ref()
                .map_or(true, |(best_timestamp, _)| timestamp > *best_timestamp)
            {
                best = Some((timestamp, source));
            }
        }
    }

    let source = best.map(|(_, source)| source);
    if let Some(source) = source.as_ref() {
        store_project_native_source(agent, source);
    }
    Ok(source)
}

fn normalized_raw_block_source_parts(
    block: &str,
) -> Option<(String, String, String, String, &str)> {
    let block = block.trim_start_matches("## ").trim();
    let (header, body) = block.split_once("\n\n")?;
    if !is_normalized_raw_header(header) {
        return None;
    }
    let mut header_parts = header.split(" · ");
    let timestamp = header_parts.next()?.trim().to_string();
    let agent_id = header_parts.next()?.trim().to_string();
    let shell = header_parts.next()?.trim().to_string();
    let session = header_parts.next()?.trim();
    let session_id = session.strip_prefix("session ")?.trim().to_string();
    let (_, metadata) = body.split_once("\n\nMetadata:\n")?;
    Some((timestamp, agent_id, shell, session_id, metadata))
}

fn native_source_kind_for_shell(shell: &str) -> String {
    match shell {
        "claude" => "claude-jsonl",
        "codex" => "codex-jsonl",
        "antigravity" => "antigravity-jsonl",
        "opencode" => "opencode-sqlite",
        "pi" => PI_SOURCE_KIND,
        "kimi" => KIMI_SOURCE_KIND,
        other => other,
    }
    .to_string()
}

fn collect_codex_source_candidates(
    dir: &Path,
    candidates: &mut Vec<(SystemTime, PathBuf)>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|err| format!("stat {}: {err}", path.display()))?;
        if metadata.is_dir() {
            collect_codex_source_candidates(&path, candidates)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        candidates.push((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), path));
    }
    Ok(())
}

fn antigravity_app_dir(home: &Path) -> PathBuf {
    home.join(".gemini").join("antigravity-cli")
}

fn antigravity_transcript_path(app_dir: &Path, conversation_id: &str) -> PathBuf {
    antigravity_transcript_path_named(app_dir, conversation_id, "transcript.jsonl")
}

fn antigravity_full_transcript_path(app_dir: &Path, conversation_id: &str) -> PathBuf {
    antigravity_transcript_path_named(app_dir, conversation_id, "transcript_full.jsonl")
}

fn antigravity_transcript_path_named(
    app_dir: &Path,
    conversation_id: &str,
    file_name: &str,
) -> PathBuf {
    app_dir
        .join("brain")
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join(file_name)
}

fn preferred_antigravity_transcript_path(app_dir: &Path, conversation_id: &str) -> PathBuf {
    let full = antigravity_full_transcript_path(app_dir, conversation_id);
    if full.is_file() {
        return full;
    }
    antigravity_transcript_path(app_dir, conversation_id)
}

fn is_antigravity_transcript_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("transcript_full.jsonl" | "transcript.jsonl")
    )
}

fn locate_antigravity_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let app_dir = antigravity_app_dir(&home);
    let cwd_candidates = antigravity_cwd_candidates(agent);

    if let Some(conversation_id) = antigravity_conversation_id_from_logs(&app_dir, &cwd_candidates)?
    {
        let path = preferred_antigravity_transcript_path(&app_dir, &conversation_id);
        return Ok(Some(NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: conversation_id,
            path,
            aux_path: None,
        }));
    }

    if let Some(session_id) = agent.session_id.as_deref() {
        let path = preferred_antigravity_transcript_path(&app_dir, session_id);
        if path.is_file() {
            return Ok(Some(NativeSource {
                kind: "antigravity-jsonl".into(),
                session_id: session_id.to_string(),
                path,
                aux_path: None,
            }));
        }
    }

    for cwd in &cwd_candidates {
        if let Some(conversation_id) = antigravity_conversation_id_for_cwd(&app_dir, &cwd)? {
            let path = preferred_antigravity_transcript_path(&app_dir, &conversation_id);
            if path.is_file() {
                return Ok(Some(NativeSource {
                    kind: "antigravity-jsonl".into(),
                    session_id: conversation_id,
                    path,
                    aux_path: None,
                }));
            }
        }
    }

    let brain_dir = app_dir.join("brain");
    let mut candidates = Vec::new();
    collect_matching_files(&brain_dir, &mut candidates, is_antigravity_transcript_file)?;
    candidates.sort_by(|left, right| {
        file_modified_time(right)
            .cmp(&file_modified_time(left))
            .then_with(|| right.cmp(left))
    });
    let mut seen_sessions = HashSet::new();
    let mut unique_candidates = Vec::new();
    for path in candidates {
        let session_id = antigravity_conversation_id_from_transcript(&path)
            .unwrap_or_else(|| source_session_id(&path));
        if seen_sessions.insert(session_id.clone()) {
            unique_candidates.push(preferred_antigravity_transcript_path(&app_dir, &session_id));
        }
    }
    let candidates = unique_candidates;
    for path in &candidates {
        if antigravity_transcript_mentions_any_cwd(path, &cwd_candidates)? {
            let session_id = antigravity_conversation_id_from_transcript(path)
                .unwrap_or_else(|| source_session_id(path));
            let path = preferred_antigravity_transcript_path(&app_dir, &session_id);
            return Ok(Some(NativeSource {
                kind: "antigravity-jsonl".into(),
                session_id,
                path,
                aux_path: None,
            }));
        }
    }
    if candidates.len() != 1 {
        return Ok(None);
    }
    let Some(path) = candidates.into_iter().next() else {
        return Ok(None);
    };
    let session_id = antigravity_conversation_id_from_transcript(&path)
        .unwrap_or_else(|| source_session_id(&path));
    let path = preferred_antigravity_transcript_path(&app_dir, &session_id);
    Ok(Some(NativeSource {
        kind: "antigravity-jsonl".into(),
        session_id,
        path,
        aux_path: None,
    }))
}

fn antigravity_conversation_id_from_logs(
    app_dir: &Path,
    cwd_candidates: &[PathBuf],
) -> Result<Option<String>, String> {
    if cwd_candidates.is_empty() {
        return Ok(None);
    }
    let log_dir = app_dir.join("log");
    let mut logs = Vec::new();
    collect_matching_files(&log_dir, &mut logs, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("log")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .map_or(false, |name| name.starts_with("cli-"))
    })?;
    logs.sort_by(|left, right| {
        file_modified_time(right)
            .cmp(&file_modified_time(left))
            .then_with(|| right.cmp(left))
    });

    for path in logs.into_iter().take(128) {
        let Some(conversation_id) = antigravity_conversation_id_from_log(&path, cwd_candidates)?
        else {
            continue;
        };
        if preferred_antigravity_transcript_path(app_dir, &conversation_id).is_file() {
            return Ok(Some(conversation_id));
        }
    }
    Ok(None)
}

fn antigravity_conversation_id_from_log(
    path: &Path,
    cwd_candidates: &[PathBuf],
) -> Result<Option<String>, String> {
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut workspace_matches = false;
    let mut latest_conversation_id = None;
    for line in reader.lines() {
        let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
        if let Some((_, workspace)) =
            line.split_once("Initializing CLI store manager for workspace ")
        {
            workspace_matches = antigravity_log_workspace_matches(workspace.trim(), cwd_candidates);
            if !workspace_matches {
                latest_conversation_id = None;
            }
            continue;
        }
        if !workspace_matches {
            continue;
        }
        if let Some(conversation_id) = antigravity_conversation_id_from_log_line(&line) {
            latest_conversation_id = Some(conversation_id);
        }
    }
    Ok(latest_conversation_id)
}

fn antigravity_log_workspace_matches(raw_workspace: &str, cwd_candidates: &[PathBuf]) -> bool {
    let workspace = raw_workspace.trim_matches(['"', '\'']);
    cwd_candidates.iter().any(|cwd| {
        workspace == path_string(cwd)
            || cwd
                .canonicalize()
                .ok()
                .map_or(false, |canonical| workspace == path_string(&canonical))
            || paths_same(Path::new(workspace), cwd)
    })
}

fn antigravity_conversation_id_from_log_line(line: &str) -> Option<String> {
    for marker in [
        "Created conversation ",
        "Streaming conversation ",
        "Forwarding user message to conversation ",
        "Sending user message to conversation ",
        "Print mode: conversation=",
    ] {
        let Some((_, tail)) = line.split_once(marker) else {
            continue;
        };
        if let Some(conversation_id) = antigravity_log_conversation_token(tail) {
            return Some(conversation_id);
        }
    }
    None
}

fn antigravity_log_conversation_token(raw: &str) -> Option<String> {
    let token = raw
        .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'')
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect::<String>();
    (token.len() >= 8 && token.contains('-')).then_some(token)
}

fn antigravity_cwd_candidates(agent: &ProjectAgent) -> Vec<PathBuf> {
    let mut paths = vec![agent.cwd.clone()];
    if let Some(launch_cwd) = antigravity_launch_workspace_for_agent(agent) {
        if !paths.iter().any(|path| paths_same(path, &launch_cwd)) {
            paths.push(launch_cwd);
        }
    }
    paths
}

fn antigravity_launch_workspace_for_agent(agent: &ProjectAgent) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let agent_id = agent
        .cwd
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&agent.agent_id);
    let project_root = agent.cwd.parent()?.parent()?;
    let project_segment = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_path_segment)
        .unwrap_or_else(|| "project".into());
    Some(
        home.join("Kota")
            .join("AgentWorkspaces")
            .join(project_segment)
            .join(sanitize_path_segment(agent_id)),
    )
}

fn antigravity_conversation_id_for_cwd(
    app_dir: &Path,
    cwd: &Path,
) -> Result<Option<String>, String> {
    let path = app_dir.join("cache").join("last_conversations.json");
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let json: JsonValue =
        serde_json::from_str(&text).map_err(|err| format!("parse {}: {err}", path.display()))?;
    let Some(map) = json.as_object() else {
        return Ok(None);
    };
    for (raw_cwd, value) in map {
        if paths_same(Path::new(raw_cwd), cwd) {
            if let Some(conversation_id) = value.as_str().filter(|value| !value.trim().is_empty()) {
                return Ok(Some(conversation_id.to_string()));
            }
        }
    }
    Ok(None)
}

fn antigravity_transcript_mentions_any_cwd(
    path: &Path,
    cwd_candidates: &[PathBuf],
) -> Result<bool, String> {
    if cwd_candidates.is_empty() {
        return Ok(false);
    }
    let mut needles = HashSet::new();
    for cwd in cwd_candidates {
        needles.insert(path_string(cwd));
        if let Ok(canonical) = cwd.canonicalize() {
            needles.insert(path_string(&canonical));
        }
    }
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().take(80) {
        let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
        if needles.iter().any(|needle| line.contains(needle)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sanitize_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.trim_matches('_').is_empty() {
        "item".into()
    } else {
        sanitized
    }
}

fn antigravity_conversation_id_from_transcript(path: &Path) -> Option<String> {
    path.parent()?
        .parent()?
        .parent()?
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn file_modified_time(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn opencode_db_path(home: &Path) -> PathBuf {
    home.join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}

fn locate_opencode_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let db_path = opencode_db_path(&home);
    if !db_path.is_file() {
        return Ok(None);
    }
    let session_id = latest_opencode_session_id(&agent.cwd)?.or(agent.session_id.clone());
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    Ok(Some(NativeSource {
        kind: "opencode-sqlite".into(),
        session_id,
        path: db_path,
        aux_path: None,
    }))
}

fn locate_pi_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let sessions_dir = pi_sessions_dir(&home);
    if !sessions_dir.is_dir() {
        return Ok(None);
    }
    if let Some(session_id) = agent.session_id.as_deref() {
        if let Some(path) = locate_pi_session_file_by_id(&sessions_dir, &agent.cwd, session_id)? {
            return Ok(Some(NativeSource {
                kind: PI_SOURCE_KIND.into(),
                session_id: session_id.to_string(),
                path,
                aux_path: None,
            }));
        }
    }
    if let Some((session_id, path)) = latest_pi_session_file(&sessions_dir, &agent.cwd)? {
        return Ok(Some(NativeSource {
            kind: PI_SOURCE_KIND.into(),
            session_id,
            path,
            aux_path: None,
        }));
    }
    Ok(None)
}

fn locate_kimi_source(agent: &ProjectAgent) -> Result<Option<NativeSource>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    locate_kimi_source_in(&kimi_code_home_from(&home), agent)
}

fn kimi_code_home_from(home: &Path) -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".kimi-code"))
}

fn locate_kimi_source_in(
    kimi_home: &Path,
    agent: &ProjectAgent,
) -> Result<Option<NativeSource>, String> {
    let workspace_dirs = kimi_workspace_session_dirs(kimi_home, &agent.cwd)?;
    if let Some(session_id) = agent
        .session_id
        .as_deref()
        .filter(|session_id| !session_id.trim().is_empty())
    {
        for workspace_dir in &workspace_dirs {
            let session_dir = workspace_dir.join(session_id);
            let wire = session_dir.join("agents").join("main").join("wire.jsonl");
            if wire.is_file() && kimi_state_matches_cwd(&session_dir, &agent.cwd)? {
                return Ok(Some(NativeSource {
                    kind: KIMI_SOURCE_KIND.into(),
                    session_id: session_id.to_string(),
                    path: wire,
                    aux_path: None,
                }));
            }
        }
    }

    let mut candidates = Vec::new();
    for workspace_dir in workspace_dirs {
        let entries = fs::read_dir(&workspace_dir)
            .map_err(|err| format!("read {}: {err}", workspace_dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|err| format!("read {} entry: {err}", workspace_dir.display()))?;
            let session_dir = entry.path();
            if !session_dir.is_dir() || !kimi_state_matches_cwd(&session_dir, &agent.cwd)? {
                continue;
            }
            let Some(session_id) = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| name.starts_with("session_"))
                .map(str::to_string)
            else {
                continue;
            };
            let wire = session_dir.join("agents").join("main").join("wire.jsonl");
            if wire.is_file() {
                candidates.push((file_modified_time(&wire), session_id, wire));
            }
        }
    }
    candidates.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    Ok(candidates
        .into_iter()
        .next()
        .map(|(_, session_id, path)| NativeSource {
            kind: KIMI_SOURCE_KIND.into(),
            session_id,
            path,
            aux_path: None,
        }))
}

fn kimi_workspace_session_dirs(kimi_home: &Path, cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let sessions_dir = kimi_home.join("sessions");
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let index_path = kimi_home.join("workspaces.json");
    if index_path.is_file() {
        let text = fs::read_to_string(&index_path)
            .map_err(|err| format!("read {}: {err}", index_path.display()))?;
        let index: JsonValue = serde_json::from_str(&text)
            .map_err(|err| format!("parse {}: {err}", index_path.display()))?;
        if let Some(workspaces) = index.get("workspaces").and_then(JsonValue::as_object) {
            for (workspace_id, workspace) in workspaces {
                let matches_cwd = workspace
                    .get("root")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|root| paths_same(Path::new(root), cwd));
                let dir = sessions_dir.join(workspace_id);
                if matches_cwd && dir.is_dir() {
                    out.push(dir);
                }
            }
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }

    let entries = fs::read_dir(&sessions_dir)
        .map_err(|err| format!("read {}: {err}", sessions_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", sessions_dir.display()))?;
        if entry.path().is_dir() {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn kimi_state_matches_cwd(session_dir: &Path, cwd: &Path) -> Result<bool, String> {
    let state_path = session_dir.join("state.json");
    if !state_path.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(&state_path)
        .map_err(|err| format!("read {}: {err}", state_path.display()))?;
    let state: JsonValue = serde_json::from_str(&text)
        .map_err(|err| format!("parse {}: {err}", state_path.display()))?;
    Ok(state
        .get("workDir")
        .and_then(JsonValue::as_str)
        .is_some_and(|work_dir| paths_same(Path::new(work_dir), cwd)))
}

fn pi_sessions_dir(home: &Path) -> PathBuf {
    home.join(".pi").join("agent").join("sessions")
}

fn pi_project_session_dir(sessions_dir: &Path, cwd: &Path) -> PathBuf {
    sessions_dir.join(pi_project_session_dir_name(cwd))
}

fn pi_project_session_dir_name(cwd: &Path) -> String {
    let normalized = cwd.to_string_lossy();
    let trimmed = normalized.trim_start_matches(['/', '\\']);
    let safe = trimmed
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect::<String>();
    format!("--{safe}--")
}

fn locate_pi_session_file_by_id(
    sessions_dir: &Path,
    cwd: &Path,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let project_dir = pi_project_session_dir(sessions_dir, cwd);
    let suffix = format!("_{session_id}.jsonl");
    for dir in [project_dir.clone(), sessions_dir.to_path_buf()] {
        let mut files = Vec::new();
        collect_matching_files(&dir, &mut files, |path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        })?;
        files.sort_by_key(|path| std::cmp::Reverse(file_modified_time(path)));
        for path in files {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == format!("{session_id}.jsonl") || name.ends_with(&suffix)
                })
            {
                if pi_session_header_matches(&path, Some(session_id), Some(cwd))? {
                    return Ok(Some(path));
                }
            }
            if pi_session_header_matches(&path, Some(session_id), Some(cwd))? {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

fn latest_pi_session_file(
    sessions_dir: &Path,
    cwd: &Path,
) -> Result<Option<(String, PathBuf)>, String> {
    let project_dir = pi_project_session_dir(sessions_dir, cwd);
    let mut files = Vec::new();
    collect_matching_files(&project_dir, &mut files, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })?;
    files.sort_by_key(|path| std::cmp::Reverse(file_modified_time(path)));
    for path in files {
        let Some((session_id, session_cwd)) = read_pi_session_header(&path)? else {
            continue;
        };
        if paths_same(&session_cwd, cwd) {
            return Ok(Some((session_id, path)));
        }
    }
    Ok(None)
}

fn pi_session_header_matches(
    path: &Path,
    session_id: Option<&str>,
    cwd: Option<&Path>,
) -> Result<bool, String> {
    let Some((header_session_id, header_cwd)) = read_pi_session_header(path)? else {
        return Ok(false);
    };
    if session_id.is_some_and(|expected| expected != header_session_id) {
        return Ok(false);
    }
    if cwd.is_some_and(|expected| !paths_same(&header_cwd, expected)) {
        return Ok(false);
    }
    Ok(true)
}

fn read_pi_session_header(path: &Path) -> Result<Option<(String, PathBuf)>, String> {
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| format!("read {} header: {err}", path.display()))?;
    let Ok(json) = serde_json::from_str::<JsonValue>(&line) else {
        return Ok(None);
    };
    if json_string(&json, &["type"]).as_deref() != Some("session") {
        return Ok(None);
    }
    let Some(session_id) = json_string(&json, &["id"]) else {
        return Ok(None);
    };
    let Some(cwd) = json_string(&json, &["cwd"]) else {
        return Ok(None);
    };
    Ok(Some((session_id, PathBuf::from(cwd))))
}

fn parse_source(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
    privacy_spans: &[PrivacySpan],
) -> Result<Vec<NativeEvent>, String> {
    match agent.shell.as_str() {
        "claude" => parse_jsonl_source_incremental(project_root, agent, source, parse_claude_line),
        "codex" => parse_jsonl_source_incremental_with_tail_bytes(
            project_root,
            agent,
            source,
            CODEX_ROOM_EXCEPTION_JSONL_TAIL_BYTES,
            |agent, source, index, json| {
                parse_codex_line_with_room_exceptions(
                    project_root,
                    privacy_spans,
                    agent,
                    source,
                    index,
                    json,
                )
            },
        ),
        "antigravity" => parse_antigravity_source(project_root, agent, source),
        "opencode" => parse_opencode_source(agent, source),
        "pi" => parse_pi_source(agent, source),
        "kimi" => parse_jsonl_source_incremental_with_malformed(
            project_root,
            agent,
            source,
            parse_kimi_line,
            parse_kimi_malformed_line,
        ),
        other => Err(format!("unsupported parser shell: {other}")),
    }
}

fn parse_claude_hook_source(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    parse_jsonl_source_incremental(project_root, agent, source, parse_claude_hook_line)
}

fn parse_kimi_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    let timestamp = kimi_timestamp(&json).unwrap_or_else(now_iso);
    let record_type = json_string(&json, &["type"]).unwrap_or_else(|| "missing".into());
    match record_type.as_str() {
        "metadata"
        | "config.update"
        | "tools.set_active_tools"
        | "permission.set_mode"
        | "context.append_message"
        | "llm.tools_snapshot"
        | "llm.request"
        // Compaction bookkeeping is session-internal context maintenance
        // (the applied summary stays agent context, not room content).
        | "full_compaction.begin"
        | "context.apply_compaction"
        | "full_compaction.complete"
        | "usage.record" => Vec::new(),
        "tools.update_store" => parse_kimi_store_update(agent, source, index, &timestamp, &json),
        "turn.steer" => parse_kimi_turn_steer(agent, source, index, &timestamp, &json),
        "turn.prompt" => {
            let text = json_string(&json, &["input"])
                .or_else(|| json.get("input").and_then(text_from_json))
                .unwrap_or_default();
            if text.trim().is_empty() {
                return Vec::new();
            }
            let mut prompt = event(
                agent,
                source,
                "user",
                "message",
                &timestamp,
                &format!("kimi:{index}:prompt"),
                text,
            );
            prompt.turn_id = json_string(&json, &["turnId"]);
            vec![prompt]
        }
        "turn.cancel" => vec![control_event(
            agent,
            source,
            &timestamp,
            &format!("kimi:{index}:turn-cancel"),
            "interrupted",
            Some("turn.cancel".into()),
            json_string(&json, &["turnId"]),
        )],
        "context.append_loop_event" => {
            parse_kimi_loop_event(agent, source, index, &timestamp, json.get("event"))
        }
        _ => vec![kimi_unknown_event(
            agent,
            source,
            &timestamp,
            "record",
            &record_type,
        )],
    }
}

fn parse_kimi_loop_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    timestamp: &str,
    loop_event: Option<&JsonValue>,
) -> Vec<NativeEvent> {
    let Some(loop_event) = loop_event else {
        return vec![kimi_unknown_event(
            agent, source, timestamp, "loop", "missing",
        )];
    };
    let event_type = json_string(loop_event, &["type"]).unwrap_or_else(|| "missing".into());
    let turn_id = json_string(loop_event, &["turnId"]);
    match event_type.as_str() {
        "step.begin" => vec![control_event(
            agent,
            source,
            timestamp,
            &format!(
                "kimi:{}:step-begin",
                json_string(loop_event, &["uuid"]).unwrap_or_else(|| index.to_string())
            ),
            "activity",
            Some("step.begin".into()),
            turn_id,
        )],
        "content.part" => {
            let Some(part) = loop_event.get("part") else {
                return vec![kimi_unknown_event(
                    agent, source, timestamp, "part", "missing",
                )];
            };
            let part_type = json_string(part, &["type"]).unwrap_or_else(|| "missing".into());
            let (kind, text) = match part_type.as_str() {
                "think" => ("thinking", json_string(part, &["think"])),
                "text" => ("message", json_string(part, &["text"])),
                _ => {
                    return vec![kimi_unknown_event(
                        agent, source, timestamp, "part", &part_type,
                    )];
                }
            };
            let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
                return Vec::new();
            };
            let mut content = event(
                agent,
                source,
                "assistant",
                kind,
                timestamp,
                &format!("kimi:{index}:content"),
                text,
            );
            content.turn_id = turn_id;
            vec![content]
        }
        "tool.call" => {
            let name = json_string(loop_event, &["name"]).unwrap_or_else(|| "Tool".into());
            let detail = json_string(loop_event, &["description"])
                .or_else(|| json_string(loop_event, &["display", "command"]))
                .filter(|detail| !detail.trim().is_empty());
            let text = detail
                .map(|detail| format!("{name}: {detail}"))
                .unwrap_or(name);
            let event_id = json_string(loop_event, &["toolCallId"])
                .or_else(|| json_string(loop_event, &["uuid"]))
                .unwrap_or_else(|| index.to_string());
            let mut tool = event(
                agent,
                source,
                "assistant",
                "tool",
                timestamp,
                &format!("kimi:{event_id}:call"),
                text,
            );
            tool.turn_id = turn_id;
            vec![tool]
        }
        "tool.result" => {
            let text = loop_event
                .get("result")
                .and_then(|result| {
                    json_string(result, &["output"]).or_else(|| text_from_json(result))
                })
                .unwrap_or_else(|| "Tool completed.".into());
            let event_id = json_string(loop_event, &["toolCallId"])
                .or_else(|| json_string(loop_event, &["parentUuid"]))
                .unwrap_or_else(|| index.to_string());
            let mut tool = event(
                agent,
                source,
                "assistant",
                "tool",
                timestamp,
                &format!("kimi:{event_id}:result"),
                text,
            );
            tool.turn_id = turn_id;
            vec![tool]
        }
        "step.end" => {
            let reason =
                json_string(loop_event, &["finishReason"]).unwrap_or_else(|| "step.end".into());
            let signal = work_signal_for_stop_reason(&reason);
            vec![control_event(
                agent,
                source,
                timestamp,
                &format!(
                    "kimi:{}:step-end",
                    json_string(loop_event, &["uuid"]).unwrap_or_else(|| index.to_string())
                ),
                signal,
                Some(reason),
                turn_id,
            )]
        }
        _ => vec![kimi_unknown_event(
            agent,
            source,
            timestamp,
            "loop",
            &event_type,
        )],
    }
}

/// `tools.update_store` mirrors the agent's internal tool state onto the wire. The
/// `todo` store is the only key reshaped into the room: each full snapshot becomes
/// one compact commentary entry, so a slow turn stays visibly alive in the folded
/// progress bubble. Snapshots stay complete on purpose — diffing would need
/// cross-record state, and the fold only surfaces the latest entry anyway. Other
/// store keys and malformed snapshots fall back to the same sanitized canary the
/// record would have produced before this arm existed.
fn parse_kimi_store_update(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    timestamp: &str,
    json: &JsonValue,
) -> Vec<NativeEvent> {
    let canary = || kimi_unknown_event(agent, source, timestamp, "record", "tools.update_store");
    if json_string(json, &["key"]).as_deref() != Some("todo") {
        return vec![canary()];
    }
    let Some(text) = kimi_todo_store_progress_text(json.get("value")) else {
        return vec![canary()];
    };
    let mut progress = event(
        agent,
        source,
        "assistant",
        "commentary",
        timestamp,
        &format!("kimi:{index}:todo-store"),
        text,
    );
    // A todo snapshot is ambient progress, never an authoritative lifecycle signal.
    progress.work_signal = None;
    vec![progress]
}

/// `turn.steer` injects input into a running turn. The only shape observed in
/// real wires so far is the CLI's own background-task notification
/// (`origin.kind == "background_task"`); that content is folded into the room's
/// progress stream. Every other origin — including whatever shape user steering
/// turns out to have — deliberately stays on the record canary until it has
/// been observed, so nothing gets misclassified on a guess.
fn parse_kimi_turn_steer(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    timestamp: &str,
    json: &JsonValue,
) -> Vec<NativeEvent> {
    let canary = || kimi_unknown_event(agent, source, timestamp, "record", "turn.steer");
    if json_string(json, &["origin", "kind"]).as_deref() != Some("background_task") {
        return vec![canary()];
    }
    let text = json
        .get("input")
        .and_then(JsonValue::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| json_string(part, &["text"]))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return vec![canary()];
    }
    let mut notice = event(
        agent,
        source,
        "assistant",
        "commentary",
        timestamp,
        &format!("kimi:{index}:steer-notice"),
        text,
    );
    // An injected notification is ambient context, never a lifecycle signal.
    notice.work_signal = None;
    vec![notice]
}

const KIMI_TODO_TITLE_MAX_CHARS: usize = 80;
const KIMI_TODO_LIST_MAX_ITEMS: usize = 20;

/// Renders one full todo-store snapshot as compact Markdown: a summary first line
/// (which the room fold surfaces as the latest state) followed by the checklist.
/// Returns `None` for anything outside the `{title, status}` schema so the caller
/// fails closed into the sanitized canary.
fn kimi_todo_store_progress_text(value: Option<&JsonValue>) -> Option<String> {
    let items = value?.as_array()?;
    if items.is_empty() {
        return Some("进度 0/0 · 待办清单已清空".into());
    }
    let mut done = 0usize;
    let mut current = None;
    let mut lines = Vec::with_capacity(items.len().min(KIMI_TODO_LIST_MAX_ITEMS) + 1);
    for item in items {
        let title = json_string(item, &["title"])?;
        // Todo titles are free-form model text: collapse embedded newlines and
        // whitespace runs so a title can never break the single-line summary
        // contract or forge checklist lines of its own.
        let title = one_line(&title);
        if title.is_empty() {
            return None;
        }
        let status = json_string(item, &["status"])?;
        let marker = match status.trim() {
            "done" => {
                done += 1;
                "- [x]"
            }
            "in_progress" => {
                current.get_or_insert_with(|| truncate_chars(&title, KIMI_TODO_TITLE_MAX_CHARS));
                "▸"
            }
            "pending" => "- [ ]",
            _ => return None,
        };
        if lines.len() < KIMI_TODO_LIST_MAX_ITEMS {
            lines.push(format!(
                "{marker} {}",
                truncate_chars(&title, KIMI_TODO_TITLE_MAX_CHARS)
            ));
        }
    }
    let total = items.len();
    let mut summary = format!("进度 {done}/{total}");
    if let Some(current) = current {
        summary.push_str(&format!(" · 当前：{current}"));
    } else if done == total {
        summary.push_str(" · 全部完成");
    } else {
        summary.push_str(" · 无进行中项");
    }
    if total > KIMI_TODO_LIST_MAX_ITEMS {
        lines.push(format!("… 其余 {} 项", total - KIMI_TODO_LIST_MAX_ITEMS));
    }
    Some(format!("{summary}\n{}", lines.join("\n")))
}

fn kimi_unknown_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    timestamp: &str,
    scope: &str,
    event_type: &str,
) -> NativeEvent {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(event_type.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let mut warning = event(
        agent,
        source,
        "assistant",
        "commentary",
        timestamp,
        &format!("kimi:unknown:{scope}:{}", &digest[..12]),
        KIMI_UNKNOWN_EVENT_WARNING.into(),
    );
    // Unknown native records are diagnostic only. They must remain visible,
    // but cannot authoritatively wake or complete an agent.
    warning.work_signal = None;
    warning
}

fn parse_kimi_malformed_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    _index: usize,
) -> Vec<NativeEvent> {
    vec![kimi_unknown_event(
        agent,
        source,
        &now_iso(),
        "json",
        "malformed",
    )]
}

fn kimi_timestamp(json: &JsonValue) -> Option<String> {
    let millis = json.get("time").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
    })?;
    millis_to_iso(millis)
}

#[derive(Clone)]
struct PiSessionEntry {
    id: String,
    parent_id: Option<String>,
    timestamp: String,
    json: JsonValue,
}

fn parse_pi_source(
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    let file = fs::File::open(&source.path)
        .map_err(|err| format!("open {}: {err}", source.path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|err| format!("read {}: {err}", source.path.display()))?;
        let Ok(json) = serde_json::from_str::<JsonValue>(&line) else {
            continue;
        };
        let Some(id) = json_string(&json, &["id"]) else {
            continue;
        };
        if json_string(&json, &["type"]).as_deref() == Some("session") {
            continue;
        }
        let parent_id = json
            .get("parentId")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let timestamp = json_string(&json, &["timestamp"])
            .or_else(|| pi_message_timestamp(&json))
            .unwrap_or_else(now_iso);
        entries.push(PiSessionEntry {
            id,
            parent_id,
            timestamp,
            json,
        });
    }

    let mut events = pi_active_path_entries(&entries)
        .into_iter()
        .flat_map(|entry| parse_pi_entry(agent, source, entry))
        .collect::<Vec<_>>();
    events = dedupe_native_events(events);
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(tail_events(events, SOURCE_EVENT_LIMIT * 4))
}

fn pi_active_path_entries(entries: &[PiSessionEntry]) -> Vec<&PiSessionEntry> {
    let mut child_parent_ids = HashSet::new();
    for entry in entries {
        if let Some(parent_id) = entry.parent_id.as_deref() {
            child_parent_ids.insert(parent_id);
        }
    }
    let Some(leaf) = entries
        .iter()
        .rev()
        .find(|entry| !child_parent_ids.contains(entry.id.as_str()))
        .or_else(|| entries.last())
    else {
        return Vec::new();
    };
    let by_id: HashMap<&str, &PiSessionEntry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(leaf);
    while let Some(entry) = current {
        if !seen.insert(entry.id.as_str()) {
            break;
        }
        path.push(entry);
        current = entry
            .parent_id
            .as_deref()
            .and_then(|parent_id| by_id.get(parent_id).copied());
    }
    path.reverse();
    path
}

fn parse_pi_entry(
    agent: &ProjectAgent,
    source: &NativeSource,
    entry: &PiSessionEntry,
) -> Vec<NativeEvent> {
    match json_string(&entry.json, &["type"]).as_deref() {
        Some("message") => parse_pi_message_entry(agent, source, entry),
        Some("compaction") => vec![pi_summary_control_event(
            agent,
            source,
            entry,
            "compacted",
            json_string(&entry.json, &["summary"])
                .unwrap_or_else(|| "Conversation compacted".into()),
        )],
        _ => Vec::new(),
    }
}

fn parse_pi_message_entry(
    agent: &ProjectAgent,
    source: &NativeSource,
    entry: &PiSessionEntry,
) -> Vec<NativeEvent> {
    let null_message = JsonValue::Null;
    let message = entry.json.get("message").unwrap_or(&null_message);
    let role = json_string(message, &["role"]).unwrap_or_default();
    let event_id = format!("pi:{}", entry.id);
    match role.as_str() {
        "user" => content_blocks_to_events(
            agent,
            source,
            "user",
            &entry.timestamp,
            &event_id,
            message.get("content").unwrap_or(&JsonValue::Null),
        ),
        "assistant" => {
            let stop_reason = json_string(message, &["stopReason"]);
            let mut events = content_blocks_to_events(
                agent,
                source,
                "assistant",
                &entry.timestamp,
                &event_id,
                message.get("content").unwrap_or(&JsonValue::Null),
            );
            if stop_reason.as_deref().is_some_and(|reason| {
                reason.eq_ignore_ascii_case("toolUse") || reason.eq_ignore_ascii_case("tool_use")
            }) {
                for event in &mut events {
                    if event.kind == "message" {
                        event.kind = "commentary".into();
                    }
                }
            }
            if let Some(reason) = stop_reason {
                let signal = work_signal_for_stop_reason(&reason);
                for event in &mut events {
                    event.work_signal = Some(signal.to_string());
                    event.stop_reason = Some(reason.clone());
                }
            }
            events
        }
        "toolResult" => pi_tool_result_event(agent, source, entry, message, &event_id)
            .into_iter()
            .collect(),
        "bashExecution" => pi_bash_execution_event(agent, source, entry, message, &event_id)
            .into_iter()
            .collect(),
        "custom" if json_bool(message, &["display"]).unwrap_or(true) => content_blocks_to_events(
            agent,
            source,
            "assistant",
            &entry.timestamp,
            &event_id,
            message.get("content").unwrap_or(&JsonValue::Null),
        ),
        _ => Vec::new(),
    }
}

fn pi_tool_result_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    entry: &PiSessionEntry,
    message: &JsonValue,
    event_id: &str,
) -> Option<NativeEvent> {
    let tool_name = json_string(message, &["toolName"]).unwrap_or_else(|| "tool".into());
    let content = message
        .get("content")
        .and_then(text_from_json)
        .unwrap_or_default();
    let mut text = if content.trim().is_empty() {
        tool_name
    } else {
        format!("{tool_name}\n{content}")
    };
    if json_bool(message, &["isError"]).unwrap_or(false) {
        text = format!("Tool error: {text}");
    }
    (!text.trim().is_empty()).then(|| {
        let mut event = event(
            agent,
            source,
            "assistant",
            "tool",
            &entry.timestamp,
            event_id,
            text,
        );
        event.work_signal = Some("activity".into());
        event
    })
}

fn pi_bash_execution_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    entry: &PiSessionEntry,
    message: &JsonValue,
    event_id: &str,
) -> Option<NativeEvent> {
    let command = json_string(message, &["command"]).unwrap_or_default();
    let output = json_string(message, &["output"]).unwrap_or_default();
    let text = match (command.trim().is_empty(), output.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("$ {command}"),
        (true, false) => output,
        (false, false) => format!("$ {command}\n{output}"),
    };
    (!text.trim().is_empty()).then(|| {
        let mut event = event(
            agent,
            source,
            "assistant",
            "tool",
            &entry.timestamp,
            event_id,
            text,
        );
        event.work_signal = Some("activity".into());
        event
    })
}

fn pi_summary_control_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    entry: &PiSessionEntry,
    reason: &str,
    text: String,
) -> NativeEvent {
    let mut event = control_event(
        agent,
        source,
        &entry.timestamp,
        &format!("pi:{}", entry.id),
        "completed",
        Some(reason.into()),
        None,
    );
    event.kind = "compaction".into();
    event.text = text;
    event
}

fn pi_message_timestamp(json: &JsonValue) -> Option<String> {
    let millis = json
        .get("message")
        .and_then(|message| message.get("timestamp"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|millis| i64::try_from(millis).ok()))
        })?;
    millis_to_iso(millis)
}

fn parse_jsonl_source_incremental<F>(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
    parser: F,
) -> Result<Vec<NativeEvent>, String>
where
    F: FnMut(&ProjectAgent, &NativeSource, usize, JsonValue) -> Vec<NativeEvent>,
{
    parse_jsonl_source_incremental_with_malformed(project_root, agent, source, parser, |_, _, _| {
        Vec::new()
    })
}

fn parse_jsonl_source_incremental_with_tail_bytes<F>(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
    tail_bytes: u64,
    parser: F,
) -> Result<Vec<NativeEvent>, String>
where
    F: FnMut(&ProjectAgent, &NativeSource, usize, JsonValue) -> Vec<NativeEvent>,
{
    parse_jsonl_source_incremental_with_malformed_and_tail_bytes(
        project_root,
        agent,
        source,
        tail_bytes,
        parser,
        |_, _, _| Vec::new(),
    )
}

fn parse_jsonl_source_incremental_with_malformed<F, M>(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
    parser: F,
    malformed: M,
) -> Result<Vec<NativeEvent>, String>
where
    F: FnMut(&ProjectAgent, &NativeSource, usize, JsonValue) -> Vec<NativeEvent>,
    M: FnMut(&ProjectAgent, &NativeSource, usize) -> Vec<NativeEvent>,
{
    parse_jsonl_source_incremental_with_malformed_and_tail_bytes(
        project_root,
        agent,
        source,
        JSONL_TAIL_BYTES,
        parser,
        malformed,
    )
}

fn parse_jsonl_source_incremental_with_malformed_and_tail_bytes<F, M>(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
    tail_bytes: u64,
    mut parser: F,
    mut malformed: M,
) -> Result<Vec<NativeEvent>, String>
where
    F: FnMut(&ProjectAgent, &NativeSource, usize, JsonValue) -> Vec<NativeEvent>,
    M: FnMut(&ProjectAgent, &NativeSource, usize) -> Vec<NativeEvent>,
{
    let cache_key = source_cache_key(source);
    let parsed_path = parsed_event_cache_path(project_root, &cache_key)?;
    let cursor_path = source_cursor_path(project_root, &cache_key)?;
    let metadata = fs::metadata(&source.path)
        .map_err(|err| format!("stat {}: {err}", source.path.display()))?;
    let file_len = metadata.len();
    let cursor = read_source_cursor(&cursor_path)?;
    let cursor_matches = cursor.as_ref().map_or(false, |cursor| {
        cursor.path == path_string(&source.path) && file_len >= cursor.offset
    });
    let reset_cache = !cursor_matches;
    let start_offset = cursor
        .as_ref()
        .filter(|_| cursor_matches)
        .map_or(0, |cursor| cursor.offset);
    let start_line_index = cursor
        .as_ref()
        .filter(|_| cursor_matches)
        .map_or(0, |cursor| cursor.line_index);
    let read = if reset_cache {
        read_jsonl_tail(&source.path, file_len, tail_bytes)?
    } else {
        read_jsonl_from_offset(&source.path, start_offset, start_line_index)?
    };

    let mut cached = if reset_cache {
        Vec::new()
    } else {
        read_cached_native_events(&parsed_path)?
    };
    let JsonlReadResult {
        lines,
        next_offset,
        next_line_index,
    } = read;
    let mut parsed_new = Vec::new();
    for (index, line) in lines {
        let json = match serde_json::from_str::<JsonValue>(&line) {
            Ok(json) => json,
            Err(_) => {
                parsed_new.extend(malformed(agent, source, index));
                continue;
            }
        };
        parsed_new.extend(parser(agent, source, index, json));
    }
    if !parsed_new.is_empty() {
        cached.extend(parsed_new);
        cached = dedupe_native_events(cached);
        cached.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        if cached.len() > PARSED_EVENT_CACHE_LIMIT {
            cached = cached.split_off(cached.len() - PARSED_EVENT_CACHE_LIMIT);
        }
        write_cached_native_events(&parsed_path, &cached)?;
    } else if reset_cache {
        write_cached_native_events(&parsed_path, &cached)?;
    }
    write_source_cursor(
        &cursor_path,
        &SourceCursor {
            path: path_string(&source.path),
            offset: next_offset,
            line_index: next_line_index,
            updated_at: now_iso(),
        },
    )?;
    Ok(tail_events(cached, SOURCE_EVENT_LIMIT * 4))
}

fn source_cache_key(source: &NativeSource) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PARSED_EVENT_CACHE_VERSION);
    hasher.update(&source.kind);
    hasher.update(&source.session_id);
    hasher.update(path_string(&source.path));
    format!("{:x}", hasher.finalize())[..24].to_string()
}

fn violet_internal_dir(project_root: &Path) -> Result<PathBuf, String> {
    let dir = project_root.join("project-memory").join(".violet");
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
    Ok(dir)
}

fn parsed_event_cache_path(project_root: &Path, cache_key: &str) -> Result<PathBuf, String> {
    let dir = violet_internal_dir(project_root)?.join("parsed-events");
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
    Ok(dir.join(format!("{cache_key}.jsonl")))
}

fn source_cursor_path(project_root: &Path, cache_key: &str) -> Result<PathBuf, String> {
    let dir = violet_internal_dir(project_root)?.join("cursors");
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
    Ok(dir.join(format!("{cache_key}.json")))
}

fn read_source_cursor(path: &Path) -> Result<Option<SourceCursor>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(serde_json::from_str(&text).ok())
}

fn write_source_cursor(path: &Path, cursor: &SourceCursor) -> Result<(), String> {
    let bytes = serde_json::to_vec(cursor).map_err(|err| format!("serialize cursor: {err}"))?;
    write_if_changed(path, &bytes)
}

fn read_jsonl_tail(path: &Path, file_len: u64, tail_bytes: u64) -> Result<JsonlReadResult, String> {
    let start = file_len.saturating_sub(tail_bytes);
    read_jsonl_range(path, start, 0, true)
}

fn read_jsonl_from_offset(
    path: &Path,
    offset: u64,
    line_index: usize,
) -> Result<JsonlReadResult, String> {
    read_jsonl_range(path, offset, line_index, false)
}

fn read_jsonl_range(
    path: &Path,
    offset: u64,
    line_index: usize,
    align_first_line: bool,
) -> Result<JsonlReadResult, String> {
    let mut file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let file_len = file
        .metadata()
        .map_err(|err| format!("stat {}: {err}", path.display()))?
        .len();
    let offset = offset.min(file_len);
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("seek {}: {err}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    if bytes.is_empty() {
        return Ok(JsonlReadResult {
            lines: Vec::new(),
            next_offset: offset,
            next_line_index: line_index,
        });
    }

    let mut base_offset = offset;
    if align_first_line && offset > 0 {
        if let Some(pos) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=pos);
            base_offset += pos as u64 + 1;
        } else {
            return Ok(JsonlReadResult {
                lines: Vec::new(),
                next_offset: file_len,
                next_line_index: line_index,
            });
        }
    }

    let complete_len = if bytes.ends_with(b"\n") {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |pos| pos + 1)
    };
    let next_offset = base_offset + complete_len as u64;
    let text = String::from_utf8_lossy(&bytes[..complete_len]);
    let mut next_line_index = line_index;
    let mut lines = VecDeque::with_capacity(JSONL_TAIL_LINES);
    for raw in text.split('\n') {
        if raw.trim().is_empty() {
            next_line_index += 1;
            continue;
        }
        if lines.len() == JSONL_TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back((next_line_index, raw.to_string()));
        next_line_index += 1;
    }
    Ok(JsonlReadResult {
        lines: lines.into_iter().collect(),
        next_offset,
        next_line_index,
    })
}

fn read_cached_native_events(path: &Path) -> Result<Vec<NativeEvent>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut lines = VecDeque::with_capacity(PARSED_EVENT_CACHE_LIMIT);
    for line in reader.lines() {
        let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if lines.len() == PARSED_EVENT_CACHE_LIMIT {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    Ok(lines
        .into_iter()
        .filter_map(|line| serde_json::from_str::<NativeEvent>(&line).ok())
        .collect())
}

fn write_cached_native_events(path: &Path, events: &[NativeEvent]) -> Result<(), String> {
    let mut out = String::new();
    for event in events {
        out.push_str(
            &serde_json::to_string(event)
                .map_err(|err| format!("serialize native event: {err}"))?,
        );
        out.push('\n');
    }
    write_if_changed(path, out.as_bytes())
}

fn parse_claude_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    if json_bool(&json, &["isMeta"]).unwrap_or(false) {
        return Vec::new();
    }
    // Compaction is not a real chat turn: Claude writes a `system/compact_boundary`
    // marker followed by a `user` message flagged `isCompactSummary` carrying the
    // injected summary. Collapse both into a single compaction marker and emit a
    // terminal work signal — neither native line carries a `stop_reason`, so the agent
    // bar would otherwise stay "working" until the frontend timeout.
    if json_string(&json, &["subtype"]).as_deref() == Some("compact_boundary") {
        let timestamp = json_string(&json, &["timestamp"]).unwrap_or_else(now_iso);
        let event_id = json_string(&json, &["uuid"])
            .or_else(|| json_string(&json, &["id"]))
            .unwrap_or_else(|| index.to_string());
        let mut event = control_event(
            agent,
            source,
            &timestamp,
            &event_id,
            "completed",
            Some("compacted".into()),
            None,
        );
        event.kind = "compaction".into();
        event.text = compact_boundary_label(&json);
        return vec![event];
    }
    if json_bool(&json, &["isCompactSummary"]).unwrap_or(false) {
        return Vec::new();
    }
    let role = json_string(&json, &["type"])
        .or_else(|| json_string(&json, &["message", "role"]))
        .unwrap_or_else(|| "assistant".into());
    if role != "user" && role != "assistant" && role != "system" {
        return Vec::new();
    }
    let timestamp = json_string(&json, &["timestamp"]).unwrap_or_else(now_iso);
    let event_id = json_string(&json, &["uuid"])
        .or_else(|| json_string(&json, &["id"]))
        .unwrap_or_else(|| index.to_string());
    let stop_reason = json_string(&json, &["stop_reason"])
        .or_else(|| json_string(&json, &["message", "stop_reason"]));
    let turn_id = json_string(&json, &["requestId"])
        .or_else(|| json_string(&json, &["request_id"]))
        .or_else(|| json_string(&json, &["message", "id"]));
    let content = json
        .get("content")
        .or_else(|| {
            json.get("message")
                .and_then(|message| message.get("content"))
        })
        .unwrap_or(&JsonValue::Null);
    let mut events = content_blocks_to_events(agent, source, &role, &timestamp, &event_id, content)
        .into_iter()
        .filter(|event| !is_harness_envelope_text(&event.text))
        .collect::<Vec<_>>();
    // Claude emits narration ("Let me check X…") on its own jsonl line just before the
    // `tool_use` line that pauses the turn; both share `stop_reason: "tool_use"`. Reclassify
    // that narration as commentary so it folds into the same collapsible room-only progress
    // bubble as Codex `phase: commentary`, instead of a plain chat message. Only `message`
    // events are retagged, so sibling tool/thinking lines are left as-is.
    if is_claude_interstitial_tool_preamble(&role, stop_reason.as_deref()) {
        for event in &mut events {
            if event.kind == "message" {
                event.kind = "commentary".into();
            }
        }
    }
    if events.is_empty() && role == "assistant" && assistant_content_has_activity_marker(content) {
        if let Some(reason) = stop_reason.as_deref() {
            let signal = work_signal_for_stop_reason(reason);
            if signal == "failed" || signal == "interrupted" {
                return vec![control_event(
                    agent,
                    source,
                    &timestamp,
                    &event_id,
                    signal,
                    Some(reason.to_string()),
                    turn_id,
                )];
            }
        }
        return vec![control_event(
            agent,
            source,
            &timestamp,
            &event_id,
            "activity",
            Some("assistant_activity".into()),
            turn_id,
        )];
    }
    if let Some(reason) = stop_reason {
        let signal = work_signal_for_stop_reason(&reason);
        if events.is_empty() {
            events.push(control_event(
                agent,
                source,
                &timestamp,
                &event_id,
                signal,
                Some(reason),
                turn_id,
            ));
        } else {
            for event in &mut events {
                if event.stop_reason.as_deref() == Some("user_question_requested") {
                    event.work_signal = Some("waiting_for_user".into());
                    event.stop_reason = Some("user_question_requested".into());
                } else {
                    event.work_signal = Some(signal.to_string());
                    event.stop_reason = Some(reason.clone());
                }
                event.turn_id = turn_id.clone();
            }
        }
    }
    events
}

/// Short, human-readable label for a Claude `compact_boundary` line, enriched with the
/// pre/post token counts from `compactMetadata` when present.
fn compact_boundary_label(json: &JsonValue) -> String {
    let token_count = |key: &str| {
        json.get("compactMetadata")
            .and_then(|meta| meta.get(key))
            .and_then(JsonValue::as_u64)
    };
    match (token_count("preTokens"), token_count("postTokens")) {
        (Some(pre), Some(post)) => format!(
            "Conversation compacted · {} → {} tokens",
            format_token_count(pre),
            format_token_count(post),
        ),
        _ => "Conversation compacted".to_string(),
    }
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{}k", (tokens + 500) / 1000)
    } else {
        tokens.to_string()
    }
}

fn parse_claude_hook_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    if json_string(&json, &["schema"]).as_deref() != Some("kota.claude.ask-user-question.v1") {
        return Vec::new();
    }
    if json_string(&json, &["agent_id"]).as_deref() != Some(agent.agent_id.as_str()) {
        return Vec::new();
    }
    let tool_name = json_string(&json, &["tool_name"]).unwrap_or_default();
    if !tool_name.eq_ignore_ascii_case("AskUserQuestion") {
        return Vec::new();
    }
    let Some(tool_input) = json.get("tool_input") else {
        return Vec::new();
    };
    let item = serde_json::json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": tool_input,
    });
    let Some(text) = format_ask_user_question(&item) else {
        return Vec::new();
    };
    let timestamp = json_string(&json, &["captured_at"]).unwrap_or_else(now_iso);
    let event_id = json_string(&json, &["tool_use_id"]).unwrap_or_else(|| index.to_string());
    let mut event = control_event(
        agent,
        source,
        &timestamp,
        &format!("claude-hook:{event_id}"),
        "waiting_for_user",
        Some("user_question_requested".into()),
        None,
    );
    event.text = text;
    vec![event]
}

/// True when an assistant entry is pre-tool narration ("Let me check X…") rather than a
/// final reply. Claude Code writes each content block on its own jsonl line — the narration
/// text, the `tool_use`, and any `thinking` arrive as *separate* assistant entries that all
/// share `stop_reason: "tool_use"`. The narration line therefore carries no tool block of its
/// own, so the pausing `stop_reason` is the only reliable signal (requiring a same-line tool
/// block would never match, leaving the narration a plain chat bubble). Such text is
/// reclassified as room-only commentary so it folds into the same collapsible progress bubble
/// as Codex `phase: commentary`; the final reply (`stop_reason: "end_turn"`) is left untouched.
fn is_claude_interstitial_tool_preamble(role: &str, stop_reason: Option<&str>) -> bool {
    role == "assistant"
        && stop_reason
            .map(stop_reason_is_interstitial_tool_use)
            .unwrap_or(false)
}

fn stop_reason_is_interstitial_tool_use(reason: &str) -> bool {
    matches!(
        reason.trim().to_ascii_lowercase().as_str(),
        "tool_use" | "pause_turn"
    )
}

fn assistant_content_has_activity_marker(content: &JsonValue) -> bool {
    match content {
        JsonValue::Array(items) => items.iter().any(assistant_content_has_activity_marker),
        JsonValue::Object(_) => {
            let block_type = json_string(content, &["type"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            block_type.contains("thinking")
                || block_type.contains("reason")
                || block_type.contains("tool")
                || block_type.contains("function")
        }
        _ => false,
    }
}

fn parse_room_exception_registry(text: &str) -> Result<RoomExceptionRegistry, String> {
    let registry = serde_json::from_str::<RoomExceptionRegistry>(text)
        .map_err(|err| format!("parse room exception registry: {err}"))?;
    if registry.schema_version != 1 {
        return Err(format!(
            "unsupported room exception schema version: {}",
            registry.schema_version
        ));
    }

    let mut ids = HashSet::new();
    let mut selectors = HashSet::new();
    for rule in &registry.exceptions {
        if rule.id.trim().is_empty()
            || rule.provider.trim().is_empty()
            || rule.record_type.trim().is_empty()
            || rule.payload_type.trim().is_empty()
            || rule.source.field.trim().is_empty()
        {
            return Err("room exception fields must not be empty".into());
        }
        if !ids.insert(rule.id.clone()) {
            return Err(format!("duplicate room exception id: {}", rule.id));
        }
        let selector = (
            rule.provider.clone(),
            rule.record_type.clone(),
            rule.payload_type.clone(),
        );
        if !selectors.insert(selector) {
            return Err(format!(
                "duplicate room exception selector: {}/{}/{}",
                rule.provider, rule.record_type, rule.payload_type
            ));
        }
        if !rule
            .source
            .field
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(format!(
                "room exception source field must be a direct field: {}",
                rule.id
            ));
        }
    }
    Ok(registry)
}

fn room_exception_registry() -> Option<&'static RoomExceptionRegistry> {
    ROOM_EXCEPTION_REGISTRY
        .get_or_init(
            || match parse_room_exception_registry(ROOM_EXCEPTION_CONFIG) {
                Ok(registry) => Some(registry),
                Err(err) => {
                    eprintln!("Kota Violet room exceptions disabled: {err}");
                    None
                }
            },
        )
        .as_ref()
}

fn matching_room_exception(
    provider: &str,
    record_type: &str,
    payload_type: &str,
) -> Option<&'static RoomExceptionRule> {
    room_exception_registry()?.exceptions.iter().find(|rule| {
        rule.provider == provider
            && rule.record_type == record_type
            && rule.payload_type == payload_type
    })
}

fn parse_codex_line_with_room_exceptions(
    project_root: &Path,
    privacy_spans: &[PrivacySpan],
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    let record_type = json_string(&json, &["type"]).unwrap_or_default();
    let timestamp = json_string(&json, &["timestamp"]).unwrap_or_else(now_iso);
    let payload = json.get("payload").unwrap_or(&JsonValue::Null);
    let payload_type = json_string(payload, &["type"]).unwrap_or_default();
    let Some(rule) = matching_room_exception(&agent.shell, &record_type, &payload_type) else {
        return parse_codex_line(agent, source, index, json);
    };

    let result = match (&rule.action, &rule.source.shape, &rule.reshape) {
        (
            RoomExceptionAction::Reshape,
            RoomExceptionSourceShape::ScalarBase64,
            RoomExceptionReshape::Base64ImageToArtifact,
        ) => reshape_base64_image_to_artifact(
            project_root,
            privacy_spans,
            agent,
            source,
            &timestamp,
            payload,
            rule,
        ),
    };
    match result {
        Ok(event) => vec![event],
        Err(reason) => {
            let skipped = ROOM_EXCEPTION_SKIPS.fetch_add(1, Ordering::Relaxed) + 1;
            eprintln!(
                "Kota Violet room exception skipped: id={} reason={reason} count={skipped}",
                rule.id
            );
            Vec::new()
        }
    }
}

fn reshape_base64_image_to_artifact(
    project_root: &Path,
    privacy_spans: &[PrivacySpan],
    agent: &ProjectAgent,
    source: &NativeSource,
    timestamp: &str,
    payload: &JsonValue,
    rule: &RoomExceptionRule,
) -> Result<NativeEvent, &'static str> {
    let status = json_string(payload, &["status"]).ok_or("missing_status")?;
    if !status.eq_ignore_ascii_case("completed") {
        return Err("not_completed");
    }
    let call_id = json_string(payload, &["call_id"]).ok_or("missing_call_id")?;
    if call_id.is_empty()
        || call_id.len() > 256
        || !call_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("invalid_call_id");
    }
    let event_id = format!("{}:{call_id}", rule.id);

    // Privacy is checked before the base64 field is read, decoded, or persisted.
    // The ordinary partition still receives a safe marker so skip accounting stays intact.
    let mut privacy_probe = event(
        agent,
        source,
        "assistant",
        "tool",
        timestamp,
        &event_id,
        "Codex generated an image.".into(),
    );
    privacy_probe.work_signal = None;
    if is_private_event(&privacy_probe, privacy_spans) {
        return Ok(privacy_probe);
    }

    let encoded = payload
        .get(&rule.source.field)
        .and_then(JsonValue::as_str)
        .ok_or("missing_image_data")?
        .trim();
    let bytes = decode_room_exception_png(encoded)?;
    let relative_path = persist_room_exception_png(project_root, &bytes)?;
    let mut artifact = event(
        agent,
        source,
        "assistant",
        "artifact",
        timestamp,
        &event_id,
        format!("Generated image\n\n{relative_path}"),
    );
    artifact.work_signal = None;
    Ok(artifact)
}

fn decode_room_exception_png(encoded: &str) -> Result<Vec<u8>, &'static str> {
    if encoded.is_empty() {
        return Err("empty_image_data");
    }
    if encoded.len() > ROOM_EXCEPTION_MAX_BASE64_CHARS {
        return Err("encoded_image_too_large");
    }
    let bytes = BASE64_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| "invalid_base64")?;
    if bytes.is_empty() || bytes.len() > ROOM_EXCEPTION_MAX_IMAGE_BYTES {
        return Err("decoded_image_too_large");
    }
    validate_room_exception_png(&bytes)?;
    Ok(bytes)
}

fn validate_room_exception_png(bytes: &[u8]) -> Result<(), &'static str> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 33 || bytes.get(..8) != Some(PNG_SIGNATURE.as_slice()) {
        return Err("unsupported_image_format");
    }
    if bytes.get(8..12) != Some([0, 0, 0, 13].as_slice())
        || bytes.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err("invalid_png_header");
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| "invalid_png_header")?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| "invalid_png_header")?);
    if width == 0 || height == 0 {
        return Err("invalid_image_dimensions");
    }
    if width > ROOM_EXCEPTION_MAX_IMAGE_DIMENSION
        || height > ROOM_EXCEPTION_MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > ROOM_EXCEPTION_MAX_IMAGE_PIXELS
    {
        return Err("image_dimensions_too_large");
    }
    if bytes[26] != 0 || bytes[27] != 0 || bytes[28] > 1 {
        return Err("invalid_png_header");
    }
    Ok(())
}

fn persist_room_exception_png(project_root: &Path, bytes: &[u8]) -> Result<String, &'static str> {
    let digest = format!("{:x}", Sha256::digest(bytes));
    let relative_path = format!("project-memory/attachments/violet/codex-generated/{digest}.png");
    let path = project_root.join(&relative_path);
    let parent = path.parent().ok_or("invalid_artifact_path")?;
    fs::create_dir_all(parent).map_err(|_| "artifact_store_unavailable")?;

    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("artifact_path_conflict");
        }
        let existing = fs::read(&path).map_err(|_| "artifact_read_failed")?;
        if existing == bytes {
            return Ok(relative_path);
        }
        return Err("artifact_hash_conflict");
    }

    let mut stored_bytes = 0u64;
    for entry in fs::read_dir(parent).map_err(|_| "artifact_store_unavailable")? {
        let entry = entry.map_err(|_| "artifact_store_unavailable")?;
        let metadata = entry.metadata().map_err(|_| "artifact_store_unavailable")?;
        if metadata.is_file() {
            stored_bytes = stored_bytes.saturating_add(metadata.len());
        }
    }
    if stored_bytes.saturating_add(bytes.len() as u64) > ROOM_EXCEPTION_ARTIFACT_STORE_MAX_BYTES {
        return Err("artifact_store_full");
    }
    write_if_changed(&path, bytes).map_err(|_| "artifact_write_failed")?;
    Ok(relative_path)
}

fn parse_codex_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    let timestamp = json_string(&json, &["timestamp"]).unwrap_or_else(now_iso);
    let line_type = json_string(&json, &["type"]).unwrap_or_default();
    if let Some(signal) = codex_control_signal(&line_type) {
        return vec![control_event(
            agent,
            source,
            &timestamp,
            &index.to_string(),
            signal,
            Some(line_type),
            json_string(&json, &["turn_id"]).or_else(|| json_string(&json, &["turnId"])),
        )];
    }
    if line_type == "session_meta" || line_type == "turn_context" {
        return Vec::new();
    }
    let payload = json.get("payload").unwrap_or(&JsonValue::Null);
    if line_type == "event_msg" {
        let event_type = json_string(payload, &["type"]).unwrap_or_default();
        if is_codex_image_generation_event_type(&event_type) {
            return Vec::new();
        }
        if let Some(signal) = codex_control_signal(&event_type) {
            return vec![control_event(
                agent,
                source,
                &timestamp,
                &index.to_string(),
                signal,
                Some(event_type),
                json_string(payload, &["turn_id"])
                    .or_else(|| json_string(payload, &["turnId"]))
                    .or_else(|| json_string(&json, &["turn_id"])),
            )];
        }
        let role = if event_type.contains("user") {
            "user"
        } else {
            "assistant"
        };
        let kind = if event_type.contains("reasoning") {
            "thinking"
        } else if event_type.contains("tool") {
            "tool"
        } else if role == "assistant" && codex_has_commentary_phase(&json, payload, None) {
            "commentary"
        } else {
            "message"
        };
        let text = json_string(payload, &["message"])
            .or_else(|| json_string(payload, &["text"]))
            .or_else(|| text_from_json(payload));
        return text
            .filter(|text| !text.trim().is_empty())
            .filter(|text| !(role == "user" && is_ignorable_codex_user_wrapper_text(text)))
            .map(|text| {
                vec![event(
                    agent,
                    source,
                    role,
                    kind,
                    &timestamp,
                    &index.to_string(),
                    text,
                )]
            })
            .unwrap_or_default();
    }
    if line_type == "response_item" {
        let item = payload.get("item").unwrap_or(payload);
        let item_type = json_string(item, &["type"]).unwrap_or_default();
        if is_codex_image_generation_event_type(&item_type) {
            return Vec::new();
        }
        if item_type == "agent_message" {
            let author = json_string(item, &["author"]).filter(|author| !author.trim().is_empty());
            let recipient =
                json_string(item, &["recipient"]).filter(|recipient| !recipient.trim().is_empty());
            if let (Some(author), Some(recipient)) = (author, recipient) {
                return codex_routed_agent_message_events(
                    agent, source, &timestamp, index, item, &author, &recipient,
                );
            }
        }
        let role = json_string(item, &["role"]).unwrap_or_else(|| {
            if item_type.contains("user") {
                "user".into()
            } else {
                "assistant".into()
            }
        });
        let kind = if item_type.contains("reasoning") {
            "thinking"
        } else if item_type.contains("tool") || item_type.contains("call") {
            "tool"
        } else if role == "assistant" && codex_has_commentary_phase(&json, payload, Some(item)) {
            "commentary"
        } else {
            "message"
        };
        let content = item.get("content").unwrap_or(item);
        return content_blocks_to_events(
            agent,
            source,
            &role,
            &timestamp,
            &index.to_string(),
            content,
        )
        .into_iter()
        .map(|mut event| {
            if event.kind != "control" && event.kind != "interrupt" {
                event.kind = kind.into();
            }
            event
        })
        .collect();
    }
    Vec::new()
}

fn codex_routed_agent_message_events(
    agent: &ProjectAgent,
    source: &NativeSource,
    timestamp: &str,
    index: usize,
    item: &JsonValue,
    author: &str,
    recipient: &str,
) -> Vec<NativeEvent> {
    let turn_id = json_string(
        item,
        &["internal_chat_message_metadata_passthrough", "turn_id"],
    )
    .filter(|turn_id| !turn_id.trim().is_empty());
    let text = match decode_codex_internal_chat_payload(item, author, recipient) {
        Ok(Some(text)) => text,
        Ok(None) => return Vec::new(),
        Err(reason) => {
            log_codex_internal_progress_format_mismatch(source, index, item, reason);
            CODEX_INTERNAL_PROGRESS_FORMAT_WARNING.to_string()
        }
    };
    let mut progress = event(
        agent,
        source,
        "assistant",
        "commentary",
        timestamp,
        &index.to_string(),
        text,
    );
    // Routed child-agent updates are observations, not authoritative lifecycle
    // signals. A late child result must not re-activate an otherwise idle agent.
    progress.work_signal = None;
    progress.turn_id = turn_id;
    vec![progress]
}

fn decode_codex_internal_chat_payload(
    item: &JsonValue,
    author: &str,
    recipient: &str,
) -> Result<Option<String>, &'static str> {
    let content = item
        .get("content")
        .and_then(JsonValue::as_array)
        .ok_or("missing content array")?;
    let mut plaintext = String::new();
    for block in content {
        match json_string(block, &["type"]).as_deref() {
            Some("input_text") => {
                let text = json_string(block, &["text"]).ok_or("input_text missing text")?;
                if !plaintext.is_empty() && !plaintext.ends_with('\n') && !text.starts_with('\n') {
                    plaintext.push('\n');
                }
                plaintext.push_str(&text);
            }
            Some("encrypted_content") => {
                json_string(block, &["encrypted_content"])
                    .filter(|encrypted| !encrypted.trim().is_empty())
                    .ok_or("encrypted_content missing payload")?;
            }
            Some(_) => return Err("unsupported collaboration content block"),
            None => return Err("collaboration content block missing type"),
        }
    }
    if plaintext.is_empty() {
        return Err("missing plaintext collaboration frame");
    }

    let body = decode_codex_internal_chat_frame(&plaintext, author, recipient)?;
    let body = body.trim();
    Ok((!body.is_empty()).then(|| body.to_string()))
}

fn decode_codex_internal_chat_frame<'a>(
    text: &'a str,
    author: &str,
    recipient: &str,
) -> Result<&'a str, &'static str> {
    let mut task_name = None;
    let mut sender = None;
    let mut offset = 0;
    let mut body_start = None;

    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "Payload:" {
            body_start = Some(offset + segment.len());
            break;
        }
        let (key, value) =
            codex_internal_chat_header(line).ok_or("invalid collaboration header line")?;
        match key {
            "Task name" => {
                if task_name.replace(value).is_some() {
                    return Err("duplicate Task name header");
                }
            }
            "Sender" => {
                if sender.replace(value).is_some() {
                    return Err("duplicate Sender header");
                }
            }
            _ => {}
        }
        offset += segment.len();
    }

    let body_start = body_start.ok_or("missing Payload header")?;
    if task_name != Some(recipient) {
        return Err("Task name does not match recipient");
    }
    if sender != Some(author) {
        return Err("Sender does not match author");
    }
    Ok(&text[body_start..])
}

fn codex_internal_chat_header(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(':')?;
    let mut key_chars = key.chars();
    if !key_chars.next()?.is_ascii_alphabetic()
        || key.ends_with(' ')
        || !key_chars.all(|ch| ch.is_ascii_alphabetic() || ch == ' ')
    {
        return None;
    }
    let mut value_chars = rest.chars();
    if !value_chars.next()?.is_whitespace() {
        return None;
    }
    Some((key, value_chars.as_str().trim()))
}

fn log_codex_internal_progress_format_mismatch(
    source: &NativeSource,
    index: usize,
    item: &JsonValue,
    reason: &str,
) {
    let mut item_keys = item
        .as_object()
        .map(|item| item.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    item_keys.sort();
    let content_types = item
        .get("content")
        .and_then(JsonValue::as_array)
        .map(|content| {
            content
                .iter()
                .map(|block| {
                    json_string(block, &["type"]).unwrap_or_else(|| "<missing>".to_string())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    crate::kota_debug_log(&format!(
        "[violet] codex routed agent_message format mismatch session={} event={} reason={} item_keys={:?} content_types={:?}",
        source.session_id, index, reason, item_keys, content_types
    ));
}

fn codex_has_commentary_phase(
    json: &JsonValue,
    payload: &JsonValue,
    item: Option<&JsonValue>,
) -> bool {
    [Some(json), Some(payload), item]
        .into_iter()
        .flatten()
        .any(|value| {
            json_string(value, &["phase"])
                .map(|phase| phase.trim().eq_ignore_ascii_case("commentary"))
                .unwrap_or(false)
        })
}

fn codex_control_signal(value: &str) -> Option<&'static str> {
    let normalized = value
        .replace('-', "_")
        .replace('.', "_")
        .to_ascii_lowercase();
    let is_turn = normalized.contains("turn")
        || normalized.contains("response")
        || normalized.contains("task");
    if !is_turn {
        return None;
    }
    if normalized.contains("interrupted")
        || normalized.contains("cancel")
        || normalized.contains("abort")
    {
        return Some("interrupted");
    }
    if normalized.contains("failed") || normalized.contains("error") {
        return Some("failed");
    }
    if normalized.contains("completed")
        || normalized.contains("complete")
        || normalized.contains("finished")
    {
        return Some("completed");
    }
    if normalized.contains("started") || normalized.contains("start") {
        return Some("started");
    }
    None
}

#[derive(Default)]
struct AntigravityParseState {
    suppress_next_planner_text: bool,
}

fn parse_antigravity_source(
    project_root: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    let mut state = AntigravityParseState::default();
    parse_jsonl_source_incremental(
        project_root,
        agent,
        source,
        move |agent, source, index, json| {
            parse_antigravity_line_with_state(agent, source, index, json, &mut state)
        },
    )
}

fn parse_antigravity_line_with_state(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
    state: &mut AntigravityParseState,
) -> Vec<NativeEvent> {
    let starts_background_wait = antigravity_is_running_progress_trigger(&json);
    let keeps_background_wait_open =
        antigravity_is_background_wait_observation(&json) || antigravity_has_any_tool_call(&json);
    let suppress_current =
        state.suppress_next_planner_text && antigravity_is_planner_text_response(&json);

    if suppress_current || antigravity_is_system_notification_echo(&json) {
        state.suppress_next_planner_text = false;
        return Vec::new();
    }

    let events = parse_antigravity_line(agent, source, index, json);
    if starts_background_wait {
        state.suppress_next_planner_text = true;
    } else if !keeps_background_wait_open {
        state.suppress_next_planner_text = false;
    }
    events
}

fn antigravity_is_running_progress_trigger(json: &JsonValue) -> bool {
    antigravity_is_model_generic_running(json) || antigravity_is_running_task_observation(json)
}

fn antigravity_has_any_tool_call(json: &JsonValue) -> bool {
    json.get("tool_calls")
        .and_then(JsonValue::as_array)
        .is_some_and(|calls| !calls.is_empty())
}

fn antigravity_is_model_generic_running(json: &JsonValue) -> bool {
    antigravity_is_background_wait_observation(json)
        && json_string(json, &["status"])
            .is_some_and(|status| status.eq_ignore_ascii_case("RUNNING"))
}

fn antigravity_is_planner_text_response(json: &JsonValue) -> bool {
    json_string(json, &["source"]).is_some_and(|source| source == "MODEL")
        && json_string(json, &["type"]).is_some_and(|record_type| record_type == "PLANNER_RESPONSE")
        && json
            .get("content")
            .and_then(text_from_json)
            .is_some_and(|text| !text.trim().is_empty())
}

fn antigravity_is_background_wait_observation(json: &JsonValue) -> bool {
    json_string(json, &["source"]).is_some_and(|source| source == "MODEL")
        && json_string(json, &["type"]).is_some_and(|record_type| record_type == "GENERIC")
}

fn antigravity_is_running_task_observation(json: &JsonValue) -> bool {
    if !antigravity_is_background_wait_observation(json) {
        return false;
    }
    let Some(text) = json.get("content").and_then(text_from_json) else {
        return false;
    };
    let mut has_task = false;
    let mut is_running = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("Task: ") {
            has_task = true;
        } else if line.eq_ignore_ascii_case("Status: RUNNING") {
            is_running = true;
        }
    }
    has_task && is_running
}

fn antigravity_is_system_notification_echo(json: &JsonValue) -> bool {
    if !antigravity_is_planner_text_response(json) {
        return false;
    }
    let Some(text) = json.get("content").and_then(text_from_json) else {
        return false;
    };
    let text = text.trim_start();
    text.starts_with("An update occurred on background task ")
        || (text.starts_with("Task ") && text.contains(" has completed"))
}

fn parse_antigravity_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    index: usize,
    json: JsonValue,
) -> Vec<NativeEvent> {
    let status = json_string(&json, &["status"]);
    if status
        .as_deref()
        .is_some_and(|status| !antigravity_status_is_terminal(status))
    {
        return Vec::new();
    }

    let record_source = json_string(&json, &["source"]).unwrap_or_default();
    let record_type = json_string(&json, &["type"]).unwrap_or_default();
    let role = if record_source == "USER_EXPLICIT" && record_type == "USER_INPUT" {
        "user"
    } else if record_source == "MODEL" {
        "assistant"
    } else {
        return Vec::new();
    };

    let Some(mut text) = json
        .get("content")
        .and_then(text_from_json)
        .filter(|text| !text.trim().is_empty())
    else {
        return Vec::new();
    };
    if role == "user" {
        text = clean_antigravity_user_text(&text);
    }
    if role == "assistant" && is_antigravity_tool_observation(&record_type, &text) {
        return Vec::new();
    }
    if text.trim().is_empty() {
        return Vec::new();
    }

    let timestamp = json_string(&json, &["created_at"])
        .or_else(|| json_string(&json, &["timestamp"]))
        .unwrap_or_else(now_iso);
    let event_id = json_string(&json, &["id"])
        .or_else(|| json_number_string(&json, "step_index"))
        .unwrap_or_else(|| index.to_string());
    let content = JsonValue::String(text);
    let has_tool_calls = antigravity_has_any_tool_call(&json);
    let mut events = content_blocks_to_events(agent, source, role, &timestamp, &event_id, &content);
    if role == "assistant" {
        let signal = if has_tool_calls {
            "activity"
        } else {
            antigravity_work_signal(status.as_deref(), Some(record_type.as_str()))
        };
        for event in &mut events {
            if has_tool_calls {
                event.kind = "commentary".into();
            }
            event.work_signal = Some(signal.into());
            event.stop_reason = Some(record_type.clone());
        }
    }
    events
}

fn antigravity_status_is_terminal(status: &str) -> bool {
    let normalized = status.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "done"
            | "completed"
            | "complete"
            | "finished"
            | "success"
            | "succeeded"
            | "failed"
            | "error"
            | "cancelled"
            | "canceled"
            | "interrupted"
            | "aborted"
    )
}

fn antigravity_work_signal(status: Option<&str>, record_type: Option<&str>) -> &'static str {
    let text = [status.unwrap_or_default(), record_type.unwrap_or_default()]
        .join(" ")
        .to_ascii_lowercase();
    if text.contains("cancel") || text.contains("interrupt") || text.contains("abort") {
        "interrupted"
    } else if text.contains("fail") || text.contains("error") {
        "failed"
    } else {
        "completed"
    }
}

fn is_antigravity_tool_observation(record_type: &str, text: &str) -> bool {
    let normalized_type = record_type.trim().to_ascii_uppercase();
    matches!(
        normalized_type.as_str(),
        "VIEW_FILE"
            | "RUN_COMMAND"
            | "LIST_DIRECTORY"
            | "GREP_SEARCH"
            | "SEARCH_WEB"
            | "CODE_ACTION"
    ) || looks_like_antigravity_tool_observation_text(text)
}

fn looks_like_antigravity_tool_observation_text(text: &str) -> bool {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    lines
        .next()
        .is_some_and(|line| line.starts_with("Created At:"))
        && lines
            .next()
            .is_some_and(|line| line.starts_with("Completed At:"))
}

fn clean_antigravity_user_text(text: &str) -> String {
    extract_tag_block(text, "USER_REQUEST")
        .unwrap_or(text)
        .trim()
        .to_string()
}

fn extract_tag_block<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = text.find(&start_tag)? + start_tag.len();
    let end = text[start..].find(&end_tag)? + start;
    Some(&text[start..end])
}

fn json_number_string(value: &JsonValue, key: &str) -> Option<String> {
    let number = value.get(key)?;
    number
        .as_i64()
        .map(|value| value.to_string())
        .or_else(|| number.as_u64().map(|value| value.to_string()))
}

fn parse_opencode_source(
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    match source.kind.as_str() {
        "opencode-sqlite" => parse_opencode_sqlite(agent, source),
        _ => parse_opencode_message_dir(agent, source),
    }
}

fn parse_opencode_sqlite(
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    let conn = open_opencode_db(&source.path)?;
    let mut stmt = conn
        .prepare(
            "select id, coalesce(time_created, 0), data from (
                select id, time_created, data
                from message
                where session_id = ?1
                order by time_created desc, id desc
                limit ?2
            )
            order by time_created asc, id asc",
        )
        .map_err(|err| format!("prepare opencode message query: {err}"))?;
    let rows = stmt
        .query_map(
            params![source.session_id, JSON_MESSAGE_TAIL as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|err| format!("query opencode messages: {err}"))?;

    let mut out = Vec::new();
    for row in rows {
        let (message_id, created_millis, data) =
            row.map_err(|err| format!("read opencode message row: {err}"))?;
        let json: JsonValue = match serde_json::from_str(&data) {
            Ok(json) => json,
            Err(_) => continue,
        };
        let role = json_string(&json, &["role"]).unwrap_or_else(|| "assistant".into());
        let timestamp = opencode_timestamp(&json)
            .or_else(|| millis_to_iso(created_millis))
            .unwrap_or_else(now_iso);
        let mut parts = load_opencode_sqlite_parts(&conn, &source.session_id, &message_id)?;
        if parts.is_empty() {
            if let Some(part) = opencode_message_error_part(&json) {
                parts.push(part);
            }
        }
        for part in parts {
            let id = format!("{message_id}:{}", part.id);
            if part.kind == "control" {
                out.push(control_event(
                    agent,
                    source,
                    &timestamp,
                    &id,
                    part.work_signal.as_deref().unwrap_or("activity"),
                    part.reason,
                    None,
                ));
                continue;
            }
            if part.text.trim().is_empty() {
                continue;
            }
            let mut event = event(agent, source, &role, &part.kind, &timestamp, &id, part.text);
            if let Some(signal) = part.work_signal {
                event.work_signal = Some(signal);
            }
            event.stop_reason = part.reason;
            out.push(event);
        }
    }
    out.extend(parse_opencode_log_controls(agent, source)?);
    Ok(out)
}

fn parse_opencode_log_controls(
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    let Some(parent) = source.path.parent() else {
        return Ok(Vec::new());
    };
    let log_dir = parent.join("log");
    if !log_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in
        fs::read_dir(&log_dir).map_err(|err| format!("read {}: {err}", log_dir.display()))?
    {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", log_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        paths.push((modified, path));
    }
    paths.sort_by(|(left_time, left_path), (right_time, right_path)| {
        right_time
            .cmp(left_time)
            .then_with(|| right_path.cmp(left_path))
    });
    if paths.len() > OPENCODE_LOG_TAIL_FILES {
        paths.truncate(OPENCODE_LOG_TAIL_FILES);
    }
    paths.sort_by(|(left_time, left_path), (right_time, right_path)| {
        left_time
            .cmp(right_time)
            .then_with(|| left_path.cmp(right_path))
    });

    let mut out = Vec::new();
    for (_, path) in paths {
        let file_len = fs::metadata(&path)
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .len();
        let tail = read_jsonl_tail(&path, file_len, JSONL_TAIL_BYTES)?;
        let mut current_session_id: Option<String> = None;
        for (line_index, line) in tail.lines {
            if let Some(session_id) = opencode_log_token_value(&line, "session.id") {
                current_session_id = Some(session_id);
            }
            if let Some(event) = opencode_permission_event_from_log_line(
                agent,
                source,
                &path,
                line_index,
                &line,
                current_session_id.as_deref(),
            ) {
                out.push(event);
            }
        }
    }
    Ok(out)
}

fn opencode_permission_event_from_log_line(
    agent: &ProjectAgent,
    source: &NativeSource,
    path: &Path,
    line_index: usize,
    line: &str,
    current_session_id: Option<&str>,
) -> Option<NativeEvent> {
    if !line.contains("service=permission")
        || !line.contains(" asking")
        || !line.contains("permission=")
    {
        return None;
    }
    let session_id = opencode_log_token_value(line, "session.id")
        .or_else(|| current_session_id.map(str::to_string))?;
    if session_id != source.session_id {
        return None;
    }

    let permission_id =
        opencode_log_token_value(line, "id").unwrap_or_else(|| line_index.to_string());
    let permission =
        opencode_log_token_value(line, "permission").unwrap_or_else(|| "permission".into());
    let target =
        opencode_log_permission_target(line).unwrap_or_else(|| "the requested resource".into());
    let timestamp = opencode_log_timestamp(line).unwrap_or_else(now_iso);
    let text = format!(
        "Permission requested: OpenCode needs {permission} approval for {target}.\nOpen the agent terminal to approve or deny it."
    );

    Some(NativeEvent {
        session_id: source.session_id.clone(),
        agent_id: agent.agent_id.clone(),
        shell: agent.shell.clone(),
        role: "system".into(),
        kind: "control".into(),
        timestamp: normalize_timestamp(&timestamp),
        text,
        source_path: path.to_path_buf(),
        native_event_id: Some(format!("opencode-log:{permission_id}")),
        work_signal: Some("activity".into()),
        turn_id: None,
        stop_reason: Some("permission_requested".into()),
    })
}

fn opencode_log_timestamp(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let _level = parts.next()?;
    let timestamp = parts.next()?;
    Some(normalize_timestamp(&format!("{timestamp}+00:00")))
}

fn opencode_log_permission_target(line: &str) -> Option<String> {
    if let Some(patterns) = opencode_log_patterns(line) {
        if let Some(pattern) = patterns
            .into_iter()
            .find(|pattern| !pattern.trim().is_empty())
        {
            return Some(pattern);
        }
    }
    opencode_log_token_value(line, "pattern")
}

fn opencode_log_patterns(line: &str) -> Option<Vec<String>> {
    let rest = line.split_once("patterns=")?.1;
    if !rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    serde_json::from_str::<Vec<String>>(&rest[..=end]).ok()
}

fn opencode_log_token_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    for token in line.split_whitespace() {
        let Some(rest) = token.strip_prefix(&prefix) else {
            continue;
        };
        if let Some(rest) = rest.strip_prefix('"') {
            let end = rest.find('"')?;
            return Some(rest[..end].to_string());
        }
        let value = rest.trim_matches(',').trim_matches(';');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn parse_opencode_message_dir(
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Vec<NativeEvent>, String> {
    let mut message_paths = Vec::new();
    collect_matching_files(&source.path, &mut message_paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("json")
    })?;
    message_paths.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    if message_paths.len() > JSON_MESSAGE_TAIL {
        message_paths = message_paths.split_off(message_paths.len() - JSON_MESSAGE_TAIL);
    }

    let mut out = Vec::new();
    for path in message_paths {
        let text =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let json: JsonValue = match serde_json::from_str(&text) {
            Ok(json) => json,
            Err(_) => continue,
        };
        let role = json_string(&json, &["role"]).unwrap_or_else(|| "assistant".into());
        let timestamp = opencode_timestamp(&json).unwrap_or_else(now_iso);
        let event_id = json_string(&json, &["id"])
            .unwrap_or_else(|| file_stem(&path).unwrap_or_else(|| source_session_id(&path)));
        let mut parts = load_opencode_parts(source.aux_path.as_deref(), &event_id)?;
        if parts.is_empty() {
            if let Some(part) = opencode_message_error_part(&json) {
                parts.push(part);
            }
        }
        for (index, part) in parts.into_iter().enumerate() {
            let id = format!("{event_id}:{}", part.id);
            if part.kind == "control" {
                out.push(control_event(
                    agent,
                    source,
                    &timestamp,
                    &id,
                    part.work_signal.as_deref().unwrap_or("activity"),
                    part.reason,
                    None,
                ));
                continue;
            }
            if part.text.trim().is_empty() {
                continue;
            }
            let mut event = event(agent, source, &role, &part.kind, &timestamp, &id, part.text);
            if let Some(signal) = part.work_signal {
                event.work_signal = Some(signal);
            }
            event.stop_reason = part.reason;
            if event.native_event_id.is_none() {
                event.native_event_id = Some(format!("{event_id}:{index}"));
            }
            out.push(event);
        }
    }
    Ok(out)
}

fn open_opencode_db(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| format!("open opencode db {}: {err}", path.display()))
}

fn open_opencode_monitor_db(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| format!("open opencode monitor db {}: {err}", path.display()))?;
    conn.busy_timeout(StdDuration::from_millis(250))
        .map_err(|err| format!("set opencode monitor busy timeout: {err}"))?;
    conn.execute_batch("PRAGMA query_only = ON;")
        .map_err(|err| format!("set opencode monitor query_only: {err}"))?;
    Ok(conn)
}

fn opencode_data_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA data_version;", [], |row| row.get(0))
        .map_err(|err| format!("query opencode data_version: {err}"))
}

fn load_opencode_sqlite_parts(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
) -> Result<Vec<OpencodePartEvent>, String> {
    let mut stmt = conn
        .prepare(
            "select id, data
             from part
             where session_id = ?1 and message_id = ?2
             order by time_created asc, id asc",
        )
        .map_err(|err| format!("prepare opencode part query: {err}"))?;
    let rows = stmt
        .query_map(params![session_id, message_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| format!("query opencode parts: {err}"))?;

    let mut out = Vec::new();
    for row in rows {
        let (part_id, data) = row.map_err(|err| format!("read opencode part row: {err}"))?;
        let Ok(json) = serde_json::from_str::<JsonValue>(&data) else {
            continue;
        };
        if let Some(mut part) = opencode_part_event(&json) {
            part.id = part_id;
            out.push(part);
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct OpencodePartEvent {
    id: String,
    kind: String,
    text: String,
    work_signal: Option<String>,
    reason: Option<String>,
}

fn opencode_part_event(json: &JsonValue) -> Option<OpencodePartEvent> {
    let raw_kind = json_string(json, &["type"]).unwrap_or_else(|| "message".into());
    if raw_kind == "step-start" {
        return Some(OpencodePartEvent {
            id: "step-start".into(),
            kind: "control".into(),
            text: String::new(),
            work_signal: Some("activity".into()),
            reason: Some(raw_kind),
        });
    }
    if raw_kind == "step-finish" {
        let reason = json_string(json, &["reason"])
            .or_else(|| json_string(json, &["state", "reason"]))
            .unwrap_or_else(|| raw_kind.clone());
        let signal = opencode_work_signal_for_step_finish(&reason);
        return Some(OpencodePartEvent {
            id: "step-finish".into(),
            kind: "control".into(),
            text: String::new(),
            work_signal: Some(signal.into()),
            reason: Some(reason),
        });
    }
    let kind = if raw_kind.contains("tool") {
        "tool"
    } else if raw_kind.contains("reason") || raw_kind.contains("think") {
        "thinking"
    } else {
        "message"
    };
    let text = if kind == "tool" {
        json_string(json, &["state", "output"])
            .or_else(|| json_string(json, &["state", "metadata", "output"]))
            .or_else(|| json_string(json, &["output"]))
            .or_else(|| text_from_json(json))
    } else {
        json_string(json, &["text"])
            .or_else(|| json_string(json, &["content"]))
            .or_else(|| text_from_json(json))
    }?;
    (!text.trim().is_empty()).then_some(OpencodePartEvent {
        id: raw_kind.clone(),
        kind: kind.into(),
        text,
        work_signal: Some("activity".into()),
        reason: None,
    })
}

fn opencode_message_error_part(json: &JsonValue) -> Option<OpencodePartEvent> {
    let message = json_string(json, &["error", "data", "message"])
        .or_else(|| json_string(json, &["error", "message"]))
        .or_else(|| json_string(json, &["error", "name"]))?;
    let status = json
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("statusCode"))
        .and_then(|status| status.as_i64());
    let provider = json_string(json, &["providerID"]);
    let model = json_string(json, &["modelID"]);
    let source = match (provider, model) {
        (Some(provider), Some(model)) => format!(" from {provider}/{model}"),
        (Some(provider), None) => format!(" from {provider}"),
        _ => String::new(),
    };
    let status = status
        .map(|status| format!(" (HTTP {status})"))
        .unwrap_or_default();
    Some(OpencodePartEvent {
        id: "error".into(),
        kind: "message".into(),
        text: format!("OpenCode error{source}: {message}{status}."),
        work_signal: Some("failed".into()),
        reason: Some("error".into()),
    })
}

fn load_opencode_parts(
    part_root: Option<&Path>,
    message_id: &str,
) -> Result<Vec<OpencodePartEvent>, String> {
    let Some(part_root) = part_root else {
        return Ok(Vec::new());
    };
    let dir = part_root.join(message_id);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_matching_files(&dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("json")
    })?;
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let Ok(json) = serde_json::from_str::<JsonValue>(&text) else {
            continue;
        };
        if let Some(mut part) = opencode_part_event(&json) {
            if let Some(stem) = file_stem(&path) {
                part.id = stem;
            }
            out.push(part);
        }
    }
    Ok(out)
}

fn opencode_work_signal_for_step_finish(reason: &str) -> &'static str {
    let reason = reason.trim().to_ascii_lowercase();
    if reason.contains("tool") || reason.contains("continue") {
        "activity"
    } else if reason.contains("fail") || reason.contains("error") {
        "failed"
    } else if reason.contains("interrupt") || reason.contains("cancel") {
        "interrupted"
    } else {
        "completed"
    }
}

fn content_blocks_to_events(
    agent: &ProjectAgent,
    source: &NativeSource,
    role: &str,
    timestamp: &str,
    event_id: &str,
    content: &JsonValue,
) -> Vec<NativeEvent> {
    match content {
        JsonValue::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if role == "user" && is_ignorable_image_content_block(item) {
                    return None;
                }

                if let Some(event) = claude_ask_user_question_event(
                    agent, source, role, timestamp, event_id, index, item,
                ) {
                    return Some(event);
                }

                let block_type = json_string(item, &["type"]).unwrap_or_else(|| "message".into());
                let kind = if block_type.contains("thinking") || block_type.contains("reason") {
                    "thinking"
                } else if block_type.contains("tool") || block_type.contains("function") {
                    "tool"
                } else {
                    "message"
                };
                let text = json_string(item, &["text"])
                    .or_else(|| json_string(item, &["content"]))
                    .or_else(|| json_string(item, &["input"]))
                    .or_else(|| text_from_json(item));
                text.filter(|text| !text.trim().is_empty())
                    .filter(|text| !(role == "user" && is_ignorable_codex_user_wrapper_text(text)))
                    .map(|text| {
                        if role == "user" && is_codex_turn_aborted_wrapper_text(&text) {
                            return codex_turn_aborted_room_event(
                                agent,
                                source,
                                timestamp,
                                &format!("{event_id}:{index}"),
                            );
                        }
                        event(
                            agent,
                            source,
                            role,
                            kind,
                            timestamp,
                            &format!("{event_id}:{index}"),
                            text,
                        )
                    })
            })
            .collect(),
        JsonValue::String(text)
            if !text.trim().is_empty()
                && !(role == "user" && is_ignorable_codex_user_wrapper_text(text)) =>
        {
            if role == "user" && is_codex_turn_aborted_wrapper_text(text) {
                return vec![codex_turn_aborted_room_event(
                    agent, source, timestamp, event_id,
                )];
            }
            vec![event(
                agent,
                source,
                role,
                "message",
                timestamp,
                event_id,
                text.clone(),
            )]
        }
        _ => text_from_json(content)
            .filter(|text| !text.trim().is_empty())
            .filter(|text| !(role == "user" && is_ignorable_codex_user_wrapper_text(text)))
            .map(|text| {
                if role == "user" && is_codex_turn_aborted_wrapper_text(&text) {
                    return vec![codex_turn_aborted_room_event(
                        agent, source, timestamp, event_id,
                    )];
                }
                vec![event(
                    agent, source, role, "message", timestamp, event_id, text,
                )]
            })
            .unwrap_or_default(),
    }
}

fn claude_ask_user_question_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    role: &str,
    timestamp: &str,
    event_id: &str,
    index: usize,
    item: &JsonValue,
) -> Option<NativeEvent> {
    if role != "assistant" {
        return None;
    }
    let block_type = json_string(item, &["type"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !block_type.contains("tool") && !block_type.contains("function") {
        return None;
    }
    let name = json_string(item, &["name"])?;
    if !name.eq_ignore_ascii_case("AskUserQuestion") {
        return None;
    }

    let mut event = control_event(
        agent,
        source,
        timestamp,
        &format!("{event_id}:{index}"),
        "waiting_for_user",
        Some("user_question_requested".into()),
        None,
    );
    event.text = format_ask_user_question(item)?;
    Some(event)
}

fn format_ask_user_question(item: &JsonValue) -> Option<String> {
    let questions = item
        .get("input")
        .and_then(|input| input.get("questions"))
        .and_then(JsonValue::as_array)?;
    let mut sections = Vec::new();
    for question in questions {
        let mut lines = Vec::new();
        if let Some(header) = json_string(question, &["header"]) {
            if !header.trim().is_empty() {
                lines.push(header);
            }
        }
        if let Some(text) = json_string(question, &["question"]) {
            if !text.trim().is_empty() {
                lines.push(text);
            }
        }
        if let Some(options) = question.get("options").and_then(JsonValue::as_array) {
            let mut option_lines = Vec::new();
            for (index, option) in options.iter().enumerate() {
                let label = json_string(option, &["label"]).unwrap_or_default();
                if label.trim().is_empty() {
                    continue;
                }
                let description = json_string(option, &["description"]).unwrap_or_default();
                if description.trim().is_empty() {
                    option_lines.push(format!("{}. {}", index + 1, label.trim()));
                } else {
                    option_lines.push(format!(
                        "{}. {} - {}",
                        index + 1,
                        label.trim(),
                        description.trim()
                    ));
                }
            }
            if !option_lines.is_empty() {
                lines.push("Options:".into());
                lines.extend(option_lines);
            }
        }
        if !lines.is_empty() {
            sections.push(lines.join("\n"));
        }
    }
    (!sections.is_empty()).then(|| {
        format!(
            "User question requested by Claude.\nOpen the agent terminal to answer it.\n\n{}",
            sections.join("\n\n")
        )
    })
}

fn is_ignorable_image_content_block(item: &JsonValue) -> bool {
    json_string(item, &["type"])
        .map(|block_type| {
            let normalized = block_type.trim().to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "input_image" | "image" | "local_image" | "output_image"
            )
        })
        .unwrap_or(false)
}

fn is_codex_image_generation_event_type(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "image_generation_end" | "image_generation_call"
    )
}

fn is_codex_image_wrapper_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("</image>") {
        return true;
    }
    if trimmed.starts_with("<image") && trimmed.ends_with('>') {
        return true;
    }
    if let Some(rest) = trimmed
        .strip_prefix("[Image #")
        .and_then(|value| value.strip_suffix(']'))
    {
        return !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit());
    }
    false
}

fn is_ignorable_codex_user_wrapper_text(text: &str) -> bool {
    is_codex_image_wrapper_text(text) || is_codex_internal_context_wrapper_text(text)
}

fn is_codex_internal_context_wrapper_text(text: &str) -> bool {
    let trimmed = trim_terminal_envelope_padding(text);
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("<codex_internal_context")
}

fn is_ignorable_codex_internal_context_message(message: &VioletChatMessage) -> bool {
    message.shell == "codex"
        && message.role == "user"
        && is_codex_internal_context_wrapper_text(&message.text)
}

fn is_ignorable_codex_internal_context_event(event: &ChathistoryEvent) -> bool {
    event.shell == "codex"
        && event.role == "user"
        && is_codex_internal_context_wrapper_text(&event.text)
}

fn is_codex_turn_aborted_wrapper_text(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("<turn_aborted>")
        && lower.ends_with("</turn_aborted>")
        && lower.contains("interrupted the previous turn")
}

fn codex_turn_aborted_room_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    timestamp: &str,
    event_id: &str,
) -> NativeEvent {
    let mut event = event(
        agent,
        source,
        "system",
        "interrupt",
        timestamp,
        event_id,
        TURN_ABORTED_ROOM_TEXT.into(),
    );
    event.work_signal = Some("interrupted".into());
    event.stop_reason = Some("turn_aborted".into());
    event
}

fn event(
    agent: &ProjectAgent,
    source: &NativeSource,
    role: &str,
    kind: &str,
    timestamp: &str,
    event_id: &str,
    text: String,
) -> NativeEvent {
    let text = truncate_chars(&text, MAX_EVENT_TEXT_CHARS);
    let work_signal = match role {
        "assistant" => Some("activity".to_string()),
        "user" => Some("started".to_string()),
        _ => None,
    };
    NativeEvent {
        session_id: source.session_id.clone(),
        agent_id: agent.agent_id.clone(),
        shell: agent.shell.clone(),
        role: role.to_string(),
        kind: kind.to_string(),
        timestamp: normalize_timestamp(timestamp),
        text,
        source_path: source.path.clone(),
        native_event_id: Some(event_id.to_string()),
        work_signal,
        turn_id: None,
        stop_reason: None,
    }
}

fn control_event(
    agent: &ProjectAgent,
    source: &NativeSource,
    timestamp: &str,
    event_id: &str,
    signal: &str,
    reason: Option<String>,
    turn_id: Option<String>,
) -> NativeEvent {
    NativeEvent {
        session_id: source.session_id.clone(),
        agent_id: agent.agent_id.clone(),
        shell: agent.shell.clone(),
        role: "system".into(),
        kind: "control".into(),
        timestamp: normalize_timestamp(timestamp),
        text: reason.clone().unwrap_or_else(|| signal.to_string()),
        source_path: source.path.clone(),
        native_event_id: Some(event_id.to_string()),
        work_signal: Some(signal.to_string()),
        turn_id,
        stop_reason: reason,
    }
}

fn native_work_event(event: &NativeEvent) -> Option<AgentWorkEvent> {
    let signal = event.work_signal.as_deref()?;
    let state = match signal {
        "activity" | "started" => "working",
        "waiting_for_user" => "maybeIdle",
        "completed" => "idle",
        "failed" => "failed",
        "interrupted" => "interrupted",
        _ => return None,
    };
    Some(AgentWorkEvent {
        agent_id: event.agent_id.clone(),
        state: state.into(),
        timestamp: event.timestamp.clone(),
        cli: Some(event.shell.clone()),
        cwd: None,
        session_id: Some(event.session_id.clone()),
        turn_id: event.turn_id.clone(),
        reason: event
            .stop_reason
            .clone()
            .or_else(|| Some(signal.to_string())),
        source_path: Some(path_string(&event.source_path)),
        native_event_id: event.native_event_id.clone(),
    })
}

fn work_signal_for_stop_reason(reason: &str) -> &'static str {
    let reason = reason.trim().to_ascii_lowercase();
    match reason.as_str() {
        "user_question_requested" => "waiting_for_user",
        "tool_use" | "pause_turn" => "activity",
        "interrupted" | "cancelled" | "canceled" => "interrupted",
        "error" | "failed" => "failed",
        _ => "completed",
    }
}

fn partition_private(events: Vec<NativeEvent>, spans: &[PrivacySpan]) -> (Vec<NativeEvent>, usize) {
    let mut visible = Vec::new();
    let mut skipped = 0;
    for event in events {
        if is_private_event(&event, spans) {
            skipped += 1;
        } else {
            visible.push(event);
        }
    }
    (visible, skipped)
}

fn split_for_violet_outputs(
    events: Vec<NativeEvent>,
    project_root: &Path,
) -> (Vec<NativeEvent>, Vec<NativeEvent>) {
    let mut room_events = Vec::new();
    let mut shared_events = Vec::new();

    for event in events {
        if let Some(room_event) = room_event_for(&event, project_root) {
            room_events.push(room_event);
        }
        if let Some(shared_event) = shared_event_for(event, project_root) {
            shared_events.push(shared_event);
        }
    }

    (room_events, shared_events)
}

fn room_event_for(event: &NativeEvent, project_root: &Path) -> Option<NativeEvent> {
    if event.kind == "control" && !is_visible_control_event(event) {
        return None;
    }
    if is_internal_agent_bus_envelope_event(event) {
        return None;
    }
    if is_harness_envelope_text(&event.text) {
        return None;
    }
    if !matches!(event.role.as_str(), "user" | "assistant" | "system") {
        return None;
    }

    if event.kind == "thinking" {
        return None;
    }

    if event.kind == "tool" {
        return None;
    }

    let mut room_event = event.clone();
    room_event.text = clean_shared_log_text(project_root, &room_event.text);
    if room_event.text.is_empty() {
        return None;
    }
    Some(room_event)
}

fn is_visible_control_event(event: &NativeEvent) -> bool {
    matches!(
        event.stop_reason.as_deref(),
        Some("permission_requested" | "user_question_requested")
    )
}

fn shared_event_for(mut event: NativeEvent, project_root: &Path) -> Option<NativeEvent> {
    if event.kind != "message" && !(event.kind == "control" && is_visible_control_event(&event)) {
        return None;
    }
    if is_internal_agent_bus_envelope_event(&event) {
        return None;
    }
    if !matches!(event.role.as_str(), "user" | "assistant" | "system") {
        return None;
    }
    if is_harness_envelope_text(&event.text) {
        return None;
    }
    event.text = trim_terminal_control_padding(&event.text).to_string();
    event.text = clean_shared_log_text(project_root, &event.text);
    if event.text.trim().is_empty() || is_bootstrap_noise_text(&event.text) {
        return None;
    }
    Some(event)
}

fn tail_events(mut events: Vec<NativeEvent>, limit: usize) -> Vec<NativeEvent> {
    if events.len() <= limit {
        return events;
    }
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    events.split_off(events.len() - limit)
}

fn compare_room_message_order(
    left: &VioletChatMessage,
    right: &VioletChatMessage,
) -> std::cmp::Ordering {
    left.timestamp
        .cmp(&right.timestamp)
        .then(
            left.violet_seq
                .unwrap_or(0)
                .cmp(&right.violet_seq.unwrap_or(0)),
        )
        .then(left.id.cmp(&right.id))
}

fn compare_chathistory_event_order(
    left: &ChathistoryEvent,
    right: &ChathistoryEvent,
) -> std::cmp::Ordering {
    left.ts
        .cmp(&right.ts)
        .then(
            left.violet_seq
                .unwrap_or(0)
                .cmp(&right.violet_seq.unwrap_or(0)),
        )
        .then(left.id.cmp(&right.id))
}

fn dedupe_room_messages(mut messages: Vec<VioletChatMessage>) -> Vec<VioletChatMessage> {
    messages.sort_by(compare_room_message_order);
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let normalized = one_line(&message.text).to_lowercase();
        let bucket = DateTime::parse_from_rfc3339(&message.timestamp)
            .map(|time| time.timestamp() / 120)
            .unwrap_or(0);
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            message.agent_id, message.role, message.kind, bucket, normalized
        );
        if seen.insert(key) {
            out.push(message);
        }
    }
    out
}

fn dedupe_native_events(mut events: Vec<NativeEvent>) -> Vec<NativeEvent> {
    // Stable sorting keeps provider/source traversal order for exact timestamp
    // ties. Chathistory persists that last trustworthy order as `violet_seq`.
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(events.len());
    for event in events {
        let key = native_event_dedupe_key(&event);
        if seen.insert(key) {
            out.push(event);
        }
    }
    out
}

fn native_event_dedupe_key(event: &NativeEvent) -> String {
    if event.kind == "control" {
        return format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            event.agent_id,
            event.role,
            event.kind,
            event.timestamp,
            event.native_event_id.as_deref().unwrap_or_default(),
            event.turn_id.as_deref().unwrap_or_default(),
            event.stop_reason.as_deref().unwrap_or_default(),
        );
    }

    let normalized = one_line(&event.text).to_lowercase();
    let bucket = DateTime::parse_from_rfc3339(&event.timestamp)
        .map(|time| time.timestamp() / 120)
        .unwrap_or(0);
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        event.agent_id, event.role, event.kind, bucket, normalized
    )
}

fn is_private_event(event: &NativeEvent, spans: &[PrivacySpan]) -> bool {
    let Ok(timestamp) =
        DateTime::parse_from_rfc3339(&event.timestamp).map(|time| time.with_timezone(&Utc))
    else {
        return false;
    };
    spans
        .iter()
        .filter(|span| span.agent_id == event.agent_id)
        .any(|span| {
            let Ok(started) =
                DateTime::parse_from_rfc3339(&span.started_at).map(|time| time.with_timezone(&Utc))
            else {
                return false;
            };
            let start = started - Duration::seconds(PRIVATE_START_BUFFER_SECS);
            let end = span
                .ended_at
                .as_deref()
                .and_then(|ended| DateTime::parse_from_rfc3339(ended).ok())
                .map(|time| time.with_timezone(&Utc))
                .unwrap_or_else(|| Utc::now() + Duration::days(3650));
            timestamp >= start && timestamp <= end
        })
}

fn filter_bootstrap_noise(events: Vec<NativeEvent>) -> Vec<NativeEvent> {
    events
        .into_iter()
        .filter(|event| !is_bootstrap_noise(event))
        .collect()
}

fn filter_internal_agent_bus_envelopes(events: Vec<NativeEvent>) -> Vec<NativeEvent> {
    events
        .into_iter()
        .filter(|event| !is_internal_agent_bus_envelope_event(event))
        .collect()
}

fn is_internal_agent_bus_envelope_event(event: &NativeEvent) -> bool {
    event.role == "user" && event.kind == "message" && is_agent_bus_envelope_text(&event.text)
}

fn is_agent_bus_envelope_text(text: &str) -> bool {
    let trimmed = trim_terminal_envelope_padding(strip_leading_provider_attachment_markers(text));
    (trimmed.starts_with("<KOTA_MESSAGE ") || trimmed.starts_with("<KOTA_MESSAGE>"))
        && trimmed.ends_with("</KOTA_MESSAGE>")
}

fn strip_leading_provider_attachment_markers(mut text: &str) -> &str {
    text = trim_terminal_envelope_padding(text);
    for _ in 0..8 {
        let Some(rest) = text.strip_prefix('[') else {
            break;
        };
        let Some(end) = rest.find(']') else {
            break;
        };
        let marker = &rest[..end];
        if !is_provider_attachment_marker(marker) {
            break;
        }
        text = trim_terminal_envelope_padding(&rest[end + 1..]);
    }
    text
}

fn trim_terminal_envelope_padding(text: &str) -> &str {
    text.trim_matches(|ch: char| ch.is_whitespace() || ch.is_control())
}

fn trim_terminal_control_padding(text: &str) -> &str {
    text.trim_matches(|ch: char| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

fn is_provider_attachment_marker(marker: &str) -> bool {
    let Some((label, ordinal)) = marker.rsplit_once('#') else {
        return false;
    };
    let label = label.trim();
    let ordinal = ordinal.trim();
    !label.is_empty() && label.len() <= 40 && ordinal.chars().all(|ch| ch.is_ascii_digit())
}

fn is_bootstrap_noise(event: &NativeEvent) -> bool {
    if event.kind == "control" {
        return false;
    }
    is_bootstrap_noise_text(&event.text)
}

fn is_bootstrap_noise_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    if is_harness_envelope_text(trimmed) {
        return true;
    }

    if looks_like_codex_task_started(trimmed) {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    const SETUP_TAGS: &[&str] = &[
        "<permissions instructions",
        "<collaboration_mode",
        "<apps_instructions",
        "<skills_instructions",
        "<plugins_instructions",
        "<system_instruction",
        "<environment_context",
    ];
    if SETUP_TAGS.iter().any(|tag| lower.starts_with(tag)) {
        return true;
    }

    let first_line = trimmed.lines().next().unwrap_or_default();
    if first_line.starts_with("# AGENTS.md instructions for ")
        || first_line.starts_with("# CLAUDE.md instructions for ")
    {
        return true;
    }

    trimmed.contains("<!-- kota:adapter:")
        || trimmed.contains("<!-- kota:runtime-context:start -->")
        || trimmed.contains("<!-- kota:rule-index:start -->")
}

fn looks_like_codex_task_started(text: &str) -> bool {
    let Ok(json) = serde_json::from_str::<JsonValue>(text) else {
        return false;
    };
    json_string(&json, &["type"]).as_deref() == Some("task_started")
        && json.get("turn_id").is_some()
        && json.get("model_context_window").is_some()
}

/// Harness-injected envelopes that must never surface as room chat: local
/// slash-command wrappers, background-task notifications (real user-role
/// turns, so `isMeta` does not catch them), and whole-message
/// `<system-reminder>` blocks. Real typed prompts never start with these
/// tags; reminder blocks appended after real text fail the ends_with check
/// and are kept.
fn is_harness_envelope_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    let trimmed = trimmed
        .strip_prefix("[SYSTEM NOTIFICATION - NOT USER INPUT]")
        .map_or(trimmed, str::trim_start);
    (trimmed.starts_with("<command-name>") && trimmed.contains("</command-name>"))
        || (trimmed.starts_with("<local-command-") && trimmed.contains("</local-command-"))
        || (trimmed.starts_with("<task-notification>") && trimmed.contains("</task-notification>"))
        || (trimmed.starts_with("<system-reminder>")
            && trimmed.trim_end().ends_with("</system-reminder>"))
}

fn clean_shared_log_text(project_root: &Path, text: &str) -> String {
    let text = strip_ansi_codes(text);
    let text = redact_obvious_secrets(&text);
    normalize_shared_attachment_paths(project_root, &text)
        .trim()
        .to_string()
}

fn strip_ansi_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() || matches!(next, '@' | '~') {
                    break;
                }
            }
        }
    }
    out
}

fn redact_obvious_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            out.push_str(&redact_token(&token));
            token.clear();
            out.push(ch);
        } else {
            token.push(ch);
        }
    }
    out.push_str(&redact_token(&token));
    out
}

fn redact_token(token: &str) -> String {
    let trimmed = token.trim_matches(|ch: char| {
        matches!(ch, '\'' | '"' | '`' | ',' | ';' | ')' | '(' | '[' | ']')
    });
    if looks_like_secret_token(trimmed) {
        token.replace(trimmed, "[redacted]")
    } else {
        token.to_string()
    }
}

fn looks_like_secret_token(token: &str) -> bool {
    (token.starts_with("gho_") && token.len() > 12)
        || (token.starts_with("ghp_") && token.len() > 12)
        || (token.starts_with("github_pat_") && token.len() > 20)
        || (token.starts_with("sk-") && token.len() > 20)
}

fn normalize_shared_attachment_paths(project_root: &Path, text: &str) -> String {
    let root = path_string(project_root);
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return text.to_string();
    }
    text.replace(
        &format!("{root}/project-memory/attachments/"),
        "project-memory/attachments/",
    )
}

fn chathistory_dir(project_root: &Path) -> PathBuf {
    project_root.join("project-memory").join("chathistory")
}

fn chathistory_events_dir(project_root: &Path) -> PathBuf {
    chathistory_dir(project_root).join("events")
}

fn chathistory_latest_path(project_root: &Path) -> PathBuf {
    chathistory_dir(project_root).join("latest.jsonl")
}

fn chathistory_manifest_path(project_root: &Path) -> PathBuf {
    chathistory_dir(project_root).join("manifest.json")
}

fn ensure_chathistory_dirs(project_root: &Path) -> Result<(), String> {
    let dir = chathistory_dir(project_root);
    fs::create_dir_all(dir.join("events"))
        .map_err(|err| format!("create {}: {err}", dir.join("events").display()))?;
    fs::create_dir_all(dir.join("summaries"))
        .map_err(|err| format!("create {}: {err}", dir.join("summaries").display()))?;
    Ok(())
}

fn chathistory_lock_path(project_root: &Path) -> PathBuf {
    chathistory_dir(project_root).join(".write.lock")
}

fn chathistory_lock_key(project_root: &Path) -> PathBuf {
    project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
}

fn chathistory_project_mutex(project_root: &Path) -> Result<Arc<Mutex<()>>, String> {
    let key = chathistory_lock_key(project_root);
    let locks = CHATHISTORY_WRITE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| "chathistory write lock registry poisoned".to_string())?;
    Ok(locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

#[cfg(unix)]
struct ChathistoryFileLock {
    file: fs::File,
}

#[cfg(unix)]
impl Drop for ChathistoryFileLock {
    fn drop(&mut self) {
        unsafe {
            let _ = flock(self.file.as_raw_fd(), CHATHISTORY_LOCK_UN);
        }
    }
}

#[cfg(unix)]
fn acquire_chathistory_file_lock(project_root: &Path) -> Result<ChathistoryFileLock, String> {
    let path = chathistory_lock_path(project_root);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid chathistory lock path: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    let result = unsafe { flock(file.as_raw_fd(), CHATHISTORY_LOCK_EX) };
    if result != 0 {
        return Err(format!(
            "lock {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(ChathistoryFileLock { file })
}

#[cfg(not(unix))]
struct ChathistoryFileLock;

#[cfg(not(unix))]
fn acquire_chathistory_file_lock(_project_root: &Path) -> Result<ChathistoryFileLock, String> {
    Ok(ChathistoryFileLock)
}

fn with_chathistory_write_lock<T>(
    project_root: &Path,
    op: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mutex = chathistory_project_mutex(project_root)?;
    let _guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _file_lock = acquire_chathistory_file_lock(project_root)?;
    op()
}

pub fn preserve_project_agent_left_identity(
    project_root: &Path,
    agent_id: &str,
) -> Result<(), String> {
    preserve_project_agent_identity_status(project_root, agent_id, "left")
}

pub fn preserve_project_agent_archived_identity(
    project_root: &Path,
    agent_id: &str,
) -> Result<(), String> {
    preserve_project_agent_identity_status(project_root, agent_id, "archived")
}

pub fn preserve_project_agent_active_identity(
    project_root: &Path,
    agent_id: &str,
) -> Result<(), String> {
    preserve_project_agent_identity_status(project_root, agent_id, "active")
}

fn preserve_project_agent_identity_status(
    project_root: &Path,
    agent_id: &str,
    status: &str,
) -> Result<(), String> {
    let Some(mut snapshot) = load_chathistory_agent_snapshot(project_root, agent_id)? else {
        return Ok(());
    };
    snapshot.status = Some(status.into());
    backfill_chathistory_agent_snapshot(project_root, agent_id, &snapshot)
}

fn load_chathistory_agent_snapshots(
    project_root: &Path,
) -> Result<HashMap<String, AgentIdentitySnapshot>, String> {
    let agents_root = project_root.join(".agent-workspaces");
    let mut snapshots = HashMap::new();
    if !agents_root.is_dir() {
        return Ok(snapshots);
    }
    for entry in fs::read_dir(&agents_root)
        .map_err(|err| format!("read {}: {err}", agents_root.display()))?
    {
        let entry = entry.map_err(|err| err.to_string())?;
        let cwd = entry.path();
        if !cwd.is_dir() || !cwd.join("agent.yaml").is_file() {
            continue;
        }
        let Some(agent_id) = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        match load_chathistory_agent_snapshot_from_cwd(&cwd, &agent_id) {
            Ok(Some(snapshot)) => {
                snapshots.insert(agent_id, snapshot);
            }
            Ok(None) => {}
            Err(err) => eprintln!("[violet] skipped agent identity snapshot for {agent_id}: {err}"),
        }
    }
    Ok(snapshots)
}

fn load_chathistory_agent_snapshot(
    project_root: &Path,
    agent_id: &str,
) -> Result<Option<AgentIdentitySnapshot>, String> {
    load_chathistory_agent_snapshot_from_cwd(
        &project_root.join(".agent-workspaces").join(agent_id),
        agent_id,
    )
}

fn load_chathistory_agent_snapshot_from_cwd(
    cwd: &Path,
    agent_id: &str,
) -> Result<Option<AgentIdentitySnapshot>, String> {
    let Some(agent_yaml) = read_yaml_file(&cwd.join("agent.yaml"))? else {
        return Ok(None);
    };
    let shell_yaml = read_yaml_file(&cwd.join("SHELL.yaml"))?;
    let provider = shell_yaml
        .as_ref()
        .and_then(|yaml| yaml_string(yaml, "provider"))
        .or_else(|| yaml_string(&agent_yaml, "provider"))
        .or_else(|| {
            shell_yaml
                .as_ref()
                .and_then(|yaml| yaml_string(yaml, "command"))
        });
    Ok(Some(AgentIdentitySnapshot {
        display_name: yaml_string(&agent_yaml, "display-name")
            .or_else(|| yaml_string(&agent_yaml, "displayName"))
            .or_else(|| Some(agent_id.to_string())),
        avatar_id: yaml_string(&agent_yaml, "avatar-id")
            .or_else(|| yaml_string(&agent_yaml, "avatarId")),
        provider,
        status: yaml_string(&agent_yaml, "status").or_else(|| Some("active".into())),
    }))
}

fn backfill_chathistory_agent_snapshot(
    project_root: &Path,
    agent_id: &str,
    snapshot: &AgentIdentitySnapshot,
) -> Result<(), String> {
    with_chathistory_write_lock(project_root, || {
        backfill_chathistory_agent_snapshot_locked(project_root, agent_id, snapshot)
    })
}

fn backfill_chathistory_agent_snapshot_locked(
    project_root: &Path,
    agent_id: &str,
    snapshot: &AgentIdentitySnapshot,
) -> Result<(), String> {
    ensure_chathistory_dirs(project_root)?;
    let events_dir = chathistory_events_dir(project_root);
    if !events_dir.is_dir() {
        return Ok(());
    }
    let mut changed = false;
    for entry in
        fs::read_dir(&events_dir).map_err(|err| format!("read {}: {err}", events_dir.display()))?
    {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let mut events = read_chathistory_event_file(&path)?;
        let mut file_changed = false;
        for event in events.iter_mut().filter(|event| event.agent_id == agent_id) {
            file_changed |= apply_agent_snapshot_to_event(event, snapshot);
        }
        if file_changed {
            let bytes = render_chathistory_events(&events)?;
            write_if_changed(&path, bytes.as_bytes())?;
            changed = true;
        }
    }
    if changed {
        write_chathistory_latest_locked(project_root)?;
        write_chathistory_manifest_locked(project_root)?;
    }
    Ok(())
}

fn apply_agent_snapshot_to_event(
    event: &mut ChathistoryEvent,
    snapshot: &AgentIdentitySnapshot,
) -> bool {
    let mut changed = false;
    if snapshot.display_name.is_some() && event.agent_display_name != snapshot.display_name {
        event.agent_display_name = snapshot.display_name.clone();
        changed = true;
    }
    if snapshot.avatar_id.is_some() && event.agent_avatar_id != snapshot.avatar_id {
        event.agent_avatar_id = snapshot.avatar_id.clone();
        changed = true;
    }
    if snapshot.provider.is_some() && event.agent_provider != snapshot.provider {
        event.agent_provider = snapshot.provider.clone();
        changed = true;
    }
    if snapshot.status.is_some() && event.agent_status != snapshot.status {
        event.agent_status = snapshot.status.clone();
        changed = true;
    }
    changed
}

fn write_chathistory_messages(
    project_root: &Path,
    messages: &[VioletChatMessage],
) -> Result<Vec<PathBuf>, String> {
    with_chathistory_write_lock(project_root, || {
        write_chathistory_messages_locked(project_root, messages)
    })
}

fn write_chathistory_messages_locked(
    project_root: &Path,
    messages: &[VioletChatMessage],
) -> Result<Vec<PathBuf>, String> {
    ensure_chathistory_dirs(project_root)?;
    if messages.is_empty() {
        write_chathistory_manifest_locked(project_root)?;
        return Ok(vec![chathistory_manifest_path(project_root)]);
    }

    let mut incoming = messages
        .iter()
        .filter(|message| {
            !message.text.trim().is_empty()
                && !is_bootstrap_noise_text(&message.text)
                && !is_ignorable_codex_internal_context_message(message)
        })
        .cloned()
        .collect::<Vec<_>>();
    if incoming.is_empty() {
        write_chathistory_manifest_locked(project_root)?;
        return Ok(vec![chathistory_manifest_path(project_root)]);
    }

    let events_dir = chathistory_events_dir(project_root);
    let days = incoming
        .iter()
        .map(|message| chathistory_day_key(&message.timestamp))
        .collect::<HashSet<_>>();
    let mut existing_by_day = BTreeMap::new();
    let mut existing_seq_by_id = HashMap::new();
    for day in days {
        let path = events_dir.join(format!("{day}.jsonl"));
        let events = read_chathistory_event_file(&path)?;
        for event in &events {
            existing_seq_by_id.insert(event.id.clone(), event.violet_seq);
        }
        existing_by_day.insert(day, events);
    }

    let mut assigned_seq_by_id = HashMap::new();
    let mut allocation_ids = Vec::new();
    let mut allocation_seen = HashSet::new();
    let mut minimum_next_seq = FIRST_VIOLET_SEQ;
    for message in &incoming {
        if existing_seq_by_id.contains_key(&message.id) {
            continue;
        }
        if let Some(seq) = message.violet_seq.filter(|seq| *seq > 0) {
            assigned_seq_by_id.entry(message.id.clone()).or_insert(seq);
            minimum_next_seq = minimum_next_seq.max(seq.saturating_add(1));
        } else if allocation_seen.insert(message.id.clone()) {
            allocation_ids.push(message.id.clone());
        }
    }
    allocation_ids.retain(|id| !assigned_seq_by_id.contains_key(id));

    let current_next_seq = read_or_recover_next_violet_seq_locked(project_root)?;
    let mut next_seq = current_next_seq.max(minimum_next_seq);
    for id in allocation_ids {
        if next_seq > MAX_SAFE_VIOLET_SEQ {
            return Err(
                "Violet message sequence exhausted the JavaScript-safe integer range.".into(),
            );
        }
        assigned_seq_by_id.insert(id, next_seq);
        next_seq = next_seq
            .checked_add(1)
            .ok_or_else(|| "Violet message sequence overflowed.".to_string())?;
    }
    if next_seq != current_next_seq {
        // Reserve before writing event files. A crash can leave harmless gaps,
        // but can never cause a sequence to be reused.
        write_chathistory_manifest_with_next_seq_locked(project_root, next_seq)?;
    }

    for message in &mut incoming {
        if let Some(existing_seq) = existing_seq_by_id.get(&message.id) {
            message.violet_seq = *existing_seq;
        } else if let Some(seq) = assigned_seq_by_id.get(&message.id) {
            message.violet_seq = Some(*seq);
        }
    }

    let agent_snapshots = load_chathistory_agent_snapshots(project_root)?;
    let mut by_day = existing_by_day;
    for message in &incoming {
        by_day
            .entry(chathistory_day_key(&message.timestamp))
            .or_default()
            .push(chathistory_event_from_message(
                message,
                agent_snapshots.get(&message.agent_id),
            ));
    }

    let mut changed_paths = Vec::new();
    for (day, mut events) in by_day {
        let path = events_dir.join(format!("{day}.jsonl"));
        events = dedupe_chathistory_events(events);
        events.retain(|event| !is_ignorable_codex_internal_context_event(event));
        let bytes = render_chathistory_events(&events)?;
        write_if_changed(&path, bytes.as_bytes())?;
        changed_paths.push(path);
    }

    write_chathistory_latest_locked(project_root)?;
    write_chathistory_manifest_locked(project_root)?;
    changed_paths.push(chathistory_latest_path(project_root));
    changed_paths.push(chathistory_manifest_path(project_root));
    Ok(changed_paths)
}

fn write_turn_credit_events_for_messages(
    project_root: &Path,
    messages: &[VioletChatMessage],
) -> Result<(), String> {
    let mut turn_messages = messages
        .iter()
        .filter(|message| message_counts_as_turn(message))
        .peekable();
    if turn_messages.peek().is_none() {
        return Ok(());
    }

    let mut existing_source_ids = read_turn_credit_source_ids(project_root)?;
    let mut hero_ids_by_agent: HashMap<String, Option<String>> = HashMap::new();
    let project_id = project_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Project".into());

    for message in turn_messages {
        let source_event_id = message.id.clone();
        if !existing_source_ids.insert(source_event_id.clone()) {
            continue;
        }
        let hero_id = hero_ids_by_agent
            .entry(message.agent_id.clone())
            .or_insert_with(|| source_hero_id_for_agent(project_root, &message.agent_id))
            .clone();
        let hero_id_value = hero_id.as_deref().unwrap_or("unknown");
        let mut event = serde_json::json!({
            "event": "turn",
            "hero_id": hero_id_value,
            "agent_id": message.agent_id.as_str(),
            "incarnation_id": message.agent_id.as_str(),
            "project_id": project_id.as_str(),
            "source": "violet-chathistory",
            "source_event_id": source_event_id,
            "session_id": message.session_id.as_str(),
            "occurred_at": message.timestamp.as_str(),
            "turn_count": 1,
        });
        if let Some(native_event_id) = message.native_event_id.as_deref() {
            if let Some(map) = event.as_object_mut() {
                map.insert(
                    "native_event_id".into(),
                    serde_json::Value::String(native_event_id.to_string()),
                );
            }
        }
        if let Some(source_path) = message.source_path.as_deref() {
            if let Some(map) = event.as_object_mut() {
                map.insert(
                    "source_path".into(),
                    serde_json::Value::String(source_path.to_string()),
                );
            }
        }

        crate::append_project_credit_event(project_root, &event)?;
        if let Some(hero_id) = hero_id
            .as_deref()
            .filter(|id| !id.trim().is_empty() && *id != "unknown")
        {
            crate::append_tavern_hero_credit_event(hero_id, &event)?;
        }
    }

    Ok(())
}

fn message_counts_as_turn(message: &VioletChatMessage) -> bool {
    message.role == "assistant"
        && message.kind == "message"
        && !message.text.trim().is_empty()
        && !is_bootstrap_noise_text(&message.text)
}

fn read_turn_credit_source_ids(project_root: &Path) -> Result<HashSet<String>, String> {
    let path = crate::credit_events_path(project_root);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(HashSet::new());
    };
    let mut ids = HashSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_str::<JsonValue>(line) else {
            continue;
        };
        if json_string(&value, &["event"]).as_deref() != Some("turn") {
            continue;
        }
        if let Some(source_id) = json_string(&value, &["source_event_id"])
            .or_else(|| json_string(&value, &["sourceEventId"]))
            .or_else(|| json_string(&value, &["message_id"]))
            .or_else(|| json_string(&value, &["messageId"]))
        {
            ids.insert(source_id);
        }
    }
    Ok(ids)
}

fn source_hero_id_for_agent(project_root: &Path, agent_id: &str) -> Option<String> {
    let yaml = read_yaml_file(
        &project_root
            .join(".agent-workspaces")
            .join(agent_id)
            .join("agent.yaml"),
    )
    .ok()
    .flatten()?;
    yaml_string(&yaml, "recruited-from")
        .or_else(|| yaml_nested_string(&yaml, &["source", "hero-id"]))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id != "unknown")
}

#[allow(dead_code)]
fn remove_chathistory_session(project_root: &Path, session_id: &str) -> Result<(), String> {
    with_chathistory_write_lock(project_root, || {
        remove_chathistory_session_locked(project_root, session_id)
    })
}

fn remove_chathistory_session_locked(project_root: &Path, session_id: &str) -> Result<(), String> {
    let events_dir = chathistory_events_dir(project_root);
    if !events_dir.is_dir() {
        return Ok(());
    }
    let mut paths = Vec::new();
    collect_matching_files(&events_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })?;
    let mut changed = false;
    for path in paths {
        let events = read_chathistory_event_file(&path)?;
        let original_len = events.len();
        let next = events
            .into_iter()
            .filter(|event| event.source.session_id != session_id)
            .collect::<Vec<_>>();
        if next.is_empty() {
            match fs::remove_file(&path) {
                Ok(()) => changed = true,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(format!("remove {}: {err}", path.display())),
            }
        } else if next.len() != original_len {
            let bytes = render_chathistory_events(&next)?;
            write_if_changed(&path, bytes.as_bytes())?;
            changed = true;
        }
    }
    if changed {
        write_chathistory_latest_locked(project_root)?;
        write_chathistory_manifest_locked(project_root)?;
    }
    Ok(())
}

fn write_chathistory_latest(project_root: &Path) -> Result<(), String> {
    with_chathistory_write_lock(project_root, || {
        write_chathistory_latest_locked(project_root)
    })
}

fn write_chathistory_latest_locked(project_root: &Path) -> Result<(), String> {
    let mut events =
        read_chathistory_events_for_latest(project_root, CHATHISTORY_LATEST_LIMIT * 2)?;
    events = dedupe_chathistory_events(events);
    events.retain(|event| !is_ignorable_codex_internal_context_event(event));
    if events.len() > CHATHISTORY_LATEST_LIMIT {
        events = events.split_off(events.len() - CHATHISTORY_LATEST_LIMIT);
    }
    let bytes = render_chathistory_events(&events)?;
    write_if_changed(&chathistory_latest_path(project_root), bytes.as_bytes())
}

fn write_chathistory_manifest_locked(project_root: &Path) -> Result<(), String> {
    let next_seq = read_or_recover_next_violet_seq_locked(project_root)?;
    write_chathistory_manifest_with_next_seq_locked(project_root, next_seq)
}

fn write_chathistory_manifest_with_next_seq_locked(
    project_root: &Path,
    next_seq: u64,
) -> Result<(), String> {
    ensure_chathistory_dirs(project_root)?;
    let manifest = serde_json::json!({
        "kind": "kota.violet.chathistory",
        "version": 1,
        "next_seq": next_seq.max(FIRST_VIOLET_SEQ),
        "latest_limit": CHATHISTORY_LATEST_LIMIT,
        "events": "events/YYYY-MM-DD.jsonl",
        "latest": "latest.jsonl",
        "summaries": "summaries/",
        "summary_log": "summaries/recent.json"
    });
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| format!("serialize chathistory manifest: {err}"))?;
    write_if_changed(&chathistory_manifest_path(project_root), &bytes)
}

fn read_or_recover_next_violet_seq_locked(project_root: &Path) -> Result<u64, String> {
    let manifest_path = chathistory_manifest_path(project_root);
    if let Ok(bytes) = fs::read(&manifest_path) {
        if let Ok(manifest) = serde_json::from_slice::<JsonValue>(&bytes) {
            if let Some(next_seq) = manifest
                .get("next_seq")
                .and_then(JsonValue::as_u64)
                .filter(|next_seq| *next_seq >= FIRST_VIOLET_SEQ)
            {
                return Ok(next_seq);
            }
        }
    }

    let max_persisted = read_chathistory_event_segments(project_root)?
        .into_iter()
        .filter_map(|event| event.violet_seq)
        .max()
        .unwrap_or(0);
    Ok(max_persisted
        .checked_add(1)
        .unwrap_or(MAX_SAFE_VIOLET_SEQ)
        .max(FIRST_VIOLET_SEQ))
}

fn build_summary_state(
    project_root: &Path,
    error: Option<String>,
) -> Result<VioletSummaryState, String> {
    let log = read_summary_log(project_root)?;
    let events = read_summary_count_events(project_root)?;
    let history = valid_summary_entries(&log);
    let latest = history.first().cloned();
    let outstanding = outstanding_summary_events(&events, latest.as_ref());
    let since_ts = latest
        .as_ref()
        .map(|entry| entry.summary_end_ts.clone())
        .or_else(|| outstanding.first().map(|event| event.ts.clone()));
    Ok(VioletSummaryState {
        latest,
        history,
        outstanding: VioletSummaryOutstanding {
            since_ts,
            message_count: outstanding.len(),
        },
        log_path: VIOLET_SUMMARY_LOG_PATH.into(),
        prompt_path: summary_prompt_path(),
        updated_at: Utc::now().to_rfc3339(),
        error,
    })
}

fn normalized_summary_config(config: Option<&VioletSummaryConfig>) -> VioletSummaryConfig {
    let provider = config
        .and_then(|config| config.provider.as_deref())
        .map(normalize_summary_provider)
        .unwrap_or_else(|| "codex".into());
    VioletSummaryConfig {
        provider: Some(provider),
        trigger_a_messages: Some(
            config
                .and_then(|config| config.trigger_a_messages)
                .filter(|value| *value > 0)
                .unwrap_or(30),
        ),
        trigger_b_hours: Some(
            config
                .and_then(|config| config.trigger_b_hours)
                .filter(|value| *value > 0)
                .unwrap_or(2),
        ),
        trigger_b_min_outstanding: Some(
            config
                .and_then(|config| config.trigger_b_min_outstanding)
                .filter(|value| *value > 0)
                .unwrap_or(5),
        ),
    }
}

fn normalize_summary_provider(value: &str) -> String {
    if value.trim().eq_ignore_ascii_case("claude") {
        "claude".into()
    } else {
        "codex".into()
    }
}

fn automatic_summary_trigger(
    state: &VioletSummaryState,
    config: &VioletSummaryConfig,
) -> Option<&'static str> {
    let outstanding = state.outstanding.message_count;
    if outstanding >= config.trigger_a_messages.unwrap_or(30) {
        return Some("auto-trigger-a");
    }
    let latest = state.latest.as_ref()?;
    let hours = config.trigger_b_hours.unwrap_or(2) as i64;
    let min_outstanding = config.trigger_b_min_outstanding.unwrap_or(5);
    let last_summary = DateTime::parse_from_rfc3339(&latest.updated_at)
        .ok()?
        .with_timezone(&Utc);
    if Utc::now().signed_duration_since(last_summary) >= Duration::hours(hours)
        && outstanding > min_outstanding
    {
        return Some("auto-trigger-b");
    }
    None
}

fn recent_summary_failure_cooldown(
    project_root: &Path,
    state: &VioletSummaryState,
    config: &VioletSummaryConfig,
) -> Result<Option<String>, String> {
    if state.outstanding.message_count == 0 {
        return Ok(None);
    }
    let provider = config
        .provider
        .as_deref()
        .map(normalize_summary_provider)
        .unwrap_or_else(|| "codex".into());
    let log = read_summary_log(project_root)?;
    let Some(entry) = log.entries.first() else {
        return Ok(None);
    };
    let Some(cli_error) = entry.cli_error.as_ref() else {
        return Ok(None);
    };
    if entry.provider != provider {
        return Ok(None);
    }
    let events = read_summary_count_events(project_root)?;
    let outstanding = outstanding_summary_events(&events, state.latest.as_ref());
    if outstanding.is_empty()
        || entry.message_count != outstanding.len()
        || outstanding.last().map(|event| event.id.as_str()) != Some(entry.last_event_id.as_str())
    {
        return Ok(None);
    }
    let updated_at = DateTime::parse_from_rfc3339(&entry.updated_at)
        .ok()
        .map(|dt| dt.with_timezone(&Utc));
    let Some(updated_at) = updated_at else {
        return Ok(None);
    };
    let age_secs = Utc::now()
        .signed_duration_since(updated_at)
        .num_seconds()
        .max(0);
    if age_secs >= VIOLET_SUMMARY_FAILURE_COOLDOWN_SECS {
        return Ok(None);
    }
    let remaining = VIOLET_SUMMARY_FAILURE_COOLDOWN_SECS - age_secs;
    Ok(Some(format!(
        "Violet summary auto retry paused for {remaining}s after recent failure: {cli_error}"
    )))
}

struct SummaryRunGuard {
    key: PathBuf,
}

impl Drop for SummaryRunGuard {
    fn drop(&mut self) {
        if let Some(runs) = VIOLET_SUMMARY_RUNS.get() {
            if let Ok(mut runs) = runs.lock() {
                runs.remove(&self.key);
            }
        }
    }
}

fn try_acquire_summary_run(project_root: &Path) -> Result<Option<SummaryRunGuard>, String> {
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let runs = VIOLET_SUMMARY_RUNS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut runs = runs
        .lock()
        .map_err(|_| "Violet summary run guard poisoned".to_string())?;
    if !runs.insert(key.clone()) {
        return Ok(None);
    }
    Ok(Some(SummaryRunGuard { key }))
}

struct EmberDreamRunGuard {
    key: PathBuf,
}

impl Drop for EmberDreamRunGuard {
    fn drop(&mut self) {
        if let Some(runs) = EMBER_DREAM_RUNS.get() {
            if let Ok(mut runs) = runs.lock() {
                runs.remove(&self.key);
            }
        }
    }
}

fn try_acquire_ember_dream_run() -> Result<Option<EmberDreamRunGuard>, String> {
    let key = ember_dreams_root();
    let runs = EMBER_DREAM_RUNS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut runs = runs
        .lock()
        .map_err(|_| "Ember dream run guard poisoned".to_string())?;
    if !runs.insert(key.clone()) {
        return Ok(None);
    }
    Ok(Some(EmberDreamRunGuard { key }))
}

fn run_summary_for_outstanding(
    project_root: &Path,
    config: &VioletSummaryConfig,
    trigger: &str,
) -> Result<Option<VioletSummaryEntry>, String> {
    let mut log = read_summary_log(project_root)?;
    let events = read_summary_count_events(project_root)?;
    let latest = log.entries.first().cloned();
    let outstanding = outstanding_summary_events(&events, latest.as_ref());
    if outstanding.is_empty() {
        return Ok(None);
    }
    let provider = config
        .provider
        .as_deref()
        .map(normalize_summary_provider)
        .unwrap_or_else(|| "codex".into());
    let previous_summary = latest
        .as_ref()
        .and_then(|entry| serde_json::to_value(entry).ok())
        .unwrap_or(JsonValue::Null);
    let slice_start = outstanding
        .first()
        .map(|event| event.ts.clone())
        .unwrap_or_else(now_iso);
    let slice_end = outstanding
        .last()
        .map(|event| event.ts.clone())
        .unwrap_or_else(now_iso);
    let last_event_id = outstanding
        .last()
        .map(|event| event.id.clone())
        .unwrap_or_default();
    let prompt = render_summary_prompt(
        project_root,
        &previous_summary,
        &outstanding,
        &slice_start,
        &slice_end,
    )?;
    let completed = match run_summary_cli(&provider, project_root, &prompt)
        .and_then(|output| parse_summary_cli_output(&output))
    {
        Ok(completed) if !completed.is_empty() => completed,
        Ok(_) => {
            let err = "summary CLI returned no completed bullets".to_string();
            record_summary_failure(
                project_root,
                &mut log,
                trigger,
                &provider,
                &slice_start,
                &slice_end,
                &last_event_id,
                outstanding.len(),
                &err,
            )?;
            return Err(err);
        }
        Err(err) => {
            record_summary_failure(
                project_root,
                &mut log,
                trigger,
                &provider,
                &slice_start,
                &slice_end,
                &last_event_id,
                outstanding.len(),
                &err,
            )?;
            return Err(err);
        }
    };
    let entry = build_summary_log_entry(
        trigger,
        &provider,
        &slice_start,
        &slice_end,
        &last_event_id,
        outstanding.len(),
        completed,
        None,
    );
    write_summary_log_entry(project_root, &mut log, entry.clone())?;
    Ok(Some(entry))
}

fn record_summary_failure(
    project_root: &Path,
    log: &mut VioletSummaryLog,
    trigger: &str,
    provider: &str,
    slice_start: &str,
    slice_end: &str,
    last_event_id: &str,
    message_count: usize,
    error: &str,
) -> Result<(), String> {
    let entry = build_summary_log_entry(
        trigger,
        provider,
        slice_start,
        slice_end,
        last_event_id,
        message_count,
        Vec::new(),
        Some(error.to_string()),
    );
    write_summary_log_entry(project_root, log, entry)
}

fn build_summary_log_entry(
    trigger: &str,
    provider: &str,
    slice_start: &str,
    slice_end: &str,
    last_event_id: &str,
    message_count: usize,
    completed: Vec<String>,
    cli_error: Option<String>,
) -> VioletSummaryEntry {
    VioletSummaryEntry {
        id: stable_summary_id(slice_start, slice_end, last_event_id),
        updated_at: Utc::now().to_rfc3339(),
        trigger: trigger.into(),
        provider: provider.into(),
        summary_start_ts: slice_start.into(),
        summary_end_ts: slice_end.into(),
        message_count,
        completed,
        last_event_id: last_event_id.into(),
        log_path: VIOLET_SUMMARY_LOG_PATH.into(),
        cli_error,
    }
}

fn write_summary_log_entry(
    project_root: &Path,
    log: &mut VioletSummaryLog,
    entry: VioletSummaryEntry,
) -> Result<(), String> {
    log.version = 1;
    log.updated_at = entry.updated_at.clone();
    log.entries.insert(0, entry);
    log.entries.truncate(VIOLET_SUMMARY_HISTORY_LIMIT);
    write_summary_log(project_root, log)
}

fn read_summary_log(project_root: &Path) -> Result<VioletSummaryLog, String> {
    let path = summary_log_path(project_root);
    if !path.is_file() {
        return Ok(VioletSummaryLog {
            version: 1,
            updated_at: Utc::now().to_rfc3339(),
            entries: Vec::new(),
        });
    }
    let text =
        fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    serde_json::from_str::<VioletSummaryLog>(&text)
        .map_err(|err| format!("parse {}: {err}", path.display()))
}

fn valid_summary_entries(log: &VioletSummaryLog) -> Vec<VioletSummaryEntry> {
    log.entries
        .iter()
        .filter(|entry| entry.cli_error.is_none())
        .cloned()
        .collect()
}

fn write_summary_log(project_root: &Path, log: &VioletSummaryLog) -> Result<(), String> {
    let path = summary_log_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let bytes =
        serde_json::to_vec_pretty(log).map_err(|err| format!("serialize summary log: {err}"))?;
    fs::write(&path, bytes).map_err(|err| format!("write {}: {err}", path.display()))
}

fn summary_log_path(project_root: &Path) -> PathBuf {
    chathistory_dir(project_root)
        .join("summaries")
        .join("recent.json")
}

fn read_summary_count_events(project_root: &Path) -> Result<Vec<ChathistoryEvent>, String> {
    let mut events = read_chathistory_event_segments(project_root)?;
    events = dedupe_chathistory_events(events);
    events.retain(is_summary_count_event);
    events.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.id.cmp(&b.id)));
    Ok(events)
}

fn is_summary_count_event(event: &ChathistoryEvent) -> bool {
    event.display
        && event.kind == "message"
        && matches!(event.role.as_str(), "user" | "assistant")
        && !is_project_agent_lifecycle_notification(event)
        && !is_harness_envelope_text(&event.text)
        && !is_ember_dream_entry_event(event)
        && !is_ignorable_codex_internal_context_event(event)
        && !event.text.trim().is_empty()
}

fn outstanding_summary_events(
    events: &[ChathistoryEvent],
    latest: Option<&VioletSummaryEntry>,
) -> Vec<ChathistoryEvent> {
    let Some(latest) = latest else {
        return events.to_vec();
    };
    if let Some(index) = events
        .iter()
        .position(|event| event.id == latest.last_event_id)
    {
        return events[index + 1..].to_vec();
    }
    events
        .iter()
        .filter(|event| message_timestamp_before(&latest.summary_end_ts, &event.ts))
        .cloned()
        .collect()
}

fn render_summary_prompt(
    project_root: &Path,
    previous_summary: &JsonValue,
    events: &[ChathistoryEvent],
    slice_start: &str,
    slice_end: &str,
) -> Result<String, String> {
    let chathistory_slice_json = events.iter().map(summary_prompt_event).collect::<Vec<_>>();
    let chathistory_slice_json = serde_json::to_string_pretty(&chathistory_slice_json)
        .map_err(|err| format!("serialize summary chathistory slice: {err}"))?;
    let previous_summary_json = serde_json::to_string_pretty(previous_summary)
        .map_err(|err| format!("serialize previous summary: {err}"))?;
    let template = read_summary_prompt_template();
    Ok(template
        .replace("{{project_root}}", &path_string(project_root))
        .replace("{{summary_log_path}}", VIOLET_SUMMARY_LOG_PATH)
        .replace("{{previous_summary_json}}", &previous_summary_json)
        .replace("{{chathistory_slice_json}}", &chathistory_slice_json)
        .replace("{{events_json}}", &chathistory_slice_json)
        .replace("{{slice_start_ts}}", slice_start)
        .replace("{{slice_end_ts}}", slice_end)
        .replace("{{message_count}}", &events.len().to_string()))
}

fn read_summary_prompt_template() -> String {
    crate::read_system_prompt_template_content(
        VIOLET_SUMMARY_PROMPT_FILE,
        VIOLET_SUMMARY_PROMPT_TEMPLATE,
    )
}

fn summary_prompt_path() -> String {
    crate::system_prompt_template_path(VIOLET_SUMMARY_PROMPT_FILE)
}

fn summary_prompt_event(event: &ChathistoryEvent) -> JsonValue {
    serde_json::json!({
        "id": event.id,
        "ts": event.ts,
        "role": event.role,
        "agent_id": event.agent_id,
        "agent_display_name": event.agent_display_name,
        "text": event.text,
    })
}

#[cfg(not(test))]
fn run_summary_cli(provider: &str, project_root: &Path, prompt: &str) -> Result<String, String> {
    let home = dirs::home_dir();
    let env_path = crate::pty::path_env::augmented_path(home.as_deref());
    let mut command = if provider == "claude" {
        let bin = crate::pty::path_env::resolve_on_augmented_path("claude", home.as_deref());
        let mut command = Command::new(bin);
        command.args([
            "-p",
            "--output-format",
            "text",
            "--no-session-persistence",
            "--permission-mode",
            "bypassPermissions",
        ]);
        command
    } else {
        let bin = crate::pty::path_env::resolve_on_augmented_path("codex", home.as_deref());
        let mut command = Command::new(bin);
        command.args([
            "exec",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--color",
            "never",
        ]);
        command
    };
    let stdout_path = summary_cli_temp_path("stdout");
    let stderr_path = summary_cli_temp_path("stderr");
    let stdout_file = fs::File::create(&stdout_path)
        .map_err(|err| format!("create {}: {err}", stdout_path.display()))?;
    let stderr_file = fs::File::create(&stderr_path)
        .map_err(|err| format!("create {}: {err}", stderr_path.display()))?;
    command
        .current_dir(project_root)
        .env("PATH", &env_path)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    configure_summary_process_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = fs::remove_file(&stdout_path);
            let _ = fs::remove_file(&stderr_path);
            return Err(format!("spawn {provider} summary CLI: {err}"));
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            kill_summary_child(&mut child);
            let _ = child.wait();
            let _ = read_and_remove_summary_cli_output(&stdout_path, &stderr_path);
            return Err(format!("open {provider} summary stdin"));
        }
    };
    let prompt = prompt.as_bytes().to_vec();
    let (stdin_tx, stdin_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = stdin
            .write_all(&prompt)
            .map_err(|err| format!("write summary prompt: {err}"));
        drop(stdin);
        let _ = stdin_tx.send(result);
    });
    let start = Instant::now();
    let mut stdin_finished = false;
    loop {
        if !stdin_finished {
            match stdin_rx.try_recv() {
                Ok(Ok(())) => stdin_finished = true,
                Ok(Err(err)) => {
                    kill_summary_child(&mut child);
                    let _ = child.wait();
                    let _ = read_and_remove_summary_cli_output(&stdout_path, &stderr_path);
                    return Err(format!("{provider} summary CLI {err}"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => stdin_finished = true,
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, stderr) =
                    read_and_remove_summary_cli_output(&stdout_path, &stderr_path)?;
                if status.success() {
                    return Ok(stdout);
                }
                return Err(format!(
                    "{provider} summary CLI exited {}: {}",
                    status,
                    stderr.trim()
                ));
            }
            Ok(None) => {
                if start.elapsed() >= StdDuration::from_secs(VIOLET_SUMMARY_CLI_TIMEOUT_SECS) {
                    kill_summary_child(&mut child);
                    let _ = child.wait();
                    let (_, stderr) =
                        read_and_remove_summary_cli_output(&stdout_path, &stderr_path)?;
                    let detail = trimmed_cli_output_tail(&stderr);
                    if detail.is_empty() {
                        return Err(format!("{provider} summary CLI timed out"));
                    }
                    return Err(format!("{provider} summary CLI timed out: {detail}"));
                }
                thread::sleep(StdDuration::from_millis(100));
            }
            Err(err) => return Err(format!("poll {provider} summary CLI: {err}")),
        }
    }
}

#[cfg(not(test))]
fn summary_cli_temp_path(kind: &str) -> PathBuf {
    let id = WRITE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "kota-violet-summary-{}-{id}-{kind}.log",
        std::process::id()
    ))
}

#[cfg(all(not(test), unix))]
fn configure_summary_process_group(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(all(not(test), not(unix)))]
fn configure_summary_process_group(_command: &mut Command) {}

#[cfg(all(not(test), unix))]
fn kill_summary_child(child: &mut Child) {
    let pid = child.id() as i32;
    unsafe {
        let _ = kill(-pid, SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(all(not(test), not(unix)))]
fn kill_summary_child(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(not(test))]
fn read_and_remove_summary_cli_output(
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(String, String), String> {
    let stdout = read_lossy_file(stdout_path)?;
    let stderr = read_lossy_file(stderr_path)?;
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
    Ok((stdout, stderr))
}

#[cfg(not(test))]
fn read_lossy_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[cfg(not(test))]
fn trimmed_cli_output_tail(text: &str) -> String {
    const MAX_CHARS: usize = 600;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.into();
    }
    let tail = trimmed
        .chars()
        .rev()
        .take(MAX_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[cfg(test)]
fn run_summary_cli(_provider: &str, _project_root: &Path, _prompt: &str) -> Result<String, String> {
    Ok(r#"{"completed":["Recorded completed room work."]}"#.into())
}

fn parse_summary_cli_output(output: &str) -> Result<Vec<String>, String> {
    let trimmed = output.trim();
    for (start, _) in trimmed.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&trimmed[start..])
            .into_iter::<VioletSummaryModelOutput>();
        if let Some(Ok(parsed)) = stream.next() {
            return Ok(clean_summary_bullets(parsed.completed));
        }
    }
    Err("summary CLI output did not contain valid summary JSON".into())
}

fn clean_summary_bullets(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        let item = item
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim()
            .to_string();
        if item.is_empty() {
            continue;
        }
        // Prompt allows a 280-char paragraph; leave headroom so a compliant
        // summary is never chopped into "..." by the sanitizer.
        let item = truncate_chars(&item, 360);
        if seen.insert(item.clone()) {
            out.push(item);
        }
        if out.len() >= 5 {
            break;
        }
    }
    if out.len() > 5 {
        out.truncate(5);
    }
    out
}

fn run_ember_dream_consolidation(
    project_roots: &[PathBuf],
    request: &EmberDreamConsolidateRequest,
) -> Result<(usize, usize), String> {
    ensure_ember_dream_dirs()?;
    let processed_ids = read_processed_dream_entry_ids()?;
    let mut project_entries = Vec::with_capacity(project_roots.len());
    for project_root in project_roots {
        project_entries.push(collect_unprocessed_dream_entries(
            project_root,
            &processed_ids,
        )?);
    }
    let collected_entries = project_entries
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if collected_entries.is_empty() {
        return Ok((0, 0));
    }
    let current_dreams = read_active_dream_entries()?;
    let (prompt_items, candidate_items) =
        build_ember_dream_consolidation_items(&current_dreams, &project_entries);
    if candidate_items.is_empty() {
        append_dream_entry_records(&collected_entries)?;
        return Ok((collected_entries.len(), 0));
    }
    let prompt = render_ember_dream_consolidation_prompt(&prompt_items)?;
    let provider = request
        .provider
        .as_deref()
        .map(normalize_summary_provider)
        .unwrap_or_else(|| "codex".into());
    let decisions = run_summary_cli(&provider, &project_roots[0], &prompt)
        .and_then(|output| parse_ember_dream_cli_output(&output))?;
    let decisions = validate_ember_dream_decisions(&prompt_items, decisions)?;
    let result = apply_ember_dream_decisions(
        &current_dreams,
        &candidate_items,
        &decisions,
        EMBER_DREAM_MAX_ACTIVE_ENTRIES,
    )?;
    append_dream_entry_records(&collected_entries)?;
    if !result.archived.is_empty() {
        append_old_dreams(&result.archived)?;
    }
    write_active_dreams_markdown(&result.active, &provider, collected_entries.len())?;
    Ok((collected_entries.len(), result.archived.len()))
}

fn build_ember_dream_state(
    processed_entry_count: usize,
    archived_entry_count: usize,
    error: Option<String>,
) -> Result<EmberDreamConsolidateState, String> {
    ensure_ember_dream_dirs()?;
    let active_entry_count = read_active_dream_entries()?.len();
    Ok(EmberDreamConsolidateState {
        account_dreams_path: path_string(&ember_dreams_path()),
        entries_dir: path_string(&ember_dream_entries_dir()),
        old_dreams_path: path_string(&ember_old_dreams_path()),
        prompt_path: ember_dream_consolidate_prompt_path(),
        processed_entry_count,
        active_entry_count,
        archived_entry_count,
        updated_at: Utc::now().to_rfc3339(),
        error,
    })
}

fn parse_ember_dream_cli_output(output: &str) -> Result<Vec<EmberDreamDecision>, String> {
    let trimmed = output.trim();
    for (start, _) in trimmed.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&trimmed[start..])
            .into_iter::<EmberDreamConsolidationModelOutput>();
        if let Some(Ok(parsed)) = stream.next() {
            return Ok(parsed.decisions);
        }
    }
    Err("Ember dream CLI output did not contain valid dream JSON".into())
}

fn render_ember_dream_consolidation_prompt(
    items: &[EmberDreamPromptItem],
) -> Result<String, String> {
    let items_json = serde_json::to_string_pretty(items)
        .map_err(|err| format!("serialize Dream consolidation items: {err}"))?;
    let template = read_ember_dream_consolidate_prompt_template();
    Ok(template.replace("{{items_json}}", &items_json))
}

fn build_ember_dream_consolidation_items(
    current_dreams: &[String],
    project_entries: &[Vec<EmberDreamEntryRecord>],
) -> (Vec<EmberDreamPromptItem>, Vec<EmberDreamCandidateItem>) {
    let mut prompt_items = current_dreams
        .iter()
        .enumerate()
        .map(|(index, text)| EmberDreamPromptItem {
            id: ember_active_dream_id(index),
            kind: "active".into(),
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    let mut candidate_items = Vec::new();
    for (project_index, entries) in project_entries.iter().enumerate() {
        for entry in entries {
            for text in split_ember_dream_entry_items(&entry.text) {
                let id = format!("candidate-{}", candidate_items.len() + 1);
                prompt_items.push(EmberDreamPromptItem {
                    id: id.clone(),
                    kind: "candidate".into(),
                    text: text.clone(),
                });
                candidate_items.push(EmberDreamCandidateItem {
                    id,
                    project_index,
                    text,
                });
            }
        }
    }
    (prompt_items, candidate_items)
}

fn ember_active_dream_id(index: usize) -> String {
    format!("active-{}", index + 1)
}

fn read_ember_dream_consolidate_prompt_template() -> String {
    crate::read_system_prompt_template_content(
        EMBER_DREAM_CONSOLIDATE_PROMPT_FILE,
        EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE,
    )
}

fn ember_dream_consolidate_prompt_path() -> String {
    crate::system_prompt_template_path(EMBER_DREAM_CONSOLIDATE_PROMPT_FILE)
}

fn collect_unprocessed_dream_entries(
    project_root: &Path,
    processed_ids: &HashSet<String>,
) -> Result<Vec<EmberDreamEntryRecord>, String> {
    let mut events = read_chathistory_event_segments(project_root)?;
    events.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.id.cmp(&b.id)));
    let mut out = Vec::new();
    for event in events {
        if processed_ids.contains(&event.id) || !is_ember_dream_entry_event(&event) {
            continue;
        }
        let Some(text) = extract_ember_dream_entry_text(&event.text) else {
            continue;
        };
        out.push(EmberDreamEntryRecord {
            event_id: event.id,
            ts: event.ts,
            agent_id: event.agent_id,
            agent_display_name: event.agent_display_name,
            text,
        });
    }
    Ok(out)
}

fn is_ember_dream_entry_event(event: &ChathistoryEvent) -> bool {
    event.kind == "message"
        && event.role == "assistant"
        && event.agent_id != "ember"
        && extract_ember_dream_entry_text(&event.text).is_some()
}

fn extract_ember_dream_entry_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A real dream entry IS the wrapper: the whole message must START with the
    // opening tag and END with the closing tag. Messages that merely quote or
    // discuss the wrapper (e.g. iterating on the dream prompt itself) carry text
    // around it and must NOT be treated as a dream entry — otherwise they get
    // silently filtered out of the room.
    if trimmed.starts_with(EMBER_DREAM_ENTRY_START) && trimmed.ends_with(EMBER_DREAM_ENTRY_END) {
        let body_start = EMBER_DREAM_ENTRY_START.len();
        let body_end = trimmed.len() - EMBER_DREAM_ENTRY_END.len();
        if body_end >= body_start {
            let body = trimmed[body_start..body_end].trim();
            if !body.is_empty() {
                return Some(body.to_string());
            }
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("dream entry")
        || lower.starts_with("**dream entry**")
        || lower.starts_with("### dream entry")
        || lower.starts_with("## dream entry")
        || lower.starts_with("# dream entry")
    {
        return Some(trimmed.to_string());
    }
    None
}

fn is_empty_ember_dream_entry_text(text: &str) -> bool {
    clean_dream_bullet_text(text) == EMBER_DREAM_EMPTY_MARKER
}

fn split_ember_dream_entry_items(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let heading = line
            .trim_matches(['#', '*'])
            .trim()
            .trim_end_matches(':')
            .to_ascii_lowercase();
        if heading == "dream entry" {
            continue;
        }
        let bullet = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "));
        if let Some(bullet) = bullet {
            if !current.is_empty() {
                items.push(current);
            }
            current = bullet.trim().to_string();
        } else if current.is_empty() {
            current = line.to_string();
        } else {
            current.push(' ');
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
        .into_iter()
        .map(|item| sanitize_ember_dream_text(&item))
        .filter(|item| !item.is_empty() && !is_empty_dream_placeholder(item))
        .collect()
}

fn sanitize_ember_dream_text(value: &str) -> String {
    let cleaned = clean_dream_bullet_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&cleaned, 240)
}

fn validate_ember_dream_decisions(
    items: &[EmberDreamPromptItem],
    decisions: Vec<EmberDreamDecision>,
) -> Result<HashMap<String, EmberDreamDecisionAction>, String> {
    let items_by_id = items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let mut actions = HashMap::new();
    for decision in decisions {
        let id = decision.id.trim();
        let Some(item) = items_by_id.get(id) else {
            return Err(format!("Ember dream decision used unknown id: {id}"));
        };
        if actions.contains_key(id) {
            return Err(format!("Ember dream decision repeated id: {id}"));
        }
        let action = match decision.op.trim().to_ascii_lowercase().as_str() {
            "keep" => EmberDreamDecisionAction::Keep,
            "drop" => EmberDreamDecisionAction::Drop,
            "rewrite" => {
                let text = sanitize_ember_dream_text(decision.text.as_deref().unwrap_or_default());
                if text.is_empty() || is_empty_dream_placeholder(&text) {
                    return Err(format!(
                        "Ember dream rewrite requires non-empty text for id: {id}"
                    ));
                }
                if normalized_dream_key(&text) == normalized_dream_key(&item.text) {
                    EmberDreamDecisionAction::Keep
                } else {
                    EmberDreamDecisionAction::Rewrite(text)
                }
            }
            op => {
                return Err(format!(
                    "Ember dream decision used unknown op '{op}' for id: {id}"
                ))
            }
        };
        actions.insert(id.to_string(), action);
    }
    let missing = items
        .iter()
        .filter(|item| !actions.contains_key(&item.id))
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Ember dream decisions omitted ids: {}",
            missing.join(", ")
        ));
    }
    Ok(actions)
}

fn apply_ember_dream_decisions(
    current: &[String],
    candidates: &[EmberDreamCandidateItem],
    decisions: &HashMap<String, EmberDreamDecisionAction>,
    max_active: usize,
) -> Result<EmberDreamApplyResult, String> {
    let mut kept = Vec::new();
    let mut refreshed = Vec::new();
    let mut archived = Vec::new();
    let mut seen = HashSet::new();

    for (index, text) in current.iter().enumerate() {
        let id = ember_active_dream_id(index);
        let action = decisions
            .get(&id)
            .ok_or_else(|| format!("missing validated Ember dream decision for id: {id}"))?;
        match action {
            EmberDreamDecisionAction::Keep => {
                if seen.insert(normalized_dream_key(text)) {
                    kept.push(text.clone());
                } else {
                    archived.push(text.clone());
                }
            }
            EmberDreamDecisionAction::Drop => archived.push(text.clone()),
            EmberDreamDecisionAction::Rewrite(rewritten) => {
                archived.push(text.clone());
                if seen.insert(normalized_dream_key(rewritten)) {
                    refreshed.push(rewritten.clone());
                }
            }
        }
    }

    let mut proposals = Vec::new();
    for candidate in candidates {
        let action = decisions.get(&candidate.id).ok_or_else(|| {
            format!(
                "missing validated Ember dream decision for id: {}",
                candidate.id
            )
        })?;
        let text = match action {
            EmberDreamDecisionAction::Keep => candidate.text.clone(),
            EmberDreamDecisionAction::Drop => continue,
            EmberDreamDecisionAction::Rewrite(rewritten) => rewritten.clone(),
        };
        if seen.insert(normalized_dream_key(&text)) {
            proposals.push(EmberDreamCandidateProposal {
                project_index: candidate.project_index,
                text,
            });
        }
    }

    if refreshed.len() > max_active {
        let overflow = refreshed.len() - max_active;
        archived.extend(refreshed.drain(0..overflow));
    }
    let candidate_capacity = max_active.saturating_sub(refreshed.len());
    let selected_candidates = fair_order_ember_dream_candidates(proposals)
        .into_iter()
        .take(candidate_capacity)
        .map(|candidate| candidate.text)
        .collect::<Vec<_>>();
    let kept_capacity = max_active.saturating_sub(refreshed.len() + selected_candidates.len());
    if kept.len() > kept_capacity {
        let overflow = kept.len() - kept_capacity;
        archived.extend(kept.drain(0..overflow));
    }

    let mut active = kept;
    active.extend(refreshed);
    active.extend(selected_candidates);
    Ok(EmberDreamApplyResult { active, archived })
}

fn fair_order_ember_dream_candidates(
    candidates: Vec<EmberDreamCandidateProposal>,
) -> Vec<EmberDreamCandidateProposal> {
    let mut queues = BTreeMap::<usize, VecDeque<EmberDreamCandidateProposal>>::new();
    for candidate in candidates {
        queues
            .entry(candidate.project_index)
            .or_default()
            .push_back(candidate);
    }
    let mut ordered = Vec::new();
    loop {
        let mut added = false;
        for queue in queues.values_mut() {
            if let Some(candidate) = queue.pop_front() {
                ordered.push(candidate);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    ordered
}

fn read_processed_dream_entry_ids() -> Result<HashSet<String>, String> {
    let dir = ember_dream_entries_dir();
    if !dir.is_dir() {
        return Ok(HashSet::new());
    }
    let mut paths = Vec::new();
    collect_matching_files(&dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })?;
    let mut ids = HashSet::new();
    for path in paths {
        let file =
            fs::File::open(&path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<EmberDreamEntryRecord>(&line) {
                ids.insert(record.event_id);
            }
        }
    }
    Ok(ids)
}

fn append_dream_entry_records(records: &[EmberDreamEntryRecord]) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    ensure_ember_dream_dirs()?;
    let path = ember_dream_entries_dir().join(format!("{}.jsonl", Utc::now().format("%Y-%m-%d")));
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    for record in records {
        let line = serde_json::to_string(record)
            .map_err(|err| format!("serialize dream entry record: {err}"))?;
        writeln!(file, "{line}").map_err(|err| format!("write {}: {err}", path.display()))?;
    }
    Ok(())
}

fn read_active_dream_entries() -> Result<Vec<String>, String> {
    let path = ember_dreams_path();
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let mut entries = Vec::new();
    let mut in_active = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            in_active = trimmed == "## Active Dreams";
            continue;
        }
        if in_active && trimmed.starts_with("- ") {
            let entry = clean_dream_bullet_text(trimmed);
            if !entry.is_empty() && !is_empty_dream_placeholder(&entry) {
                entries.push(entry);
            }
        }
    }
    if entries.is_empty() {
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("- ") || trimmed == "- Never" {
                continue;
            }
            let entry = clean_dream_bullet_text(trimmed);
            if !entry.is_empty() && !is_empty_dream_placeholder(&entry) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

fn write_active_dreams_markdown(
    active: &[String],
    provider: &str,
    processed_count: usize,
) -> Result<(), String> {
    ensure_ember_dream_dirs()?;
    let now = Utc::now().to_rfc3339();
    let mut out = String::new();
    out.push_str("# Dreams\n\n");
    out.push_str("Dreams are what Kota has learned about the user in the past: durable preferences, fun facts, user-life context, recurring workflows, and open threads that help future agents continue naturally.\n\n");
    out.push_str("## Active Dreams\n");
    if active.is_empty() {
        out.push_str("- _No dream entries yet._\n");
    } else {
        for entry in active {
            out.push_str("- ");
            out.push_str(entry);
            out.push('\n');
        }
    }
    out.push_str("\n## Last Dream\n");
    out.push_str(&format!(
        "- {now}, provider: {provider}, processed entries: {processed_count}\n"
    ));
    write_if_changed(&ember_dreams_path(), out.as_bytes())
}

fn append_old_dreams(entries: &[String]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    ensure_ember_dream_dirs()?;
    let path = ember_old_dreams_path();
    let existed = path.is_file();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    if !existed {
        writeln!(file, "# Old Dreams\n")
            .map_err(|err| format!("write {}: {err}", path.display()))?;
    }
    writeln!(file, "## {}", Utc::now().to_rfc3339())
        .map_err(|err| format!("write {}: {err}", path.display()))?;
    for entry in entries {
        writeln!(file, "- {}", clean_dream_bullet_text(entry))
            .map_err(|err| format!("write {}: {err}", path.display()))?;
    }
    writeln!(file).map_err(|err| format!("write {}: {err}", path.display()))
}

fn ensure_ember_dream_dirs() -> Result<(), String> {
    fs::create_dir_all(ember_dream_entries_dir())
        .map_err(|err| format!("create {}: {err}", ember_dream_entries_dir().display()))?;
    fs::create_dir_all(ember_dream_archive_dir())
        .map_err(|err| format!("create {}: {err}", ember_dream_archive_dir().display()))?;
    Ok(())
}

fn ember_dreams_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Kota")
        .join("dreams")
}

fn ember_dreams_path() -> PathBuf {
    ember_dreams_root().join("dreams.md")
}

fn ember_old_dreams_path() -> PathBuf {
    ember_dreams_root().join("old_dreams.md")
}

fn ember_dream_entries_dir() -> PathBuf {
    ember_dreams_root().join("entries")
}

fn ember_dream_archive_dir() -> PathBuf {
    ember_dreams_root().join("archive")
}

fn clean_dream_bullet_text(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(['-', '*', '•'])
        .trim()
        .trim_matches('"')
        .trim()
        .to_string()
}

fn is_empty_dream_placeholder(value: &str) -> bool {
    if is_empty_ember_dream_entry_text(value) {
        return true;
    }
    let normalized = normalized_dream_key(value);
    normalized.is_empty()
        || normalized.contains("no durable user facts")
        || normalized.contains("no dream entries yet")
}

fn normalized_dream_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric() || ch.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn stable_summary_id(start: &str, end: &str, last_event_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(start.as_bytes());
    hasher.update(b"\0");
    hasher.update(end.as_bytes());
    hasher.update(b"\0");
    hasher.update(last_event_id.as_bytes());
    format!("violet-summary-{:x}", hasher.finalize())[..32].to_string()
}

fn read_chathistory_messages(
    project_root: &Path,
    request: &VioletRoomRequest,
) -> Result<Vec<VioletChatMessage>, String> {
    let requested_limit = request.limit.unwrap_or(ROOM_LIMIT_DEFAULT).max(1);
    let can_use_latest = request.before.is_none() && requested_limit <= CHATHISTORY_LATEST_LIMIT;
    let latest_path = chathistory_latest_path(project_root);
    let requested_agent_ids = request
        .agent_ids
        .as_ref()
        .map(|ids| ids.iter().cloned().collect::<HashSet<_>>());
    let used_latest = can_use_latest && latest_path.is_file();
    let events = if used_latest {
        read_chathistory_event_file(&latest_path)?
    } else {
        read_chathistory_event_segments(project_root)?
    };
    let mut messages = apply_room_request_window(
        chathistory_events_to_room_messages(events, requested_agent_ids.as_ref()),
        request,
    );
    if used_latest && requested_agent_ids.is_some() && messages.len() < requested_limit {
        messages = apply_room_request_window(
            chathistory_events_to_room_messages(
                read_chathistory_event_segments(project_root)?,
                requested_agent_ids.as_ref(),
            ),
            request,
        );
    }
    Ok(messages)
}

fn chathistory_events_to_room_messages(
    events: Vec<ChathistoryEvent>,
    requested_agent_ids: Option<&HashSet<String>>,
) -> Vec<VioletChatMessage> {
    events
        .into_iter()
        .filter(|event| event.display && event.kind != "tool")
        .filter(|event| !is_project_agent_lifecycle_notification(event))
        .filter(|event| !is_harness_envelope_text(&event.text))
        .filter(|event| !is_ember_dream_entry_event(event))
        .filter(|event| !is_ignorable_codex_internal_context_event(event))
        .map(message_from_chathistory_event)
        .filter(|message| {
            requested_agent_ids.map_or(true, |ids| {
                violet_message_matches_agent_filter(message, ids)
            })
        })
        .collect::<Vec<_>>()
}

fn violet_message_matches_agent_filter(
    message: &VioletChatMessage,
    agent_ids: &HashSet<String>,
) -> bool {
    agent_ids.contains(&message.agent_id)
        || message
            .target_agent_ids
            .iter()
            .any(|agent_id| agent_ids.contains(agent_id))
}

fn is_project_agent_lifecycle_notification(event: &ChathistoryEvent) -> bool {
    event.source.session_id.starts_with("lifecycle-")
        || event.source.path.as_deref().map_or(false, |path| {
            path.ends_with("project-memory/.violet/lifecycle")
        })
        || event.text.trim_end().ends_with(" left this project.")
}

fn read_chathistory_event_segments(project_root: &Path) -> Result<Vec<ChathistoryEvent>, String> {
    let events_dir = chathistory_events_dir(project_root);
    if !events_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_matching_files(&events_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })?;
    paths.sort();
    let mut events = Vec::new();
    for path in paths {
        events.extend(read_chathistory_event_file(&path)?);
    }
    Ok(dedupe_chathistory_events(events))
}

fn read_chathistory_events_for_latest(
    project_root: &Path,
    soft_limit: usize,
) -> Result<Vec<ChathistoryEvent>, String> {
    let events_dir = chathistory_events_dir(project_root);
    if !events_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_matching_files(&events_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
    })?;
    paths.sort();
    paths.reverse();
    let mut events = Vec::new();
    for path in paths {
        events.extend(read_chathistory_event_file(&path)?);
        if events.len() >= soft_limit {
            break;
        }
    }
    Ok(events)
}

fn read_chathistory_event_file(path: &Path) -> Result<Vec<ChathistoryEvent>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut events = Vec::new();
    let mut line_bytes = Vec::new();
    let mut line_number = 0usize;
    loop {
        line_bytes.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line_bytes)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        line_number += 1;
        while matches!(line_bytes.last(), Some(b'\n' | b'\r')) {
            line_bytes.pop();
        }
        if line_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let line = match std::str::from_utf8(&line_bytes) {
            Ok(line) => line,
            Err(err) => {
                crate::kota_debug_log(&format!(
                    "[violet] skipped non-utf8 chathistory line {} in {}: {}",
                    line_number,
                    path.display(),
                    err
                ));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ChathistoryEvent>(&line) {
            Ok(event) => events.push(event),
            Err(err) => crate::kota_debug_log(&format!(
                "[violet] skipped malformed chathistory event in {}: {}",
                path.display(),
                err
            )),
        }
    }
    Ok(events)
}

fn render_chathistory_events(events: &[ChathistoryEvent]) -> Result<String, String> {
    let mut out = String::new();
    for event in events {
        out.push_str(
            &serde_json::to_string(event)
                .map_err(|err| format!("serialize chathistory event: {err}"))?,
        );
        out.push('\n');
    }
    Ok(out)
}

fn dedupe_chathistory_events(mut events: Vec<ChathistoryEvent>) -> Vec<ChathistoryEvent> {
    events.sort_by(compare_chathistory_event_order);
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(events.len());
    for event in events {
        if seen.insert(event.id.clone()) {
            out.push(event);
        }
    }
    out
}

fn chathistory_event_from_message(
    message: &VioletChatMessage,
    snapshot: Option<&AgentIdentitySnapshot>,
) -> ChathistoryEvent {
    ChathistoryEvent {
        id: message.id.clone(),
        ts: message.timestamp.clone(),
        violet_seq: message.violet_seq,
        role: message.role.clone(),
        agent_id: message.agent_id.clone(),
        shell: message.shell.clone(),
        kind: message.kind.clone(),
        display: true,
        agent_visible: message.kind == "message",
        text: trim_terminal_control_padding(&message.text).to_string(),
        source: ChathistorySource {
            session_id: message.session_id.clone(),
            path: message.source_path.clone(),
            native_event_id: message.native_event_id.clone(),
            line_start: None,
            line_end: None,
            byte_start: None,
            byte_end: None,
        },
        target_agent_ids: message.target_agent_ids.clone(),
        actor_intent: message.actor_intent.clone(),
        agent_display_name: message
            .agent_display_name
            .clone()
            .or_else(|| snapshot.and_then(|snapshot| snapshot.display_name.clone())),
        agent_avatar_id: message
            .agent_avatar_id
            .clone()
            .or_else(|| snapshot.and_then(|snapshot| snapshot.avatar_id.clone())),
        agent_provider: message
            .agent_provider
            .clone()
            .or_else(|| snapshot.and_then(|snapshot| snapshot.provider.clone())),
        agent_status: message
            .agent_status
            .clone()
            .or_else(|| snapshot.and_then(|snapshot| snapshot.status.clone())),
    }
}

fn message_from_chathistory_event(event: ChathistoryEvent) -> VioletChatMessage {
    VioletChatMessage {
        id: event.id,
        session_id: event.source.session_id,
        agent_id: event.agent_id,
        shell: event.shell,
        role: event.role,
        kind: event.kind,
        timestamp: event.ts,
        text: event.text,
        source_path: event.source.path,
        native_event_id: event.source.native_event_id,
        violet_seq: event.violet_seq,
        actor_intent: event.actor_intent,
        target_agent_ids: event.target_agent_ids,
        agent_display_name: event.agent_display_name,
        agent_avatar_id: event.agent_avatar_id,
        agent_provider: event.agent_provider,
        agent_status: event.agent_status,
    }
}

fn chathistory_day_key(timestamp: &str) -> String {
    let key = DateTime::parse_from_rfc3339(timestamp)
        .map(|time| time.with_timezone(&Utc).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| {
            timestamp
                .chars()
                .take(10)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .chars()
        .map(|ch| {
            if ch.is_ascii_digit() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if key.is_empty() {
        Utc::now().format("%Y-%m-%d").to_string()
    } else {
        key
    }
}

fn write_session_outputs(
    project_root: &Path,
    session_id: &str,
    events: &[NativeEvent],
) -> Result<(), String> {
    let raw_dir = project_root.join("project-memory").join("raw_logs");
    fs::create_dir_all(&raw_dir).map_err(|err| format!("create {}: {err}", raw_dir.display()))?;

    let raw = render_raw_log(events);
    write_if_changed(&raw_dir.join(format!("{session_id}.md")), raw.as_bytes())
}

// Reserved for explicit repair/reset flows. Ordinary sync must only remove the
// raw projection so chathistory does not shrink to the current tail window.
#[allow(dead_code)]
fn remove_session_outputs(project_root: &Path, session_id: &str) -> Result<(), String> {
    remove_raw_session_output(project_root, session_id)?;
    remove_chathistory_session(project_root, session_id)
}

fn remove_raw_session_output(project_root: &Path, session_id: &str) -> Result<(), String> {
    let paths = [project_root
        .join("project-memory")
        .join("raw_logs")
        .join(format!("{session_id}.md"))];
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("remove {}: {err}", path.display())),
        }
    }
    Ok(())
}

fn render_raw_log(events: &[NativeEvent]) -> String {
    let mut out = String::new();
    for event in events {
        let title_role = match event.role.as_str() {
            _ if event.kind == "tool" => "Tool",
            _ if event.kind == "thinking" => "Thinking",
            _ if event.kind == "control" => "Control",
            "user" => "User",
            "system" => "System",
            _ => "Assistant",
        };
        out.push_str(&format!(
            "## {} · {} · {} · session {}\n\n",
            event.timestamp, event.agent_id, event.shell, event.session_id
        ));
        out.push_str(title_role);
        out.push_str(":\n");
        out.push_str(event.text.trim());
        out.push_str("\n\nMetadata:\n");
        out.push_str(&format!("- agent_id: {}\n", event.agent_id));
        out.push_str(&format!("- shell: {}\n", event.shell));
        out.push_str(&format!("- native_log: {}\n", event.source_path.display()));
        out.push_str(&format!("- kind: {}\n", event.kind));
        if let Some(native_event_id) = event.native_event_id.as_deref() {
            out.push_str(&format!("- native_event_id: {}\n", native_event_id));
        }
        out.push('\n');
    }
    out
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
    }
    let tmp = unique_write_tmp_path(path);
    let result = (|| {
        {
            let mut file =
                fs::File::create(&tmp).map_err(|err| format!("create {}: {err}", tmp.display()))?;
            file.write_all(bytes)
                .map_err(|err| format!("write {}: {err}", tmp.display()))?;
        }
        fs::rename(&tmp, path)
            .map_err(|err| format!("rename {} -> {}: {err}", tmp.display(), path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn unique_write_tmp_path(path: &Path) -> PathBuf {
    let counter = WRITE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}.{}",
        std::process::id(),
        nanos,
        counter
    ))
}

#[cfg(test)]
fn read_normalized_room_messages(raw_log_dir: &Path) -> Result<Vec<VioletChatMessage>, String> {
    if !raw_log_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_matching_files(raw_log_dir, &mut paths, |path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("md")
    })?;
    let mut messages = Vec::new();
    for path in paths {
        let session_id = file_stem(&path).unwrap_or_else(|| source_session_id(&path));
        let text =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        for block in split_normalized_raw_blocks(&text) {
            if let Some(message) = parse_normalized_block(block, &session_id, &path) {
                messages.push(message);
            }
        }
    }
    Ok(messages)
}

fn read_actor_raw_log_messages(project_root: &Path) -> Result<ActorRawReplayBatch, String> {
    let cursor_path = actor_raw_replay_cursor_path(project_root)?;
    let (mut cursor, mut cursor_changed) = read_actor_raw_replay_cursor(&cursor_path)?;
    let raw_log_dir = project_root.join("project-memory").join("raw_logs");
    if !raw_log_dir.is_dir() {
        return Ok(ActorRawReplayBatch {
            messages: Vec::new(),
            cursor_path,
            cursor,
            cursor_changed,
        });
    }
    let mut paths = collect_actor_raw_log_files(&raw_log_dir)?;
    paths.sort();
    let mut messages = Vec::new();
    let mut seen_paths = HashSet::new();
    for path in paths {
        let key = path_string(&path);
        seen_paths.insert(key.clone());
        let metadata =
            fs::metadata(&path).map_err(|err| format!("stat {}: {err}", path.display()))?;
        let file_len = metadata.len();
        let modified_millis = metadata
            .modified()
            .map(system_time_millis)
            .unwrap_or_default();
        let previous = cursor.files.get(&key);
        let start_offset = previous
            .filter(|cursor| cursor.offset <= file_len)
            .filter(|cursor| cursor.offset != file_len || cursor.modified_millis == modified_millis)
            .map_or(0, |cursor| cursor.offset);
        if start_offset == file_len {
            continue;
        }
        let session_id = file_stem(&path).unwrap_or_else(|| source_session_id(&path));
        let text = read_text_from_offset(&path, start_offset)?;
        for block in split_normalized_raw_blocks(&text) {
            if let Some(message) = parse_normalized_block(block, &session_id, &path) {
                messages.push(message);
            }
        }
        cursor.files.insert(
            key,
            ActorRawFileCursor {
                offset: file_len,
                modified_millis,
            },
        );
        cursor_changed = true;
    }
    let stale_paths = cursor
        .files
        .keys()
        .filter(|path| !seen_paths.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !stale_paths.is_empty() {
        for path in stale_paths {
            cursor.files.remove(&path);
        }
        cursor_changed = true;
    }
    Ok(ActorRawReplayBatch {
        messages,
        cursor_path,
        cursor,
        cursor_changed,
    })
}

fn actor_raw_replay_cursor_path(project_root: &Path) -> Result<PathBuf, String> {
    source_cursor_path(project_root, "actor-raw-replay")
}

fn empty_actor_raw_replay_cursor() -> ActorRawReplayCursor {
    ActorRawReplayCursor {
        version: ACTOR_RAW_REPLAY_CURSOR_VERSION.into(),
        files: BTreeMap::new(),
        updated_at: String::new(),
    }
}

fn read_actor_raw_replay_cursor(path: &Path) -> Result<(ActorRawReplayCursor, bool), String> {
    if !path.is_file() {
        return Ok((empty_actor_raw_replay_cursor(), false));
    }
    let text = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let Ok(cursor) = serde_json::from_str::<ActorRawReplayCursor>(&text) else {
        return Ok((empty_actor_raw_replay_cursor(), true));
    };
    if cursor.version != ACTOR_RAW_REPLAY_CURSOR_VERSION {
        return Ok((empty_actor_raw_replay_cursor(), true));
    }
    Ok((cursor, false))
}

fn write_actor_raw_replay_cursor(path: &Path, cursor: &ActorRawReplayCursor) -> Result<(), String> {
    let bytes = serde_json::to_vec(cursor)
        .map_err(|err| format!("serialize actor replay cursor: {err}"))?;
    write_if_changed(path, &bytes)
}

fn collect_actor_raw_log_files(raw_log_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(raw_log_dir)
        .map_err(|err| format!("read {}: {err}", raw_log_dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", raw_log_dir.display()))?;
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("md")
            && path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("actor-"))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn read_text_from_offset(path: &Path, offset: u64) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("seek {}: {err}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(text)
}

fn system_time_millis(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn split_normalized_raw_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut cursor = 0;

    for line in text.split_inclusive('\n') {
        let line_start = cursor;
        let line_end = line_start + line.len();
        let line_no_newline = line.strip_suffix('\n').unwrap_or(line);
        if let Some(header) = line_no_newline.strip_prefix("## ") {
            if is_normalized_raw_header(header) {
                if let Some(start) = block_start {
                    let block = text[start..line_start].trim();
                    if !block.is_empty() {
                        blocks.push(block);
                    }
                }
                block_start = Some(line_start);
            }
        }
        cursor = line_end;
    }

    if let Some(start) = block_start {
        let block = text[start..].trim();
        if !block.is_empty() {
            blocks.push(block);
        }
    }

    blocks
}

fn is_normalized_raw_header(header: &str) -> bool {
    let mut parts = header.split(" · ");
    let timestamp = parts.next().unwrap_or_default().trim();
    let agent_id = parts.next().unwrap_or_default().trim();
    let shell = parts.next().unwrap_or_default().trim();
    let session = parts.next().unwrap_or_default().trim();
    parts.next().is_none()
        && DateTime::parse_from_rfc3339(timestamp).is_ok()
        && !agent_id.is_empty()
        && !shell.is_empty()
        && session
            .strip_prefix("session ")
            .is_some_and(|id| !id.trim().is_empty())
}

fn parse_normalized_block(block: &str, session_id: &str, path: &Path) -> Option<VioletChatMessage> {
    let block = block.trim_start_matches("## ").trim();
    let (header, body) = block.split_once("\n\n")?;
    if !is_normalized_raw_header(header) {
        return None;
    }
    let mut header_parts = header.split(" · ");
    let timestamp = header_parts.next()?.trim().to_string();
    let agent_id = header_parts.next().unwrap_or("agent").trim().to_string();
    let shell = header_parts.next().unwrap_or("unknown").trim().to_string();
    let (content, metadata) = body
        .split_once("\n\nMetadata:\n")
        .map_or((body, ""), |(content, metadata)| (content, metadata));
    let role = if body.starts_with("User:\n") {
        "user"
    } else if body.starts_with("System:\n") {
        "system"
    } else {
        "assistant"
    };
    let header_kind = if body.starts_with("Thinking:\n") {
        "thinking"
    } else if body.starts_with("Tool:\n") {
        "tool"
    } else {
        "message"
    };
    let metadata_kind = metadata_value(metadata, "kind");
    let kind = metadata_kind.as_deref().unwrap_or(header_kind);
    if kind == "tool" || looks_like_legacy_tool_block(content, metadata) {
        return None;
    }
    let native_event_id = metadata_value(metadata, "native_event_id");
    let actor_name = metadata_value(metadata, "actor_name");
    let target_agent_ids = metadata_value(metadata, "target_agent_ids")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let text = content
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    if is_bootstrap_noise_text(&text) {
        return None;
    }
    let is_system_actor = shell == "system";
    let agent_avatar_id = is_system_actor.then(|| agent_id.clone());
    let agent_provider = is_system_actor.then(|| "system".into());
    Some(VioletChatMessage {
        id: stable_message_id(session_id, &agent_id, &timestamp, &text),
        session_id: session_id.to_string(),
        agent_id,
        shell,
        role: role.into(),
        kind: kind.to_string(),
        timestamp,
        text,
        source_path: Some(path_string(path)),
        native_event_id,
        violet_seq: None,
        actor_intent: metadata_value(metadata, "actor_intent"),
        target_agent_ids,
        agent_display_name: actor_name,
        agent_avatar_id,
        agent_provider,
        agent_status: None,
    })
}

fn metadata_value(metadata: &str, key: &str) -> Option<String> {
    let prefix = format!("- {key}:");
    metadata.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(|value| value.trim().to_string())
    })
}

fn looks_like_legacy_tool_block(content: &str, metadata: &str) -> bool {
    let metadata_tool = metadata.lines().any(|line| line.trim() == "- kind: tool");
    metadata_tool || content.starts_with("Tool:\n")
}

fn event_to_message(event: NativeEvent) -> VioletChatMessage {
    VioletChatMessage {
        id: stable_message_id(
            &event.session_id,
            &event.agent_id,
            &event.timestamp,
            &event.text,
        ),
        session_id: event.session_id,
        agent_id: event.agent_id,
        shell: event.shell,
        role: event.role,
        kind: event.kind,
        timestamp: event.timestamp,
        text: event.text,
        source_path: Some(path_string(&event.source_path)),
        native_event_id: event.native_event_id,
        violet_seq: None,
        actor_intent: None,
        target_agent_ids: Vec::new(),
        agent_display_name: None,
        agent_avatar_id: None,
        agent_provider: None,
        agent_status: None,
    }
}

fn load_privacy_spans(project_root: &Path) -> Result<Vec<PrivacySpan>, String> {
    let path = project_root
        .join("project-memory")
        .join(".violet")
        .join("privacy-spans.jsonl");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut spans = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(span) = serde_json::from_str::<PrivacySpan>(&line) {
            spans.push(span);
        }
    }
    Ok(spans)
}

fn read_codex_session_meta(path: &Path) -> Result<Option<CodexSessionMeta>, String> {
    let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let mut lines = std::io::BufReader::new(file).lines();
    let Some(line) = lines.next() else {
        return Ok(None);
    };
    let line = line.map_err(|err| format!("read {}: {err}", path.display()))?;
    let meta: CodexSessionMetaLine = match serde_json::from_str(&line) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    if meta.kind.as_deref() != Some("session_meta") {
        return Ok(None);
    }
    let Some(payload) = meta.payload else {
        return Ok(None);
    };
    let CodexSessionMetaPayload {
        id,
        cwd,
        meta,
        markers,
    } = payload;
    let nested = meta.unwrap_or_default();
    let is_subagent = markers.is_subagent() || nested.markers.is_subagent();
    let id = id.or(nested.id);
    let cwd = cwd.or(nested.cwd);
    Ok(id.zip(cwd).map(|(session_id, cwd)| CodexSessionMeta {
        session_id,
        cwd: PathBuf::from(cwd),
        is_subagent,
    }))
}

struct CodexSessionMeta {
    session_id: String,
    cwd: PathBuf,
    is_subagent: bool,
}

#[derive(Default, Deserialize)]
struct CodexSessionMetaLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<CodexSessionMetaPayload>,
}

#[derive(Default, Deserialize)]
struct CodexSessionMetaPayload {
    id: Option<String>,
    cwd: Option<String>,
    meta: Option<CodexSessionMetaPayloadMeta>,
    #[serde(flatten)]
    markers: CodexSessionMetaMarkers,
}

#[derive(Default, Deserialize)]
struct CodexSessionMetaPayloadMeta {
    id: Option<String>,
    cwd: Option<String>,
    #[serde(flatten)]
    markers: CodexSessionMetaMarkers,
}

#[derive(Default, Deserialize)]
struct CodexSessionMetaMarkers {
    #[serde(alias = "threadSource")]
    thread_source: Option<String>,
    source: Option<JsonValue>,
}

impl CodexSessionMetaMarkers {
    fn is_subagent(&self) -> bool {
        self.thread_source
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("subagent"))
            || self
                .source
                .as_ref()
                .and_then(|source| source.get("subagent"))
                .is_some_and(|subagent| !subagent.is_null())
    }
}

fn latest_opencode_session_id(cwd: &Path) -> Result<Option<String>, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let db_path = opencode_db_path(&home);
    if !db_path.is_file() {
        return Ok(None);
    }
    latest_opencode_session_id_in_db(&db_path, cwd)
}

fn latest_opencode_session_id_in_db(db_path: &Path, cwd: &Path) -> Result<Option<String>, String> {
    let conn = open_opencode_db(db_path)?;
    let mut stmt = conn
        .prepare(
            "select id, directory, coalesce(time_updated, 0)
             from session
             order by time_updated desc
             limit 250",
        )
        .map_err(|err| format!("prepare opencode session query: {err}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|err| format!("query opencode sessions: {err}"))?;
    let mut best: Option<(i64, String)> = None;
    while let Some(row) = rows
        .next()
        .map_err(|err| format!("read opencode session row: {err}"))?
    {
        let id: String = row
            .get(0)
            .map_err(|err| format!("read opencode session id: {err}"))?;
        let directory: Option<String> = row
            .get(1)
            .map_err(|err| format!("read opencode session directory: {err}"))?;
        let Some(directory) = directory else {
            continue;
        };
        if !paths_same(Path::new(&directory), cwd) {
            continue;
        }
        let updated = row
            .get::<_, i64>(2)
            .map_err(|err| format!("read opencode session updated time: {err}"))?;
        if best
            .as_ref()
            .map_or(true, |(best_updated, _)| updated > *best_updated)
        {
            best = Some((updated, id));
        }
    }
    Ok(best.map(|(_, id)| id))
}

fn latest_file_by_mtime<F>(dir: &Path, pred: F) -> Result<Option<(SystemTime, PathBuf)>, String>
where
    F: Fn(&Path) -> bool,
{
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
        let path = entry.path();
        if !pred(&path) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if best
            .as_ref()
            .map_or(true, |(best_modified, _)| modified > *best_modified)
        {
            best = Some((modified, path));
        }
    }
    Ok(best)
}

fn collect_matching_files<F>(dir: &Path, out: &mut Vec<PathBuf>, pred: F) -> Result<(), String>
where
    F: Fn(&Path) -> bool + Copy,
{
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
        let path = entry.path();
        if entry
            .metadata()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            collect_matching_files(&path, out, pred)?;
        } else if pred(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn json_string(value: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(str::to_string)
}

fn json_bool(value: &JsonValue, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_bool()
}

fn read_yaml_file(path: &Path) -> Result<Option<YamlValue>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    serde_yaml::from_str::<YamlValue>(&text)
        .map(Some)
        .map_err(|err| format!("parse {}: {err}", path.display()))
}

fn yaml_string(value: &YamlValue, key: &str) -> Option<String> {
    let YamlValue::Mapping(map) = value else {
        return None;
    };
    map.get(&YamlValue::String(key.to_string()))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn yaml_nested_string(value: &YamlValue, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        let YamlValue::Mapping(map) = current else {
            return None;
        };
        current = map.get(&YamlValue::String(key.to_string()))?;
    }
    current.as_str().map(str::to_string)
}

fn yaml_mapping_set_string(map: &mut serde_yaml::Mapping, key: &str, value: &str) {
    map.insert(
        YamlValue::String(key.to_string()),
        YamlValue::String(value.to_string()),
    );
}

fn yaml_mapping_remove(map: &mut serde_yaml::Mapping, key: &str) {
    map.remove(&YamlValue::String(key.to_string()));
}

fn text_from_json(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => None,
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Array(items) => {
            let text = items
                .iter()
                .filter_map(text_from_json)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        JsonValue::Object(map) => {
            for key in [
                "text",
                "content",
                "message",
                "output",
                "result",
                "error",
                "summary",
                "command",
                "cmd",
                "path",
                "file_path",
                "name",
            ] {
                if let Some(text) = map.get(key).and_then(text_from_json) {
                    if !text.trim().is_empty() {
                        return Some(text);
                    }
                }
            }
            None
        }
    }
}

fn opencode_timestamp(json: &JsonValue) -> Option<String> {
    let millis = json
        .get("time")
        .and_then(|time| time.get("completed").or_else(|| time.get("created")))
        .and_then(|value| value.as_i64())?;
    millis_to_iso(millis)
}

fn millis_to_iso(millis: i64) -> Option<String> {
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|time| time.to_rfc3339())
}

fn normalize_timestamp(raw: &str) -> String {
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(raw) {
        return timestamp.with_timezone(&Utc).to_rfc3339();
    }
    raw.to_string()
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn normalize_shell(raw: &str) -> String {
    match raw {
        "claude" | "cc" | "claude-code" => "claude",
        "antigravity" | "agy" | "antigravity-cli" => "antigravity",
        "opencode" | "open-code" => "opencode",
        "codex" => "codex",
        "kimi" | "kimi-code" => "kimi",
        other => other,
    }
    .to_string()
}

fn claude_project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '.' => '-',
            other => other,
        })
        .collect()
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn source_session_id(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_string(path));
    format!("violet-{:x}", hasher.finalize())[..23].to_string()
}

fn stable_message_id(session_id: &str, agent_id: &str, timestamp: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id);
    hasher.update(agent_id);
    hasher.update(timestamp);
    hasher.update(text);
    format!("violet-{:x}", hasher.finalize())[..24].to_string()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> ProjectAgent {
        ProjectAgent {
            agent_id: "alice".into(),
            shell: "codex".into(),
            cwd: PathBuf::from("/tmp/kota/.agent-workspaces/alice"),
            session_id: None,
        }
    }

    fn source() -> NativeSource {
        NativeSource {
            kind: "codex-jsonl".into(),
            session_id: "rollout-test".into(),
            path: PathBuf::from("/tmp/rollout-test.jsonl"),
            aux_path: None,
        }
    }

    fn pi_agent(cwd: &Path) -> ProjectAgent {
        ProjectAgent {
            agent_id: "alice".into(),
            shell: "pi".into(),
            cwd: cwd.to_path_buf(),
            session_id: Some("pi-session".into()),
        }
    }

    fn pi_source(path: &Path) -> NativeSource {
        NativeSource {
            kind: PI_SOURCE_KIND.into(),
            session_id: "pi-session".into(),
            path: path.to_path_buf(),
            aux_path: None,
        }
    }

    fn kimi_agent(cwd: &Path) -> ProjectAgent {
        ProjectAgent {
            agent_id: "alice".into(),
            shell: "kimi".into(),
            cwd: cwd.to_path_buf(),
            session_id: None,
        }
    }

    fn kimi_source(path: &Path) -> NativeSource {
        NativeSource {
            kind: KIMI_SOURCE_KIND.into(),
            session_id: "session_kimi".into(),
            path: path.to_path_buf(),
            aux_path: None,
        }
    }

    #[test]
    fn parse_kimi_wire_maps_known_events_without_locking_protocol_version() {
        let cwd = Path::new("/tmp/kota-kimi-agent");
        let agent = kimi_agent(cwd);
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let records = [
            serde_json::json!({"type":"metadata","time":1784393127000i64,"protocol_version":"99.0"}),
            serde_json::json!({"type":"turn.prompt","time":1784393127001i64,"input":"你好 🌙"}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127002i64,"event":{"type":"step.begin","uuid":"step-1","turnId":"0","step":1}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127003i64,"event":{"type":"content.part","part":{"type":"think","think":"checking"}}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127004i64,"event":{"type":"tool.call","uuid":"tool-1","toolCallId":"tool-1","name":"Bash","description":"Running: pwd","turnId":"0"}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127005i64,"event":{"type":"tool.result","parentUuid":"tool-1","toolCallId":"tool-1","result":{"output":"/tmp/kota-kimi-agent"}}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127006i64,"event":{"type":"step.end","uuid":"step-1","turnId":"0","finishReason":"tool_use"}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127007i64,"event":{"type":"content.part","part":{"type":"text","text":"完成"}}}),
            serde_json::json!({"type":"context.append_loop_event","time":1784393127008i64,"event":{"type":"step.end","uuid":"step-2","turnId":"0","finishReason":"end_turn"}}),
            serde_json::json!({"type":"turn.cancel","time":1784393127009i64}),
        ];
        let events = records
            .into_iter()
            .enumerate()
            .flat_map(|(index, record)| parse_kimi_line(&agent, &source, index, record))
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 9);
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].text, "你好 🌙");
        assert_eq!(events[1].work_signal.as_deref(), Some("activity"));
        assert_eq!(events[2].kind, "thinking");
        assert_eq!(events[3].text, "Bash: Running: pwd");
        assert_eq!(events[4].text, "/tmp/kota-kimi-agent");
        assert_eq!(events[5].work_signal.as_deref(), Some("activity"));
        assert_eq!(events[6].text, "完成");
        assert_eq!(events[7].work_signal.as_deref(), Some("completed"));
        assert_eq!(events[8].work_signal.as_deref(), Some("interrupted"));
    }

    #[test]
    fn parse_kimi_unknown_events_are_visible_sanitized_and_deduplicable() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let first = parse_kimi_line(
            &agent,
            &source,
            1,
            serde_json::json!({"type":"future.secret.event","payload":"do not leak"}),
        );
        let second = parse_kimi_line(
            &agent,
            &source,
            99,
            serde_json::json!({"type":"future.secret.event","payload":"another secret"}),
        );

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, KIMI_UNKNOWN_EVENT_WARNING);
        assert!(!first[0].text.contains("secret"));
        assert_eq!(first[0].work_signal, None);
        assert_eq!(first[0].native_event_id, second[0].native_event_id);
    }

    #[test]
    fn parse_kimi_malformed_lines_emit_one_sanitized_canary() {
        let root = temp_violet_dir("kimi-malformed");
        let wire = root.join("wire.jsonl");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &wire,
            "first private malformed line\nsecond private malformed line\n",
        )
        .unwrap();
        let agent = kimi_agent(&root);
        let source = kimi_source(&wire);

        let first = parse_jsonl_source_incremental_with_malformed(
            &root,
            &agent,
            &source,
            parse_kimi_line,
            parse_kimi_malformed_line,
        )
        .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, KIMI_UNKNOWN_EVENT_WARNING);
        assert!(!first[0].text.contains("private"));
        assert_eq!(first[0].work_signal, None);

        fs::write(
            &wire,
            "first private malformed line\nsecond private malformed line\nthird private malformed line\n",
        )
        .unwrap();
        let second = parse_jsonl_source_incremental_with_malformed(
            &root,
            &agent,
            &source,
            parse_kimi_line,
            parse_kimi_malformed_line,
        )
        .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].native_event_id, first[0].native_event_id);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn kimi_todo_store_snapshots_become_progress_commentary() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let record = serde_json::json!({
            "type": "tools.update_store",
            "key": "todo",
            "time": 1784443967514i64,
            "value": [
                {"title": "查证过滤链路", "status": "done"},
                {"title": "实现快照整形", "status": "in_progress"},
                {"title": "补测试", "status": "pending"}
            ]
        });
        let events = parse_kimi_line(&agent, &source, 7, record);

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.role, "assistant");
        assert_eq!(event.kind, "commentary");
        assert_eq!(event.work_signal, None);
        assert_eq!(event.native_event_id.as_deref(), Some("kimi:7:todo-store"));
        assert_eq!(
            event.text,
            "进度 1/3 · 当前：实现快照整形\n- [x] 查证过滤链路\n▸ 实现快照整形\n- [ ] 补测试"
        );
    }

    #[test]
    fn kimi_todo_store_terminal_states_have_no_current_item() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let all_done = parse_kimi_line(
            &agent,
            &source,
            8,
            serde_json::json!({
                "type": "tools.update_store",
                "key": "todo",
                "value": [
                    {"title": "甲", "status": "done"},
                    {"title": "乙", "status": "done"}
                ]
            }),
        );
        assert_eq!(all_done[0].text, "进度 2/2 · 全部完成\n- [x] 甲\n- [x] 乙");

        let cleared = parse_kimi_line(
            &agent,
            &source,
            9,
            serde_json::json!({"type": "tools.update_store", "key": "todo", "value": []}),
        );
        assert_eq!(cleared[0].text, "进度 0/0 · 待办清单已清空");
    }

    #[test]
    fn kimi_todo_store_snapshots_keep_distinct_ids_for_history() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let record = || {
            serde_json::json!({
                "type": "tools.update_store",
                "key": "todo",
                "value": [{"title": "唯一事项", "status": "in_progress"}]
            })
        };
        let first = parse_kimi_line(&agent, &source, 7, record());
        let second = parse_kimi_line(&agent, &source, 8, record());
        assert_ne!(first[0].native_event_id, second[0].native_event_id);
    }

    #[test]
    fn kimi_todo_store_malformed_values_fall_back_to_record_canary() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let cases = [
            serde_json::json!({"type":"tools.update_store","key":"todo","value":[{"title":"x","status":"blocked"}]}),
            serde_json::json!({"type":"tools.update_store","key":"todo","value":[{"title":"  ","status":"done"}]}),
            serde_json::json!({"type":"tools.update_store","key":"todo","value":[{"status":"done"}]}),
            serde_json::json!({"type":"tools.update_store","key":"todo","value":{"title":"x"}}),
            serde_json::json!({"type":"tools.update_store","key":"clipboard","value":[{"title":"x","status":"done"}]}),
            serde_json::json!({"type":"tools.update_store"}),
        ];
        let mut ids = std::collections::HashSet::new();
        for record in cases {
            let events = parse_kimi_line(&agent, &source, 3, record);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].text, KIMI_UNKNOWN_EVENT_WARNING);
            assert_eq!(events[0].work_signal, None);
            ids.insert(events[0].native_event_id.clone());
        }
        // Every failure mode collapses into the same deduplicable canary the
        // record produced before the reshape arm existed.
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn kimi_todo_store_caps_long_titles_and_overflow_items() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let long_title = "长".repeat(120);
        let mut value = vec![serde_json::json!({"title": long_title, "status": "in_progress"})];
        for index in 0..25 {
            value.push(serde_json::json!({"title": format!("事项 {index}"), "status": "pending"}));
        }
        let events = parse_kimi_line(
            &agent,
            &source,
            7,
            serde_json::json!({"type": "tools.update_store", "key": "todo", "value": value}),
        );
        let text = &events[0].text;
        assert!(text.starts_with(&format!(
            "进度 0/26 · 当前：{}...",
            "长".repeat(KIMI_TODO_TITLE_MAX_CHARS)
        )));
        assert!(text.contains("… 其余 6 项"));
        assert!(!text.contains("事项 19"));
    }

    #[test]
    fn kimi_todo_store_titles_collapse_to_single_lines() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let events = parse_kimi_line(
            &agent,
            &source,
            7,
            serde_json::json!({
                "type": "tools.update_store",
                "key": "todo",
                "value": [
                    {"title": "第一行\n▸ 伪造进度   连续  空白", "status": "in_progress"},
                    {"title": "正常事项", "status": "pending"}
                ]
            }),
        );
        let text = &events[0].text;
        assert_eq!(
            text.as_str(),
            "进度 0/2 · 当前：第一行 ▸ 伪造进度 连续 空白\n▸ 第一行 ▸ 伪造进度 连续 空白\n- [ ] 正常事项"
        );
        // One snapshot line per todo item plus the summary line: nothing a
        // title carries can add lines of its own.
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn kimi_turn_steer_background_task_notifications_fold_into_progress() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        let events = parse_kimi_line(
            &agent,
            &source,
            11,
            serde_json::json!({
                "type": "turn.steer",
                "time": 1784493831736i64,
                "input": [
                    {"type": "text", "text": "<notification id=\"task:bash-1:completed\">\nTitle: Background process completed\n采样完成。\n</notification>"}
                ],
                "origin": {"kind": "background_task", "taskId": "bash-1", "status": "completed"}
            }),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "assistant");
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].work_signal, None);
        assert_eq!(
            events[0].native_event_id.as_deref(),
            Some("kimi:11:steer-notice")
        );
        assert!(events[0].text.contains("Background process completed"));
        assert!(events[0].text.contains("采样完成。"));
    }

    #[test]
    fn kimi_turn_steer_other_origins_keep_the_exact_record_canary() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        // The exact canary this record type produced before the arm existed:
        // same scope ("record") and same type string, so ids stay identical
        // with history and keep deduplicating against frozen entries.
        let unknown_type_canary = kimi_unknown_event(&agent, &source, "t", "record", "turn.steer");

        let cases = [
            // A hypothetical user steering shape must NOT be folded on a guess.
            serde_json::json!({
                "type": "turn.steer",
                "input": [{"type": "text", "text": "user steering text"}],
                "origin": {"kind": "user"}
            }),
            // Missing origin entirely.
            serde_json::json!({
                "type": "turn.steer",
                "input": [{"type": "text", "text": "orphan input"}]
            }),
            // Confirmed origin but empty input still has nothing to fold.
            serde_json::json!({
                "type": "turn.steer",
                "input": [],
                "origin": {"kind": "background_task"}
            }),
        ];
        for record in cases {
            let events = parse_kimi_line(&agent, &source, 3, record);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].text, KIMI_UNKNOWN_EVENT_WARNING);
            assert!(!events[0].text.contains("steering"));
            assert_eq!(events[0].work_signal, None);
            assert_eq!(
                events[0].native_event_id,
                unknown_type_canary.native_event_id
            );
        }
    }

    #[test]
    fn kimi_compaction_records_are_silently_ignored() {
        let agent = kimi_agent(Path::new("/tmp/kota-kimi-agent"));
        let source = kimi_source(Path::new("/tmp/kimi-wire.jsonl"));
        // Shapes observed in a real session wire: compaction lifecycle is
        // session-internal bookkeeping (the applied summary stays agent
        // context), so all three record types surface zero room events.
        let cases = [
            serde_json::json!({"type": "full_compaction.begin", "source": "auto", "time": 1784493831736i64}),
            serde_json::json!({
                "type": "context.apply_compaction",
                "summary": "agent-internal handoff notes",
                "contextSummary": "…",
                "compactedCount": 1280,
                "tokensBefore": 2100000,
                "tokensAfter": 120000,
                "keptUserMessageCount": 4,
                "droppedCount": 1280,
                "time": 1784493831740i64
            }),
            serde_json::json!({"type": "full_compaction.complete", "time": 1784493831751i64}),
        ];
        for record in cases {
            assert!(parse_kimi_line(&agent, &source, 7, record).is_empty());
        }
        // The fail-closed net is untouched: an unknown compaction-flavoured
        // type still produces exactly the record-scope canary.
        let events = parse_kimi_line(
            &agent,
            &source,
            7,
            serde_json::json!({"type": "context.apply_compaction_v2", "time": 1784493831799i64}),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, KIMI_UNKNOWN_EVENT_WARNING);
        assert_eq!(
            events[0].native_event_id,
            kimi_unknown_event(
                &agent,
                &source,
                "t",
                "record",
                "context.apply_compaction_v2"
            )
            .native_event_id
        );
    }

    #[test]
    fn locate_kimi_source_uses_matching_workspace_main_wire_only() {
        let root = temp_violet_dir("kimi-locator");
        let kimi_home = root.join("kimi-home");
        let cwd = root.join("agent-cwd");
        let session_dir = kimi_home.join("sessions/wd_match/session_kimi");
        let main_wire = session_dir.join("agents/main/wire.jsonl");
        let child_wire = session_dir.join("agents/child-1/wire.jsonl");
        fs::create_dir_all(main_wire.parent().unwrap()).unwrap();
        fs::create_dir_all(child_wire.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            kimi_home.join("workspaces.json"),
            serde_json::json!({
                "version": 1,
                "workspaces": {"wd_match": {"root": path_string(&cwd)}}
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            session_dir.join("state.json"),
            serde_json::json!({"workDir": path_string(&cwd)}).to_string(),
        )
        .unwrap();
        fs::write(&main_wire, "{}\n").unwrap();
        fs::write(&child_wire, "{}\n{}\n").unwrap();

        let source = locate_kimi_source_in(&kimi_home, &kimi_agent(&cwd))
            .unwrap()
            .expect("Kimi main source");
        assert_eq!(source.kind, KIMI_SOURCE_KIND);
        assert_eq!(source.session_id, "session_kimi");
        assert_eq!(source.path, main_wire);
        assert_ne!(source.path, child_wire);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_pi_source_projects_active_leaf_path_only() {
        let root = temp_violet_dir("pi-active-path");
        fs::create_dir_all(&root).unwrap();
        let cwd = root.join("agent");
        fs::create_dir_all(&cwd).unwrap();
        let session = root.join("2026-06-30T00-00-00-000Z_pi-session.jsonl");
        fs::write(
            &session,
            [
                serde_json::json!({"type":"session","version":3,"id":"pi-session","timestamp":"2026-06-30T00:00:00Z","cwd":path_string(&cwd)}),
                serde_json::json!({"type":"message","id":"root0001","parentId":null,"timestamp":"2026-06-30T00:00:01Z","message":{"role":"user","content":"start here","timestamp":1782806401000i64}}),
                serde_json::json!({"type":"message","id":"old00001","parentId":"root0001","timestamp":"2026-06-30T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"old branch reply"}],"stopReason":"stop","timestamp":1782806402000i64}}),
                serde_json::json!({"type":"message","id":"new00001","parentId":"root0001","timestamp":"2026-06-30T00:00:03Z","message":{"role":"user","content":"switch branch","timestamp":1782806403000i64}}),
                serde_json::json!({"type":"message","id":"new00002","parentId":"new00001","timestamp":"2026-06-30T00:00:04Z","message":{"role":"assistant","content":[{"type":"text","text":"active branch reply"}],"stopReason":"stop","timestamp":1782806404000i64}}),
            ]
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n",
        )
        .unwrap();

        let events = parse_pi_source(&pi_agent(&cwd), &pi_source(&session)).unwrap();
        let text = events
            .iter()
            .map(|event| event.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("start here"));
        assert!(text.contains("switch branch"));
        assert!(text.contains("active branch reply"));
        assert!(!text.contains("old branch reply"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_pi_tool_results_stay_out_of_room() {
        let root = temp_violet_dir("pi-tool");
        fs::create_dir_all(&root).unwrap();
        let cwd = root.join("agent");
        fs::create_dir_all(&cwd).unwrap();
        let session = root.join("2026-06-30T00-00-00-000Z_pi-session.jsonl");
        fs::write(
            &session,
            [
                serde_json::json!({"type":"session","version":3,"id":"pi-session","timestamp":"2026-06-30T00:00:00Z","cwd":path_string(&cwd)}),
                serde_json::json!({"type":"message","id":"root0001","parentId":null,"timestamp":"2026-06-30T00:00:01Z","message":{"role":"user","content":"run ls"}}),
                serde_json::json!({"type":"message","id":"tool0001","parentId":"root0001","timestamp":"2026-06-30T00:00:02Z","message":{"role":"toolResult","toolName":"bash","content":[{"type":"text","text":"file.txt"}],"isError":false}}),
            ]
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n",
        )
        .unwrap();

        let events = parse_pi_source(&pi_agent(&cwd), &pi_source(&session)).unwrap();
        assert!(events.iter().any(|event| event.kind == "tool"));
        let (room, shared) = split_for_violet_outputs(events, &root);
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].text, "run ls");
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].text, "run ls");
        let _ = fs::remove_dir_all(root);
    }

    fn native_event(role: &str, kind: &str, text: &str) -> NativeEvent {
        NativeEvent {
            session_id: "s".into(),
            agent_id: "alice".into(),
            shell: "codex".into(),
            role: role.into(),
            kind: kind.into(),
            timestamp: "2026-05-14T10:00:00Z".into(),
            text: text.into(),
            source_path: PathBuf::from("/tmp/a.jsonl"),
            native_event_id: None,
            work_signal: None,
            turn_id: None,
            stop_reason: None,
        }
    }

    #[test]
    fn native_event_timestamp_ties_preserve_source_order() {
        let mut user = native_event("user", "message", "prompt");
        user.native_event_id = Some("z-user".into());
        let mut assistant = native_event("assistant", "message", "reply");
        assistant.native_event_id = Some("a-assistant".into());

        let events = dedupe_native_events(vec![user, assistant]);

        assert_eq!(
            events
                .iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["prompt", "reply"]
        );
    }

    #[test]
    fn harness_task_notification_turn_is_dropped_from_room() {
        let json = serde_json::json!({
            "timestamp": "2026-06-09T13:33:00Z",
            "type": "user",
            "uuid": "tn1",
            "message": {
                "role": "user",
                "content": "<task-notification>\n<task-id>bedlg1lzp</task-id>\n<status>completed</status>\n</task-notification>"
            }
        });

        let events = parse_claude_line(&agent(), &source(), 0, json);
        assert!(
            events.is_empty(),
            "task-notification must not become a room message"
        );
    }

    #[test]
    fn harness_envelope_text_detection() {
        assert!(is_harness_envelope_text(
            "<task-notification>\n<task-id>x</task-id>\n</task-notification>"
        ));
        assert!(is_harness_envelope_text(
            "[SYSTEM NOTIFICATION - NOT USER INPUT]\n<task-notification>\n<status>completed</status>\n</task-notification>"
        ));
        assert!(is_harness_envelope_text(
            "<system-reminder>recalled memory</system-reminder>"
        ));
        // A real prompt with a reminder appended must be kept.
        assert!(!is_harness_envelope_text(
            "<system-reminder>context</system-reminder>\n帮我看看这个 bug"
        ));
        assert!(!is_harness_envelope_text("修一下 diff view 的拖动"));
    }

    #[test]
    fn claude_end_turn_marks_work_completed_without_chat_control() {
        let json = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "assistant",
            "uuid": "u1",
            "message": {
                "id": "turn-a",
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "done" }]
            }
        });

        let events = parse_claude_line(&agent(), &source(), 0, json);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, "done");
        assert_eq!(events[0].work_signal.as_deref(), Some("completed"));
        assert_eq!(events[0].stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(
            room_event_for(&events[0], Path::new("/tmp/kota"))
                .unwrap()
                .text,
            "done"
        );
        assert_eq!(native_work_event(&events[0]).unwrap().state, "idle");
    }

    #[test]
    fn claude_compact_boundary_collapses_and_clears_work() {
        let boundary = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "system",
            "subtype": "compact_boundary",
            "uuid": "c1",
            "content": "Conversation compacted",
            "isMeta": false,
            "compactMetadata": { "trigger": "manual", "preTokens": 372985, "postTokens": 3967 }
        });

        let events = parse_claude_line(&agent(), &source(), 0, boundary);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "compaction");
        assert_eq!(events[0].text, "Conversation compacted · 373k → 4k tokens");
        // The marker still drives the agent bar back to idle (BUG A).
        assert_eq!(native_work_event(&events[0]).unwrap().state, "idle");
        // …and survives as a room bubble instead of being dropped like a hidden control
        // event (BUG B), so the frontend can render its own collapsed variant.
        assert!(room_event_for(&events[0], Path::new("/tmp/kota")).is_some());
    }

    #[test]
    fn claude_compact_summary_is_dropped() {
        let summary = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "user",
            "uuid": "c2",
            "isCompactSummary": true,
            "isVisibleInTranscriptOnly": true,
            "message": { "role": "user", "content": "This session is being continued…" }
        });

        let events = parse_claude_line(&agent(), &source(), 1, summary);

        assert!(events.is_empty());
    }

    #[test]
    fn claude_empty_thinking_marks_work_activity() {
        let json = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "assistant",
            "uuid": "u1",
            "message": {
                "id": "turn-a",
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{ "type": "thinking", "thinking": "", "signature": "sig" }]
            }
        });

        let events = parse_claude_line(&agent(), &source(), 0, json);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "control");
        assert_eq!(events[0].work_signal.as_deref(), Some("activity"));
        assert_eq!(events[0].stop_reason.as_deref(), Some("assistant_activity"));
        assert!(room_event_for(&events[0], Path::new("/tmp/kota")).is_none());
        assert_eq!(native_work_event(&events[0]).unwrap().state, "working");
    }

    #[test]
    fn claude_ask_user_question_becomes_visible_control() {
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = "claude-jsonl".into();
        source.session_id = "claude-session".into();
        let json = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "assistant",
            "uuid": "u1",
            "requestId": "turn-a",
            "message": {
                "id": "message-a",
                "role": "assistant",
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "AskUserQuestion",
                    "input": {
                        "questions": [{
                            "header": "Ultracode",
                            "question": "effort 下拉里的 ultracode 这项，你想怎么处理？",
                            "options": [
                                { "label": "就封顶 xhigh/max", "description": "不做 ultracode。" },
                                { "label": "维持现状", "description": "会话内手动开。" }
                            ]
                        }]
                    }
                }]
            }
        });

        let events = parse_claude_line(&agent, &source, 0, json);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "system");
        assert_eq!(events[0].kind, "control");
        assert_eq!(events[0].work_signal.as_deref(), Some("waiting_for_user"));
        assert_eq!(
            events[0].stop_reason.as_deref(),
            Some("user_question_requested")
        );
        assert_eq!(events[0].turn_id.as_deref(), Some("turn-a"));
        assert!(events[0].text.contains("Open the agent terminal"));
        assert!(events[0].text.contains("Ultracode"));
        assert!(events[0].text.contains("1. 就封顶 xhigh/max"));
        assert_eq!(native_work_event(&events[0]).unwrap().state, "maybeIdle");

        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "control");
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].kind, "control");
    }

    #[test]
    fn claude_hook_ask_user_question_becomes_visible_control() {
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = CLAUDE_HOOK_SOURCE_KIND.into();
        source.session_id = "claude-session".into();
        let json = serde_json::json!({
            "schema": "kota.claude.ask-user-question.v1",
            "captured_at": "2026-05-14T10:00:00Z",
            "agent_id": "alice",
            "session_id": "claude-session",
            "tool_name": "AskUserQuestion",
            "tool_use_id": "toolu_hook_1",
            "tool_input": {
                "questions": [{
                    "header": "Choice",
                    "question": "Pick one?",
                    "options": [
                        { "label": "A", "description": "first" },
                        { "label": "B", "description": "second" }
                    ]
                }]
            }
        });

        let events = parse_claude_hook_line(&agent, &source, 7, json);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "system");
        assert_eq!(events[0].kind, "control");
        assert_eq!(events[0].work_signal.as_deref(), Some("waiting_for_user"));
        assert_eq!(
            events[0].stop_reason.as_deref(),
            Some("user_question_requested")
        );
        assert_eq!(
            events[0].native_event_id.as_deref(),
            Some("claude-hook:toolu_hook_1")
        );
        assert!(events[0].text.contains("Open the agent terminal"));
        assert!(events[0].text.contains("Pick one?"));
        assert!(events[0].text.contains("1. A - first"));
        assert_eq!(native_work_event(&events[0]).unwrap().state, "maybeIdle");
        assert!(room_event_for(&events[0], Path::new("/tmp/kota")).is_some());
    }

    #[test]
    fn claude_interstitial_tool_preamble_becomes_room_only_commentary() {
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = "claude-jsonl".into();
        source.session_id = "claude-session".into();
        let json = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "assistant",
            "uuid": "u1",
            "requestId": "turn-a",
            "message": {
                "id": "message-a",
                "role": "assistant",
                "stop_reason": "tool_use",
                "content": [
                    { "type": "text", "text": "Clean worktree. Applying all edits now." },
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "Bash",
                        "input": { "command": "git status --short" }
                    }
                ]
            }
        });

        let events = parse_claude_line(&agent, &source, 0, json);

        // The pre-tool-call narration is reclassified as commentary (collapsible
        // progress), never a plain chat message.
        assert!(events.iter().all(|event| event.kind != "message"));
        let commentary = events
            .iter()
            .find(|event| event.kind == "commentary")
            .expect("preamble text becomes a commentary event");
        assert!(commentary.text.contains("Applying all edits now"));
        assert!(events.iter().any(|event| event.kind == "tool"));
        assert!(events
            .iter()
            .all(|event| event.work_signal.as_deref() == Some("activity")));
        // Visible as room-only progress, still kept out of the shared cross-agent log.
        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "commentary");
        assert!(shared.is_empty());
    }

    #[test]
    fn claude_split_interstitial_text_line_becomes_commentary() {
        // Claude Code writes the narration text and the `tool_use` on separate jsonl lines
        // that share `stop_reason: "tool_use"`. The text line has no tool block of its own,
        // yet must still collapse into room-only progress commentary.
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = "claude-jsonl".into();
        source.session_id = "claude-session".into();
        let text_line = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "assistant",
            "uuid": "u-text",
            "requestId": "turn-a",
            "message": {
                "id": "message-text",
                "role": "assistant",
                "stop_reason": "tool_use",
                "content": [
                    { "type": "text", "text": "Let me check the worktree first." }
                ]
            }
        });

        let events = parse_claude_line(&agent, &source, 0, text_line);

        assert!(events.iter().all(|event| event.kind != "message"));
        let commentary = events
            .iter()
            .find(|event| event.kind == "commentary")
            .expect("a lone narration line still becomes commentary");
        assert!(commentary.text.contains("check the worktree"));
        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "commentary");
        assert!(shared.is_empty());
    }

    #[test]
    fn claude_final_reply_stays_a_plain_message() {
        // The end-of-turn reply (`stop_reason: "end_turn"`) is the real answer and must stay
        // a visible chat message, never fold into progress commentary.
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = "claude-jsonl".into();
        source.session_id = "claude-session".into();
        let final_line = serde_json::json!({
            "timestamp": "2026-05-14T10:00:05Z",
            "type": "assistant",
            "uuid": "u-final",
            "message": {
                "id": "message-final",
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [
                    { "type": "text", "text": "All done — the fix is in." }
                ]
            }
        });

        let events = parse_claude_line(&agent, &source, 0, final_line);

        assert!(events.iter().any(|event| event.kind == "message"));
        assert!(events.iter().all(|event| event.kind != "commentary"));
    }

    #[test]
    fn claude_parser_skips_local_command_envelopes() {
        let mut agent = agent();
        agent.shell = "claude".into();
        let mut source = source();
        source.kind = "claude-jsonl".into();
        source.session_id = "claude-session".into();

        let local_command_events = [
            serde_json::json!({
                "timestamp": "2026-05-14T10:00:00Z",
                "type": "user",
                "isMeta": true,
                "message": {
                    "role": "user",
                    "content": "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-05-14T10:00:01Z",
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<command-name>/login</command-name>\n<command-message>login</command-message>\n<command-args></command-args>"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-05-14T10:00:02Z",
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<local-command-stdout>Login successful</local-command-stdout>"
                }
            }),
        ];

        for (index, line) in local_command_events.into_iter().enumerate() {
            assert!(parse_claude_line(&agent, &source, index, line).is_empty());
        }

        let user_line = serde_json::json!({
            "timestamp": "2026-05-14T10:00:03Z",
            "type": "user",
            "message": {
                "role": "user",
                "content": "look at this project"
            }
        });
        let events = parse_claude_line(&agent, &source, 3, user_line);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].text, "look at this project");
        assert_eq!(events[0].work_signal.as_deref(), Some("started"));
        assert_eq!(native_work_event(&events[0]).unwrap().state, "working");
    }

    #[test]
    fn opencode_step_finish_becomes_work_control_not_chat() {
        let json = serde_json::json!({
            "type": "step-finish",
            "reason": "stop"
        });

        let part = opencode_part_event(&json).unwrap();
        assert_eq!(part.kind, "control");
        assert_eq!(part.work_signal.as_deref(), Some("completed"));

        let event = control_event(
            &agent(),
            &source(),
            "2026-05-14T10:00:00Z",
            "step",
            part.work_signal.as_deref().unwrap(),
            part.reason,
            None,
        );
        assert!(room_event_for(&event, Path::new("/tmp/kota")).is_none());
        assert_eq!(native_work_event(&event).unwrap().state, "idle");
    }

    #[test]
    fn opencode_permission_log_control_is_visible_chat() {
        let mut agent = agent();
        agent.shell = "opencode".into();
        agent.agent_id = "agent-opencode".into();
        let source = NativeSource {
            kind: "opencode-sqlite".into(),
            session_id: "ses_test".into(),
            path: PathBuf::from("/tmp/opencode/opencode.db"),
            aux_path: None,
        };
        let line = r#"INFO  2026-05-24T00:46:43 +0ms service=permission id=per_123 permission=external_directory patterns=["/tmp/project-memory/attachments/*"] asking"#;

        assert_eq!(
            opencode_log_token_value(
                "INFO service=session.prompt session.id=ses_test id=per_123",
                "id"
            )
            .as_deref(),
            Some("per_123")
        );
        let event = opencode_permission_event_from_log_line(
            &agent,
            &source,
            Path::new("/tmp/opencode/log/2026-05-24T004622.log"),
            42,
            line,
            Some("ses_test"),
        )
        .unwrap();

        assert_eq!(event.role, "system");
        assert_eq!(event.kind, "control");
        assert_eq!(event.stop_reason.as_deref(), Some("permission_requested"));
        assert!(event.text.contains("external_directory approval"));
        assert!(event.text.contains("/tmp/project-memory/attachments/*"));
        assert_eq!(
            room_event_for(&event, Path::new("/tmp/kota")).unwrap().kind,
            "control"
        );
    }

    fn temp_violet_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kota-violet-{label}-{nanos}"))
    }

    fn chat_message(
        id: &str,
        role: &str,
        kind: &str,
        timestamp: &str,
        text: &str,
    ) -> VioletChatMessage {
        VioletChatMessage {
            id: id.into(),
            session_id: "s".into(),
            agent_id: "alice".into(),
            shell: "codex".into(),
            role: role.into(),
            kind: kind.into(),
            timestamp: timestamp.into(),
            text: text.into(),
            source_path: Some("/tmp/s.jsonl".into()),
            native_event_id: Some(id.into()),
            violet_seq: None,
            actor_intent: None,
            target_agent_ids: Vec::new(),
            agent_display_name: Some("Alice".into()),
            agent_avatar_id: None,
            agent_provider: None,
            agent_status: None,
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VioletMessageOrderFixture {
        name: String,
        messages: Vec<VioletMessageOrderFixtureMessage>,
        expected_ids: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VioletMessageOrderFixtureMessage {
        id: String,
        timestamp: String,
        violet_seq: Option<u64>,
    }

    #[test]
    fn violet_message_order_matches_shared_frontend_fixture() {
        let fixtures = serde_json::from_str::<Vec<VioletMessageOrderFixture>>(include_str!(
            "../../../tests/fixtures/violet-message-order.json"
        ))
        .unwrap();

        for fixture in fixtures {
            let messages = fixture
                .messages
                .into_iter()
                .map(|message| {
                    let mut event = chat_message(
                        &message.id,
                        "assistant",
                        "message",
                        &message.timestamp,
                        &format!("text for {}", message.id),
                    );
                    event.violet_seq = message.violet_seq;
                    event
                })
                .collect::<Vec<_>>();
            let ordered = apply_room_request_window(
                messages,
                &VioletRoomRequest {
                    project_root: None,
                    limit: Some(100),
                    before: None,
                    agent_ids: None,
                    watch_agent_ids: None,
                },
            )
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();

            assert_eq!(ordered, fixture.expected_ids, "fixture: {}", fixture.name);
        }
    }

    #[test]
    fn chathistory_writer_assigns_and_reuses_violet_sequences() {
        let root = temp_violet_dir("violet-sequence");
        fs::create_dir_all(&root).unwrap();
        let messages = vec![
            chat_message(
                "z-user-hash",
                "user",
                "message",
                "2026-07-27T10:00:00Z",
                "prompt",
            ),
            chat_message(
                "a-assistant-hash",
                "assistant",
                "message",
                "2026-07-27T10:00:00Z",
                "reply",
            ),
        ];

        write_chathistory_messages(&root, &messages).unwrap();
        let first = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(
            first
                .messages
                .iter()
                .map(|message| (message.id.as_str(), message.violet_seq))
                .collect::<Vec<_>>(),
            vec![("z-user-hash", Some(1)), ("a-assistant-hash", Some(2))]
        );

        write_chathistory_messages(&root, &[messages[1].clone(), messages[0].clone()]).unwrap();
        let second = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(
            second
                .messages
                .iter()
                .map(|message| (message.id.as_str(), message.violet_seq))
                .collect::<Vec<_>>(),
            vec![("z-user-hash", Some(1)), ("a-assistant-hash", Some(2))]
        );
        let manifest = fs::read_to_string(chathistory_manifest_path(&root)).unwrap();
        let manifest = serde_json::from_str::<JsonValue>(&manifest).unwrap();
        assert_eq!(
            manifest.get("next_seq").and_then(JsonValue::as_u64),
            Some(3)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chathistory_sequence_recovers_when_manifest_is_missing() {
        let root = temp_violet_dir("violet-sequence-recovery");
        ensure_chathistory_dirs(&root).unwrap();
        let mut existing = chathistory_event_from_message(
            &chat_message(
                "existing",
                "assistant",
                "message",
                "2026-07-27T10:00:00Z",
                "existing",
            ),
            None,
        );
        existing.violet_seq = Some(7);
        let path = chathistory_events_dir(&root).join("2026-07-27.jsonl");
        fs::write(&path, render_chathistory_events(&[existing]).unwrap()).unwrap();

        write_chathistory_messages(
            &root,
            &[chat_message(
                "next",
                "assistant",
                "message",
                "2026-07-27T10:00:01Z",
                "next",
            )],
        )
        .unwrap();

        let events = read_chathistory_event_file(&path).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.id.as_str(), event.violet_seq))
                .collect::<Vec<_>>(),
            vec![("existing", Some(7)), ("next", Some(8))]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn turn_credit_counts_only_assistant_messages_once() {
        let root = temp_violet_dir("turn-credit");
        fs::create_dir_all(&root).unwrap();
        let messages = vec![
            chat_message(
                "assistant-turn",
                "assistant",
                "message",
                "2026-05-21T10:00:00Z",
                "implemented the change",
            ),
            chat_message(
                "assistant-progress",
                "assistant",
                "commentary",
                "2026-05-21T10:00:01Z",
                "checking files",
            ),
            chat_message(
                "user-turn",
                "user",
                "message",
                "2026-05-21T10:00:02Z",
                "please continue",
            ),
        ];

        write_turn_credit_events_for_messages(&root, &messages).unwrap();
        write_turn_credit_events_for_messages(&root, &messages).unwrap();

        let ledger = fs::read_to_string(crate::credit_events_path(&root)).unwrap();
        let events = ledger
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(json_string(&events[0], &["event"]).as_deref(), Some("turn"));
        assert_eq!(
            json_string(&events[0], &["source_event_id"]).as_deref(),
            Some("assistant-turn")
        );
        assert_eq!(
            json_string(&events[0], &["agent_id"]).as_deref(),
            Some("alice")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_hero_id_for_agent_reads_current_yaml_shapes() {
        let root = temp_violet_dir("turn-credit-hero");
        let alice = root.join(".agent-workspaces").join("alice");
        let bob = root.join(".agent-workspaces").join("bob");
        fs::create_dir_all(&alice).unwrap();
        fs::create_dir_all(&bob).unwrap();
        fs::write(
            alice.join("agent.yaml"),
            "id: alice\nrecruited-from: hero-dex\n",
        )
        .unwrap();
        fs::write(
            bob.join("agent.yaml"),
            "id: bob\nsource:\n  hero-id: hero-gem\n",
        )
        .unwrap();

        assert_eq!(
            source_hero_id_for_agent(&root, "alice").as_deref(),
            Some("hero-dex")
        );
        assert_eq!(
            source_hero_id_for_agent(&root, "bob").as_deref(),
            Some("hero-gem")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_count_uses_only_end_turn_messages() {
        let root = temp_violet_dir("summary-count");
        fs::create_dir_all(&root).unwrap();
        write_chathistory_messages(
            &root,
            &[
                chat_message(
                    "user-end",
                    "user",
                    "message",
                    "2026-05-21T10:00:00Z",
                    "please fix it",
                ),
                chat_message(
                    "assistant-commentary",
                    "assistant",
                    "commentary",
                    "2026-05-21T10:01:00Z",
                    "I am reading files.",
                ),
                chat_message(
                    "assistant-progress",
                    "assistant",
                    "progress",
                    "2026-05-21T10:02:00Z",
                    "Running tests.",
                ),
                chat_message("tool", "assistant", "tool", "2026-05-21T10:03:00Z", "Bash"),
                chat_message(
                    "compaction",
                    "assistant",
                    "compaction",
                    "2026-05-21T10:04:00Z",
                    "compact",
                ),
                chat_message(
                    "system-message",
                    "system",
                    "message",
                    "2026-05-21T10:05:00Z",
                    "system note",
                ),
                chat_message(
                    "assistant-end",
                    "assistant",
                    "message",
                    "2026-05-21T10:06:00Z",
                    "done",
                ),
            ],
        )
        .unwrap();

        let events = read_summary_count_events(&root).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-end", "assistant-end"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_summary_writes_log_and_clears_outstanding() {
        let root = temp_violet_dir("summary-manual");
        fs::create_dir_all(&root).unwrap();
        write_chathistory_messages(
            &root,
            &[
                chat_message(
                    "user-end",
                    "user",
                    "message",
                    "2026-05-21T10:00:00Z",
                    "please fix it",
                ),
                chat_message(
                    "assistant-end",
                    "assistant",
                    "message",
                    "2026-05-21T10:06:00Z",
                    "fixed it",
                ),
            ],
        )
        .unwrap();

        let before = summary_status(
            &root,
            VioletSummaryRequest {
                project_root: None,
                config: None,
                auto_run: None,
            },
        )
        .unwrap();
        assert_eq!(before.outstanding.message_count, 2);

        let state = summarize_now(
            &root,
            VioletSummaryRequest {
                project_root: None,
                config: Some(VioletSummaryConfig {
                    provider: Some("codex".into()),
                    trigger_a_messages: Some(30),
                    trigger_b_hours: Some(2),
                    trigger_b_min_outstanding: Some(5),
                }),
                auto_run: None,
            },
        )
        .unwrap();

        let latest = state.latest.as_ref().unwrap();
        assert_eq!(latest.message_count, 2);
        assert_eq!(latest.completed, vec!["Recorded completed room work."]);
        assert_eq!(state.outstanding.message_count, 0);
        assert!(summary_log_path(&root).is_file());
        assert_eq!(read_summary_log(&root).unwrap().entries.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_status_auto_run_is_pure_read() {
        let root = temp_violet_dir("summary-status-pure");
        fs::create_dir_all(&root).unwrap();
        let messages = (0..30)
            .map(|index| {
                chat_message(
                    &format!("message-{index}"),
                    "user",
                    "message",
                    &format!("2026-05-21T10:{index:02}:00Z"),
                    "please summarize later",
                )
            })
            .collect::<Vec<_>>();
        write_chathistory_messages(&root, &messages).unwrap();

        let state = summary_status(
            &root,
            VioletSummaryRequest {
                project_root: None,
                config: Some(VioletSummaryConfig {
                    provider: Some("codex".into()),
                    trigger_a_messages: Some(30),
                    trigger_b_hours: Some(2),
                    trigger_b_min_outstanding: Some(5),
                }),
                auto_run: Some(true),
            },
        )
        .unwrap();

        assert_eq!(state.outstanding.message_count, 30);
        assert!(state.latest.is_none());
        assert!(!summary_log_path(&root).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_summary_writes_log_when_triggered() {
        let root = temp_violet_dir("summary-auto");
        fs::create_dir_all(&root).unwrap();
        let messages = (0..3)
            .map(|index| {
                chat_message(
                    &format!("message-{index}"),
                    "assistant",
                    "message",
                    &format!("2026-05-21T10:{index:02}:00Z"),
                    "completed work",
                )
            })
            .collect::<Vec<_>>();
        write_chathistory_messages(&root, &messages).unwrap();

        let state = summarize_auto(
            &root,
            VioletSummaryRequest {
                project_root: None,
                config: Some(VioletSummaryConfig {
                    provider: Some("codex".into()),
                    trigger_a_messages: Some(3),
                    trigger_b_hours: Some(2),
                    trigger_b_min_outstanding: Some(5),
                }),
                auto_run: None,
            },
        )
        .unwrap();

        let latest = state.latest.as_ref().unwrap();
        assert_eq!(latest.trigger, "auto-trigger-a");
        assert_eq!(latest.message_count, 3);
        assert_eq!(state.outstanding.message_count, 0);
        assert_eq!(read_summary_log(&root).unwrap().entries.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_summary_pauses_after_recent_failure_for_same_slice() {
        let root = temp_violet_dir("summary-auto-failure-cooldown");
        fs::create_dir_all(&root).unwrap();
        let messages = (0..3)
            .map(|index| {
                chat_message(
                    &format!("message-{index}"),
                    "assistant",
                    "message",
                    &format!("2026-05-21T10:{index:02}:00Z"),
                    "completed work",
                )
            })
            .collect::<Vec<_>>();
        write_chathistory_messages(&root, &messages).unwrap();

        let mut log = VioletSummaryLog {
            version: 1,
            updated_at: Utc::now().to_rfc3339(),
            entries: Vec::new(),
        };
        write_summary_log_entry(
            &root,
            &mut log,
            build_summary_log_entry(
                "auto-trigger-a",
                "codex",
                "2026-05-21T10:00:00Z",
                "2026-05-21T10:02:00Z",
                "message-2",
                3,
                Vec::new(),
                Some("codex summary CLI timed out".into()),
            ),
        )
        .unwrap();

        let state = summarize_auto(
            &root,
            VioletSummaryRequest {
                project_root: None,
                config: Some(VioletSummaryConfig {
                    provider: Some("codex".into()),
                    trigger_a_messages: Some(3),
                    trigger_b_hours: Some(2),
                    trigger_b_min_outstanding: Some(5),
                }),
                auto_run: None,
            },
        )
        .unwrap();

        assert!(state.latest.is_none());
        assert_eq!(state.outstanding.message_count, 3);
        assert!(state
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("auto retry paused"));
        assert_eq!(read_summary_log(&root).unwrap().entries.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_summary_cli_output_ignores_other_json_events() {
        let output = r#"{"type":"status","message":"running"}
{"completed":["Summarized the completed room work."]}
done"#;

        let completed = parse_summary_cli_output(output).unwrap();

        assert_eq!(completed, vec!["Summarized the completed room work."]);
    }

    #[test]
    fn locate_codex_source_prefers_newer_cwd_session_over_bound_session() {
        let root = temp_violet_dir("codex-new-session");
        let sessions_dir = root.join("sessions");
        let day_dir = sessions_dir.join("2026").join("06").join("02");
        let cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&day_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let old_path = day_dir.join("rollout-old.jsonl");
        fs::write(
            &old_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-old",
                        "cwd": path_string(&cwd)
                    }
                })
            ),
        )
        .unwrap();
        std::thread::sleep(StdDuration::from_millis(5));
        let new_path = day_dir.join("rollout-new.jsonl");
        fs::write(
            &new_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-new",
                        "cwd": path_string(&cwd),
                        "thread_source": "user",
                        "source": "cli"
                    }
                })
            ),
        )
        .unwrap();

        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "codex".into(),
            cwd,
            session_id: Some("session-old".into()),
        };

        let source = locate_codex_source_in(&sessions_dir, &agent)
            .unwrap()
            .expect("new Codex /new session should be detected");
        assert_eq!(source.session_id, "session-new");
        assert_eq!(source.path, new_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locate_codex_source_ignores_newer_subagent_session_for_same_cwd() {
        let root = temp_violet_dir("codex-subagent-session");
        let sessions_dir = root.join("sessions");
        let day_dir = sessions_dir.join("2026").join("07").join("13");
        let cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&day_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let main_path = day_dir.join("rollout-main.jsonl");
        fs::write(
            &main_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-main",
                        "cwd": path_string(&cwd),
                        "thread_source": "user",
                        "source": "cli"
                    }
                })
            ),
        )
        .unwrap();
        std::thread::sleep(StdDuration::from_millis(5));
        let subagent_path = day_dir.join("rollout-subagent.jsonl");
        fs::write(
            &subagent_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-subagent",
                        "cwd": path_string(&cwd),
                        "thread_source": "subagent",
                        "forked_from_id": "session-main",
                        "source": {
                            "subagent": {
                                "thread_spawn": {
                                    "parent_thread_id": "session-main",
                                    "depth": 1,
                                    "agent_path": "/root/audit"
                                }
                            }
                        }
                    }
                })
            ),
        )
        .unwrap();

        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "codex".into(),
            cwd,
            session_id: Some("session-main".into()),
        };

        let source = locate_codex_source_in(&sessions_dir, &agent)
            .unwrap()
            .expect("bound main Codex session should remain the Violet source");
        assert_eq!(source.session_id, "session-main");
        assert_eq!(source.path, main_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_session_meta_only_marks_explicit_subagent_sources() {
        let root = temp_violet_dir("codex-session-meta-kind");
        let cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&cwd).unwrap();

        let user_path = root.join("user.jsonl");
        fs::write(
            &user_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-user",
                        "cwd": path_string(&cwd),
                        "thread_source": "user",
                        "forked_from_id": "session-parent",
                        "source": "cli"
                    }
                })
            ),
        )
        .unwrap();
        let user_meta = read_codex_session_meta(&user_path).unwrap().unwrap();
        assert!(!user_meta.is_subagent);

        let nested_subagent_path = root.join("nested-subagent.jsonl");
        fs::write(
            &nested_subagent_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "meta": {
                            "id": "session-subagent",
                            "cwd": path_string(&cwd),
                            "source": { "subagent": { "thread_spawn": {} } }
                        }
                    }
                })
            ),
        )
        .unwrap();
        let subagent_meta = read_codex_session_meta(&nested_subagent_path)
            .unwrap()
            .unwrap();
        assert!(subagent_meta.is_subagent);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locate_claude_source_prefers_newer_project_transcript_over_bound_session() {
        let root = temp_violet_dir("claude-new-session");
        let home = root.join("home");
        let cwd = root.join(".agent-workspaces").join("alice");
        let project_dir = claude_project_dir(&home, &cwd);
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let old_path = project_dir.join("old-session.jsonl");
        fs::write(&old_path, "{}\n").unwrap();
        std::thread::sleep(StdDuration::from_millis(5));
        let new_path = project_dir.join("new-session.jsonl");
        fs::write(&new_path, "{}\n").unwrap();

        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "claude".into(),
            cwd,
            session_id: Some("old-session".into()),
        };

        let source = locate_claude_source_in(&home, &agent)
            .unwrap()
            .expect("new Claude /new transcript should be detected");
        assert_eq!(source.session_id, "new-session");
        assert_eq!(source.path, new_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn collect_claude_watch_paths_includes_transcript_parent() {
        let root = temp_violet_dir("claude-watch");
        let cwd = root.join(".agent-workspaces").join("alice");
        let transcript_dir = root.join(".claude").join("projects").join("alice");
        let source_path = transcript_dir.join("old-session.jsonl");
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&source_path, "{}\n").unwrap();

        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "claude".into(),
            cwd,
            session_id: Some("old-session".into()),
        };
        let source = NativeSource {
            kind: "claude-jsonl".into(),
            session_id: "old-session".into(),
            path: source_path.clone(),
            aux_path: None,
        };
        let mut plan = VioletWatchPlan::default();
        collect_violet_source_watch_paths(&agent, &source, &mut plan);

        assert!(plan.watched_paths.contains(&source_path));
        assert!(plan.watched_paths.contains(&transcript_dir));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_native_session_binding_to_agent_yaml() {
        let root = temp_violet_dir("session-binding");
        let cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            "id: alice\nstatus: active\nsessionId: stale-session\nsession-reset-at: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "codex".into(),
            cwd: cwd.clone(),
            session_id: Some("stale-session".into()),
        };
        let source = NativeSource {
            kind: "codex-jsonl".into(),
            session_id: "native-session".into(),
            path: root.join("native.jsonl"),
            aux_path: None,
        };

        write_agent_session_binding(&agent, &source).unwrap();

        let yaml = fs::read_to_string(cwd.join("agent.yaml")).unwrap();
        assert!(yaml.contains("id: alice"));
        assert!(yaml.contains("session-id: native-session"));
        assert!(yaml.contains("session-source: native"));
        assert!(yaml.contains("session-updated-at:"));
        assert!(!yaml.contains("sessionId:"));
        assert!(!yaml.contains("session-reset-at:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_reset_cutoff_ignores_older_native_source() {
        let root = temp_violet_dir("session-reset-cutoff");
        let cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&cwd).unwrap();
        let old_source_path = root.join("old.jsonl");
        fs::write(&old_source_path, "{}\n").unwrap();
        std::thread::sleep(StdDuration::from_millis(5));
        fs::write(
            cwd.join("agent.yaml"),
            format!(
                "id: alice\nstatus: active\nsession-reset-at: {}\n",
                now_iso()
            ),
        )
        .unwrap();
        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "claude".into(),
            cwd: cwd.clone(),
            session_id: None,
        };
        let old_source = NativeSource {
            kind: "claude-jsonl".into(),
            session_id: "old".into(),
            path: old_source_path,
            aux_path: None,
        };
        assert!(!native_source_is_after_agent_session_reset(
            &agent,
            &old_source
        ));

        std::thread::sleep(StdDuration::from_millis(5));
        let new_source_path = root.join("new.jsonl");
        fs::write(&new_source_path, "{}\n").unwrap();
        let new_source = NativeSource {
            kind: "claude-jsonl".into(),
            session_id: "new".into(),
            path: new_source_path,
            aux_path: None,
        };
        assert!(native_source_is_after_agent_session_reset(
            &agent,
            &new_source
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn antigravity_log_binds_workspace_to_latest_conversation() {
        let root = temp_violet_dir("antigravity-log");
        let app_dir = root.join("antigravity-cli");
        let log_dir = app_dir.join("log");
        let cwd = root.join("AgentWorkspaces").join("project").join("agent-a");
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let conversation_id = "0da22d80-5d25-4cbf-aeeb-d7cf38d3a7cf";
        let transcript = antigravity_transcript_path(&app_dir, conversation_id);
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(&transcript, "{}\n").unwrap();
        fs::write(
            log_dir.join("cli-test.log"),
            format!(
                "I0521 manager.go:249] Initializing CLI store manager for workspace {}\nI0521 server.go:747] Created conversation {}\nI0521 conversation_manager.go:378] Streaming conversation {}\n",
                cwd.display(),
                conversation_id,
                conversation_id
            ),
        )
        .unwrap();

        let found = antigravity_conversation_id_from_logs(&app_dir, &[cwd.clone()]).unwrap();

        assert_eq!(found.as_deref(), Some(conversation_id));
        assert_eq!(
            antigravity_conversation_id_from_log(
                &log_dir.join("cli-test.log"),
                &[root.join("other")]
            )
            .unwrap(),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_codex_user_message() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": { "type": "user_message", "message": "hello" }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].text, "hello");
    }

    #[test]
    fn internal_agent_bus_envelope_is_not_materialized_as_user_message() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "<KOTA_MESSAGE id=\"agentbus-agent-a-message-1\" from=\"agent-a\" to=\"agent-b\" intent=\"handoff\">\nhello\n</KOTA_MESSAGE>"
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert!(is_internal_agent_bus_envelope_event(&events[0]));
        let receipt = agent_bus_receipt_from_event(&events[0]).unwrap();
        assert_eq!(receipt.event_id, "agentbus-agent-a-message-1");
        assert_eq!(receipt.agent_id, "alice");

        let events = filter_internal_agent_bus_envelopes(events);
        assert!(events.is_empty());

        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "[Image #13]<KOTA_MESSAGE id=\"agentbus-agent-a-message-2\" from=\"agent-a\" to=\"agent-b\" intent=\"handoff\">\nhello\n</KOTA_MESSAGE>"
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert!(is_internal_agent_bus_envelope_event(&events[0]));
        let events = filter_internal_agent_bus_envelopes(events);
        assert!(events.is_empty());

        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "<KOTA_MESSAGE id=\"agentbus-agent-a-message-3\" from=\"agent-a\" to=\"agent-b\" intent=\"handoff\">\nhello\n</KOTA_MESSAGE>\u{15}"
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert!(is_internal_agent_bus_envelope_event(&events[0]));
        let events = filter_internal_agent_bus_envelopes(events);
        assert!(events.is_empty());

        let synthetic_actor_message = native_event("assistant", "message", "hello");
        let (room, shared) =
            split_for_violet_outputs(vec![synthetic_actor_message], Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(shared.len(), 1);
    }

    #[test]
    fn codex_commentary_phase_becomes_room_only_progress() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "I am checking the layout first.",
                "phase": "commentary"
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "assistant");
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].work_signal.as_deref(), Some("activity"));

        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "commentary");
        assert!(shared.is_empty());
    }

    #[test]
    fn codex_response_item_commentary_phase_becomes_commentary() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "phase": "commentary",
                "content": [
                    { "type": "output_text", "text": "Next I will run the focused test." }
                ]
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].text, "Next I will run the focused test.");
    }

    #[test]
    fn codex_routed_agent_message_body_becomes_room_only_progress() {
        let expected =
            "Child result.\n\nMessage Type: MESSAGE\nPayload:\nstill part of the child result";
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:12Z",
            "type": "response_item",
            "payload": {
                "type": "agent_message",
                "author": "/root/child",
                "recipient": "/root",
                "content": [{
                    "type": "input_text",
                    "text": format!(
                        "Message Type: A_FUTURE_KIND\nTask name: /root\nSender: /root/child\nPayload:\n{expected}"
                    )
                }],
                "internal_chat_message_metadata_passthrough": {
                    "turn_id": "turn-routed-child"
                }
            }
        });

        let events = parse_codex_line(&agent(), &source(), 7, line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_id, "alice");
        assert_eq!(events[0].role, "assistant");
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].text, expected);
        assert_eq!(events[0].turn_id.as_deref(), Some("turn-routed-child"));
        assert_eq!(events[0].work_signal, None);
        assert!(native_work_event(&events[0]).is_none());

        let message = event_to_message(events[0].clone());
        assert!(!message_counts_as_turn(&message));
        let history = chathistory_event_from_message(&message, None);
        assert!(!history.agent_visible);
        assert!(!is_summary_count_event(&history));

        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "commentary");
        assert!(shared.is_empty());
    }

    #[test]
    fn codex_routed_agent_message_without_plaintext_payload_stays_hidden() {
        let header = "Message Type: MESSAGE\nTask name: /root\nSender: /root/child\nPayload:\n";
        let encrypted: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:12Z",
            "type": "response_item",
            "payload": {
                "type": "agent_message",
                "author": "/root/child",
                "recipient": "/root",
                "content": [
                    { "type": "input_text", "text": header },
                    { "type": "encrypted_content", "encrypted_content": "ciphertext" }
                ]
            }
        });
        let empty: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:13Z",
            "type": "response_item",
            "payload": {
                "type": "agent_message",
                "author": "/root/child",
                "recipient": "/root",
                "content": [{ "type": "input_text", "text": header }]
            }
        });

        assert!(parse_codex_line(&agent(), &source(), 8, encrypted).is_empty());
        assert!(parse_codex_line(&agent(), &source(), 9, empty).is_empty());
    }

    #[test]
    fn malformed_codex_routed_agent_message_becomes_sanitized_progress() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:12Z",
            "type": "response_item",
            "payload": {
                "type": "agent_message",
                "author": "/root/child",
                "recipient": "/root",
                "content": [{
                    "type": "input_text",
                    "text": "Message Type: MESSAGE\nnot a header line\nTask name: /root\nSender: /root/child\nPayload:\nsecret internal text"
                }]
            }
        });

        let events = parse_codex_line(&agent(), &source(), 10, line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].text, CODEX_INTERNAL_PROGRESS_FORMAT_WARNING);
        assert!(!events[0].text.contains("secret internal text"));
        assert_eq!(events[0].work_signal, None);

        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert!(shared.is_empty());

        let missing_frame: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:13Z",
            "type": "response_item",
            "payload": {
                "type": "agent_message",
                "author": "/root/child",
                "recipient": "/root",
                "content": [
                    { "type": "encrypted_content", "encrypted_content": "ciphertext" }
                ]
            }
        });
        let events = parse_codex_line(&agent(), &source(), 11, missing_frame);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, CODEX_INTERNAL_PROGRESS_FORMAT_WARNING);
    }

    #[test]
    fn codex_metadata_text_in_conversation_and_unknown_items_stays_visible() {
        let text = "Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/child\nPayload:\nquoted by a normal speaker";
        for role in ["user", "assistant"] {
            let line: JsonValue = serde_json::json!({
                "timestamp": "2026-07-10T21:41:12Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": role,
                    "content": [{ "type": "input_text", "text": text }]
                }
            });
            let events = parse_codex_line(&agent(), &source(), 11, line);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].role, role);
            assert_eq!(events[0].kind, "message");
            assert_eq!(events[0].text, text);
        }

        let unknown: JsonValue = serde_json::json!({
            "timestamp": "2026-07-10T21:41:13Z",
            "type": "response_item",
            "payload": {
                "type": "future_provider_notice",
                "content": [{ "type": "input_text", "text": "provider schema changed" }]
            }
        });
        let events = parse_codex_line(&agent(), &source(), 12, unknown);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "assistant");
        assert_eq!(events[0].kind, "message");
        assert_eq!(events[0].text, "provider schema changed");
    }

    #[test]
    fn codex_image_wrapper_text_does_not_become_user_bubbles() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "<image name=[Image #1]>" },
                    { "type": "input_image", "image_url": "data:image/png;base64,abc" },
                    { "type": "input_text", "text": "</image>" },
                    { "type": "input_text", "text": "[Image #1]" }
                ]
            }
        });
        let events = parse_codex_line(&agent(), &source(), 0, line);
        assert!(events.is_empty());
    }

    #[test]
    fn codex_internal_context_does_not_become_user_bubbles() {
        let text = "<codex_internal_context source=\"goal\">\nContinue working toward the active thread goal.\n\n<objective>\nKeep going.\n</objective>\n</codex_internal_context>";
        let response_item: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": text }
                ]
            }
        });
        let event_msg: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": text
            }
        });

        assert!(parse_codex_line(&agent(), &source(), 0, response_item).is_empty());
        assert!(parse_codex_line(&agent(), &source(), 1, event_msg).is_empty());
    }

    #[test]
    fn persisted_codex_internal_context_does_not_render_or_count_for_summary() {
        let root = temp_violet_dir("codex-internal-context-persisted");
        ensure_chathistory_dirs(&root).unwrap();
        let leaked = chathistory_event_from_message(
            &chat_message(
                "goal-context",
                "user",
                "message",
                "2026-05-14T10:00:00Z",
                "<codex_internal_context source=\"goal\">\nContinue working toward the active thread goal.",
            ),
            None,
        );
        let normal = chathistory_event_from_message(
            &chat_message(
                "normal-user",
                "user",
                "message",
                "2026-05-14T10:01:00Z",
                "real user-facing prompt",
            ),
            None,
        );
        let bytes = render_chathistory_events(&[leaked, normal]).unwrap();
        fs::write(
            chathistory_events_dir(&root).join("2026-05-14.jsonl"),
            bytes.as_bytes(),
        )
        .unwrap();
        fs::write(chathistory_latest_path(&root), bytes).unwrap();

        let messages = read_chathistory_messages(
            &root,
            &VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "real user-facing prompt");

        let summary_events = read_summary_count_events(&root).unwrap();
        assert_eq!(summary_events.len(), 1);
        assert_eq!(summary_events[0].text, "real user-facing prompt");

        write_chathistory_latest(&root).unwrap();
        let latest_text = fs::read_to_string(chathistory_latest_path(&root)).unwrap();
        assert!(!latest_text.contains("codex_internal_context"));
        assert!(latest_text.contains("real user-facing prompt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_image_generation_end_becomes_one_content_addressed_artifact() {
        const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z6KAAAAAASUVORK5CYII=";
        let root = temp_violet_dir("codex-generated-image");
        let event_msg: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": "image-call-1",
                "status": "completed",
                "result": PNG_BASE64
            }
        });
        let response_item: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "response_item",
            "payload": {
                "type": "image_generation_call",
                "status": "completed",
                "result": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"
            }
        });

        let first = parse_codex_line_with_room_exceptions(
            &root,
            &[],
            &agent(),
            &source(),
            0,
            event_msg.clone(),
        );
        let second =
            parse_codex_line_with_room_exceptions(&root, &[], &agent(), &source(), 91, event_msg);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].role, "assistant");
        assert_eq!(first[0].kind, "artifact");
        assert_eq!(first[0].work_signal, None);
        assert_eq!(first[0].native_event_id, second[0].native_event_id);
        assert!(!first[0].text.contains(PNG_BASE64));
        let relative_path = first[0].text.lines().last().unwrap();
        assert!(relative_path.starts_with("project-memory/attachments/violet/codex-generated/"));
        let expected = BASE64_STANDARD.decode(PNG_BASE64).unwrap();
        assert_eq!(fs::read(root.join(relative_path)).unwrap(), expected);

        let deduped = dedupe_native_events(vec![first[0].clone(), second[0].clone()]);
        assert_eq!(deduped.len(), 1);
        let (room, shared) = split_for_violet_outputs(first, &root);
        assert_eq!(room.len(), 1);
        assert!(shared.is_empty());
        assert!(parse_codex_line_with_room_exceptions(
            &root,
            &[],
            &agent(),
            &source(),
            1,
            response_item,
        )
        .is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_image_generation_cold_read_keeps_large_jsonl_record() {
        const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z6KAAAAAASUVORK5CYII=";
        let root = temp_violet_dir("codex-generated-image-cold-read");
        fs::create_dir_all(&root).unwrap();
        let mut png = BASE64_STANDARD.decode(PNG_BASE64).unwrap();
        png.resize((JSONL_TAIL_BYTES as usize * 3 / 4) + 64 * 1024, 0);
        let line = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": "large-image-call",
                "status": "completed",
                "result": BASE64_STANDARD.encode(&png)
            }
        })
        .to_string();
        assert!(line.len() as u64 > JSONL_TAIL_BYTES);
        assert!((line.len() as u64) < CODEX_ROOM_EXCEPTION_JSONL_TAIL_BYTES);
        let rollout = root.join("rollout.jsonl");
        fs::write(&rollout, format!("{line}\n")).unwrap();
        let mut native_source = source();
        native_source.path = rollout;
        native_source.session_id = "large-image-session".into();

        let first = parse_source(&root, &agent(), &native_source, &[]).unwrap();
        let second = parse_source(&root, &agent(), &native_source, &[]).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, "artifact");
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].text, second[0].text);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_image_generation_exception_is_fail_closed_and_privacy_first() {
        let root = temp_violet_dir("codex-generated-image-private");
        let private_line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "image_generation_end",
                "call_id": "private-image-call",
                "status": "completed",
                "result": "this is deliberately not base64"
            }
        });
        let spans = vec![PrivacySpan {
            agent_id: "alice".into(),
            started_at: "2026-05-14T09:59:59Z".into(),
            ended_at: Some("2026-05-14T10:00:01Z".into()),
        }];
        let private = parse_codex_line_with_room_exceptions(
            &root,
            &spans,
            &agent(),
            &source(),
            0,
            private_line,
        );
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].kind, "tool");
        assert_eq!(private[0].work_signal, None);
        let (visible, skipped) = partition_private(private, &spans);
        assert!(visible.is_empty());
        assert_eq!(skipped, 1);
        assert!(!root
            .join("project-memory/attachments/violet/codex-generated")
            .exists());

        for (status, result) in [
            ("failed", "ignored"),
            ("completed", "not-base64"),
            ("completed", "bm90IGEgcG5n"),
        ] {
            let line: JsonValue = serde_json::json!({
                "timestamp": "2026-05-14T10:01:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "image_generation_end",
                    "call_id": format!("bad-{status}-{}", result.len()),
                    "status": status,
                    "result": result
                }
            });
            assert!(parse_codex_line_with_room_exceptions(
                &root,
                &[],
                &agent(),
                &source(),
                0,
                line,
            )
            .is_empty());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_custom_tool_images_remain_filtered_from_room() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call_output",
                "call_id": "composer-image-replay",
                "output": [
                    { "type": "input_text", "text": "Viewed Image" },
                    { "type": "input_image", "image_url": "data:image/png;base64,abc" }
                ]
            }
        });
        let events = parse_codex_line_with_room_exceptions(
            Path::new("/tmp/kota"),
            &[],
            &agent(),
            &source(),
            0,
            line,
        );
        assert!(events
            .iter()
            .all(|event| room_event_for(event, Path::new("/tmp/kota")).is_none()));
    }

    #[test]
    fn room_exception_registry_is_typed_and_rejects_dsl_fields() {
        let registry = parse_room_exception_registry(ROOM_EXCEPTION_CONFIG).unwrap();
        assert_eq!(registry.schema_version, 1);
        assert_eq!(registry.exceptions.len(), 1);
        assert_eq!(registry.exceptions[0].id, "codex-image-generation-v1");

        let with_unknown_field = ROOM_EXCEPTION_CONFIG.replacen(
            "\"field\": \"result\"",
            "\"field\": \"result\", \"requiredStatus\": \"completed\"",
            1,
        );
        assert!(parse_room_exception_registry(&with_unknown_field).is_err());
        let with_json_path = ROOM_EXCEPTION_CONFIG.replacen(
            "\"field\": \"result\"",
            "\"field\": \"payload.result\"",
            1,
        );
        assert!(parse_room_exception_registry(&with_json_path).is_err());
    }

    #[test]
    fn room_exception_png_validation_rejects_oversized_dimensions() {
        const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z6KAAAAAASUVORK5CYII=";
        let mut png = BASE64_STANDARD.decode(PNG_BASE64).unwrap();
        png[16..20].copy_from_slice(&(ROOM_EXCEPTION_MAX_IMAGE_DIMENSION + 1).to_be_bytes());
        let encoded = BASE64_STANDARD.encode(png);
        assert_eq!(
            decode_room_exception_png(&encoded),
            Err("image_dimensions_too_large")
        );
    }

    #[test]
    fn codex_user_message_keeps_text_but_drops_image_placeholder_only_messages() {
        let normal: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "看这张图",
                "local_images": ["/tmp/kota/project-memory/attachments/composer/att/original.png"]
            }
        });
        let placeholder: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "[Image #1]",
                "local_images": ["/tmp/kota/project-memory/attachments/composer/att/original.png"]
            }
        });

        let normal_events = parse_codex_line(&agent(), &source(), 0, normal);
        assert_eq!(normal_events.len(), 1);
        assert_eq!(normal_events[0].text, "看这张图");

        let placeholder_events = parse_codex_line(&agent(), &source(), 1, placeholder);
        assert!(placeholder_events.is_empty());
    }

    #[test]
    fn codex_turn_aborted_wrapper_becomes_system_interrupt_marker() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "<turn_aborted>\nThe user interrupted the previous turn on purpose. Any running unified exec processes may still be running in the background.\n</turn_aborted>"
                    }
                ]
            }
        });

        let events = parse_codex_line(&agent(), &source(), 42, line);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "system");
        assert_eq!(events[0].kind, "interrupt");
        assert_eq!(events[0].text, TURN_ABORTED_ROOM_TEXT);
        assert_eq!(events[0].work_signal.as_deref(), Some("interrupted"));
        assert_eq!(events[0].stop_reason.as_deref(), Some("turn_aborted"));

        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].kind, "interrupt");
        assert!(shared.is_empty());
    }

    #[test]
    fn codex_turn_aborted_event_marks_work_interrupted_without_room_message() {
        let line: JsonValue = serde_json::json!({
            "timestamp": "2026-05-14T10:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "turn_aborted",
                "turn_id": "turn-a",
                "reason": "interrupted"
            }
        });

        let events = parse_codex_line(&agent(), &source(), 43, line);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "system");
        assert_eq!(events[0].kind, "control");
        assert_eq!(events[0].work_signal.as_deref(), Some("interrupted"));
        assert_eq!(events[0].turn_id.as_deref(), Some("turn-a"));
        assert!(room_event_for(&events[0], Path::new("/tmp/kota")).is_none());
    }

    #[test]
    fn dedupe_keeps_repeated_codex_controls_in_same_time_bucket() {
        let first = control_event(
            &agent(),
            &source(),
            "2026-05-14T10:00:10Z",
            "10",
            "completed",
            Some("task_complete".into()),
            Some("turn-a".into()),
        );
        let second = control_event(
            &agent(),
            &source(),
            "2026-05-14T10:00:50Z",
            "20",
            "completed",
            Some("task_complete".into()),
            Some("turn-b".into()),
        );

        let events = dedupe_native_events(vec![first, second]);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.work_signal.as_deref() == Some("completed"))
                .count(),
            2
        );
    }

    #[test]
    fn cached_native_source_from_raw_logs_reuses_codex_active_session() {
        let root = temp_violet_dir("raw-log-source-cache");
        let cwd = root.join(".agent-workspaces").join("alice");
        let raw_dir = root.join("project-memory").join("raw_logs");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&raw_dir).unwrap();
        let source_path = root.join("codex-session.jsonl");
        fs::write(
            &source_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "session-a",
                        "cwd": path_string(&cwd)
                    }
                })
            ),
        )
        .unwrap();
        fs::write(
            raw_dir.join("session-a.md"),
            format!(
                "## 2026-05-21T05:23:30Z · alice · codex · session session-a\n\nUser:\nhello\n\nMetadata:\n- agent_id: alice\n- shell: codex\n- native_log: {}\n- kind: message\n\n",
                source_path.display()
            ),
        )
        .unwrap();
        let agent = ProjectAgent {
            agent_id: "alice".into(),
            shell: "codex".into(),
            cwd,
            session_id: None,
        };

        let source = cached_native_source_from_raw_logs(&root, &agent)
            .unwrap()
            .expect("raw logs should identify the active Codex source");
        assert_eq!(source.session_id, "session-a");
        assert_eq!(source.path, source_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_message_is_read_as_actor_message_with_target() {
        let root = temp_violet_dir("actor-message");
        fs::create_dir_all(&root).unwrap();

        record_actor_message(
            &root,
            &ActorMessageRecord {
                actor_id: "bartender".into(),
                actor_name: "Bartender".into(),
                text: "Please resolve this conflict.".into(),
                target_agent_ids: vec!["alice".into()],
                event_id: "bartender-conflict:alice:abc123:def456".into(),
                actor_intent: Some("conflict".into()),
            },
        )
        .unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        let message = &cache.messages[0];
        assert_eq!(message.agent_id, "bartender");
        assert_eq!(message.role, "assistant");
        assert_eq!(message.kind, "message");
        assert_eq!(message.text, "Please resolve this conflict.");
        assert_eq!(message.target_agent_ids, vec!["alice"]);
        assert_eq!(message.agent_display_name.as_deref(), Some("Bartender"));
        assert_eq!(message.agent_avatar_id.as_deref(), Some("bartender"));
        assert_eq!(message.agent_provider.as_deref(), Some("system"));
        assert_eq!(message.actor_intent.as_deref(), Some("conflict"));
        assert_eq!(
            message.native_event_id.as_deref(),
            Some("bartender-conflict:alice:abc123:def456")
        );
        assert!(actor_event_exists(&root, "bartender-conflict:alice:abc123:def456").unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_raw_replay_restores_missing_chathistory_message() {
        let root = temp_violet_dir("actor-raw-replay");
        let raw_dir = root.join("project-memory").join("raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        fs::write(
            raw_dir.join("actor-bartender.md"),
            "## 2026-05-21T10:00:00Z · bartender · system · session actor-bartender\n\nAssistant:\nRecovered actor note.\n\nMetadata:\n- agent_id: bartender\n- actor_name: Bartender\n- shell: system\n- native_log: /tmp/actor-messages\n- kind: message\n- native_event_id: actor-note-one\n- actor_intent: conflict\n- target_agent_ids: alice\n\n",
        )
        .unwrap();

        sync_actor_raw_messages_to_chathistory(&root).unwrap();
        sync_actor_raw_messages_to_chathistory(&root).unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        let message = &cache.messages[0];
        assert_eq!(message.agent_id, "bartender");
        assert_eq!(message.text, "Recovered actor note.");
        assert_eq!(message.target_agent_ids, vec!["alice"]);
        assert_eq!(message.agent_display_name.as_deref(), Some("Bartender"));
        assert_eq!(message.agent_avatar_id.as_deref(), Some("bartender"));
        assert_eq!(message.agent_provider.as_deref(), Some("system"));
        assert_eq!(message.native_event_id.as_deref(), Some("actor-note-one"));
        assert_eq!(message.actor_intent.as_deref(), Some("conflict"));

        let events = read_chathistory_event_segments(&root).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.source.native_event_id.as_deref() == Some("actor-note-one")
                })
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_raw_replay_dedupes_immediate_actor_chathistory_write() {
        let root = temp_violet_dir("actor-raw-replay-immediate-dedupe");
        fs::create_dir_all(&root).unwrap();
        record_actor_message(
            &root,
            &ActorMessageRecord {
                actor_id: "bartender".into(),
                actor_name: "Bartender".into(),
                text: "Already projected actor note.".into(),
                target_agent_ids: vec!["alice".into()],
                event_id: "actor-note-immediate".into(),
                actor_intent: None,
            },
        )
        .unwrap();

        sync_actor_raw_messages_to_chathistory(&root).unwrap();
        sync_actor_raw_messages_to_chathistory(&root).unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(cache.messages.len(), 1);
        assert_eq!(cache.messages[0].text, "Already projected actor note.");

        let events = read_chathistory_event_segments(&root).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.source.native_event_id.as_deref() == Some("actor-note-immediate")
                })
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_raw_replay_cursor_reads_appended_blocks_only() {
        let root = temp_violet_dir("actor-raw-replay-cursor-append");
        let raw_dir = root.join("project-memory").join("raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        let raw_path = raw_dir.join("actor-bartender.md");
        let first = "## 2026-05-21T10:00:00Z · bartender · system · session actor-bartender\n\nAssistant:\nFirst actor note.\n\nMetadata:\n- agent_id: bartender\n- actor_name: Bartender\n- shell: system\n- native_log: /tmp/actor-messages\n- kind: message\n- native_event_id: actor-note-one\n- target_agent_ids: alice\n\n";
        fs::write(&raw_path, first).unwrap();

        sync_actor_raw_messages_to_chathistory(&root).unwrap();
        let cursor_path = actor_raw_replay_cursor_path(&root).unwrap();
        let (cursor, _) = read_actor_raw_replay_cursor(&cursor_path).unwrap();
        let key = path_string(&raw_path);
        assert_eq!(cursor.files.get(&key).unwrap().offset, first.len() as u64);
        assert!(sync_actor_raw_messages_to_chathistory(&root)
            .unwrap()
            .is_empty());

        let second = "## 2026-05-21T10:01:00Z · bartender · system · session actor-bartender\n\nAssistant:\nSecond actor note.\n\nMetadata:\n- agent_id: bartender\n- actor_name: Bartender\n- shell: system\n- native_log: /tmp/actor-messages\n- kind: message\n- native_event_id: actor-note-two\n- target_agent_ids: alice\n\n";
        OpenOptions::new()
            .append(true)
            .open(&raw_path)
            .unwrap()
            .write_all(second.as_bytes())
            .unwrap();

        sync_actor_raw_messages_to_chathistory(&root).unwrap();
        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();
        let texts = cache
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["First actor note.", "Second actor note."]);
        let (cursor, _) = read_actor_raw_replay_cursor(&cursor_path).unwrap();
        assert_eq!(
            cursor.files.get(&key).unwrap().offset,
            fs::metadata(&raw_path).unwrap().len()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_raw_replay_cursor_resets_when_file_shrinks() {
        let root = temp_violet_dir("actor-raw-replay-cursor-reset");
        let raw_dir = root.join("project-memory").join("raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        let raw_path = raw_dir.join("actor-bartender.md");
        fs::write(
            &raw_path,
            "## 2026-05-21T10:00:00Z · bartender · system · session actor-bartender\n\nAssistant:\nA much longer actor note before rotation.\n\nMetadata:\n- agent_id: bartender\n- actor_name: Bartender\n- shell: system\n- native_log: /tmp/actor-messages\n- kind: message\n- native_event_id: actor-note-before-rotation\n- target_agent_ids: alice\n\n",
        )
        .unwrap();
        sync_actor_raw_messages_to_chathistory(&root).unwrap();

        fs::write(
            &raw_path,
            "## 2026-05-21T10:02:00Z · bartender · system · session actor-bartender\n\nAssistant:\nReset note.\n\nMetadata:\n- agent_id: bartender\n- actor_name: Bartender\n- shell: system\n- native_log: /tmp/actor-messages\n- kind: message\n- native_event_id: actor-note-after-reset\n- target_agent_ids: alice\n\n",
        )
        .unwrap();
        sync_actor_raw_messages_to_chathistory(&root).unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert!(cache
            .messages
            .iter()
            .any(|message| message.text == "Reset note."));
        let cursor_path = actor_raw_replay_cursor_path(&root).unwrap();
        let (cursor, _) = read_actor_raw_replay_cursor(&cursor_path).unwrap();
        assert_eq!(
            cursor.files.get(&path_string(&raw_path)).unwrap().offset,
            fs::metadata(&raw_path).unwrap().len()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chathistory_reader_hides_legacy_tool_events() {
        let root = temp_violet_dir("chathistory-tool-filter");
        fs::create_dir_all(&root).unwrap();
        write_chathistory_messages(
            &root,
            &[
                VioletChatMessage {
                    id: "tool-event".into(),
                    session_id: "s".into(),
                    agent_id: "alice".into(),
                    shell: "codex".into(),
                    role: "assistant".into(),
                    kind: "tool".into(),
                    timestamp: "2026-05-21T10:00:00Z".into(),
                    text: "Bash".into(),
                    source_path: Some("/tmp/s.jsonl".into()),
                    native_event_id: Some("tool".into()),
                    violet_seq: None,
                    actor_intent: None,
                    target_agent_ids: Vec::new(),
                    agent_display_name: None,
                    agent_avatar_id: None,
                    agent_provider: None,
                    agent_status: None,
                },
                VioletChatMessage {
                    id: "message-event".into(),
                    session_id: "s".into(),
                    agent_id: "alice".into(),
                    shell: "codex".into(),
                    role: "assistant".into(),
                    kind: "message".into(),
                    timestamp: "2026-05-21T10:01:00Z".into(),
                    text: "done".into(),
                    source_path: Some("/tmp/s.jsonl".into()),
                    native_event_id: Some("message".into()),
                    violet_seq: None,
                    actor_intent: None,
                    target_agent_ids: Vec::new(),
                    agent_display_name: None,
                    agent_avatar_id: None,
                    agent_provider: None,
                    agent_status: None,
                },
            ],
        )
        .unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        assert_eq!(cache.messages[0].text, "done");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chathistory_reader_hides_local_command_envelopes() {
        let root = temp_violet_dir("chathistory-local-command-filter");
        fs::create_dir_all(&root).unwrap();
        write_chathistory_messages(
            &root,
            &[
                VioletChatMessage {
                    id: "local-command".into(),
                    session_id: "s".into(),
                    agent_id: "alice".into(),
                    shell: "claude".into(),
                    role: "user".into(),
                    kind: "message".into(),
                    timestamp: "2026-05-21T10:00:00Z".into(),
                    text: "<local-command-stdout>Login successful</local-command-stdout>".into(),
                    source_path: Some("/tmp/s.jsonl".into()),
                    native_event_id: Some("local-command".into()),
                    violet_seq: None,
                    actor_intent: None,
                    target_agent_ids: Vec::new(),
                    agent_display_name: None,
                    agent_avatar_id: None,
                    agent_provider: None,
                    agent_status: None,
                },
                VioletChatMessage {
                    id: "real-user".into(),
                    session_id: "s".into(),
                    agent_id: "alice".into(),
                    shell: "claude".into(),
                    role: "user".into(),
                    kind: "message".into(),
                    timestamp: "2026-05-21T10:01:00Z".into(),
                    text: "look at this project".into(),
                    source_path: Some("/tmp/s.jsonl".into()),
                    native_event_id: Some("real-user".into()),
                    violet_seq: None,
                    actor_intent: None,
                    target_agent_ids: Vec::new(),
                    agent_display_name: None,
                    agent_avatar_id: None,
                    agent_provider: None,
                    agent_status: None,
                },
            ],
        )
        .unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        assert_eq!(cache.messages[0].text, "look at this project");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chathistory_writer_skips_non_utf8_event_lines() {
        let root = temp_violet_dir("chathistory-non-utf8-line");
        ensure_chathistory_dirs(&root).unwrap();
        let path = chathistory_events_dir(&root).join("2026-05-21.jsonl");
        let existing = chathistory_event_from_message(
            &chat_message(
                "existing",
                "assistant",
                "message",
                "2026-05-21T10:00:00Z",
                "old",
            ),
            None,
        );
        let mut bytes = render_chathistory_events(&[existing]).unwrap().into_bytes();
        bytes.extend_from_slice(&[0x98, b'{', b'"', b'b', b'r', b'o', b'k', b'e', b'n', b'\n']);
        fs::write(&path, bytes).unwrap();

        write_chathistory_messages(
            &root,
            &[chat_message(
                "next",
                "assistant",
                "message",
                "2026-05-21T10:01:00Z",
                "new",
            )],
        )
        .unwrap();

        let events = read_chathistory_event_file(&path).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["existing", "next"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_temp_paths_are_unique_per_write() {
        let root = temp_violet_dir("write-tmp-path");
        let path = root.join("project-memory/chathistory/events/2026-05-21.jsonl");
        let first = unique_write_tmp_path(&path);
        let second = unique_write_tmp_path(&path);

        assert_ne!(first, second);
        assert_eq!(first.parent(), path.parent());
        assert_eq!(second.parent(), path.parent());
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .starts_with(".2026-05-21.jsonl.tmp."));
    }

    #[test]
    fn read_cache_pages_messages_before_cursor() {
        let root = temp_violet_dir("violet-cache-pages");
        fs::create_dir_all(&root).unwrap();
        let message = |timestamp: &str, text: &str| VioletChatMessage {
            id: stable_message_id("s", "alice", timestamp, text),
            session_id: "s".into(),
            agent_id: "alice".into(),
            shell: "codex".into(),
            role: "assistant".into(),
            kind: "message".into(),
            timestamp: timestamp.into(),
            text: text.into(),
            source_path: Some("/tmp/s.jsonl".into()),
            native_event_id: Some(text.into()),
            violet_seq: None,
            actor_intent: None,
            target_agent_ids: Vec::new(),
            agent_display_name: None,
            agent_avatar_id: None,
            agent_provider: None,
            agent_status: None,
        };
        write_chathistory_messages(
            &root,
            &[
                message("2026-05-21T10:00:00Z", "one"),
                message("2026-05-21T10:01:00Z", "two"),
                message("2026-05-21T10:02:00Z", "three"),
            ],
        )
        .unwrap();

        let latest = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(2),
                before: None,
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );

        let older = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(2),
                before: Some("2026-05-21T10:02:00Z".into()),
                agent_ids: None,
                watch_agent_ids: None,
            },
        )
        .unwrap();
        assert_eq!(
            older
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filtered_cache_falls_back_to_segments_when_latest_is_sparse() {
        let root = temp_violet_dir("violet-filter-fallback");
        fs::create_dir_all(&root).unwrap();
        let message = |agent_id: &str, timestamp: &str, text: &str| VioletChatMessage {
            id: stable_message_id("s", agent_id, timestamp, text),
            session_id: "s".into(),
            agent_id: agent_id.into(),
            shell: "codex".into(),
            role: "assistant".into(),
            kind: "message".into(),
            timestamp: timestamp.into(),
            text: text.into(),
            source_path: Some("/tmp/s.jsonl".into()),
            native_event_id: Some(text.into()),
            violet_seq: None,
            actor_intent: None,
            target_agent_ids: Vec::new(),
            agent_display_name: None,
            agent_avatar_id: None,
            agent_provider: None,
            agent_status: None,
        };
        let mut messages = vec![message(
            "alice",
            "2026-05-20T09:00:00Z",
            "older alice reply outside latest",
        )];
        messages.extend((0..CHATHISTORY_LATEST_LIMIT).map(|index| {
            let hour = 10 + (index / 60);
            let minute = index % 60;
            message(
                "bob",
                &format!("2026-05-20T{hour:02}:{minute:02}:00Z"),
                &format!("bob latest {index}"),
            )
        }));
        write_chathistory_messages(&root, &messages).unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        assert_eq!(cache.messages[0].text, "older alice reply outside latest");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filtered_cache_includes_targeted_actor_messages() {
        let root = temp_violet_dir("violet-filter-targets");
        fs::create_dir_all(&root).unwrap();
        record_actor_message(
            &root,
            &ActorMessageRecord {
                actor_id: "bartender".into(),
                actor_name: "Bartender".into(),
                text: "Resolve worktree conflict.".into(),
                target_agent_ids: vec!["alice".into()],
                event_id: "bartender-conflict:alice:one:two".into(),
                actor_intent: Some("conflict".into()),
            },
        )
        .unwrap();

        let cache = read_cache(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(30),
                before: None,
                agent_ids: Some(vec!["alice".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert_eq!(cache.messages.len(), 1);
        assert_eq!(cache.messages[0].agent_id, "bartender");
        assert_eq!(cache.messages[0].target_agent_ids, vec!["alice"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incremental_jsonl_parser_keeps_cache_and_reads_appended_lines() {
        let root = temp_violet_dir("incremental");
        fs::create_dir_all(&root).unwrap();
        let log_path = root.join("codex-session.jsonl");
        fs::write(
            &log_path,
            r#"{"timestamp":"2026-05-14T10:00:00Z","type":"event_msg","payload":{"type":"agent_message","message":"first"}}"#,
        )
        .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();

        let mut source = source();
        source.path = log_path.clone();
        let first =
            parse_jsonl_source_incremental(&root, &agent(), &source, parse_codex_line).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, "first");

        fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap()
            .write_all(
                br#"{"timestamp":"2026-05-14T10:00:01Z","type":"event_msg","payload":{"type":"agent_message","message":"second"}}
"#,
            )
            .unwrap();
        let second =
            parse_jsonl_source_incremental(&root, &agent(), &source, parse_codex_line).unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(
            second
                .iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bootstrap_filter_drops_codex_setup_events() {
        let task_started = NativeEvent {
            session_id: "s".into(),
            agent_id: "alice".into(),
            shell: "codex".into(),
            role: "assistant".into(),
            kind: "message".into(),
            timestamp: "2026-05-14T10:00:00Z".into(),
            text: r#"{"collaboration_mode_kind":"default","model_context_window":258400,"turn_id":"turn","type":"task_started"}"#.into(),
            source_path: PathBuf::from("/tmp/a.jsonl"),
            native_event_id: None,
            work_signal: None,
            turn_id: None,
            stop_reason: None,
        };
        assert!(is_bootstrap_noise(&task_started));

        let adapter = NativeEvent {
            text: "# AGENTS.md instructions for /tmp/kota/.agent-workspaces/alice\n\n<INSTRUCTIONS>\n<!-- kota:adapter:AGENTS.md -->\n</INSTRUCTIONS>".into(),
            ..task_started
        };
        assert!(is_bootstrap_noise(&adapter));
    }

    #[test]
    fn bootstrap_filter_keeps_real_user_prompts() {
        assert!(!is_bootstrap_noise_text(
            "Please inspect AGENTS.md instructions for this repo."
        ));
        assert!(!is_bootstrap_noise_text(
            "Use this XML example:\n<INSTRUCTIONS>\nhello\n</INSTRUCTIONS>"
        ));
        assert!(!is_bootstrap_noise_text("/tmp/attachments/image-v20.png"));
    }

    #[test]
    fn normalized_cache_reader_skips_bootstrap_blocks() {
        let block = "## 2026-05-14T10:00:00Z · alice · codex · session rollout\n\nUser:\n# AGENTS.md instructions for /tmp/kota/.agent-workspaces/alice\n\n<INSTRUCTIONS>\n<!-- kota:adapter:AGENTS.md -->\n</INSTRUCTIONS>\n\nMetadata:\n- agent_id: alice\n- shell: codex\n- native_log: /tmp/a.jsonl\n- kind: message\n";
        assert!(parse_normalized_block(block, "rollout", Path::new("/tmp/rollout.md")).is_none());
    }

    #[test]
    fn normalized_cache_reader_skips_local_command_envelopes() {
        let block = "## 2026-05-14T10:00:00Z · alice · claude · session rollout\n\nUser:\n<command-name>/login</command-name>\n<command-message>login</command-message>\n<command-args></command-args>\n\nMetadata:\n- agent_id: alice\n- shell: claude\n- native_log: /tmp/a.jsonl\n- kind: message\n";
        assert!(parse_normalized_block(block, "rollout", Path::new("/tmp/rollout.md")).is_none());
    }

    #[test]
    fn normalized_cache_reader_preserves_markdown_headings_in_messages() {
        let root = temp_violet_dir("markdown-headings");
        let raw_dir = root.join("raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        fs::write(
            raw_dir.join("s.md"),
            "## 2026-05-14T10:00:00Z · alice · claude · session s\n\nAssistant:\nFirst paragraph.\n\n## Details\n\n### Nested\nbody\n\nMetadata:\n- agent_id: alice\n- shell: claude\n- native_log: /tmp/a.jsonl\n- kind: message\n\n## 2026-05-14T10:00:01Z · alice · claude · session s\n\nUser:\nnext message\n\nMetadata:\n- agent_id: alice\n- shell: claude\n- native_log: /tmp/a.jsonl\n- kind: message\n\n",
        )
        .unwrap();

        let messages = read_normalized_room_messages(&raw_dir).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].text,
            "First paragraph.\n\n## Details\n\n### Nested\nbody"
        );
        assert_eq!(messages[1].text, "next message");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn room_and_shared_events_drop_tool_calls_and_results() {
        let events = vec![
            native_event("user", "message", "show cwd"),
            native_event("assistant", "tool", "Bash"),
            native_event("user", "tool", "total 56\n-rw-r--r-- file"),
            native_event("assistant", "message", "cwd has files"),
        ];
        let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));

        assert_eq!(
            room.iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["show cwd", "cwd has files"]
        );
        assert!(room.iter().all(|event| event.kind == "message"));
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|event| event.kind == "message"));
        assert_eq!(
            shared
                .iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["show cwd", "cwd has files"]
        );
    }

    #[test]
    fn room_events_normalize_shared_attachment_paths_like_raw_cache() {
        let root = Path::new("/tmp/kota");
        let events = vec![native_event(
            "user",
            "message",
            "/tmp/kota/project-memory/attachments/composer/att_1/original.png 图上是什么",
        )];

        let (room, shared) = split_for_violet_outputs(events, root);

        assert_eq!(room.len(), 1);
        assert_eq!(
            room[0].text,
            "project-memory/attachments/composer/att_1/original.png 图上是什么"
        );
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].text, room[0].text);
    }

    #[test]
    fn requested_sync_does_not_render_raw_cache_when_native_source_is_missing() {
        let root = temp_violet_dir("requested-cache-no-fallback");
        fs::create_dir_all(root.join(".agent-workspaces/agent-1234567890")).unwrap();
        fs::write(
            root.join(".agent-workspaces/agent-1234567890/agent.yaml"),
            "id: agent-1234567890\nshell: claude\n",
        )
        .unwrap();
        let raw_dir = root.join("project-memory/raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        fs::write(
            raw_dir.join("s.md"),
            "## 2026-05-14T10:00:00Z · agent-1234567890 · claude · session s\n\nAssistant:\nhello from cache\n\nMetadata:\n- agent_id: agent-1234567890\n- shell: claude\n- native_log: /missing.jsonl\n- kind: message\n\n",
        )
        .unwrap();

        let state = sync_project(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: Some(vec!["agent-1234567890".into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert!(state.messages.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_preserves_existing_chathistory_for_session_tail_window() {
        let root = temp_violet_dir("sync-preserves-chathistory");
        let agent_id = "agent-preserve123";
        let session_id = "session-preserve";
        let cwd = root.join(".agent-workspaces").join(agent_id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            format!("id: {agent_id}\nshell: codex\nsession-id: {session_id}\n"),
        )
        .unwrap();

        let native_path = root.join("native-codex.jsonl");
        fs::write(
            &native_path,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": session_id,
                        "cwd": path_string(&cwd)
                    }
                }),
                serde_json::json!({
                    "timestamp": "2026-05-14T10:05:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "new tail message"
                    }
                })
            ),
        )
        .unwrap();

        write_chathistory_messages(
            &root,
            &[VioletChatMessage {
                id: "old-message".into(),
                session_id: session_id.into(),
                agent_id: agent_id.into(),
                shell: "codex".into(),
                role: "user".into(),
                kind: "message".into(),
                timestamp: "2026-05-14T10:00:00Z".into(),
                text: "old persisted message".into(),
                source_path: Some(path_string(&native_path)),
                native_event_id: Some("old-native".into()),
                violet_seq: None,
                actor_intent: None,
                target_agent_ids: Vec::new(),
                agent_display_name: None,
                agent_avatar_id: None,
                agent_provider: None,
                agent_status: None,
            }],
        )
        .unwrap();

        let raw_dir = root.join("project-memory/raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        fs::write(
            raw_dir.join(format!("{session_id}.md")),
            format!(
                "## 2026-05-14T10:05:00Z · {agent_id} · codex · session {session_id}\n\nUser:\nnew tail message\n\nMetadata:\n- agent_id: {agent_id}\n- shell: codex\n- native_log: {}\n- kind: message\n\n",
                native_path.display()
            ),
        )
        .unwrap();

        let state = sync_project(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: Some(vec![agent_id.into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        let events = read_chathistory_event_segments(&root).unwrap();
        let texts = events
            .iter()
            .map(|event| event.text.as_str())
            .collect::<Vec<_>>();
        assert!(texts.contains(&"old persisted message"));
        assert!(texts.contains(&"new tail message"));
        assert_eq!(state.messages.len(), 2);
        assert!(state
            .messages
            .iter()
            .any(|message| message.text == "old persisted message"));
        assert!(state
            .messages
            .iter()
            .any(|message| message.text == "new tail message"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_empty_tail_removes_raw_projection_not_chathistory() {
        let root = temp_violet_dir("sync-empty-tail-preserves-chathistory");
        let agent_id = "agent-emptytail123";
        let session_id = "session-emptytail";
        let cwd = root.join(".agent-workspaces").join(agent_id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            format!("id: {agent_id}\nshell: codex\nsession-id: {session_id}\n"),
        )
        .unwrap();

        let native_path = root.join("native-codex-tool-only.jsonl");
        fs::write(
            &native_path,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": session_id,
                        "cwd": path_string(&cwd)
                    }
                }),
                serde_json::json!({
                    "timestamp": "2026-05-14T10:05:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "tool_call",
                        "message": "tool output"
                    }
                })
            ),
        )
        .unwrap();

        write_chathistory_messages(
            &root,
            &[VioletChatMessage {
                id: "old-visible-message".into(),
                session_id: session_id.into(),
                agent_id: agent_id.into(),
                shell: "codex".into(),
                role: "user".into(),
                kind: "message".into(),
                timestamp: "2026-05-14T10:00:00Z".into(),
                text: "old visible message".into(),
                source_path: Some(path_string(&native_path)),
                native_event_id: Some("old-visible-native".into()),
                violet_seq: None,
                actor_intent: None,
                target_agent_ids: Vec::new(),
                agent_display_name: None,
                agent_avatar_id: None,
                agent_provider: None,
                agent_status: None,
            }],
        )
        .unwrap();

        let raw_dir = root.join("project-memory/raw_logs");
        fs::create_dir_all(&raw_dir).unwrap();
        let raw_path = raw_dir.join(format!("{session_id}.md"));
        fs::write(
            &raw_path,
            format!(
                "## 2026-05-14T10:05:00Z · {agent_id} · codex · session {session_id}\n\nTool:\ntool output\n\nMetadata:\n- agent_id: {agent_id}\n- shell: codex\n- native_log: {}\n- kind: tool\n\n",
                native_path.display()
            ),
        )
        .unwrap();

        let state = sync_project(
            &root,
            VioletRoomRequest {
                project_root: None,
                limit: Some(10),
                before: None,
                agent_ids: Some(vec![agent_id.into()]),
                watch_agent_ids: None,
            },
        )
        .unwrap();

        assert!(!raw_path.exists());
        let events = read_chathistory_event_segments(&root).unwrap();
        assert!(events
            .iter()
            .any(|event| event.text == "old visible message"));
        assert!(state
            .messages
            .iter()
            .any(|message| message.text == "old visible message"));
        assert_eq!(
            state.sources.first().map(|source| source.status.as_str()),
            Some("empty")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalized_cache_reader_skips_legacy_tool_result_blocks() {
        let block = "## 2026-05-14T10:00:00Z · alice · claude · session rollout\n\nUser:\ntotal 56\n-rw-r--r-- file\n\nMetadata:\n- agent_id: alice\n- shell: claude\n- native_log: /tmp/a.jsonl\n- kind: tool\n";
        assert!(parse_normalized_block(block, "rollout", Path::new("/tmp/rollout.md")).is_none());
    }

    #[test]
    fn shared_log_text_strips_ansi_redacts_tokens_and_relativizes_attachments() {
        let cleaned = clean_shared_log_text(
            Path::new("/tmp/kota"),
            "\u{1b}[0;32mToken:\u{1b}[0m gho_abcdefghijklmnopqrstuvwxyz /tmp/kota/project-memory/attachments/composer/att/original.png",
        );
        assert_eq!(
            cleaned,
            "Token: [redacted] project-memory/attachments/composer/att/original.png"
        );
    }

    #[test]
    fn discover_project_agents_skips_inactive_statuses() {
        let root = temp_violet_dir("agent-status");
        let agents_root = root.join(".agent-workspaces");
        fs::create_dir_all(&agents_root).unwrap();
        for (id, status) in [
            ("active", "active"),
            ("legacy", ""),
            ("archived", "archived"),
            ("dismissed", "dismissed"),
            ("removed", "removed"),
        ] {
            let dir = agents_root.join(id);
            fs::create_dir_all(&dir).unwrap();
            let status_line = if status.is_empty() {
                String::new()
            } else {
                format!("status: {status}\n")
            };
            fs::write(
                dir.join("agent.yaml"),
                format!("id: {id}\nshell: codex\n{status_line}"),
            )
            .unwrap();
        }

        let ids = discover_project_agents(&root)
            .unwrap()
            .into_iter()
            .map(|agent| agent.agent_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["active", "legacy"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discover_project_agents_reads_provider_from_shell_yaml() {
        let root = temp_violet_dir("agent-shell-yaml-provider");
        let cwd = root.join(".agent-workspaces/agent-antigravity");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            "id: agent-antigravity\nstatus: active\n",
        )
        .unwrap();
        fs::write(
            cwd.join("SHELL.yaml"),
            "provider: antigravity\ncommand: agy\n",
        )
        .unwrap();

        let agents = discover_project_agents(&root).unwrap();

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "agent-antigravity");
        assert_eq!(agents[0].shell, "antigravity");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn antigravity_transcript_yields_user_and_assistant_messages() {
        let root = temp_violet_dir("antigravity-transcript");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("transcript.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:21Z\",\"content\":\"<USER_REQUEST>\\nReply with exactly: KOTA_AGY_LOG_PROBE\\n</USER_REQUEST>\\n<ADDITIONAL_METADATA>\\nThe current local time is: 2026-05-20T11:21:21-07:00.\\n</ADDITIONAL_METADATA>\"}\n",
                "{\"step_index\":1,\"source\":\"SYSTEM\",\"type\":\"CONVERSATION_HISTORY\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:21Z\"}\n",
                "{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:21Z\",\"content\":\"KOTA_AGY_LOG_PROBE\"}\n",
            ),
        )
        .unwrap();
        let agent = ProjectAgent {
            agent_id: "agent-antigravity".into(),
            shell: "antigravity".into(),
            cwd: root.clone(),
            session_id: None,
        };
        let source = NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: "conversation-id".into(),
            path,
            aux_path: None,
        };

        let events =
            parse_jsonl_source_incremental(&root, &agent, &source, parse_antigravity_line).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].text, "Reply with exactly: KOTA_AGY_LOG_PROBE");
        assert_eq!(events[1].role, "assistant");
        assert_eq!(events[1].text, "KOTA_AGY_LOG_PROBE");
        assert_eq!(events[1].work_signal.as_deref(), Some("completed"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn antigravity_parser_skips_view_file_observations() {
        let agent = ProjectAgent {
            agent_id: "agent-antigravity".into(),
            shell: "antigravity".into(),
            cwd: PathBuf::from("/tmp/agent-antigravity"),
            session_id: None,
        };
        let source = NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: "conversation-id".into(),
            path: PathBuf::from("/tmp/transcript.jsonl"),
            aux_path: None,
        };

        let events = parse_antigravity_line(
            &agent,
            &source,
            6,
            serde_json::json!({
                "step_index": 6,
                "source": "MODEL",
                "type": "VIEW_FILE",
                "status": "DONE",
                "created_at": "2026-05-21T23:19:20Z",
                "content": "Created At: 2026-05-21T23:19:20Z\nCompleted At: 2026-05-21T23:19:20Z\nFile Path: `file:///tmp/kota/README.md`\nTotal Lines: 1\nTotal Bytes: 7\nShowing lines 1 to 1\n1: # Kota\nThe above content shows the entire, complete file contents of the requested file."
            }),
        );

        assert!(events.is_empty());
    }

    #[test]
    fn antigravity_parser_accepts_terminal_status_variants() {
        let agent = ProjectAgent {
            agent_id: "agent-antigravity".into(),
            shell: "antigravity".into(),
            cwd: PathBuf::from("/tmp/agent-antigravity"),
            session_id: None,
        };
        let source = NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: "conversation-id".into(),
            path: PathBuf::from("/tmp/transcript.jsonl"),
            aux_path: None,
        };

        let completed = parse_antigravity_line(
            &agent,
            &source,
            2,
            serde_json::json!({
                "step_index": 2,
                "source": "MODEL",
                "type": "FINAL_RESPONSE",
                "status": "COMPLETED",
                "created_at": "2026-05-20T18:21:21Z",
                "content": "done"
            }),
        );
        assert_eq!(completed[0].work_signal.as_deref(), Some("completed"));
        assert_eq!(native_work_event(&completed[0]).unwrap().state, "idle");

        let failed = parse_antigravity_line(
            &agent,
            &source,
            3,
            serde_json::json!({
                "step_index": 3,
                "source": "MODEL",
                "type": "ERROR_RESPONSE",
                "status": "ERROR",
                "created_at": "2026-05-20T18:21:22Z",
                "content": "failed"
            }),
        );
        assert_eq!(failed[0].work_signal.as_deref(), Some("failed"));
        assert_eq!(native_work_event(&failed[0]).unwrap().state, "failed");
    }

    #[test]
    fn antigravity_parser_suppresses_planner_text_after_background_wait_records() {
        let root = temp_violet_dir("antigravity-background-wait");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("transcript.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"step_index\":0,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:21Z\",\"tool_calls\":[{\"name\":\"schedule\",\"args\":{}}]}\n",
                "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"GENERIC\",\"status\":\"RUNNING\",\"created_at\":\"2026-05-20T18:21:22Z\",\"content\":\"Created At: 2026-05-20T18:21:22Z\\nTool is running as a background task with task id: task-1\"}\n",
                "{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:23Z\",\"content\":\"This would be visible without structural suppression.\"}\n",
                "{\"step_index\":3,\"source\":\"MODEL\",\"type\":\"GENERIC\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:24Z\",\"content\":\"Created At: 2026-05-20T18:21:24Z\\nCompleted At: 2026-05-20T18:21:24Z\\nTask: task-2\\nStatus: RUNNING\"}\n",
                "{\"step_index\":4,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:25Z\",\"content\":\"This second arbitrary planner response is also internal progress.\"}\n",
                "{\"step_index\":5,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:26Z\",\"content\":\"An update occurred on background task task-467.\\nLast progress: never\"}\n",
                "{\"step_index\":6,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-05-20T18:21:27Z\",\"content\":\"Actual user-visible answer.\"}\n",
            ),
        )
        .unwrap();
        let agent = ProjectAgent {
            agent_id: "agent-antigravity".into(),
            shell: "antigravity".into(),
            cwd: root.clone(),
            session_id: None,
        };
        let source = NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: "conversation-id".into(),
            path,
            aux_path: None,
        };

        let events = parse_antigravity_source(&root, &agent, &source).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, "Actual user-visible answer.");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn antigravity_planner_text_with_tool_calls_is_commentary() {
        let agent = ProjectAgent {
            agent_id: "agent-antigravity".into(),
            shell: "antigravity".into(),
            cwd: PathBuf::from("/tmp/agent-antigravity"),
            session_id: None,
        };
        let source = NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: "conversation-id".into(),
            path: PathBuf::from("/tmp/transcript.jsonl"),
            aux_path: None,
        };

        let events = parse_antigravity_line(
            &agent,
            &source,
            6,
            serde_json::json!({
                "step_index": 6,
                "source": "MODEL",
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "created_at": "2026-05-20T18:21:21Z",
                "content": "I need to inspect one more file.",
                "tool_calls": [{ "name": "view_file", "args": {} }]
            }),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "commentary");
        assert_eq!(events[0].work_signal.as_deref(), Some("activity"));
    }

    #[test]
    fn antigravity_transcript_can_be_matched_by_launch_cwd() {
        let root = temp_violet_dir("antigravity-cwd-match");
        let cwd = root.join("AgentWorkspaces/project/agent-a");
        fs::create_dir_all(&cwd).unwrap();
        let transcript = root.join("transcript.jsonl");
        fs::write(
            &transcript,
            format!(
                "{{\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"tool_calls\":[{{\"args\":{{\"DirectoryPath\":\"\\\"{}\\\"\"}}}}]}}\n",
                cwd.display()
            ),
        )
        .unwrap();

        assert!(antigravity_transcript_mentions_any_cwd(&transcript, &[cwd]).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn antigravity_prefers_full_transcript_when_available() {
        let root = temp_violet_dir("antigravity-full-transcript");
        let app_dir = root.join("antigravity-cli");
        let logs = app_dir
            .join("brain")
            .join("conversation-id")
            .join(".system_generated")
            .join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("transcript.jsonl"), "truncated\n").unwrap();
        fs::write(logs.join("transcript_full.jsonl"), "complete\n").unwrap();

        let preferred = preferred_antigravity_transcript_path(&app_dir, "conversation-id");

        assert_eq!(
            preferred.file_name().and_then(|name| name.to_str()),
            Some("transcript_full.jsonl")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_monitor_connection_detects_external_wal_commits() {
        let root = temp_violet_dir("opencode-monitor-data-version");
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("opencode.db");
        let writer = Connection::open(&db_path).unwrap();
        writer
            .execute_batch(
                "
                PRAGMA journal_mode=WAL;
                create table item (
                    id integer primary key,
                    value text
                );
                insert into item (value) values ('initial');
                ",
            )
            .unwrap();

        let monitor = open_opencode_monitor_db(&db_path).unwrap();
        let initial_version = opencode_data_version(&monitor).unwrap();
        assert!(monitor
            .execute("insert into item (value) values ('blocked')", [])
            .is_err());

        writer
            .execute("insert into item (value) values ('external')", [])
            .unwrap();
        let next_version = opencode_data_version(&monitor).unwrap();

        assert_ne!(initial_version, next_version);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_opencode_sqlite_reads_text_parts() {
        let root = temp_violet_dir("opencode-sqlite");
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            create table message (
                id text primary key,
                session_id text,
                time_created integer,
                time_updated integer,
                data text
            );
            create table part (
                id text primary key,
                session_id text,
                message_id text,
                time_created integer,
                time_updated integer,
                data text
            );
            ",
        )
        .unwrap();
        conn.execute(
            "insert into message (id, session_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?5)",
            params![
                "msg_user",
                "ses_test",
                1770000000000_i64,
                1770000000000_i64,
                r#"{"role":"user","time":{"created":1770000000000}}"#
            ],
        )
        .unwrap();
        conn.execute(
            "insert into message (id, session_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?5)",
            params![
                "msg_assistant",
                "ses_test",
                1770000000100_i64,
                1770000000100_i64,
                r#"{"role":"assistant","time":{"created":1770000000100}}"#
            ],
        )
        .unwrap();
        conn.execute(
            "insert into part (id, session_id, message_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "prt_user",
                "ses_test",
                "msg_user",
                1770000000000_i64,
                1770000000000_i64,
                r#"{"type":"text","text":"list cwd"}"#
            ],
        )
        .unwrap();
        conn.execute(
            "insert into part (id, session_id, message_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "prt_assistant",
                "ses_test",
                "msg_assistant",
                1770000000100_i64,
                1770000000100_i64,
                r#"{"type":"text","text":"found App.tsx"}"#
            ],
        )
        .unwrap();
        drop(conn);

        let agent = ProjectAgent {
            agent_id: "agent-opencode".into(),
            shell: "opencode".into(),
            cwd: root.clone(),
            session_id: Some("ses_test".into()),
        };
        let source = NativeSource {
            kind: "opencode-sqlite".into(),
            session_id: "ses_test".into(),
            path: db_path,
            aux_path: None,
        };

        let events = parse_opencode_sqlite(&agent, &source).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].role, "user");
        assert_eq!(events[0].text, "list cwd");
        assert_eq!(events[1].role, "assistant");
        assert_eq!(events[1].text, "found App.tsx");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_opencode_sqlite_surfaces_message_error() {
        let root = temp_violet_dir("opencode-sqlite-error");
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            create table message (
                id text primary key,
                session_id text,
                time_created integer,
                time_updated integer,
                data text
            );
            create table part (
                id text primary key,
                session_id text,
                message_id text,
                time_created integer,
                time_updated integer,
                data text
            );
            ",
        )
        .unwrap();
        conn.execute(
            "insert into message (id, session_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?5)",
            params![
                "msg_error",
                "ses_test",
                1770000000100_i64,
                1770000000200_i64,
                r#"{"role":"assistant","providerID":"kimi-for-coding","modelID":"k2p6","time":{"created":1770000000100,"completed":1770000000200},"error":{"name":"APIError","data":{"message":"Invalid request Error","statusCode":400}}}"#
            ],
        )
        .unwrap();
        drop(conn);

        let agent = ProjectAgent {
            agent_id: "agent-opencode".into(),
            shell: "opencode".into(),
            cwd: root.clone(),
            session_id: Some("ses_test".into()),
        };
        let source = NativeSource {
            kind: "opencode-sqlite".into(),
            session_id: "ses_test".into(),
            path: db_path,
            aux_path: None,
        };

        let events = parse_opencode_sqlite(&agent, &source).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].role, "assistant");
        assert_eq!(events[0].kind, "message");
        assert_eq!(
            events[0].text,
            "OpenCode error from kimi-for-coding/k2p6: Invalid request Error (HTTP 400)."
        );
        assert_eq!(events[0].work_signal.as_deref(), Some("failed"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ember_dream_none_marker_is_parsed_and_filtered() {
        let wrapped = format!(
            "{EMBER_DREAM_ENTRY_START}\n{EMBER_DREAM_EMPTY_MARKER}\n{EMBER_DREAM_ENTRY_END}"
        );

        assert_eq!(
            extract_ember_dream_entry_text(&wrapped).as_deref(),
            Some(EMBER_DREAM_EMPTY_MARKER)
        );
        assert!(is_empty_ember_dream_entry_text(EMBER_DREAM_EMPTY_MARKER));
        assert!(is_empty_ember_dream_entry_text(&format!(
            "- {EMBER_DREAM_EMPTY_MARKER}"
        )));
        assert!(is_empty_dream_placeholder(EMBER_DREAM_EMPTY_MARKER));
        assert!(!is_empty_ember_dream_entry_text(
            "- Prefers concise reviews."
        ));
    }

    #[test]
    fn ember_dream_entries_split_into_atomic_candidates() {
        assert_eq!(
            split_ember_dream_entry_items(
                "- Prefers concise reviews.\n- Notices hidden coordination costs."
            ),
            vec![
                "Prefers concise reviews.".to_string(),
                "Notices hidden coordination costs.".to_string(),
            ]
        );
        assert!(split_ember_dream_entry_items(&format!("- {EMBER_DREAM_EMPTY_MARKER}")).is_empty());
    }

    #[test]
    fn ember_dream_prompt_items_hide_projects_and_keep_private_mapping() {
        let current = vec!["Existing portrait.".to_string()];
        let projects = vec![
            vec![EmberDreamEntryRecord {
                event_id: "event-1".into(),
                ts: "2026-07-18T12:00:00Z".into(),
                agent_id: "agent-1".into(),
                agent_display_name: Some("Agent One".into()),
                text: "- First observation.\n- Second observation.".into(),
            }],
            vec![EmberDreamEntryRecord {
                event_id: "event-2".into(),
                ts: "2026-07-18T12:01:00Z".into(),
                agent_id: "agent-2".into(),
                agent_display_name: Some("Agent Two".into()),
                text: EMBER_DREAM_EMPTY_MARKER.into(),
            }],
        ];

        let (prompt_items, candidates) = build_ember_dream_consolidation_items(&current, &projects);

        assert_eq!(prompt_items.len(), 3);
        assert_eq!(prompt_items[0].id, "active-1");
        assert_eq!(prompt_items[1].id, "candidate-1");
        assert_eq!(prompt_items[2].id, "candidate-2");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].project_index, 0);
        assert_eq!(candidates[1].project_index, 0);
        let serialized = serde_json::to_string(&prompt_items).unwrap();
        assert!(!serialized.contains("project"));
        assert!(!serialized.contains("event-1"));
        assert!(!serialized.contains("agent-1"));
    }

    #[test]
    fn ember_dream_rewrite_refreshes_and_archives_the_old_text() {
        let current = vec![
            "Oldest Dream.".to_string(),
            "Dream needing detail.".to_string(),
            "Recent Dream.".to_string(),
        ];
        let candidates = vec![EmberDreamCandidateItem {
            id: "candidate-1".into(),
            project_index: 0,
            text: "Brand-new Dream.".into(),
        }];
        let decisions = HashMap::from([
            ("active-1".into(), EmberDreamDecisionAction::Keep),
            (
                "active-2".into(),
                EmberDreamDecisionAction::Rewrite("Rewritten Dream.".into()),
            ),
            ("active-3".into(), EmberDreamDecisionAction::Keep),
            ("candidate-1".into(), EmberDreamDecisionAction::Keep),
        ]);

        let result = apply_ember_dream_decisions(&current, &candidates, &decisions, 3).unwrap();

        assert_eq!(
            result.active,
            vec!["Recent Dream.", "Rewritten Dream.", "Brand-new Dream."]
        );
        assert_eq!(
            result.archived,
            vec!["Dream needing detail.", "Oldest Dream."]
        );
    }

    #[test]
    fn ember_dream_candidate_slots_round_robin_across_projects() {
        let candidates = vec![
            EmberDreamCandidateItem {
                id: "candidate-1".into(),
                project_index: 0,
                text: "Project zero first.".into(),
            },
            EmberDreamCandidateItem {
                id: "candidate-2".into(),
                project_index: 0,
                text: "Project zero second.".into(),
            },
            EmberDreamCandidateItem {
                id: "candidate-3".into(),
                project_index: 1,
                text: "Project one first.".into(),
            },
        ];
        let decisions = candidates
            .iter()
            .map(|candidate| (candidate.id.clone(), EmberDreamDecisionAction::Keep))
            .collect::<HashMap<_, _>>();

        let result = apply_ember_dream_decisions(&[], &candidates, &decisions, 2).unwrap();

        assert_eq!(
            result.active,
            vec!["Project zero first.", "Project one first."]
        );
        assert!(result.archived.is_empty());
    }

    #[test]
    fn ember_dream_decisions_require_full_valid_coverage() {
        let items = vec![
            EmberDreamPromptItem {
                id: "active-1".into(),
                kind: "active".into(),
                text: "Existing Dream.".into(),
            },
            EmberDreamPromptItem {
                id: "candidate-1".into(),
                kind: "candidate".into(),
                text: "Candidate Dream.".into(),
            },
        ];
        let incomplete = vec![EmberDreamDecision {
            id: "active-1".into(),
            op: "keep".into(),
            text: None,
        }];
        assert!(validate_ember_dream_decisions(&items, incomplete).is_err());

        let complete = vec![
            EmberDreamDecision {
                id: "active-1".into(),
                op: "rewrite".into(),
                text: Some("Existing Dream.".into()),
            },
            EmberDreamDecision {
                id: "candidate-1".into(),
                op: "drop".into(),
                text: None,
            },
        ];
        let validated = validate_ember_dream_decisions(&items, complete).unwrap();
        assert_eq!(
            validated.get("active-1"),
            Some(&EmberDreamDecisionAction::Keep)
        );
    }

    #[test]
    fn privacy_span_filters_event() {
        let event = native_event("assistant", "message", "secret");
        let spans = vec![PrivacySpan {
            agent_id: "alice".into(),
            started_at: "2026-05-14T09:59:59Z".into(),
            ended_at: Some("2026-05-14T10:01:00Z".into()),
        }];
        assert!(is_private_event(&event, &spans));
    }
}
