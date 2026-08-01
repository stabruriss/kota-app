//! Laughing Man — Telegram bridge (MVP).
//!
//! Spec: `product-design/Laughing Man - MVP Handoff.md` (v2.0-frozen).
//! Kota only sends/receives; no routing. The bot side always has one
//! explicitly selected project + input target; while selected (and not muted)
//! every visible end-turn from that project is pushed to the phone.
//!
//! Module boundary: telegram API calls, panel rendering and state handling
//! are plain functions over plain data so the loop can later move into a
//! sidecar/VPS runner; only the thread entrypoints take an `AppHandle`
//! (same pattern as the agent bus / Ember dispatch watchers).

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

const STATE_SCHEMA: &str = "kota.laughing-man.state.v1";
const LOG_SCHEMA: &str = "kota.laughing-man.log.v1";
const POLL_TIMEOUT_SECS: u64 = 50;
const CATALOG_REFRESH_SECS: u64 = 45;
const STANDBY_LOOP_SECS: u64 = 30;
const STANDBY_PULL_LIMIT: usize = 20;
const STANDBY_SEEN_RING_MAX: usize = 600;
const STANDBY_PROTOCOL: &str = "kota-lm-standby.v1";
const STANDBY_RECOMMENDED_VERSION: &str = "0.1.2";
const API_BASE: &str = "https://api.telegram.org";
const MAX_TELEGRAM_TEXT_CHARS: usize = 3_800;
const MAX_LOG_ENTRIES: usize = 500;
const PUSHED_EVENT_RING_MAX: usize = 600;
const OUTBOUND_REPLAY_LIMIT: usize = 2;
const OUTBOUND_REPLAY_ON_SWITCH: bool = false;
const MAX_MEDIA_BYTES: u64 = 20 * 1024 * 1024;
const BACKLOG_DRAIN_GAP_MS: u64 = 1500;
const TYPING_ACTION_SECS: u64 = 15;
const TRANSIENT_POLL_ERROR_UI_DELAY_SECS: u64 = 30 * 60;
/// Bridge-side wake retry (per handoff §2: bounded backoff, do NOT touch the
/// generic agent_bus grace).
const BUS_RETRY_DELAYS_SECS: [u64; 3] = [5, 10, 20];

// ─────────────────────────── storage layout ───────────────────────────

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn lm_dir() -> PathBuf {
    home_dir().join("Kota").join("laughing-man")
}

fn token_path() -> PathBuf {
    lm_dir().join("token")
}

fn state_path() -> PathBuf {
    lm_dir().join("state.json")
}

fn log_path() -> PathBuf {
    lm_dir().join("messages.jsonl")
}

fn lock_path() -> PathBuf {
    lm_dir().join("lock")
}

// ─────────────────────────── state model ───────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmSelected {
    pub project_id: String,
    pub project_root: String,
    pub project_name: String,
    pub agent_id: String,
    pub agent_name: String,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmPanelSession {
    #[serde(default)]
    pub message_id: Option<i64>,
    #[serde(default)]
    pub revision: u64,
    /// index → (project_id, project_root, project_name)
    #[serde(default)]
    pub projects: Vec<(String, String, String)>,
    /// index → (agent_id, agent_name) for the currently shown project page
    #[serde(default)]
    pub agents: Vec<(String, String)>,
    #[serde(default)]
    pub agents_project: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmCatalogProject {
    pub project_id: String,
    pub project_root: String,
    pub project_name: String,
    #[serde(default)]
    pub agents: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbyConfig {
    pub worker_url: String,
    pub device_id: String,
    pub desktop_secret: String,
    pub webhook_secret: String,
    pub paired_at: String,
    #[serde(default)]
    pub relay_version: Option<String>,
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub last_heartbeat_at: Option<String>,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub queue_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbyStatus {
    pub worker_url: String,
    pub live: bool,
    pub last_heartbeat_at: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub relay_version: Option<String>,
    pub protocol_version: Option<String>,
    pub recommended_version: String,
    pub update_available: bool,
    pub queue_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbyQueueItem {
    pub id: String,
    pub received_at: String,
    pub preview: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub sent_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
    #[serde(default)]
    pub update: JsonValue,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbyConnectRequest {
    pub worker_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbySendRequest {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStandbyDeleteRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmPendingClaim {
    pub user_id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmState {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub owner_user_id: Option<i64>,
    #[serde(default)]
    pub owner_chat_id: Option<i64>,
    #[serde(default)]
    pub bot_username: Option<String>,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub selected: Option<LmSelected>,
    #[serde(default)]
    pub panel: LmPanelSession,
    #[serde(default)]
    pub catalog: Vec<LmCatalogProject>,
    #[serde(default)]
    pub catalog_updated_at: Option<String>,
    #[serde(default)]
    pub standby: Option<LmStandbyConfig>,
    #[serde(default)]
    pub standby_queue: Vec<LmStandbyQueueItem>,
    #[serde(default)]
    pub standby_seen_queue_ids: VecDeque<String>,
    #[serde(default)]
    pub pending_claim: Option<LmPendingClaim>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// First unix second in the current transient getUpdates failure streak.
    #[serde(default)]
    pub transient_poll_failure_since: Option<u64>,
    #[serde(default)]
    pub last_transient_poll_error: Option<String>,
    /// chathistory byte offsets per project root (outbound incremental read)
    #[serde(default)]
    pub chathistory_offsets: std::collections::BTreeMap<String, u64>,
    /// recently pushed outbound event ids (chathistory + explicit sends)
    #[serde(default)]
    pub pushed_event_ids: VecDeque<String>,
    /// telegram update_ids already fully handled (persisted idempotency ring)
    #[serde(default)]
    pub processed_update_ids: VecDeque<i64>,
    /// delivery attempts per update_id (gives up + advances after limit)
    #[serde(default)]
    pub delivery_attempts: std::collections::BTreeMap<i64, u32>,
    /// projects whose chathistory has been seeded (no history replay)
    #[serde(default)]
    pub seeded_projects: std::collections::BTreeSet<String>,
    /// last visible project event seen by the outbound loop, keyed by project root
    #[serde(default)]
    pub outbound_last_seen_event_ids: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmLogEntry {
    pub schema: String,
    pub id: String,
    pub ts: String,
    /// "in" (phone → agent) | "out" (agent → phone) | "system"
    pub direction: String,
    pub project_id: String,
    pub project_name: String,
    pub agent_id: String,
    pub agent_name: String,
    pub preview: String,
    #[serde(default)]
    pub media_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_recorded_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStatus {
    pub configured: bool,
    pub enabled: bool,
    pub running: bool,
    pub bot_username: Option<String>,
    pub owner_user_id: Option<i64>,
    pub pending_claim: Option<LmPendingClaim>,
    pub selected: Option<LmSelected>,
    pub last_error: Option<String>,
    pub latest: Option<LmLogEntry>,
    pub standby: Option<LmStandbyStatus>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmEmberReminderRequest {
    pub event_id: String,
    pub text: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static STATE_LOCK: Mutex<()> = Mutex::new(());
static EMBER_REMINDER_LOCK: Mutex<()> = Mutex::new(());
static WORKING_AGENT_IDS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

fn working_agent_ids() -> &'static Mutex<BTreeSet<String>> {
    WORKING_AGENT_IDS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// All state read-modify-writes go through here — poll loop, outbound loop,
/// callbacks, ack timers and IPC all touch state.json concurrently (颦儿 P1).
pub fn update_state<T>(mutate: impl FnOnce(&mut LmState) -> T) -> T {
    let _guard = STATE_LOCK.lock().expect("lm state lock poisoned");
    let mut state = load_state_unlocked();
    let out = mutate(&mut state);
    let _ = save_state_unlocked(&state);
    out
}

pub fn load_state() -> LmState {
    let _guard = STATE_LOCK.lock().expect("lm state lock poisoned");
    load_state_unlocked()
}

fn load_state_unlocked() -> LmState {
    let path = state_path();
    let mut state = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LmState>(&bytes).ok())
        .unwrap_or_default();
    state.schema = STATE_SCHEMA.into();
    state
}

pub fn save_state(state: &LmState) -> Result<()> {
    let _guard = STATE_LOCK.lock().expect("lm state lock poisoned");
    save_state_unlocked(state)
}

fn save_state_unlocked(state: &LmState) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

// Token: plain file, 0600, never logged (handoff §4 sanctioned fallback;
// Keychain is the P1 upgrade).
pub fn load_token() -> Option<String> {
    fs::read_to_string(token_path())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn save_token(token: &str) -> Result<()> {
    let path = token_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(token.trim().as_bytes())?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    {
        fs::write(&path, token.trim())?;
        Ok(())
    }
}

pub fn delete_token() {
    let _ = fs::remove_file(token_path());
}

fn append_log(entry: &LmLogEntry) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(line) = serde_json::to_string(entry) {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{line}");
        }
    }
    // Keep the log bounded: compact to the newest MAX_LOG_ENTRIES once the
    // file grows past twice that.
    if let Ok(content) = fs::read_to_string(&path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > MAX_LOG_ENTRIES * 2 {
            let keep = lines[lines.len() - MAX_LOG_ENTRIES..].join("\n");
            let tmp = path.with_extension("jsonl.tmp");
            if fs::write(&tmp, format!("{keep}\n")).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }
}

pub fn read_log(limit: usize) -> Vec<LmLogEntry> {
    let Ok(content) = fs::read_to_string(log_path()) else {
        return Vec::new();
    };
    let mut entries: Vec<LmLogEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if entries.len() > limit {
        entries = entries.split_off(entries.len() - limit);
    }
    entries.reverse(); // newest first
    entries
}

fn log_preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = compact.chars().take(120).collect();
    if compact.chars().count() > 120 {
        out.push('…');
    }
    out
}

fn mint_log_id(direction: &str) -> String {
    format!("lm-{direction}-{}", uuid::Uuid::new_v4().simple())
}

fn remember_pushed_event(state: &mut LmState, event_id: String) {
    if !state.pushed_event_ids.contains(&event_id) {
        state.pushed_event_ids.push_back(event_id);
    }
    while state.pushed_event_ids.len() > PUSHED_EVENT_RING_MAX {
        state.pushed_event_ids.pop_front();
    }
}

fn standby_status_from_config(config: &LmStandbyConfig) -> LmStandbyStatus {
    let update_available = config
        .relay_version
        .as_deref()
        .is_some_and(|version| version != STANDBY_RECOMMENDED_VERSION);
    LmStandbyStatus {
        worker_url: config.worker_url.clone(),
        live: config.live,
        last_heartbeat_at: config.last_heartbeat_at.clone(),
        last_sync_at: config.last_sync_at.clone(),
        last_error: config.last_error.clone(),
        relay_version: config.relay_version.clone(),
        protocol_version: config.protocol_version.clone(),
        recommended_version: STANDBY_RECOMMENDED_VERSION.into(),
        update_available,
        queue_count: config.queue_count,
    }
}

fn catalog_from_projects(projects: Vec<LmProjectInfo>) -> Vec<LmCatalogProject> {
    projects
        .into_iter()
        .map(|project| LmCatalogProject {
            project_id: project.project_id,
            project_root: project.project_root,
            project_name: project.project_name,
            agents: project.agents,
        })
        .collect()
}

fn catalog_project_rows(catalog: &[LmCatalogProject]) -> Vec<(String, String, String)> {
    catalog
        .iter()
        .map(|project| {
            (
                project.project_id.clone(),
                project.project_root.clone(),
                project.project_name.clone(),
            )
        })
        .collect()
}

fn catalog_agents(catalog: &[LmCatalogProject], project_id: &str) -> Vec<(String, String)> {
    catalog
        .iter()
        .find(|project| project.project_id == project_id)
        .map(|project| project.agents.clone())
        .unwrap_or_default()
}

fn reconcile_selected_with_catalog(state: &mut LmState) {
    let Some(selected) = state.selected.as_mut() else {
        return;
    };
    let Some(project) = state
        .catalog
        .iter()
        .find(|project| project.project_id == selected.project_id)
    else {
        return;
    };
    selected.project_root = project.project_root.clone();
    selected.project_name = project.project_name.clone();
    if let Some((_, agent_name)) = project
        .agents
        .iter()
        .find(|(agent_id, _)| agent_id == &selected.agent_id)
    {
        selected.agent_name = agent_name.clone();
    }
}

fn apply_catalog_in_state(state: &mut LmState, catalog: Vec<LmCatalogProject>) {
    let catalog_changed = state.catalog != catalog;
    if catalog_changed || state.catalog_updated_at.is_none() {
        state.catalog_updated_at = Some(now_iso());
    }
    if catalog_changed {
        state.catalog = catalog;
    }
    reconcile_selected_with_catalog(state);
}

fn ensure_catalog_in_state(state: &mut LmState, list_projects: &std::sync::Arc<ProjectListFn>) {
    if state.catalog.is_empty() {
        let catalog = catalog_from_projects(list_projects());
        apply_catalog_in_state(state, catalog);
    } else {
        reconcile_selected_with_catalog(state);
    }
}

pub fn refresh_project_catalog(projects: Vec<LmProjectInfo>) {
    let catalog = catalog_from_projects(projects);
    update_state(|state| {
        apply_catalog_in_state(state, catalog);
    });
}

pub fn set_selected_muted(muted: bool) -> Result<()> {
    update_state(|state| {
        let Some(selected) = state.selected.as_mut() else {
            bail!("Laughing Man target not selected");
        };
        selected.muted = muted;
        Ok(())
    })
}

fn standby_target_from_json(
    target: &JsonValue,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        target
            .get("projectId")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
        target
            .get("projectName")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
        target
            .get("agentId")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
        target
            .get("agentName")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
    )
}

fn selected_from_standby_target(state: &LmState, target: &JsonValue) -> Result<LmSelected> {
    let (project_id, _, agent_id, _) = standby_target_from_json(target);
    let item = LmStandbyQueueItem {
        id: "target-preview".into(),
        received_at: now_iso(),
        preview: String::new(),
        project_id,
        project_name: None,
        agent_id,
        agent_name: None,
        status: "queued".into(),
        sent_at: None,
        delivery_error: None,
        update: JsonValue::Null,
    };
    let mut selected = queued_target_selection(state, &item)?;
    if state.selected.as_ref().is_some_and(|current| {
        current.project_id == selected.project_id && current.agent_id == selected.agent_id
    }) {
        selected.muted = state
            .selected
            .as_ref()
            .map(|current| current.muted)
            .unwrap_or(false);
    } else {
        selected.muted = target
            .get("muted")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
    }
    Ok(selected)
}

fn sync_selected_from_standby_response(state: &mut LmState, response: &JsonValue) {
    if !response.get("selectedTarget").is_some() {
        return;
    }
    if response
        .get("selectedTarget")
        .is_some_and(|target| target.is_null())
    {
        state.selected = None;
        return;
    }
    if let Some(target) = response.get("selectedTarget") {
        match selected_from_standby_target(state, target) {
            Ok(selected) => {
                let project_changed = state
                    .selected
                    .as_ref()
                    .map(|current| current.project_root != selected.project_root)
                    .unwrap_or(true);
                state.selected = Some(selected.clone());
                if project_changed && !OUTBOUND_REPLAY_ON_SWITCH {
                    let project_events = read_project_outbound_events(&selected, &state.catalog);
                    mark_project_stream_seen(state, &selected, &project_events);
                }
            }
            Err(err) => crate::kota_debug_log(&format!(
                "[laughing-man] standby selected target ignored: {err}"
            )),
        }
    }
}

fn remember_standby_queue_id(state: &mut LmState, id: &str) {
    if id.is_empty() || state.standby_seen_queue_ids.iter().any(|seen| seen == id) {
        return;
    }
    state.standby_seen_queue_ids.push_back(id.to_string());
    while state.standby_seen_queue_ids.len() > STANDBY_SEEN_RING_MAX {
        state.standby_seen_queue_ids.pop_front();
    }
}

fn standby_queue_seen(state: &LmState, id: &str) -> bool {
    state.standby_queue.iter().any(|existing| existing.id == id)
        || state.standby_seen_queue_ids.iter().any(|seen| seen == id)
}

fn import_standby_queue_item_with_error(
    state: &mut LmState,
    item: &JsonValue,
    delivery_error: Option<String>,
) {
    let id = item
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .trim();
    if id.is_empty() {
        return;
    }
    if standby_queue_seen(state, id) {
        return;
    }
    let target = item.get("target").cloned().unwrap_or(JsonValue::Null);
    let (project_id, project_name, agent_id, agent_name) = standby_target_from_json(&target);
    let fallback_preview = item
        .get("update")
        .and_then(|update| {
            update
                .pointer("/message/text")
                .or_else(|| update.pointer("/message/caption"))
                .and_then(JsonValue::as_str)
        })
        .map(log_preview)
        .unwrap_or_default();
    let entry = LmStandbyQueueItem {
        id: id.to_string(),
        received_at: item
            .get("receivedAt")
            .and_then(JsonValue::as_str)
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
        preview: item
            .get("preview")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_preview)
            .to_string(),
        project_id,
        project_name,
        agent_id,
        agent_name,
        status: "queued".into(),
        sent_at: None,
        delivery_error,
        update: item.get("update").cloned().unwrap_or(JsonValue::Null),
    };
    remember_standby_queue_id(state, id);
    state.standby_queue.push(entry);
    state
        .standby_queue
        .sort_by(|a, b| a.received_at.cmp(&b.received_at));
    if state.standby_queue.len() > 200 {
        let drop_count = state.standby_queue.len() - 200;
        state.standby_queue.drain(0..drop_count);
    }
}

fn import_standby_queue_item(state: &mut LmState, item: &JsonValue) {
    import_standby_queue_item_with_error(state, item, None);
}

fn standby_state_payload(state: &LmState) -> JsonValue {
    json!({
        "ownerUserId": state.owner_user_id,
        "botUsername": state.bot_username,
        "selected": state.selected.as_ref().map(|selected| json!({
            "projectId": selected.project_id,
            "projectName": selected.project_name,
            "agentId": selected.agent_id,
            "agentName": selected.agent_name,
            "muted": selected.muted,
        })),
        "catalog": state.catalog,
        "catalogUpdatedAt": state.catalog_updated_at,
    })
}

pub fn refresh_selected_agent_metadata(project_root: &Path, agent_id: &str, agent_name: &str) {
    update_state(|state| {
        refresh_selected_agent_metadata_in_state(state, project_root, agent_id, agent_name);
    });
}

pub fn update_working_agent_ids(agent_ids: Vec<String>) {
    let next = agent_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<BTreeSet<_>>();
    let mut current = working_agent_ids()
        .lock()
        .expect("lm working agent lock poisoned");
    if *current != next {
        *current = next;
    }
}

fn refresh_selected_agent_metadata_in_state(
    state: &mut LmState,
    _project_root: &Path,
    agent_id: &str,
    agent_name: &str,
) {
    let agent_id = agent_id.trim();
    let agent_name = agent_name.trim();
    if agent_id.is_empty() || agent_name.is_empty() {
        return;
    }
    if let Some(selected) = state.selected.as_mut() {
        if selected.agent_id == agent_id {
            selected.agent_name = agent_name.to_string();
        }
    }
    for (panel_agent_id, panel_agent_name) in &mut state.panel.agents {
        if panel_agent_id == agent_id {
            *panel_agent_name = agent_name.to_string();
        }
    }
    for project in &mut state.catalog {
        for (catalog_agent_id, catalog_agent_name) in &mut project.agents {
            if catalog_agent_id == agent_id {
                *catalog_agent_name = agent_name.to_string();
            }
        }
    }
}

// ─────────────────────────── telegram api ───────────────────────────

/// GUI apps launched from Finder/launchd do not inherit shell proxy env, and
/// ureq does not read the macOS system proxy on its own — so a desktop where
/// the Telegram client works fine would still time out here. Honor proxy env
/// vars first, then the system proxy (scutil). Still zero config UI.
fn telegram_proxy_url() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    macos_system_proxy()
}

#[cfg(target_os = "macos")]
fn macos_system_proxy() -> Option<String> {
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let get = |key: &str| {
        text.lines()
            .map(str::trim)
            .find(|line| line.starts_with(key) && line[key.len()..].trim_start().starts_with(':'))
            .and_then(|line| line.splitn(2, ':').nth(1))
            .map(|value| value.trim().to_string())
    };
    let enabled = |key: &str| get(key).as_deref() == Some("1");
    if enabled("HTTPSEnable") {
        if let (Some(host), Some(port)) = (get("HTTPSProxy"), get("HTTPSPort")) {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if enabled("HTTPEnable") {
        if let (Some(host), Some(port)) = (get("HTTPProxy"), get("HTTPPort")) {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if enabled("SOCKSEnable") {
        if let (Some(host), Some(port)) = (get("SOCKSProxy"), get("SOCKSPort")) {
            return Some(format!("socks5://{host}:{port}"));
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn macos_system_proxy() -> Option<String> {
    None
}

/// Fresh agent per call: cheap at our call rates, and proxy toggles take
/// effect without an app restart.
fn telegram_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new();
    if let Some(url) = telegram_proxy_url() {
        if let Ok(proxy) = ureq::Proxy::new(&url) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

fn redact_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "<token>")
}

fn api_call(
    token: &str,
    method: &str,
    payload: &JsonValue,
    timeout_secs: u64,
) -> Result<JsonValue> {
    let url = format!("{API_BASE}/bot{token}/{method}");
    let response = telegram_agent()
        .post(&url)
        .timeout(Duration::from_secs(timeout_secs))
        .send_json(payload.clone());
    let body: JsonValue = match response {
        Ok(resp) => resp
            .into_json()
            .map_err(|err| anyhow!("telegram {method}: bad json: {err}"))?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!(
                "telegram {method}: http {code}: {}",
                redact_token(&log_preview(&text), token)
            );
        }
        Err(err) => {
            // Note which proxy (if any) was in play, but no advice text —
            // a timeout on a healthy network must not steer people toward
            // proxy hunting (learned 2026-06-11).
            let hint = match telegram_proxy_url() {
                Some(url) => format!(" (via proxy {url})"),
                None => String::new(),
            };
            bail!(
                "telegram {method}: {}{hint}",
                redact_token(&err.to_string(), token)
            );
        }
    };
    if body.get("ok").and_then(JsonValue::as_bool) != Some(true) {
        bail!(
            "telegram {method}: {}",
            body.get("description")
                .and_then(JsonValue::as_str)
                .unwrap_or("not ok")
        );
    }
    Ok(body.get("result").cloned().unwrap_or(JsonValue::Null))
}

fn worker_endpoint(worker_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        worker_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn normalize_worker_url(raw: &str) -> Result<String> {
    let value = raw.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        bail!("Worker URL is required");
    }
    if !value.starts_with("https://") {
        bail!("Worker URL must use https");
    }
    let host = value
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("");
    if host.is_empty() || host.contains(char::is_whitespace) {
        bail!("Worker URL is missing a host");
    }
    Ok(value)
}

fn standby_secret(prefix: &str) -> String {
    format!(
        "{prefix}-{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn worker_get_json(worker_url: &str, path: &str, timeout_secs: u64) -> Result<JsonValue> {
    let url = worker_endpoint(worker_url, path);
    let response = telegram_agent()
        .get(&url)
        .timeout(Duration::from_secs(timeout_secs))
        .call();
    match response {
        Ok(resp) => resp
            .into_json()
            .map_err(|err| anyhow!("standby {path}: parse json: {err}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!("standby {path}: http {code}: {}", log_preview(&text))
        }
        Err(err) => bail!("standby {path}: {err}"),
    }
}

fn worker_post_json(
    worker_url: &str,
    path: &str,
    payload: &JsonValue,
    secret: Option<&str>,
    timeout_secs: u64,
) -> Result<JsonValue> {
    let url = worker_endpoint(worker_url, path);
    let mut request = telegram_agent()
        .post(&url)
        .timeout(Duration::from_secs(timeout_secs));
    if let Some(secret) = secret {
        request = request.set("X-Kota-Standby-Secret", secret);
    }
    let response = request.send_json(payload.clone());
    match response {
        Ok(resp) => resp
            .into_json()
            .map_err(|err| anyhow!("standby {path}: parse json: {err}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!("standby {path}: http {code}: {}", log_preview(&text))
        }
        Err(err) => bail!("standby {path}: {err}"),
    }
}

fn set_webhook(token: &str, worker_url: &str, webhook_secret: &str) -> Result<()> {
    let webhook_url = worker_endpoint(worker_url, "telegram");
    api_call(
        token,
        "setWebhook",
        &json!({
            "url": webhook_url,
            "secret_token": webhook_secret,
            "allowed_updates": ["message", "callback_query"],
            "drop_pending_updates": false,
        }),
        20,
    )?;
    Ok(())
}

fn delete_webhook(token: &str) -> Result<()> {
    api_call(
        token,
        "deleteWebhook",
        &json!({ "drop_pending_updates": false }),
        20,
    )?;
    Ok(())
}

pub fn get_me(token: &str) -> Result<String> {
    let result = api_call(token, "getMe", &json!({}), 15)?;
    result
        .get("username")
        .and_then(JsonValue::as_str)
        .map(|name| format!("@{name}"))
        .ok_or_else(|| anyhow!("getMe: bot username missing"))
}

fn send_text(
    token: &str,
    chat_id: i64,
    text: &str,
    reply_markup: Option<JsonValue>,
) -> Result<i64> {
    let mut payload = json!({ "chat_id": chat_id, "text": text });
    if let Some(markup) = reply_markup {
        payload["reply_markup"] = markup;
    }
    let result = api_call(token, "sendMessage", &payload, 20)?;
    Ok(result
        .get("message_id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0))
}

fn send_chat_action(token: &str, chat_id: i64, action: &str) -> Result<()> {
    api_call(
        token,
        "sendChatAction",
        &json!({ "chat_id": chat_id, "action": action }),
        10,
    )?;
    Ok(())
}

fn send_html_text(
    token: &str,
    chat_id: i64,
    html: &str,
    reply_markup: Option<JsonValue>,
) -> Result<i64> {
    let mut payload = json!({
        "chat_id": chat_id,
        "text": html,
        "parse_mode": "HTML",
    });
    if let Some(markup) = reply_markup {
        payload["reply_markup"] = markup;
    }
    let result = api_call(token, "sendMessage", &payload, 20)?;
    Ok(result
        .get("message_id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0))
}

fn send_html_with_plain_fallback(
    token: &str,
    chat_id: i64,
    html: &str,
    plain: &str,
    reply_markup: Option<JsonValue>,
) -> Result<i64> {
    match send_html_text(token, chat_id, html, reply_markup.clone()) {
        Ok(message_id) => Ok(message_id),
        Err(err) => {
            crate::kota_debug_log(&format!("[laughing-man] sendMessage HTML fallback: {err}"));
            send_text(token, chat_id, plain, reply_markup)
        }
    }
}

fn edit_text(
    token: &str,
    chat_id: i64,
    message_id: i64,
    text: &str,
    markup: JsonValue,
) -> Result<()> {
    let payload = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": text,
        "reply_markup": markup,
    });
    match api_call(token, "editMessageText", &payload, 20) {
        Ok(_) => Ok(()),
        // "message is not modified" / too old → caller falls back to resend
        Err(err) => Err(err),
    }
}

fn answer_callback(token: &str, callback_id: &str, text: Option<&str>) {
    let mut payload = json!({ "callback_query_id": callback_id });
    if let Some(text) = text {
        payload["text"] = json!(text);
    }
    let _ = api_call(token, "answerCallbackQuery", &payload, 10);
}

/// Hand-rolled multipart upload (ureq has no multipart feature).
fn send_file(
    token: &str,
    chat_id: i64,
    method: &str,
    field: &str,
    path: &Path,
    caption: Option<&str>,
) -> Result<()> {
    let bytes = fs::read(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let boundary = format!("kotalm{}", uuid::Uuid::new_v4().simple());
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 1024);
    let mut push_field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };
    push_field("chat_id", &chat_id.to_string());
    if let Some(caption) = caption {
        push_field("caption", &log_preview(caption));
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = format!("{API_BASE}/bot{token}/{method}");
    let response = telegram_agent()
        .post(&url)
        .timeout(Duration::from_secs(120))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body);
    match response {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!(
                "telegram {method}: http {code}: {}",
                redact_token(&log_preview(&text), token)
            )
        }
        Err(err) => bail!(
            "telegram {method}: {}",
            redact_token(&err.to_string(), token)
        ),
    }
}

fn download_file(token: &str, file_id: &str, dest_dir: &Path) -> Result<PathBuf> {
    let info = api_call(token, "getFile", &json!({ "file_id": file_id }), 30)?;
    let remote_path = info
        .get("file_path")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("getFile: file_path missing"))?;
    if let Some(size) = info.get("file_size").and_then(JsonValue::as_u64) {
        if size > MAX_MEDIA_BYTES {
            bail!("file larger than 20 MB");
        }
    }
    let ext = Path::new(remote_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    fs::create_dir_all(dest_dir)?;
    let dest = dest_dir.join(format!("original.{ext}"));
    let url = format!("{API_BASE}/file/bot{token}/{remote_path}");
    let response = telegram_agent()
        .get(&url)
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|err| {
            anyhow!(
                "telegram download: {}",
                redact_token(&err.to_string(), token)
            )
        })?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    std::io::Read::take(&mut reader, MAX_MEDIA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| anyhow!("telegram download read: {err}"))?;
    if bytes.len() as u64 > MAX_MEDIA_BYTES {
        bail!("file larger than 20 MB");
    }
    fs::write(&dest, &bytes)?;
    Ok(dest)
}

// ─────────────────────────── panel rendering ───────────────────────────

fn keyboard_button(text: &str, data: &str) -> JsonValue {
    json!({ "text": text, "callback_data": data })
}

fn reply_keyboard(selected: Option<&LmSelected>) -> JsonValue {
    let label = match selected {
        Some(sel) => format!("🔌 {} / {}", sel.project_name, sel.agent_name),
        None => "🔌 Not connected".to_string(),
    };
    json!({
        "keyboard": [[{ "text": label }]],
        "resize_keyboard": true,
        "is_persistent": true,
    })
}

fn is_reply_keyboard_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("🔌") || trimmed == "Switch"
}

fn send_reply_keyboard_status(token: &str, chat_id: i64, selected: &LmSelected) {
    let text = format!(
        "🟢 {} — sending to {}.",
        selected.project_name, selected.agent_name
    );
    if let Err(err) = api_call(
        token,
        "sendMessage",
        &json!({
            "chat_id": chat_id,
            "text": text,
            "reply_markup": reply_keyboard(Some(selected)),
            "disable_notification": true,
        }),
        15,
    ) {
        crate::kota_debug_log(&format!(
            "[laughing-man] reply keyboard refresh failed: {err}"
        ));
    }
}

fn project_panel(panel: &LmPanelSession) -> (String, JsonValue) {
    let text = "Kota Laughing Man\nChoose a project to connect.".to_string();
    let rows: Vec<JsonValue> = panel
        .projects
        .iter()
        .enumerate()
        .map(|(index, (_, _, name))| {
            json!([keyboard_button(
                name,
                &format!("lm:{}:p:{}", panel.revision, index)
            )])
        })
        .collect();
    (text, json!({ "inline_keyboard": rows }))
}

fn agent_panel(panel: &LmPanelSession, project_name: &str) -> (String, JsonValue) {
    let text = format!("Kota Laughing Man\nProject: {project_name}\nChoose an agent.");
    let mut rows: Vec<JsonValue> = Vec::new();
    // ≤8 agents per project (8 seats) — two columns, no pagination.
    for pair in panel.agents.chunks(2) {
        let row: Vec<JsonValue> = pair
            .iter()
            .enumerate()
            .map(|(offset, (_, name))| {
                let index = rows.len() * 2 + offset;
                keyboard_button(name, &format!("lm:{}:a:{}", panel.revision, index))
            })
            .collect();
        rows.push(JsonValue::Array(row));
    }
    rows.push(json!([keyboard_button(
        "‹ Projects",
        &format!("lm:{}:back:0", panel.revision)
    )]));
    (text, json!({ "inline_keyboard": rows }))
}

fn connected_panel(panel: &LmPanelSession, selected: &LmSelected) -> (String, JsonValue) {
    let text = format!(
        "Connected\nProject: {}\nSending to: {}\n{}Send any message to prompt this agent.",
        selected.project_name,
        selected.agent_name,
        if selected.muted {
            "Muted: desktop replies stay off your phone.\n"
        } else {
            ""
        },
    );
    let markup = json!({
        "inline_keyboard": [
            [
                keyboard_button("Switch agent", &format!("lm:{}:switch_agent:0", panel.revision)),
                keyboard_button("Switch project", &format!("lm:{}:back:0", panel.revision)),
            ]
        ]
    });
    (text, markup)
}

// ─────────────────────────── kota data access ───────────────────────────

#[derive(Clone, Debug)]
pub struct LmProjectInfo {
    pub project_id: String,
    pub project_root: String,
    pub project_name: String,
    pub agents: Vec<(String, String)>, // (agent_id, display name)
}

/// Provided by lib.rs (workspace + agent registry access stays out of this
/// module so the loop itself remains portable).
pub type ProjectListFn = dyn Fn() -> Vec<LmProjectInfo> + Send + Sync;
pub type InboundDeliverFn = dyn Fn(&str, &str, &str, &str) -> Result<(), String> + Send + Sync; // (project_root, agent_id, text, event_id)

// ─────────────────────────── manager ───────────────────────────

pub struct LaughingManManager {
    running: AtomicBool,
    /// Fresh Arc per start(): older threads hold the previous (true) flag and
    /// exit even if they wake from a long-poll after a quick disable/enable.
    stop_flag: Mutex<std::sync::Arc<AtomicBool>>,
    thread_handles: Mutex<Vec<thread::JoinHandle<()>>>,
    lock_guard: Mutex<Option<fs::File>>,
}

impl Default for LaughingManManager {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            stop_flag: Mutex::new(std::sync::Arc::new(AtomicBool::new(true))),
            thread_handles: Mutex::new(Vec::new()),
            lock_guard: Mutex::new(None),
        }
    }
}

fn spawn_lm_thread<F>(name: &str, f: F) -> Result<thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map_err(|err| anyhow!("spawn {name}: {err}"))
}

impl LaughingManManager {
    pub fn status(&self) -> LmStatus {
        let state = load_state();
        let standby = state.standby.as_ref().map(standby_status_from_config);
        LmStatus {
            configured: load_token().is_some() && state.owner_user_id.is_some(),
            enabled: state.enabled,
            running: self.running.load(Ordering::Acquire),
            bot_username: state.bot_username.clone(),
            owner_user_id: state.owner_user_id,
            pending_claim: state.pending_claim.clone(),
            selected: state.selected.clone(),
            last_error: state.last_error.clone(),
            latest: read_log(1).into_iter().next(),
            standby,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn acquire_lock(&self) -> Result<()> {
        let mut guard = self.lock_guard.lock().expect("lm lock poisoned");
        if guard.is_some() {
            return Ok(());
        }
        let path = lock_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(existing) = fs::read_to_string(&path) {
            if let Ok(pid) = existing.trim().parse::<i32>() {
                if pid > 0 && pid != std::process::id() as i32 && process_alive(pid) {
                    bail!("another Laughing Man poller is active (pid {pid})");
                }
            }
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        writeln!(file, "{}", std::process::id())?;
        *guard = Some(file);
        Ok(())
    }

    fn release_lock(&self) {
        let mut guard = self.lock_guard.lock().expect("lm lock poisoned");
        *guard = None;
        let _ = fs::remove_file(lock_path());
    }

    pub fn stop(&self) {
        self.stop_flag
            .lock()
            .expect("lm stop flag poisoned")
            .store(true, Ordering::Release);
        let handles = {
            let mut guard = self
                .thread_handles
                .lock()
                .expect("lm thread handles poisoned");
            std::mem::take(&mut *guard)
        };
        let current = thread::current().id();
        for handle in handles {
            if handle.thread().id() == current {
                continue;
            }
            let _ = handle.join();
        }
        self.running.store(false, Ordering::Release);
        self.release_lock();
    }

    pub fn start(
        &self,
        app: AppHandle,
        list_projects: std::sync::Arc<ProjectListFn>,
        deliver: std::sync::Arc<InboundDeliverFn>,
    ) -> Result<()> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let Some(token) = load_token() else {
            self.running.store(false, Ordering::Release);
            bail!("Laughing Man token not configured");
        };
        if let Err(err) = self.acquire_lock() {
            self.running.store(false, Ordering::Release);
            record_error(&format!("start: {err}"));
            return Err(err);
        }
        let fresh = std::sync::Arc::new(AtomicBool::new(false));
        *self.stop_flag.lock().expect("lm stop flag poisoned") = fresh.clone();
        let stop = fresh.clone();
        let stop_out = fresh.clone();
        let stop_catalog = fresh.clone();
        let stop_typing = fresh;
        let token_out = token.clone();
        let token_typing = token.clone();
        let app_out = app.clone();
        let app_inbound = app.clone();
        let list_projects_poll = list_projects.clone();
        let list_projects_catalog = list_projects;
        let mut handles = Vec::new();

        let inbound_handle = if load_state().standby.is_some() {
            spawn_lm_thread("lm-standby", move || {
                standby_loop(app_inbound, token, stop, list_projects_poll, deliver)
            })
        } else {
            spawn_lm_thread("lm-poll", move || {
                poll_loop(app, token, stop, list_projects_poll, deliver)
            })
        };
        match inbound_handle {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                self.running.store(false, Ordering::Release);
                self.release_lock();
                return Err(err);
            }
        }
        match spawn_lm_thread("lm-outbound", move || {
            outbound_loop(app_out, token_out, stop_out)
        }) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                self.stop_flag
                    .lock()
                    .expect("lm stop flag poisoned")
                    .store(true, Ordering::Release);
                for handle in handles {
                    let _ = handle.join();
                }
                self.running.store(false, Ordering::Release);
                self.release_lock();
                return Err(err);
            }
        }
        match spawn_lm_thread("lm-catalog", move || {
            catalog_loop(stop_catalog, list_projects_catalog)
        }) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                self.stop_flag
                    .lock()
                    .expect("lm stop flag poisoned")
                    .store(true, Ordering::Release);
                for handle in handles {
                    let _ = handle.join();
                }
                self.running.store(false, Ordering::Release);
                self.release_lock();
                return Err(err);
            }
        }
        match spawn_lm_thread("lm-typing", move || typing_loop(token_typing, stop_typing)) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                self.stop_flag
                    .lock()
                    .expect("lm stop flag poisoned")
                    .store(true, Ordering::Release);
                for handle in handles {
                    let _ = handle.join();
                }
                self.running.store(false, Ordering::Release);
                self.release_lock();
                return Err(err);
            }
        }
        *self
            .thread_handles
            .lock()
            .expect("lm thread handles poisoned") = handles;
        clear_error();
        Ok(())
    }
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    unsafe { libc_kill(pid, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    false
}

fn catalog_loop(stop: std::sync::Arc<AtomicBool>, list_projects: std::sync::Arc<ProjectListFn>) {
    crate::kota_debug_log("[laughing-man] catalog loop started");
    while !stop.load(Ordering::Acquire) {
        refresh_project_catalog(list_projects());
        for _ in 0..CATALOG_REFRESH_SECS {
            if stop.load(Ordering::Acquire) {
                crate::kota_debug_log("[laughing-man] catalog loop stopped");
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
    crate::kota_debug_log("[laughing-man] catalog loop stopped");
}

fn selected_project_has_working_agent(
    state: &LmState,
    selected: &LmSelected,
    working_agent_ids: &BTreeSet<String>,
) -> bool {
    if working_agent_ids.is_empty() {
        return false;
    }
    let project_agents = project_agent_names(&state.catalog, selected);
    project_agents
        .keys()
        .any(|agent_id| working_agent_ids.contains(agent_id))
}

fn typing_loop(token: String, stop: std::sync::Arc<AtomicBool>) {
    crate::kota_debug_log("[laughing-man] typing loop started");
    while !stop.load(Ordering::Acquire) {
        let state = load_state();
        if !state.enabled {
            break;
        }
        if let (Some(selected), Some(chat_id)) = (state.selected.as_ref(), state.owner_chat_id) {
            let has_working = {
                let ids = working_agent_ids()
                    .lock()
                    .expect("lm working agent lock poisoned");
                selected_project_has_working_agent(&state, selected, &ids)
            };
            if has_working {
                let _ = send_chat_action(&token, chat_id, "typing");
            }
        }
        for _ in 0..TYPING_ACTION_SECS {
            if stop.load(Ordering::Acquire) {
                crate::kota_debug_log("[laughing-man] typing loop stopped");
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
    crate::kota_debug_log("[laughing-man] typing loop stopped");
}

fn standby_ack(config: &LmStandbyConfig, ids: &[String], status: &str) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    worker_post_json(
        &config.worker_url,
        "desktop/ack",
        &json!({ "ids": ids, "status": status }),
        Some(&config.desktop_secret),
        15,
    )
    .map(|_| ())
}

fn standby_offline_item_is_owner_message(state: &LmState, item: &JsonValue) -> bool {
    let Some(message) = item.get("update").and_then(|update| update.get("message")) else {
        return false;
    };
    let (user_id, _, _, _, private) = message_sender(message);
    private && state.owner_user_id == Some(user_id)
}

fn message_text_or_caption(message: &JsonValue) -> &str {
    message
        .get("text")
        .or_else(|| message.get("caption"))
        .and_then(JsonValue::as_str)
        .unwrap_or("")
}

fn message_is_control_text(message: &JsonValue) -> bool {
    let text = message_text_or_caption(message).trim();
    text == "/start" || is_reply_keyboard_text(text)
}

fn standby_offline_item_is_control_message(item: &JsonValue) -> bool {
    item.get("update")
        .and_then(|update| update.get("message"))
        .map(message_is_control_text)
        .unwrap_or(false)
}

fn mark_standby_item_recipient_missing(state: &mut LmState, item: &JsonValue) {
    import_standby_queue_item_with_error(state, item, Some("Recipient Missing".into()));
}

fn mark_standby_item_delivery_failed(state: &mut LmState, item: &JsonValue) {
    import_standby_queue_item_with_error(state, item, Some("Delivery Failed".into()));
}

enum StandbyOnlineOutcome {
    Handled,
    RecipientMissing,
    DeliveryFailed,
}

fn deliver_standby_online_message(
    token: &str,
    item: &JsonValue,
    deliver: &std::sync::Arc<InboundDeliverFn>,
) -> StandbyOnlineOutcome {
    let state = load_state();
    let message = match item.get("update").and_then(|update| update.get("message")) {
        Some(message) => message,
        None => return StandbyOnlineOutcome::Handled,
    };
    let (user_id, chat_id, _, _, private) = message_sender(message);
    if !private || state.owner_user_id != Some(user_id) {
        return StandbyOnlineOutcome::Handled;
    }
    if message_is_control_text(message) {
        return StandbyOnlineOutcome::Handled;
    }
    let local_item = {
        let target = item.get("target").cloned().unwrap_or(JsonValue::Null);
        let (project_id, project_name, agent_id, agent_name) = standby_target_from_json(&target);
        LmStandbyQueueItem {
            id: item
                .get("id")
                .and_then(JsonValue::as_str)
                .unwrap_or("standby-online")
                .to_string(),
            received_at: item
                .get("receivedAt")
                .and_then(JsonValue::as_str)
                .unwrap_or("1970-01-01T00:00:00Z")
                .to_string(),
            preview: item
                .get("preview")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string(),
            project_id,
            project_name,
            agent_id,
            agent_name,
            status: "queued".into(),
            sent_at: None,
            delivery_error: None,
            update: item.get("update").cloned().unwrap_or(JsonValue::Null),
        }
    };
    let selected = match queued_target_selection(&state, &local_item) {
        Ok(selected) => selected,
        Err(err) => {
            crate::kota_debug_log(&format!("[laughing-man] standby recipient missing: {err}"));
            return StandbyOnlineOutcome::RecipientMissing;
        }
    };
    update_state(|state| {
        let mut next = selected.clone();
        if state.selected.as_ref().is_some_and(|current| {
            current.project_id == next.project_id && current.agent_id == next.agent_id
        }) {
            next.muted = state
                .selected
                .as_ref()
                .map(|current| current.muted)
                .unwrap_or(false);
        }
        state.selected = Some(next);
    });
    let (body, media_count) = match body_from_queued_message(token, &selected, message) {
        Ok(body) => body,
        Err(err) => {
            crate::kota_debug_log(&format!(
                "[laughing-man] standby message body skipped: {err}"
            ));
            return StandbyOnlineOutcome::Handled;
        }
    };
    let event_id = standby_delivery_event_id(&local_item);
    let mut delivered = false;
    let mut last_err = String::new();
    for (attempt, delay) in std::iter::once(&0u64)
        .chain(BUS_RETRY_DELAYS_SECS.iter())
        .enumerate()
    {
        if *delay > 0 {
            thread::sleep(Duration::from_secs(*delay));
        }
        match deliver(&selected.project_root, &selected.agent_id, &body, &event_id) {
            Ok(()) => {
                delivered = true;
                break;
            }
            Err(err) => {
                last_err = err;
                if attempt == 0 && chat_id != 0 {
                    let _ = send_text(token, chat_id, "Waking the agent…", None);
                }
            }
        }
    }
    if !delivered {
        crate::kota_debug_log(&format!(
            "[laughing-man] standby online delivery failed: {}",
            log_preview(&last_err)
        ));
        return StandbyOnlineOutcome::DeliveryFailed;
    }
    append_log(&LmLogEntry {
        schema: LOG_SCHEMA.into(),
        id: mint_log_id("in"),
        ts: now_iso(),
        direction: "in".into(),
        project_id: selected.project_id,
        project_name: selected.project_name,
        agent_id: selected.agent_id,
        agent_name: selected.agent_name,
        preview: log_preview(&body),
        media_count,
        offline_recorded_at: None,
    });
    StandbyOnlineOutcome::Handled
}

fn standby_loop(
    app: AppHandle,
    token: String,
    stop: std::sync::Arc<AtomicBool>,
    list_projects: std::sync::Arc<ProjectListFn>,
    deliver: std::sync::Arc<InboundDeliverFn>,
) {
    crate::kota_debug_log("[laughing-man] standby loop started");
    while !stop.load(Ordering::Acquire) {
        let state = load_state();
        if !state.enabled {
            break;
        }
        let Some(config) = state.standby.clone() else {
            break;
        };
        let payload = json!({ "state": standby_state_payload(&state) });
        match worker_post_json(
            &config.worker_url,
            "desktop/heartbeat",
            &payload,
            Some(&config.desktop_secret),
            15,
        ) {
            Ok(response) => {
                let relay_version = response
                    .get("relayVersion")
                    .and_then(JsonValue::as_str)
                    .map(ToOwned::to_owned);
                let protocol_version = response
                    .get("protocolVersion")
                    .and_then(JsonValue::as_str)
                    .map(ToOwned::to_owned);
                let queue_count = response
                    .get("queueCount")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0) as usize;
                update_state(|state| {
                    sync_selected_from_standby_response(state, &response);
                    if let Some(standby) = state.standby.as_mut() {
                        standby.live = true;
                        standby.last_heartbeat_at = Some(now_iso());
                        standby.last_error = None;
                        standby.queue_count = queue_count;
                        if relay_version.is_some() {
                            standby.relay_version = relay_version.clone();
                        }
                        if protocol_version.is_some() {
                            standby.protocol_version = protocol_version.clone();
                        }
                    }
                });
            }
            Err(err) => {
                let preview = log_preview(&err.to_string());
                update_state(|state| {
                    if let Some(standby) = state.standby.as_mut() {
                        standby.live = false;
                        standby.last_error = Some(preview.clone());
                    }
                });
                crate::kota_debug_log(&format!("[laughing-man] standby heartbeat: {err}"));
                sleep_standby_interval(&stop);
                continue;
            }
        }

        pull_standby_updates(&app, &token, &config, &list_projects, &deliver);
        sleep_standby_interval(&stop);
    }
    crate::kota_debug_log("[laughing-man] standby loop stopped");
}

fn sleep_standby_interval(stop: &std::sync::Arc<AtomicBool>) {
    for _ in 0..STANDBY_LOOP_SECS {
        if stop.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn pull_standby_updates(
    app: &AppHandle,
    token: &str,
    config: &LmStandbyConfig,
    list_projects: &std::sync::Arc<ProjectListFn>,
    deliver: &std::sync::Arc<InboundDeliverFn>,
) {
    let response = match worker_post_json(
        &config.worker_url,
        "desktop/pull",
        &json!({ "limit": STANDBY_PULL_LIMIT }),
        Some(&config.desktop_secret),
        20,
    ) {
        Ok(response) => response,
        Err(err) => {
            let preview = log_preview(&err.to_string());
            update_state(|state| {
                if let Some(standby) = state.standby.as_mut() {
                    standby.live = false;
                    standby.last_error = Some(preview.clone());
                }
            });
            crate::kota_debug_log(&format!("[laughing-man] standby pull: {err}"));
            return;
        }
    };
    let queue_count = response
        .get("queueCount")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0) as usize;
    let mut ack_sent = Vec::new();
    let mut ack_discarded = Vec::new();
    let mut imported = Vec::new();
    if let Some(items) = response.get("items").and_then(JsonValue::as_array) {
        for item in items {
            let id = item
                .get("id")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let offline = item
                .get("offline")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            if offline {
                let state = load_state();
                if !standby_offline_item_is_owner_message(&state, item) {
                    ack_discarded.push(id);
                } else if standby_offline_item_is_control_message(item) {
                    ack_discarded.push(id);
                } else if standby_queue_seen(&state, &id) {
                    ack_sent.push(id);
                } else {
                    imported.push(item.clone());
                    ack_sent.push(id);
                }
                continue;
            }
            let update = item.get("update").cloned().unwrap_or(JsonValue::Null);
            let update_id = update
                .get("update_id")
                .and_then(JsonValue::as_i64)
                .unwrap_or(0);
            if load_state().processed_update_ids.contains(&update_id) {
                ack_sent.push(id);
                continue;
            }
            if update.get("message").is_some() {
                match deliver_standby_online_message(token, item, deliver) {
                    StandbyOnlineOutcome::Handled => {
                        remember_processed_update(update_id);
                        ack_sent.push(id);
                    }
                    StandbyOnlineOutcome::RecipientMissing => {
                        update_state(|state| {
                            mark_standby_item_recipient_missing(state, item);
                        });
                        remember_processed_update(update_id);
                        ack_sent.push(id);
                    }
                    StandbyOnlineOutcome::DeliveryFailed => {
                        update_state(|state| {
                            mark_standby_item_delivery_failed(state, item);
                        });
                        remember_processed_update(update_id);
                        ack_sent.push(id);
                    }
                }
                continue;
            }
            if handle_update(app, token, &update, list_projects, deliver) == UpdateOutcome::Handled
            {
                remember_processed_update(update_id);
                ack_sent.push(id);
            }
        }
    }
    if !imported.is_empty() {
        update_state(|state| {
            for item in &imported {
                import_standby_queue_item(state, item);
            }
        });
    }
    if let Err(err) = standby_ack(config, &ack_sent, "sent") {
        crate::kota_debug_log(&format!("[laughing-man] standby ack: {err}"));
    }
    if let Err(err) = standby_ack(config, &ack_discarded, "discarded") {
        crate::kota_debug_log(&format!("[laughing-man] standby discard ack: {err}"));
    }
    update_state(|state| {
        if let Some(standby) = state.standby.as_mut() {
            standby.live = true;
            standby.last_sync_at = Some(now_iso());
            standby.last_error = None;
            standby.queue_count = queue_count;
        }
    });
}

fn remember_processed_update(update_id: i64) {
    if update_id <= 0 {
        return;
    }
    update_state(|state| {
        if !state.processed_update_ids.contains(&update_id) {
            state.processed_update_ids.push_back(update_id);
            while state.processed_update_ids.len() > 100 {
                state.processed_update_ids.pop_front();
            }
        }
    });
}

fn record_error(message: &str) {
    let preview = log_preview(message);
    update_state(|state| {
        state.last_error = Some(preview);
        state.transient_poll_failure_since = None;
        state.last_transient_poll_error = None;
    });
    crate::kota_debug_log(&format!("[laughing-man] {message}"));
}

fn clear_error() {
    let mut state = load_state();
    if state.last_error.is_none()
        && state.transient_poll_failure_since.is_none()
        && state.last_transient_poll_error.is_none()
    {
        return;
    }
    state.last_error = None;
    state.transient_poll_failure_since = None;
    state.last_transient_poll_error = None;
    let _ = save_state(&state);
}

fn record_poll_error(message: &str) {
    if !is_transient_poll_error(message) {
        record_error(message);
        return;
    }
    crate::kota_debug_log(&format!("[laughing-man] {message}"));
    let now = now_unix();
    let preview = log_preview(message);
    update_state(|state| {
        let failure_since = state.transient_poll_failure_since.get_or_insert(now);
        state.last_transient_poll_error = Some(preview.clone());
        if now.saturating_sub(*failure_since) >= TRANSIENT_POLL_ERROR_UI_DELAY_SECS {
            state.last_error = Some(preview);
        } else if state
            .last_error
            .as_deref()
            .is_some_and(is_poll_get_updates_error)
        {
            state.last_error = None;
        }
    });
}

fn is_poll_get_updates_error(message: &str) -> bool {
    message.contains("poll: telegram getUpdates:")
}

fn is_transient_poll_error(message: &str) -> bool {
    if !is_poll_get_updates_error(message) {
        return false;
    }
    let lower = message.to_ascii_lowercase();
    lower.contains("network error")
        || lower.contains("dns failed")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("connection aborted")
        || lower.contains("failed to lookup")
}

// ─────────────────────────── poll loop (inbound) ───────────────────────────

fn poll_loop(
    app: AppHandle,
    token: String,
    stop: std::sync::Arc<AtomicBool>,
    list_projects: std::sync::Arc<ProjectListFn>,
    deliver: std::sync::Arc<InboundDeliverFn>,
) {
    crate::kota_debug_log("[laughing-man] poll loop started");
    let mut backlog_mode = true;
    while !stop.load(Ordering::Acquire) {
        let state = load_state();
        if !state.enabled {
            break;
        }
        let payload = json!({
            "offset": state.offset,
            "timeout": POLL_TIMEOUT_SECS,
            "allowed_updates": ["message", "callback_query"],
        });
        let updates = match api_call(&token, "getUpdates", &payload, POLL_TIMEOUT_SECS + 15) {
            Ok(JsonValue::Array(updates)) => updates,
            Ok(_) => Vec::new(),
            Err(err) => {
                record_poll_error(&format!("poll: {err}"));
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        clear_error();
        if updates.is_empty() {
            backlog_mode = false;
            continue;
        }
        'updates: for update in updates {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let update_id = update
                .get("update_id")
                .and_then(JsonValue::as_i64)
                .unwrap_or(0);
            let already = load_state().processed_update_ids.contains(&update_id);
            let outcome = if already {
                UpdateOutcome::Handled
            } else {
                handle_update(&app, &token, &update, &list_projects, &deliver)
            };
            match outcome {
                UpdateOutcome::Handled => {
                    // Advance offset only once the update is fully handled —
                    // enqueue succeeded or it was an intentional control/drop
                    // (handoff §2 + 颦儿 P0-3). One atomic state write.
                    update_state(|state| {
                        if update_id >= state.offset {
                            state.offset = update_id + 1;
                        }
                        state.delivery_attempts.remove(&update_id);
                        if !state.processed_update_ids.contains(&update_id) {
                            state.processed_update_ids.push_back(update_id);
                            while state.processed_update_ids.len() > 100 {
                                state.processed_update_ids.pop_front();
                            }
                        }
                    });
                }
                UpdateOutcome::DeliverFailed => {
                    // Do not advance: Telegram re-delivers this update on the
                    // next poll, which is our retry loop. Give up (and move
                    // on) after 3 attempts so one dead agent can't dam the
                    // queue forever.
                    let attempts = update_state(|state| {
                        let count = state.delivery_attempts.entry(update_id).or_insert(0);
                        *count += 1;
                        *count
                    });
                    if attempts >= 3 {
                        update_state(|state| {
                            if update_id >= state.offset {
                                state.offset = update_id + 1;
                            }
                            state.delivery_attempts.remove(&update_id);
                        });
                        if let Some(chat_id) = load_state().owner_chat_id {
                            let _ = send_text(
                                &token,
                                chat_id,
                                "Gave up delivering that message after 3 tries — please resend later.",
                                None,
                            );
                        }
                    } else {
                        // Re-poll the same update after a pause.
                        thread::sleep(Duration::from_secs(3));
                        continue 'updates;
                    }
                }
            }
            if backlog_mode {
                thread::sleep(Duration::from_millis(BACKLOG_DRAIN_GAP_MS));
            }
        }
    }
    crate::kota_debug_log("[laughing-man] poll loop stopped");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateOutcome {
    /// Fully handled (delivered, control input, or intentional drop).
    Handled,
    /// Bus delivery failed — eligible for Telegram-driven retry.
    DeliverFailed,
}

fn handle_update(
    app: &AppHandle,
    token: &str,
    update: &JsonValue,
    list_projects: &std::sync::Arc<ProjectListFn>,
    deliver: &std::sync::Arc<InboundDeliverFn>,
) -> UpdateOutcome {
    let update_id = update
        .get("update_id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    if let Some(message) = update.get("message") {
        handle_message(app, token, update_id, message, list_projects, deliver)
    } else if let Some(callback) = update.get("callback_query") {
        handle_callback(token, callback, list_projects);
        UpdateOutcome::Handled
    } else {
        UpdateOutcome::Handled
    }
}

fn message_sender(message: &JsonValue) -> (i64, i64, Option<String>, Option<String>, bool) {
    let user = message.get("from").cloned().unwrap_or(JsonValue::Null);
    let user_id = user.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
    let chat = message.get("chat").cloned().unwrap_or(JsonValue::Null);
    let chat_id = chat.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
    let username = user
        .get("username")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned);
    let first_name = user
        .get("first_name")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned);
    let private = chat.get("type").and_then(JsonValue::as_str) == Some("private");
    (user_id, chat_id, username, first_name, private)
}

fn handle_message(
    app: &AppHandle,
    token: &str,
    update_id: i64,
    message: &JsonValue,
    list_projects: &std::sync::Arc<ProjectListFn>,
    deliver: &std::sync::Arc<InboundDeliverFn>,
) -> UpdateOutcome {
    let (user_id, chat_id, username, first_name, private) = message_sender(message);
    if !private || user_id == 0 {
        return UpdateOutcome::Handled; // groups rejected silently (handoff §4)
    }
    let owner = load_state().owner_user_id;
    match owner {
        None => {
            // Cold start: surface the sender for one-click claim in settings.
            update_state(|state| {
                state.pending_claim = Some(LmPendingClaim {
                    user_id,
                    username: username.clone(),
                    first_name: first_name.clone(),
                });
            });
            let _ = send_text(
                token,
                chat_id,
                "Kota: ownership not claimed yet. Finish setup on the desktop (Claim as Owner), then talk to me.",
                None,
            );
            return UpdateOutcome::Handled;
        }
        Some(owner) if owner != user_id => {
            // Strangers: silent drop + local log (anti-probing).
            crate::kota_debug_log(&format!(
                "[laughing-man] dropped message from non-owner uid {user_id}"
            ));
            return UpdateOutcome::Handled;
        }
        _ => {}
    }
    update_state(|state| {
        if state.owner_chat_id != Some(chat_id) {
            state.owner_chat_id = Some(chat_id);
        }
    });

    let text = message_text_or_caption(message).to_string();

    // Reply-keyboard buttons & /start: control inputs, never delivered.
    if message_is_control_text(message) {
        refresh_panel(token, chat_id, list_projects, true);
        return UpdateOutcome::Handled;
    }

    let Some(selected) = load_state().selected.clone() else {
        refresh_panel(token, chat_id, list_projects, true);
        return UpdateOutcome::Handled;
    };

    // Media → existing composer attachment layout, paths inlined into prompt.
    let mut media_paths: Vec<String> = Vec::new();
    let mut media_error: Option<String> = None;
    let attachment_root = PathBuf::from(&selected.project_root)
        .join("project-memory")
        .join("attachments")
        .join("composer");
    let mut file_ids: Vec<String> = Vec::new();
    if let Some(photos) = message.get("photo").and_then(JsonValue::as_array) {
        if let Some(best) = photos.last() {
            if let Some(id) = best.get("file_id").and_then(JsonValue::as_str) {
                file_ids.push(id.to_string());
            }
        }
    }
    if let Some(doc) = message.get("document") {
        if let Some(id) = doc.get("file_id").and_then(JsonValue::as_str) {
            file_ids.push(id.to_string());
        }
    }
    for file_id in file_ids {
        let dir = attachment_root.join(format!(
            "att_{}_lm",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        match download_file(token, &file_id, &dir) {
            Ok(path) => media_paths.push(path.to_string_lossy().to_string()),
            Err(err) => media_error = Some(err.to_string()),
        }
    }
    if let Some(err) = media_error {
        let _ = send_text(token, chat_id, &format!("Attachment skipped: {err}"), None);
    }

    let mut body = text.trim().to_string();
    if !media_paths.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&media_paths.join("\n"));
    }
    if body.is_empty() {
        return UpdateOutcome::Handled;
    }

    // Phone activity auto-unmutes (decision log) — under the state lock.
    if selected.muted {
        update_state(|state| {
            if let Some(sel) = state.selected.as_mut() {
                sel.muted = false;
            }
        });
    }

    // Stable per-update id: Telegram redelivers unacked updates, and the bus
    // dedupe key + processed ring fold those into one delivery (颦儿 P0-3).
    let event_id = format!("lm-update-{update_id}");
    // Bridge-side bounded backoff (handoff §2) — generic bus grace untouched.
    let mut delivered = false;
    let mut last_err = String::new();
    for (attempt, delay) in std::iter::once(&0u64)
        .chain(BUS_RETRY_DELAYS_SECS.iter())
        .enumerate()
    {
        if *delay > 0 {
            thread::sleep(Duration::from_secs(*delay));
        }
        match deliver(&selected.project_root, &selected.agent_id, &body, &event_id) {
            Ok(()) => {
                delivered = true;
                break;
            }
            Err(err) => {
                last_err = err;
                if attempt == 0 {
                    let _ = send_text(token, chat_id, "Waking the agent…", None);
                }
            }
        }
    }
    let _ = app; // reserved for future room-side notification hooks

    if delivered {
        append_log(&LmLogEntry {
            schema: LOG_SCHEMA.into(),
            id: mint_log_id("in"),
            ts: now_iso(),
            direction: "in".into(),
            project_id: selected.project_id.clone(),
            project_name: selected.project_name.clone(),
            agent_id: selected.agent_id.clone(),
            agent_name: selected.agent_name.clone(),
            preview: log_preview(&body),
            media_count: media_paths.len(),
            offline_recorded_at: None,
        });
        UpdateOutcome::Handled
    } else {
        let _ = send_text(
            token,
            chat_id,
            &format!(
                "Could not reach {} ({}). Retrying…",
                selected.agent_name,
                log_preview(&last_err)
            ),
            None,
        );
        UpdateOutcome::DeliverFailed
    }
}

fn refresh_panel(
    token: &str,
    chat_id: i64,
    list_projects: &std::sync::Arc<ProjectListFn>,
    force_send: bool,
) {
    let mut state = load_state(); // working copy; persisted under lock at the end
    ensure_catalog_in_state(&mut state, list_projects);
    state.panel.revision = state.panel.revision.wrapping_add(1);
    state.panel.projects = catalog_project_rows(&state.catalog);
    state.panel.agents.clear();
    state.panel.agents_project = None;

    let (text, markup) = match state.selected.clone() {
        Some(selected) => connected_panel(&state.panel, &selected),
        None => project_panel(&state.panel),
    };
    let keyboard = reply_keyboard(state.selected.as_ref());

    let edited = if let (Some(message_id), false) = (state.panel.message_id, force_send) {
        edit_text(token, chat_id, message_id, &text, markup.clone()).is_ok()
    } else {
        false
    };
    if !edited {
        // Panel resend doubles as the reply-keyboard carrier: no extra "·"
        // junk messages in the chat stream (颦儿 P1). Keyboard labels update
        // lazily on the next real bot message.
        let _ = keyboard;
        if let Ok(message_id) = send_text(token, chat_id, &text, Some(markup)) {
            state.panel.message_id = Some(message_id);
        }
    }
    let final_panel = state.panel.clone();
    let final_selected = state.selected.clone();
    let final_catalog = state.catalog.clone();
    let final_catalog_updated_at = state.catalog_updated_at.clone();
    update_state(|persisted| {
        persisted.panel = final_panel;
        persisted.selected = final_selected;
        persisted.catalog = final_catalog;
        persisted.catalog_updated_at = final_catalog_updated_at;
    });
}

fn handle_callback(
    token: &str,
    callback: &JsonValue,
    list_projects: &std::sync::Arc<ProjectListFn>,
) {
    let callback_id = callback.get("id").and_then(JsonValue::as_str).unwrap_or("");
    let data = callback
        .get("data")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let from_id = callback
        .pointer("/from/id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    let chat_id = callback
        .pointer("/message/chat/id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    let message_id = callback
        .pointer("/message/message_id")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);

    let mut state = load_state();
    if state.owner_user_id != Some(from_id) {
        answer_callback(token, callback_id, None);
        return;
    }
    // data: lm:<revision>:<action>:<idx>
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() != 4 || parts[0] != "lm" {
        answer_callback(token, callback_id, None);
        return;
    }
    let revision: u64 = parts[1].parse().unwrap_or(0);
    if revision != state.panel.revision {
        answer_callback(token, callback_id, Some("Panel refreshed"));
        refresh_panel(token, chat_id, list_projects, false);
        return;
    }
    let action = parts[2];
    let index: usize = parts[3].parse().unwrap_or(0);

    match action {
        "p" => {
            ensure_catalog_in_state(&mut state, list_projects);
            let Some((project_id, project_root, project_name)) =
                state.panel.projects.get(index).cloned()
            else {
                answer_callback(token, callback_id, Some("Panel refreshed"));
                refresh_panel(token, chat_id, list_projects, false);
                return;
            };
            let agents = catalog_agents(&state.catalog, &project_id);
            state.panel.agents = agents;
            state.panel.agents_project = Some(index);
            let (text, markup) = agent_panel(&state.panel, &project_name);
            let _ = edit_text(token, chat_id, message_id, &text, markup);
            {
                let panel = state.panel.clone();
                let selected = state.selected.clone();
                let catalog = state.catalog.clone();
                let catalog_updated_at = state.catalog_updated_at.clone();
                update_state(|persisted| {
                    persisted.panel = panel;
                    persisted.selected = selected;
                    persisted.catalog = catalog;
                    persisted.catalog_updated_at = catalog_updated_at;
                });
            }
            let _ = project_root; // selection completes at agent step
            answer_callback(token, callback_id, None);
        }
        "a" => {
            let previous_project_root = state
                .selected
                .as_ref()
                .map(|selected| selected.project_root.clone());
            let project = state
                .panel
                .agents_project
                .and_then(|project_index| state.panel.projects.get(project_index).cloned());
            let agent = state.panel.agents.get(index).cloned();
            let (Some((project_id, project_root, project_name)), Some((agent_id, agent_name))) =
                (project, agent)
            else {
                answer_callback(token, callback_id, Some("Panel refreshed"));
                refresh_panel(token, chat_id, list_projects, false);
                return;
            };
            state.selected = Some(LmSelected {
                project_id,
                project_root,
                project_name,
                agent_id,
                agent_name,
                muted: false,
            });
            let selected = state.selected.clone().expect("just set");
            let project_changed =
                previous_project_root.as_deref() != Some(selected.project_root.as_str());
            let project_events = if project_changed {
                read_project_outbound_events(&selected, &state.catalog)
            } else {
                Vec::new()
            };
            let replay_events = if OUTBOUND_REPLAY_ON_SWITCH {
                latest_outbound_events(&project_events, OUTBOUND_REPLAY_LIMIT)
            } else {
                Vec::new()
            };
            let (text, markup) = connected_panel(&state.panel, &selected);
            let _ = edit_text(token, chat_id, message_id, &text, markup);
            {
                let panel = state.panel.clone();
                let selected_for_state = selected.clone();
                let project_events = project_events.clone();
                update_state(|persisted| {
                    persisted.panel = panel;
                    persisted.selected = Some(selected_for_state.clone());
                    if project_changed {
                        mark_project_stream_seen(persisted, &selected_for_state, &project_events);
                    }
                });
            }
            answer_callback(token, callback_id, Some("Connected"));
            // Single quiet confirmation that doubles as the reply-keyboard
            // carrier — the only final-status message a selection produces.
            send_reply_keyboard_status(token, chat_id, &selected);
            if OUTBOUND_REPLAY_ON_SWITCH && project_changed && !selected.muted {
                for event in replay_events {
                    push_reply(token, chat_id, &selected, &event.agent_name, &event.text);
                    append_outbound_log(&selected, &event);
                }
            }
        }
        "back" => {
            ensure_catalog_in_state(&mut state, list_projects);
            state.panel.projects = catalog_project_rows(&state.catalog);
            state.panel.agents.clear();
            state.panel.agents_project = None;
            let (text, markup) = project_panel(&state.panel);
            let _ = edit_text(token, chat_id, message_id, &text, markup);
            {
                let panel = state.panel.clone();
                let selected = state.selected.clone();
                let catalog = state.catalog.clone();
                let catalog_updated_at = state.catalog_updated_at.clone();
                update_state(|persisted| {
                    persisted.panel = panel;
                    persisted.selected = selected;
                    persisted.catalog = catalog;
                    persisted.catalog_updated_at = catalog_updated_at;
                });
            }
            answer_callback(token, callback_id, None);
        }
        "switch_agent" => {
            if let Some(selected) = state.selected.clone() {
                ensure_catalog_in_state(&mut state, list_projects);
                if state.panel.projects.is_empty() {
                    state.panel.projects = catalog_project_rows(&state.catalog);
                }
                let mut project_index = state
                    .panel
                    .projects
                    .iter()
                    .position(|(id, _, _)| *id == selected.project_id);
                if project_index.is_none() {
                    state.panel.projects = catalog_project_rows(&state.catalog);
                    project_index = state
                        .panel
                        .projects
                        .iter()
                        .position(|(id, _, _)| *id == selected.project_id);
                }
                if let Some(project_index) = project_index {
                    state.panel.agents = catalog_agents(&state.catalog, &selected.project_id);
                    state.panel.agents_project = Some(project_index);
                    let (text, markup) = agent_panel(&state.panel, &selected.project_name);
                    let _ = edit_text(token, chat_id, message_id, &text, markup);
                    {
                        let panel = state.panel.clone();
                        let selected = state.selected.clone();
                        let catalog = state.catalog.clone();
                        let catalog_updated_at = state.catalog_updated_at.clone();
                        update_state(|persisted| {
                            persisted.panel = panel;
                            persisted.selected = selected;
                            persisted.catalog = catalog;
                            persisted.catalog_updated_at = catalog_updated_at;
                        });
                    }
                }
            }
            answer_callback(token, callback_id, None);
        }
        "mute" => {
            if let Some(sel) = state.selected.as_mut() {
                sel.muted = !sel.muted;
            }
            let selected = state.selected.clone();
            if let Some(selected) = selected {
                let (text, markup) = connected_panel(&state.panel, &selected);
                let _ = edit_text(token, chat_id, message_id, &text, markup);
                answer_callback(
                    token,
                    callback_id,
                    Some(if selected.muted { "Muted" } else { "Unmuted" }),
                );
            }
            {
                let panel = state.panel.clone();
                let selected = state.selected.clone();
                update_state(|persisted| {
                    persisted.panel = panel;
                    persisted.selected = selected;
                });
            }
        }
        "disconnect" => {
            state.selected = None;
            let (text, markup) = project_panel(&state.panel);
            let _ = edit_text(token, chat_id, message_id, &text, markup);
            {
                let panel = state.panel.clone();
                let selected = state.selected.clone();
                update_state(|persisted| {
                    persisted.panel = panel;
                    persisted.selected = selected;
                });
            }
            answer_callback(token, callback_id, Some("Disconnected"));
            let _ = api_call(
                token,
                "sendMessage",
                &json!({
                    "chat_id": chat_id,
                    "text": "Disconnected.",
                    "reply_markup": { "remove_keyboard": true },
                    "disable_notification": true,
                }),
                15,
            );
        }
        _ => answer_callback(token, callback_id, None),
    }
}

// ─────────────────────────── outbound loop ───────────────────────────

#[derive(Clone, Debug)]
struct LmOutboundEvent {
    id: String,
    agent_id: String,
    agent_name: String,
    text: String,
}

fn project_agent_names(
    catalog: &[LmCatalogProject],
    selected: &LmSelected,
) -> BTreeMap<String, String> {
    let mut names: BTreeMap<String, String> = catalog
        .iter()
        .find(|project| project.project_id == selected.project_id)
        .map(|project| project.agents.iter().cloned().collect())
        .unwrap_or_default();
    if names.is_empty() {
        names.insert(selected.agent_id.clone(), selected.agent_name.clone());
    }
    names
}

fn outbound_event_from_json(
    event: JsonValue,
    agent_names: &BTreeMap<String, String>,
) -> Option<LmOutboundEvent> {
    let str_field = |key: &str| event.get(key).and_then(JsonValue::as_str).unwrap_or("");
    if str_field("role") != "assistant" || str_field("kind") != "message" {
        return None;
    }
    if event.get("display").and_then(JsonValue::as_bool) != Some(true) {
        return None;
    }
    let agent_id = str_field("agent_id");
    let agent_name = agent_names.get(agent_id)?;
    let text = event
        .get("text")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return None;
    }
    if outbound_text_is_internal_wrapper(&text) {
        return None;
    }
    Some(LmOutboundEvent {
        id: event.get("id").and_then(JsonValue::as_str)?.to_string(),
        agent_id: agent_id.to_string(),
        agent_name: agent_name.clone(),
        text,
    })
}

fn outbound_text_is_internal_wrapper(text: &str) -> bool {
    let trimmed = text.trim_matches(|ch: char| ch.is_whitespace() || ch.is_control());
    trimmed.starts_with("<KOTA_DREAM_ENTRY") && trimmed.ends_with("</KOTA_DREAM_ENTRY>")
}

fn read_project_outbound_events(
    selected: &LmSelected,
    catalog: &[LmCatalogProject],
) -> Vec<LmOutboundEvent> {
    let history_path = PathBuf::from(&selected.project_root)
        .join("project-memory")
        .join("chathistory")
        .join("latest.jsonl");
    let Ok(content) = fs::read_to_string(&history_path) else {
        return Vec::new();
    };
    let agent_names = project_agent_names(catalog, selected);
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<JsonValue>(line).ok())
        .filter_map(|event| outbound_event_from_json(event, &agent_names))
        .collect()
}

fn latest_outbound_events(events: &[LmOutboundEvent], limit: usize) -> Vec<LmOutboundEvent> {
    let start = events.len().saturating_sub(limit);
    events[start..].to_vec()
}

fn outbound_events_after_anchor(
    events: &[LmOutboundEvent],
    anchor: &str,
) -> Option<Vec<LmOutboundEvent>> {
    let index = events.iter().position(|event| event.id == anchor)?;
    Some(events[index + 1..].to_vec())
}

fn outbound_events_after_cursor(
    events: &[LmOutboundEvent],
    anchor: &str,
) -> Option<Vec<LmOutboundEvent>> {
    if anchor.is_empty() {
        return Some(latest_outbound_events(events, OUTBOUND_REPLAY_LIMIT));
    }
    outbound_events_after_anchor(events, anchor)
}

fn remember_outbound_events_seen(state: &mut LmState, events: &[LmOutboundEvent]) {
    for event in events {
        remember_pushed_event(state, event.id.clone());
    }
}

fn mark_project_stream_seen(
    state: &mut LmState,
    selected: &LmSelected,
    events: &[LmOutboundEvent],
) {
    remember_outbound_events_seen(state, events);
    state.seeded_projects.insert(selected.project_root.clone());
    let event_id = events
        .last()
        .map(|event| event.id.clone())
        .unwrap_or_default();
    state
        .outbound_last_seen_event_ids
        .insert(selected.project_root.clone(), event_id);
}

fn append_outbound_log(selected: &LmSelected, event: &LmOutboundEvent) {
    append_log(&LmLogEntry {
        schema: LOG_SCHEMA.into(),
        id: mint_log_id("out"),
        ts: now_iso(),
        direction: "out".into(),
        project_id: selected.project_id.clone(),
        project_name: selected.project_name.clone(),
        agent_id: event.agent_id.clone(),
        agent_name: event.agent_name.clone(),
        preview: log_preview(&event.text),
        media_count: 0,
        offline_recorded_at: None,
    });
}

/// Push scope = current selected project stream: assistant `message` events
/// that are display-visible and authored by active agents in the project.
/// Reads the materialized chathistory incrementally — zero Violet internals
/// touched. The selected agent remains only the Telegram input target.
fn outbound_loop(_app: AppHandle, token: String, stop: std::sync::Arc<AtomicBool>) {
    crate::kota_debug_log("[laughing-man] outbound loop started");
    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_secs(2));
        let state = load_state();
        if !state.enabled {
            break;
        }
        let (Some(selected), Some(chat_id)) = (state.selected.clone(), state.owner_chat_id) else {
            continue;
        };
        // latest.jsonl is a rolling projection (Violet rewrites the whole
        // file: dedupe + 500-line cap). Byte offsets are not a valid cursor
        // (颦儿 P0-2) — scan all ≤500 lines and dedupe by event id instead.
        let matching = read_project_outbound_events(&selected, &state.catalog);

        let anchor = state
            .outbound_last_seen_event_ids
            .get(&selected.project_root)
            .cloned();
        let pending = match anchor {
            None => {
                // First attach to this project, or first run after upgrading to
                // the project stream cursor: mark the rolling projection as
                // seen. Project switches perform their explicit two-message
                // replay in the callback path, not here.
                update_state(|persisted| {
                    mark_project_stream_seen(persisted, &selected, &matching);
                });
                continue;
            }
            Some(anchor) => {
                match outbound_events_after_cursor(&matching, &anchor) {
                    Some(events) => events,
                    None => {
                        // The rolling projection no longer contains the
                        // anchor. Fail closed to current tail instead of
                        // replaying up to 500 old events.
                        update_state(|persisted| {
                            mark_project_stream_seen(persisted, &selected, &matching);
                        });
                        continue;
                    }
                }
            }
        };

        for event in pending {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let fresh = load_state();
            if fresh.pushed_event_ids.contains(&event.id) {
                continue;
            }
            if fresh.selected.as_ref().map(|sel| sel.muted).unwrap_or(true) {
                // Muted (or deselected mid-scan): mark seen without pushing —
                // mute means "keep desktop traffic off my phone", not "queue
                // it for later".
                update_state(|persisted| {
                    remember_pushed_event(persisted, event.id.clone());
                    persisted
                        .outbound_last_seen_event_ids
                        .insert(selected.project_root.clone(), event.id.clone());
                });
                continue;
            }
            push_reply(&token, chat_id, &selected, &event.agent_name, &event.text);
            update_state(|persisted| {
                remember_pushed_event(persisted, event.id.clone());
                persisted
                    .outbound_last_seen_event_ids
                    .insert(selected.project_root.clone(), event.id.clone());
            });
            append_outbound_log(&selected, &event);
        }
    }
    crate::kota_debug_log("[laughing-man] outbound loop stopped");
}

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Conservative path detection, mirroring the BBS rule: only paths that the
/// agent explicitly wrote into its reply are ever uploaded.
fn detect_media_paths(text: &str, allowed_roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut images = Vec::new();
    let mut docs = Vec::new();
    let allowed_roots: Vec<PathBuf> = allowed_roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();

    for raw in text.split_whitespace() {
        let cleaned = clean_path_token(raw);
        let lower = cleaned.to_ascii_lowercase();
        let Some(ext) = lower.rsplit('.').next() else {
            continue;
        };
        if !IMAGE_EXTS.contains(&ext) {
            continue;
        }
        let Some(path) = allowed_media_path(cleaned, &allowed_roots) else {
            continue;
        };
        if images.len() < 10 && !images.contains(&path) {
            images.push(path);
        }
    }

    for raw in text.lines() {
        let cleaned = clean_path_token(raw.trim());
        let lower = cleaned.to_ascii_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or("");
        if IMAGE_EXTS.contains(&ext) {
            continue;
        }
        let Some(path) = allowed_media_path(cleaned, &allowed_roots) else {
            continue;
        };
        if docs.len() < 4 && !docs.contains(&path) {
            docs.push(path);
        }
    }
    (images, docs)
}

fn clean_path_token(raw: &str) -> &str {
    raw.trim_matches(|c: char| "()[]{}\"'`,;:。，".contains(c))
}

fn allowed_media_path(cleaned: &str, allowed_roots: &[PathBuf]) -> Option<PathBuf> {
    if !cleaned.starts_with('/') {
        return None;
    }
    let path = PathBuf::from(cleaned);
    let canonical = path.canonicalize().ok()?;
    if !allowed_roots.iter().any(|root| canonical.starts_with(root)) {
        return None;
    }
    let metadata = fs::metadata(&canonical).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MEDIA_BYTES {
        return None;
    }
    Some(path)
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            '\n' => escaped.push('\n'),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn telegram_label_plain(label: &str, suffix: &str) -> String {
    format!("[✨{label}{suffix}✨]")
}

fn telegram_label_html(label: &str, suffix: &str) -> String {
    format!("[✨<b>{}</b>✨]", html_escape(&format!("{label}{suffix}")))
}

fn split_table_row(line: &str) -> Vec<String> {
    let mut trimmed = line.trim();
    if trimmed.starts_with('|') {
        trimmed = &trimmed[1..];
    }
    if trimmed.ends_with('|') {
        trimmed = &trimmed[..trimmed.len().saturating_sub(1)];
    }
    trimmed
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_table_separator(line: &str) -> bool {
    if !line.contains('|') || line.contains("\\|") {
        return false;
    }
    let cells = split_table_row(line);
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let trimmed = cell.trim();
            trimmed.contains('-')
                && trimmed
                    .chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        })
}

fn table_block_is_simple(block: &[&str]) -> Option<Vec<Vec<String>>> {
    if block.len() < 3 || block.iter().any(|line| line.contains("\\|")) {
        return None;
    }
    let header = split_table_row(block[0]);
    let column_count = header.len();
    let row_count = block.len().saturating_sub(2);
    if !(2..=5).contains(&column_count) || row_count == 0 || row_count > 12 {
        return None;
    }
    if header
        .iter()
        .any(|cell| cell.is_empty() || cell.chars().count() > 80)
    {
        return None;
    }
    let mut rows = Vec::new();
    for line in &block[2..] {
        let row = split_table_row(line);
        if row.len() != column_count || row.iter().any(|cell| cell.chars().count() > 160) {
            return None;
        }
        rows.push(row);
    }
    Some(std::iter::once(header).chain(rows).collect())
}

fn render_simple_table_as_mobile_notes(block: &[&str]) -> Option<String> {
    let rows = table_block_is_simple(block)?;
    let headers = rows.first()?;
    let mut groups = Vec::new();
    for (index, row) in rows.iter().skip(1).enumerate() {
        let title = row
            .first()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Row {}", index + 1));
        let mut lines = vec![title];
        for (header, value) in headers.iter().skip(1).zip(row.iter().skip(1)) {
            if value.trim().is_empty() {
                continue;
            }
            lines.push(format!("• {header}: {value}"));
        }
        groups.push(lines.join("\n"));
    }
    Some(groups.join("\n\n"))
}

fn rewrite_simple_gfm_tables(text: &str) -> String {
    if !text.contains('|') || !text.contains('-') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(line.to_string());
            i += 1;
            continue;
        }
        if !in_fence
            && line.contains('|')
            && i + 1 < lines.len()
            && is_table_separator(lines[i + 1])
        {
            let mut end = i + 2;
            while end < lines.len() && !lines[end].trim().is_empty() && lines[end].contains('|') {
                end += 1;
            }
            if let Some(rendered) = render_simple_table_as_mobile_notes(&lines[i..end]) {
                out.push(rendered);
                i = end;
                continue;
            }
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

fn is_safe_telegram_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !url.chars().any(|ch| ch.is_control() || "<>\"".contains(ch))
}

fn push_escaped_char(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(ch),
    }
}

fn next_char_at(text: &str, index: usize) -> Option<(char, usize)> {
    text.get(index..)?
        .chars()
        .next()
        .map(|ch| (ch, index + ch.len_utf8()))
}

fn format_lm_inline_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(after_tick) = rest.strip_prefix('`') {
            if let Some(end) = after_tick.find('`') {
                let code = &after_tick[..end];
                if !code.is_empty() {
                    out.push_str("<code>");
                    out.push_str(&html_escape(code));
                    out.push_str("</code>");
                    i += end + 2;
                    continue;
                }
            }
        }
        if let Some(after_bold) = rest.strip_prefix("**") {
            if let Some(end) = after_bold.find("**") {
                let bold = &after_bold[..end];
                if !bold.trim().is_empty() {
                    out.push_str("<b>");
                    out.push_str(&html_escape(bold));
                    out.push_str("</b>");
                    i += end + 4;
                    continue;
                }
            }
        }
        if rest.starts_with('[') {
            if let Some(close_label) = rest.find("](") {
                if let Some(close_url) = rest[close_label + 2..].find(')') {
                    let label = &rest[1..close_label];
                    let url = &rest[close_label + 2..close_label + 2 + close_url];
                    if !label.is_empty() && is_safe_telegram_link(url) {
                        out.push_str("<a href=\"");
                        out.push_str(&html_escape(url));
                        out.push_str("\">");
                        out.push_str(&format_lm_inline_html(label));
                        out.push_str("</a>");
                        i += close_label + 3 + close_url;
                        continue;
                    }
                }
            }
        }
        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                let italic = &rest[1..1 + end];
                if !italic.trim().is_empty()
                    && !italic.contains('*')
                    && italic.chars().count() <= 120
                    && !italic.contains('_')
                {
                    out.push_str("<i>");
                    out.push_str(&html_escape(italic));
                    out.push_str("</i>");
                    i += end + 2;
                    continue;
                }
            }
        }
        if let Some((ch, next)) = next_char_at(text, i) {
            push_escaped_char(&mut out, ch);
            i = next;
        } else {
            break;
        }
    }
    out
}

fn parse_heading_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let marker_count = trimmed.chars().take_while(|ch| *ch == '#').count();
    if marker_count == 0 || marker_count > 6 {
        return None;
    }
    let rest = trimmed.get(marker_count..)?;
    if !rest.starts_with(' ') {
        return None;
    }
    let heading = rest.trim();
    (!heading.is_empty()).then_some(heading)
}

fn parse_list_line(line: &str) -> Option<(usize, String, &str)> {
    let leading_spaces = line.chars().take_while(|ch| *ch == ' ').count();
    let trimmed = line.trim_start();
    let depth = (leading_spaces / 2).min(3);
    for marker in ["- ", "* ", "+ "] {
        if let Some(content) = trimmed.strip_prefix(marker) {
            if let Some(task) = content.strip_prefix("[ ] ") {
                return Some((depth, "☐ ".into(), task));
            }
            if let Some(task) = content
                .strip_prefix("[x] ")
                .or_else(|| content.strip_prefix("[X] "))
            {
                return Some((depth, "☑ ".into(), task));
            }
            return Some((depth, "• ".into(), content));
        }
    }
    let marker_end = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if marker_end > 0 {
        let marker = &trimmed[..marker_end];
        let rest = &trimmed[marker_end..];
        if let Some(content) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((depth, format!("{marker}. "), content));
        }
    }
    None
}

fn format_lm_telegram_html(text: &str) -> String {
    let rewritten = rewrite_simple_gfm_tables(text);
    let mut out = String::with_capacity(rewritten.len());
    let mut in_code = false;
    for line in rewritten.lines() {
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("```") {
            if in_code {
                out.push_str("</pre>\n");
                in_code = false;
            } else {
                out.push_str("<pre>");
                in_code = true;
            }
            continue;
        }
        if in_code {
            out.push_str(&html_escape(line));
            out.push('\n');
            continue;
        }
        if let Some(heading) = parse_heading_line(line) {
            out.push_str("<b>");
            out.push_str(&html_escape(heading));
            out.push_str("</b>\n");
            continue;
        }
        if let Some((depth, marker, content)) = parse_list_line(line) {
            out.push_str(&"  ".repeat(depth));
            out.push_str(&marker);
            out.push_str(&format_lm_inline_html(content));
            out.push('\n');
            continue;
        }
        out.push_str(&format_lm_inline_html(line));
        out.push('\n');
    }
    if in_code {
        out.push_str("</pre>\n");
    }
    out.trim_end_matches('\n').to_string()
}

fn byte_index_after_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn split_telegram_text(text: &str, max_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut rest = trimmed;
    while rest.chars().count() > max_chars {
        let hard_split = byte_index_after_chars(rest, max_chars);
        let candidate = &rest[..hard_split];
        let min_soft_split = byte_index_after_chars(rest, max_chars / 2);
        let soft_split = ["\n\n", "\n", ". ", " "]
            .iter()
            .filter_map(|pattern| candidate.rfind(pattern).map(|index| (index, pattern.len())))
            .filter(|(index, _)| *index >= min_soft_split)
            .map(|(index, len)| index + len)
            .max()
            .unwrap_or(hard_split);
        let chunk = rest[..soft_split].trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        rest = rest[soft_split..].trim_start();
    }
    if !rest.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

fn send_labeled_text_chunks(
    token: &str,
    chat_id: i64,
    label: &str,
    text: &str,
    reply_markup: Option<JsonValue>,
) -> Result<()> {
    let chunks = split_telegram_text(text, MAX_TELEGRAM_TEXT_CHARS);
    if chunks.is_empty() {
        return Ok(());
    }
    let total = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        let suffix = if total > 1 {
            format!(" {}/{}", index + 1, total)
        } else {
            String::new()
        };
        let plain_header = telegram_label_plain(label, &suffix);
        let plain = format!("{plain_header}\n{chunk}");
        let rich = format!(
            "{}\n{}",
            telegram_label_html(label, &suffix),
            format_lm_telegram_html(chunk)
        );
        let chunk_markup = if index + 1 == total {
            reply_markup.clone()
        } else {
            None
        };
        send_html_with_plain_fallback(token, chat_id, &rich, &plain, chunk_markup)?;
    }
    Ok(())
}

fn push_reply(token: &str, chat_id: i64, selected: &LmSelected, speaker_name: &str, text: &str) {
    let keyboard = reply_keyboard(Some(selected));
    let _ = send_labeled_text_chunks(token, chat_id, speaker_name, text, Some(keyboard));
    let allowed_roots = [
        PathBuf::from(&selected.project_root),
        home_dir().join("Kota"),
    ];
    let (images, docs) = detect_media_paths(text, &allowed_roots);
    for image in images {
        if let Err(err) = send_file(token, chat_id, "sendPhoto", "photo", &image, None) {
            // Oversized/unsupported photos fall back to document delivery.
            if send_file(token, chat_id, "sendDocument", "document", &image, None).is_err() {
                crate::kota_debug_log(&format!("[laughing-man] photo push failed: {err}"));
            }
        }
    }
    for doc in docs {
        let _ = send_file(token, chat_id, "sendDocument", "document", &doc, None);
    }
}

pub fn send_ember_reminder(request: LmEmberReminderRequest) -> Result<()> {
    let _guard = EMBER_REMINDER_LOCK
        .lock()
        .map_err(|_| anyhow!("Laughing Man Ember reminder lock poisoned"))?;
    let event_id = request.event_id.trim();
    if event_id.is_empty() {
        bail!("Laughing Man reminder requires an event id");
    }
    let prompt = request.text.trim();
    if prompt.is_empty() {
        bail!("Laughing Man reminder requires text");
    }
    let dedupe_id = format!("ember-reminder:{event_id}");
    if load_state().pushed_event_ids.contains(&dedupe_id) {
        return Ok(());
    }

    let token = load_token().ok_or_else(|| anyhow!("Laughing Man token is not configured"))?;
    let state = load_state();
    if state.owner_user_id.is_none() {
        bail!("Laughing Man owner is not claimed");
    }
    let chat_id = state
        .owner_chat_id
        .ok_or_else(|| anyhow!("Laughing Man owner chat is not available"))?;

    let body = format!("Ember Timed Message: {prompt}");
    send_labeled_text_chunks(&token, chat_id, "Ember", &body, None)?;

    update_state(|persisted| {
        remember_pushed_event(persisted, dedupe_id);
    });
    let project_id = request
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("current")
        .to_string();
    let project_name = request
        .project_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&project_id)
        .to_string();
    append_log(&LmLogEntry {
        schema: LOG_SCHEMA.into(),
        id: mint_log_id("out"),
        ts: now_iso(),
        direction: "out".into(),
        project_id,
        project_name,
        agent_id: "ember".into(),
        agent_name: "Ember".into(),
        preview: log_preview(&body),
        media_count: 0,
        offline_recorded_at: None,
    });
    Ok(())
}

pub(crate) fn ember_reminder_route_configured() -> bool {
    let state = load_state();
    load_token().is_some() && state.owner_user_id.is_some()
}

fn message_from_update(update: &JsonValue) -> Result<&JsonValue> {
    update
        .get("message")
        .ok_or_else(|| anyhow!("queued Standby item is not a Telegram message"))
}

fn body_from_queued_message(
    token: &str,
    selected: &LmSelected,
    message: &JsonValue,
) -> Result<(String, usize)> {
    let text = message_text_or_caption(message).trim().to_string();
    let attachment_root = PathBuf::from(&selected.project_root)
        .join("project-memory")
        .join("attachments")
        .join("composer");
    let mut file_ids: Vec<String> = Vec::new();
    if let Some(photos) = message.get("photo").and_then(JsonValue::as_array) {
        if let Some(best) = photos.last() {
            if let Some(id) = best.get("file_id").and_then(JsonValue::as_str) {
                file_ids.push(id.to_string());
            }
        }
    }
    if let Some(doc) = message.get("document") {
        if let Some(id) = doc.get("file_id").and_then(JsonValue::as_str) {
            file_ids.push(id.to_string());
        }
    }
    let mut media_paths = Vec::new();
    for file_id in file_ids {
        let dir = attachment_root.join(format!(
            "att_{}_lm",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        media_paths.push(
            download_file(token, &file_id, &dir)?
                .to_string_lossy()
                .to_string(),
        );
    }
    let mut body = text;
    if !media_paths.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&media_paths.join("\n"));
    }
    if body.trim().is_empty() {
        bail!("queued Standby message is empty");
    }
    Ok((body, media_paths.len()))
}

fn queued_target_selection(state: &LmState, item: &LmStandbyQueueItem) -> Result<LmSelected> {
    let project_id = item
        .project_id
        .as_deref()
        .ok_or_else(|| anyhow!("queued Standby message has no project snapshot"))?;
    let agent_id = item
        .agent_id
        .as_deref()
        .ok_or_else(|| anyhow!("queued Standby message has no agent snapshot"))?;
    let project = state
        .catalog
        .iter()
        .find(|project| project.project_id == project_id)
        .ok_or_else(|| anyhow!("project target is no longer available"))?;
    let agent_name = project
        .agents
        .iter()
        .find(|(candidate, _)| candidate == agent_id)
        .map(|(_, name)| name.clone())
        .ok_or_else(|| anyhow!("agent target is no longer available in this project"))?;
    Ok(LmSelected {
        project_id: project_id.to_string(),
        project_root: project.project_root.clone(),
        project_name: project.project_name.clone(),
        agent_id: agent_id.to_string(),
        agent_name,
        muted: false,
    })
}

pub fn read_standby_queue(limit: usize) -> Vec<LmStandbyQueueItem> {
    let mut items = load_state().standby_queue;
    items.retain(|item| item.status == "queued");
    if items.len() > limit {
        let drop_count = items.len() - limit;
        items.drain(0..drop_count);
    }
    items
}

fn standby_delivery_event_id(item: &LmStandbyQueueItem) -> String {
    item.update
        .get("update_id")
        .and_then(JsonValue::as_i64)
        .filter(|id| *id > 0)
        .map(|id| format!("lm-update-{id}"))
        .unwrap_or_else(|| format!("lm-standby-{}", item.id))
}

fn mark_standby_queued_discarded(id: &str) -> Result<LmStandbyQueueItem> {
    let mut item = remove_standby_queued(id)?;
    item.status = "discarded".into();
    item.sent_at = Some(now_iso());
    Ok(item)
}

fn remove_standby_queued(id: &str) -> Result<LmStandbyQueueItem> {
    update_state(|state| {
        let index = state
            .standby_queue
            .iter()
            .position(|entry| entry.id == id)?;
        Some(state.standby_queue.remove(index))
    })
    .ok_or_else(|| anyhow!("queued Standby message disappeared"))
}

pub fn delete_standby_queued(request: LmStandbyDeleteRequest) -> Result<LmStandbyQueueItem> {
    mark_standby_queued_discarded(&request.id)
}

pub fn send_standby_queued(
    request: LmStandbySendRequest,
    deliver: std::sync::Arc<InboundDeliverFn>,
) -> Result<LmStandbyQueueItem> {
    let token = load_token().ok_or_else(|| anyhow!("Laughing Man token is not configured"))?;
    let state = load_state();
    let item = state
        .standby_queue
        .iter()
        .find(|item| item.id == request.id)
        .cloned()
        .ok_or_else(|| anyhow!("queued Standby message not found"))?;
    if item.status == "sent" {
        return Ok(item);
    }
    let message = message_from_update(&item.update)?;
    let (user_id, _, _, _, private) = message_sender(message);
    if !private || state.owner_user_id != Some(user_id) {
        bail!("queued Standby message is not from the claimed owner");
    }
    if message_is_control_text(message) {
        return mark_standby_queued_discarded(&request.id);
    }
    let selected = match queued_target_selection(&state, &item) {
        Ok(selected) => selected,
        Err(err) => {
            update_state(|state| {
                if let Some(entry) = state
                    .standby_queue
                    .iter_mut()
                    .find(|entry| entry.id == request.id)
                {
                    entry.delivery_error = Some("Recipient Missing".into());
                }
            });
            crate::kota_debug_log(&format!("[laughing-man] queued recipient missing: {err}"));
            bail!("Recipient Missing");
        }
    };
    update_state(|state| {
        let mut next = selected.clone();
        if state.selected.as_ref().is_some_and(|current| {
            current.project_id == next.project_id && current.agent_id == next.agent_id
        }) {
            next.muted = state
                .selected
                .as_ref()
                .map(|current| current.muted)
                .unwrap_or(false);
        }
        state.selected = Some(next);
    });
    let (body, media_count) = body_from_queued_message(&token, &selected, message)?;
    let event_id = standby_delivery_event_id(&item);
    deliver(&selected.project_root, &selected.agent_id, &body, &event_id)
        .map_err(|err| anyhow!(err))?;

    let sent_at = now_iso();
    let mut updated = remove_standby_queued(&request.id)?;
    updated.status = "sent".into();
    updated.sent_at = Some(sent_at.clone());
    updated.project_id = Some(selected.project_id.clone());
    updated.project_name = Some(selected.project_name.clone());
    updated.agent_id = Some(selected.agent_id.clone());
    updated.agent_name = Some(selected.agent_name.clone());
    append_log(&LmLogEntry {
        schema: LOG_SCHEMA.into(),
        id: mint_log_id("in"),
        ts: sent_at,
        direction: "in".into(),
        project_id: selected.project_id,
        project_name: selected.project_name,
        agent_id: selected.agent_id,
        agent_name: selected.agent_name,
        preview: log_preview(&body),
        media_count,
        offline_recorded_at: Some(item.received_at),
    });
    Ok(updated)
}

// ─────────────────────────── config IPC helpers ───────────────────────────

/// Normalize pasted tokens: strip whitespace/newlines anywhere, drop an
/// accidental "bot" prefix, then sanity-check the `digits:secret` shape so
/// format mistakes fail fast locally instead of as a Telegram 404.
fn normalize_token(raw: &str) -> Result<String> {
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact
        .strip_prefix("bot")
        .filter(|rest| rest.contains(':'))
        .unwrap_or(&compact)
        .to_string();
    let valid = compact.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.chars().all(|c| c.is_ascii_digit())
            && secret.len() >= 30
            && secret
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    if !valid {
        bail!(
            "that doesn't look like a bot token (expected digits:secret, ~46 chars) — copy only the token line from @BotFather"
        );
    }
    Ok(compact)
}

pub fn configure_token(token: &str) -> Result<String> {
    let token = normalize_token(token)?;
    let username = get_me(&token)?;
    save_token(&token)?;
    update_state(|state| {
        state.bot_username = Some(username.clone());
        // Token present == bridge runs (no separate enabled switch).
        state.enabled = true;
        state.last_error = None;
        state.transient_poll_failure_since = None;
        state.last_transient_poll_error = None;
    });
    Ok(username)
}

pub fn claim_owner() -> Result<LmState> {
    let mut state = load_state();
    let Some(claim) = state.pending_claim.clone() else {
        bail!("no pending Telegram user to claim — message the bot first");
    };
    state.owner_user_id = Some(claim.user_id);
    state.pending_claim = None;
    save_state(&state)?;
    Ok(state)
}

pub fn connect_standby(request: LmStandbyConnectRequest) -> Result<LmState> {
    let token = load_token().ok_or_else(|| anyhow!("Laughing Man token is not configured"))?;
    let worker_url = normalize_worker_url(&request.worker_url)?;
    let mut state = load_state();
    let owner_user_id = state
        .owner_user_id
        .ok_or_else(|| anyhow!("claim the Telegram owner before setting up 24/7 Standby"))?;
    let health = worker_get_json(&worker_url, "health", 15)?;
    let health_protocol = health
        .get("protocolVersion")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    if health_protocol != STANDBY_PROTOCOL {
        bail!(
            "Worker protocol mismatch: expected {STANDBY_PROTOCOL}, got {}",
            if health_protocol.is_empty() {
                "unknown"
            } else {
                health_protocol
            }
        );
    }
    if health.get("paired").and_then(JsonValue::as_bool) == Some(true) {
        bail!("this Worker is already paired; deploy a fresh Worker or clear its Durable Object storage");
    }
    let device_id = format!("kota-desktop-{}", uuid::Uuid::new_v4().simple());
    let desktop_secret = standby_secret("desktop");
    let webhook_secret = standby_secret("telegram");
    let paired_at = now_iso();
    let pair = worker_post_json(
        &worker_url,
        "pair",
        &json!({
            "deviceId": device_id,
            "desktopSecret": desktop_secret,
            "webhookSecret": webhook_secret,
            "ownerUserId": owner_user_id,
            "state": standby_state_payload(&state),
        }),
        None,
        20,
    )?;
    let relay_version = pair
        .get("relayVersion")
        .and_then(JsonValue::as_str)
        .or_else(|| health.get("relayVersion").and_then(JsonValue::as_str))
        .map(ToOwned::to_owned);
    let protocol_version = pair
        .get("protocolVersion")
        .and_then(JsonValue::as_str)
        .or_else(|| health.get("protocolVersion").and_then(JsonValue::as_str))
        .map(ToOwned::to_owned);
    let config = LmStandbyConfig {
        worker_url,
        device_id,
        desktop_secret,
        webhook_secret,
        paired_at,
        relay_version,
        protocol_version,
        live: true,
        last_heartbeat_at: Some(now_iso()),
        last_sync_at: None,
        last_error: None,
        queue_count: health
            .get("queueCount")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize,
    };
    if let Err(err) = set_webhook(&token, &config.worker_url, &config.webhook_secret) {
        let _ = worker_post_json(
            &config.worker_url,
            "desktop/disconnect",
            &json!({}),
            Some(&config.desktop_secret),
            15,
        );
        return Err(err);
    }
    state.standby = Some(config.clone());
    sync_selected_from_standby_response(&mut state, &pair);
    state.last_error = None;
    state.transient_poll_failure_since = None;
    state.last_transient_poll_error = None;
    if let Err(err) = save_state(&state) {
        let _ = delete_webhook(&token);
        let _ = worker_post_json(
            &config.worker_url,
            "desktop/disconnect",
            &json!({}),
            Some(&config.desktop_secret),
            15,
        );
        return Err(err);
    }
    Ok(state)
}

pub fn disconnect_standby() -> Result<LmState> {
    if let Some(config) = load_state().standby {
        let _ = worker_post_json(
            &config.worker_url,
            "desktop/disconnect",
            &json!({}),
            Some(&config.desktop_secret),
            15,
        );
    }
    if let Some(token) = load_token() {
        let _ = delete_webhook(&token);
    }
    let mut state = load_state();
    state.standby = None;
    state.standby_queue.clear();
    state.last_error = None;
    state.transient_poll_failure_since = None;
    state.last_transient_poll_error = None;
    save_state(&state)?;
    Ok(state)
}

pub fn set_enabled(enabled: bool) -> Result<LmState> {
    let mut state = load_state();
    state.enabled = enabled;
    if !enabled {
        state.last_error = None;
        state.transient_poll_failure_since = None;
        state.last_transient_poll_error = None;
    }
    save_state(&state)?;
    Ok(state)
}

/// Remove the bot token and stop the bridge. The owner uid is KEPT — it
/// identifies who may use the bridge, independent of which bot carries it
/// (2026-06-11 mock decision), so swapping bots needs no re-claim.
pub fn revoke() -> Result<()> {
    if let Some(config) = load_state().standby {
        let _ = worker_post_json(
            &config.worker_url,
            "desktop/disconnect",
            &json!({}),
            Some(&config.desktop_secret),
            15,
        );
    }
    let token = load_token();
    if let Some(token) = token.as_deref() {
        if load_state().standby.is_some() {
            let _ = delete_webhook(token);
        }
    }
    if let (Some(token), Some(chat_id)) = (token.as_deref(), load_state().owner_chat_id) {
        let _ = api_call(
            token,
            "sendMessage",
            &json!({
                "chat_id": chat_id,
                "text": "Laughing Man disabled from desktop.",
                "reply_markup": { "remove_keyboard": true },
            }),
            10,
        );
    }
    delete_token();
    update_state(|state| {
        state.enabled = false;
        state.bot_username = None;
        state.selected = None;
        state.standby = None;
        state.standby_queue.clear();
        state.pending_claim = None;
        state.last_error = None;
        state.transient_poll_failure_since = None;
        state.last_transient_poll_error = None;
    });
    Ok(())
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_matches_project_stream_semantics() {
        let agent_names = BTreeMap::from([
            ("agent-1".to_string(), "A1".to_string()),
            ("agent-2".to_string(), "A2".to_string()),
        ]);
        let own = json!({"id":"event-1","role":"assistant","kind":"message","display":true,"agent_id":"agent-1","text":"hello"});
        assert_eq!(
            outbound_event_from_json(own, &agent_names)
                .unwrap()
                .agent_name,
            "A1"
        );
        let other = json!({"id":"event-2","role":"assistant","kind":"message","display":true,"agent_id":"agent-2","text":"world"});
        assert_eq!(
            outbound_event_from_json(other, &agent_names)
                .unwrap()
                .agent_name,
            "A2"
        );
        // Targeted system/actor messages must NOT push (颦儿 P0-1: this is
        // exactly the Laughing Man inbound echo shape).
        let targeted = json!({
            "role":"assistant","kind":"message","display":true,
            "agent_id":"laughing-man","target_agent_ids":["agent-1"]
        });
        assert!(outbound_event_from_json(targeted, &agent_names).is_none());
        let commentary =
            json!({"role":"assistant","kind":"commentary","display":true,"agent_id":"agent-1"});
        assert!(outbound_event_from_json(commentary, &agent_names).is_none());
        let hidden =
            json!({"role":"assistant","kind":"message","display":false,"agent_id":"agent-1"});
        assert!(outbound_event_from_json(hidden, &agent_names).is_none());
        let user = json!({"role":"user","kind":"message","display":true,"agent_id":"agent-1"});
        assert!(outbound_event_from_json(user, &agent_names).is_none());
        let blank = json!({"id":"event-3","role":"assistant","kind":"message","display":true,"agent_id":"agent-1","text":" \n\t"});
        assert!(outbound_event_from_json(blank, &agent_names).is_none());
        let dream = json!({"id":"event-4","role":"assistant","kind":"message","display":true,"agent_id":"agent-1","text":"<KOTA_DREAM_ENTRY>\n- memory\n</KOTA_DREAM_ENTRY>\u{15}"});
        assert!(outbound_event_from_json(dream, &agent_names).is_none());
        let raw_bus = json!({"id":"event-5","role":"assistant","kind":"message","display":true,"agent_id":"agent-1","text":"\n<KOTA_MESSAGE from=\"x\" to=\"y\">\nbody\n</KOTA_MESSAGE>"});
        assert!(outbound_event_from_json(raw_bus, &agent_names).is_some());
        let discussion = json!({"id":"event-6","role":"assistant","kind":"message","display":true,"agent_id":"agent-1","text":"We should filter <KOTA_DREAM_ENTRY> wrappers."});
        assert!(outbound_event_from_json(discussion, &agent_names).is_some());
    }

    #[test]
    fn latest_outbound_events_returns_last_two_in_order() {
        let events: Vec<LmOutboundEvent> = (1..=4)
            .map(|index| LmOutboundEvent {
                id: format!("event-{index}"),
                agent_id: "agent-1".into(),
                agent_name: "A1".into(),
                text: format!("text {index}"),
            })
            .collect();
        let tail = latest_outbound_events(&events, 2);
        assert_eq!(
            tail.iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-3", "event-4"]
        );
        let empty_anchor = outbound_events_after_cursor(&events, "").unwrap();
        assert_eq!(
            empty_anchor
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-3", "event-4"]
        );
        let after = outbound_events_after_anchor(&events, "event-2").unwrap();
        assert_eq!(
            after
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-3", "event-4"]
        );
        assert!(outbound_events_after_anchor(&events, "missing").is_none());
    }

    #[test]
    fn stream_seen_cursor_records_empty_project() {
        let selected = LmSelected {
            project_id: "p".into(),
            project_root: "/tmp/p".into(),
            project_name: "P".into(),
            agent_id: "agent-1".into(),
            agent_name: "A1".into(),
            muted: false,
        };
        let mut state = LmState::default();
        mark_project_stream_seen(&mut state, &selected, &[]);
        assert_eq!(
            state
                .outbound_last_seen_event_ids
                .get("/tmp/p")
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn typing_status_uses_selected_project_working_agents() {
        let selected = LmSelected {
            project_id: "p".into(),
            project_root: "/tmp/p".into(),
            project_name: "P".into(),
            agent_id: "agent-1".into(),
            agent_name: "A1".into(),
            muted: false,
        };
        let state = LmState {
            selected: Some(selected.clone()),
            catalog: vec![LmCatalogProject {
                project_id: "p".into(),
                project_root: "/tmp/p".into(),
                project_name: "P".into(),
                agents: vec![
                    ("agent-1".into(), "A1".into()),
                    ("agent-2".into(), "A2".into()),
                ],
            }],
            ..LmState::default()
        };
        let working = BTreeSet::from(["agent-2".to_string()]);
        assert!(selected_project_has_working_agent(
            &state, &selected, &working
        ));
        let working = BTreeSet::from(["agent-other-project".to_string()]);
        assert!(!selected_project_has_working_agent(
            &state, &selected, &working
        ));
    }

    #[test]
    fn token_normalization_catches_paste_mistakes() {
        assert_eq!(
            normalize_token(" bot123456789:AAAbbbCCCdddEEEfffGGGhhhIIIjjj12 \n").unwrap(),
            "123456789:AAAbbbCCCdddEEEfffGGGhhhIIIjjj12"
        );
        assert_eq!(
            normalize_token("1234 56789:AAAbbbCCCdddEEEfffGGGhhhIIIjjj12").unwrap(),
            "123456789:AAAbbbCCCdddEEEfffGGGhhhIIIjjj12"
        );
        assert!(normalize_token("Use this token: 123").is_err());
        assert!(normalize_token("no-colon-here").is_err());
        assert!(normalize_token("123456789:short").is_err());
    }

    #[test]
    fn reply_keyboard_text_is_swallowed() {
        assert!(is_reply_keyboard_text(" 🔌 Kota / 肥波 "));
        assert!(is_reply_keyboard_text("Switch"));
        assert!(!is_reply_keyboard_text("帮我看下打包进度"));
    }

    #[test]
    fn standby_control_messages_are_detected() {
        let control = json!({
            "id": "tg-43",
            "update": {
                "message": {
                    "text": "🔌 Kota / 蘅芜君",
                    "from": {"id": 1},
                    "chat": {"id": 1, "type": "private"}
                }
            }
        });
        assert!(standby_offline_item_is_control_message(&control));

        let switch = json!({
            "id": "tg-44",
            "update": {
                "message": {
                    "text": "Switch",
                    "from": {"id": 1},
                    "chat": {"id": 1, "type": "private"}
                }
            }
        });
        assert!(standby_offline_item_is_control_message(&switch));

        let normal = json!({
            "id": "tg-45",
            "update": {
                "message": {
                    "text": "帮我看下打包进度",
                    "from": {"id": 1},
                    "chat": {"id": 1, "type": "private"}
                }
            }
        });
        assert!(!standby_offline_item_is_control_message(&normal));
    }

    #[test]
    fn transient_poll_errors_are_deferred() {
        assert!(is_transient_poll_error(
            "poll: telegram getUpdates: https://api.telegram.org/bot<token>/getUpdates: Network Error: timed out reading response"
        ));
        assert!(is_transient_poll_error(
            "poll: telegram getUpdates: Dns Failed: failed to lookup address information"
        ));
        assert!(is_transient_poll_error(
            "poll: telegram getUpdates: Network Error: Connection reset by peer"
        ));
    }

    #[test]
    fn http_poll_errors_are_not_deferred() {
        assert!(!is_transient_poll_error(
            "poll: telegram getUpdates: http 401: Unauthorized"
        ));
        assert!(!is_transient_poll_error(
            "poll: telegram getUpdates: http 409: Conflict"
        ));
        assert!(!is_transient_poll_error(
            "poll: telegram sendMessage: Network Error: timed out reading response"
        ));
    }

    #[test]
    fn media_path_detection_is_conservative() {
        let dir = std::env::temp_dir().join(format!(
            "lm-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::create_dir_all(&dir);
        let img = dir.join("shot.png");
        fs::write(&img, b"x").unwrap();
        let doc = dir.join("note.md");
        fs::write(&doc, b"# note").unwrap();
        let big = dir.join("big.bin");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_MEDIA_BYTES + 1)
            .unwrap();
        let text = format!(
            "结果见 {} 和 {} 以及不存在的 /nope/gone.png 和相对 path.png",
            img.display(),
            doc.display()
        );
        let (images, docs) = detect_media_paths(&text, &[dir.clone()]);
        assert_eq!(images, vec![img.clone()]);
        assert!(docs.is_empty());

        let standalone = format!("附件如下:\n{}\n{}\n", doc.display(), big.display());
        let (images, docs) = detect_media_paths(&standalone, &[dir.clone()]);
        assert!(images.is_empty());
        assert_eq!(docs, vec![doc.clone()]);

        #[cfg(unix)]
        {
            let outside_dir = std::env::temp_dir().join(format!(
                "lm-test-outside-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            fs::create_dir_all(&outside_dir).unwrap();
            let outside_doc = outside_dir.join("secret.md");
            fs::write(&outside_doc, b"secret").unwrap();
            let link = dir.join("secret-link.md");
            std::os::unix::fs::symlink(&outside_doc, &link).unwrap();
            let (_, docs) = detect_media_paths(&format!("{}\n", link.display()), &[dir.clone()]);
            assert!(docs.is_empty());
            let _ = fs::remove_dir_all(&outside_dir);
        }

        // Outside the allowed roots → never uploaded, even if it exists.
        let (outside, _) = detect_media_paths(&text, &[PathBuf::from("/srv/elsewhere")]);
        assert!(outside.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn panel_callback_data_stays_within_64_bytes() {
        let panel = LmPanelSession {
            revision: u64::MAX,
            projects: vec![("id".into(), "/root".into(), "名字很长的项目名字很长".into())],
            agents: vec![("agent-490e160d76".into(), "肥波".into()); 8],
            ..Default::default()
        };
        let (_, markup) = agent_panel(&panel, "项目");
        let rows = markup["inline_keyboard"].as_array().unwrap();
        for row in rows {
            for button in row.as_array().unwrap() {
                let data = button["callback_data"].as_str().unwrap();
                assert!(data.len() <= 64, "callback_data too long: {data}");
            }
        }
    }

    #[test]
    fn refresh_selected_agent_metadata_updates_live_names_only() {
        let mut state = LmState {
            selected: Some(LmSelected {
                project_id: "project-one".into(),
                project_root: "/tmp/project-one".into(),
                project_name: "Project One".into(),
                agent_id: "agent-one".into(),
                agent_name: "Old Name".into(),
                muted: true,
            }),
            panel: LmPanelSession {
                agents: vec![
                    ("agent-one".into(), "Old Name".into()),
                    ("agent-two".into(), "Other".into()),
                ],
                ..Default::default()
            },
            catalog: vec![LmCatalogProject {
                project_id: "project-one".into(),
                project_root: "/tmp/project-one".into(),
                project_name: "Project One".into(),
                agents: vec![
                    ("agent-one".into(), "Old Name".into()),
                    ("agent-two".into(), "Other".into()),
                ],
            }],
            ..Default::default()
        };
        refresh_selected_agent_metadata_in_state(
            &mut state,
            Path::new("/tmp/project-one-agent-worktree-context"),
            "agent-one",
            "New Name",
        );
        let selected = state.selected.unwrap();
        assert_eq!(selected.agent_name, "New Name");
        assert!(selected.muted);
        assert_eq!(state.panel.agents[0].1, "New Name");
        assert_eq!(state.panel.agents[1].1, "Other");
        assert_eq!(state.catalog[0].agents[0].1, "New Name");
        assert_eq!(state.catalog[0].agents[1].1, "Other");
    }

    #[test]
    fn catalog_refresh_reconciles_selected_metadata() {
        let mut state = LmState {
            selected: Some(LmSelected {
                project_id: "project-one".into(),
                project_root: "/old/root".into(),
                project_name: "Old Project".into(),
                agent_id: "agent-one".into(),
                agent_name: "Old Agent".into(),
                muted: true,
            }),
            ..Default::default()
        };
        apply_catalog_in_state(
            &mut state,
            vec![LmCatalogProject {
                project_id: "project-one".into(),
                project_root: "/new/root".into(),
                project_name: "New Project".into(),
                agents: vec![("agent-one".into(), "New Agent".into())],
            }],
        );
        let selected = state.selected.unwrap();
        assert_eq!(selected.project_root, "/new/root");
        assert_eq!(selected.project_name, "New Project");
        assert_eq!(selected.agent_name, "New Agent");
        assert!(selected.muted);
        assert!(state.catalog_updated_at.is_some());
    }

    #[test]
    fn telegram_text_chunks_preserve_content() {
        let text = "alpha beta gamma delta epsilon zeta";
        let chunks = split_telegram_text(text, 12);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.join(" "), text);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 12));
    }

    #[test]
    fn html_escape_preserves_newlines() {
        assert_eq!(
            html_escape("<a&b>\n\"c\""),
            "&lt;a&amp;b&gt;\n&quot;c&quot;"
        );
    }

    #[test]
    fn telegram_label_wraps_bold_agent_name_with_sparkles() {
        assert_eq!(telegram_label_plain("紫鹃", ""), "[✨紫鹃✨]");
        assert_eq!(telegram_label_html("紫鹃", " 1/2"), "[✨<b>紫鹃 1/2</b>✨]");
    }

    #[test]
    fn telegram_markdown_html_formats_common_inline_blocks() {
        let html = format_lm_telegram_html("**bold** and `code` and [Kota](https://example.com)");
        assert_eq!(
            html,
            "<b>bold</b> and <code>code</code> and <a href=\"https://example.com\">Kota</a>"
        );
        let unsafe_link = format_lm_telegram_html("[bad](file:///tmp/a)");
        assert_eq!(unsafe_link, "[bad](file:///tmp/a)");
    }

    #[test]
    fn telegram_markdown_html_leaves_code_fence_contents_literal() {
        let html = format_lm_telegram_html("```rs\n**not bold**\nlet x = 1 < 2;\n```");
        assert_eq!(html, "<pre>**not bold**\nlet x = 1 &lt; 2;\n</pre>");
    }

    #[test]
    fn telegram_markdown_html_formats_headings_and_lists() {
        let html = format_lm_telegram_html(
            "## Deploy\n- **Open** settings\n  1. Run `Connect`\n- [x] Done",
        );
        assert_eq!(
            html,
            "<b>Deploy</b>\n• <b>Open</b> settings\n  1. Run <code>Connect</code>\n☑ Done"
        );
    }

    #[test]
    fn simple_gfm_tables_rewrite_to_mobile_notes() {
        let html = format_lm_telegram_html(
            "| 项目 | 选择 | 评价 |\n|---|---|---|\n| Iron Oak | **标准版** | 含 feather |\n| Washer | #6698 | `防松` |",
        );
        assert_eq!(
            html,
            "Iron Oak\n• 选择: <b>标准版</b>\n• 评价: 含 feather\n\nWasher\n• 选择: #6698\n• 评价: <code>防松</code>"
        );
    }

    #[test]
    fn complex_gfm_tables_stay_literal() {
        let mismatched = "| A | B |\n|---|---|\n| 1 | 2 | 3 |";
        assert_eq!(
            format_lm_telegram_html(mismatched),
            "| A | B |\n|---|---|\n| 1 | 2 | 3 |"
        );
        let fenced = "```\n| A | B |\n|---|---|\n| 1 | 2 |\n```";
        assert_eq!(
            format_lm_telegram_html(fenced),
            "<pre>| A | B |\n|---|---|\n| 1 | 2 |\n</pre>"
        );
    }

    #[test]
    fn standby_queue_target_fails_closed_and_uses_current_catalog() {
        let state = LmState {
            selected: Some(LmSelected {
                project_id: "current-project".into(),
                project_root: "/tmp/current".into(),
                project_name: "Current".into(),
                agent_id: "current-agent".into(),
                agent_name: "Current Agent".into(),
                muted: false,
            }),
            catalog: vec![LmCatalogProject {
                project_id: "project-one".into(),
                project_root: "/tmp/project-one".into(),
                project_name: "Project One".into(),
                agents: vec![("agent-one".into(), "Renamed Agent".into())],
            }],
            ..Default::default()
        };
        let missing_snapshot = LmStandbyQueueItem {
            id: "tg-1".into(),
            received_at: now_iso(),
            preview: "hello".into(),
            project_id: None,
            project_name: None,
            agent_id: None,
            agent_name: None,
            status: "queued".into(),
            sent_at: None,
            delivery_error: None,
            update: JsonValue::Null,
        };
        assert!(queued_target_selection(&state, &missing_snapshot).is_err());

        let mut stale_agent = missing_snapshot.clone();
        stale_agent.project_id = Some("project-one".into());
        stale_agent.agent_id = Some("agent-old".into());
        assert!(queued_target_selection(&state, &stale_agent).is_err());

        let mut valid = stale_agent;
        valid.agent_id = Some("agent-one".into());
        valid.agent_name = Some("Old Agent".into());
        let selected = queued_target_selection(&state, &valid).unwrap();
        assert_eq!(selected.project_root, "/tmp/project-one");
        assert_eq!(selected.project_name, "Project One");
        assert_eq!(selected.agent_name, "Renamed Agent");
    }

    #[test]
    fn standby_seen_ring_blocks_duplicate_offline_imports_after_trim() {
        let mut state = LmState::default();
        let item = json!({
            "id": "tg-42",
            "receivedAt": "2026-06-16T01:02:03Z",
            "preview": "hello",
            "target": {
                "projectId": "project-one",
                "projectName": "Project One",
                "agentId": "agent-one",
                "agentName": "Agent One"
            },
            "update": { "update_id": 42, "message": { "text": "hello" } }
        });
        import_standby_queue_item(&mut state, &item);
        assert_eq!(state.standby_queue.len(), 1);
        state.standby_queue.clear();
        import_standby_queue_item(&mut state, &item);
        assert!(state.standby_queue.is_empty());
        assert!(standby_queue_seen(&state, "tg-42"));
    }
}

#[cfg(test)]
mod connectivity_probe {
    use super::*;

    /// Live-network probe (run manually): a bogus token must yield HTTP 401
    /// from Telegram — proving the exact ureq call path connects fine.
    #[test]
    #[ignore]
    fn probe_get_me_connectivity() {
        let err = get_me("000000:probe-invalid-token")
            .unwrap_err()
            .to_string();
        println!("probe result: {err}");
        assert!(
            err.contains("401") || err.contains("Unauthorized") || err.contains("Not Found"),
            "expected an HTTP-level error (connection OK), got: {err}"
        );
    }
}
