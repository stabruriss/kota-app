//! Dance: project-local settings, bounded Jev classification and a durable composer receipt.
//! Only an explicit main-composer route calls this module; ordinary/LM/BBS/Ember sends do not.
use crate::{
    integrations::IntegrationManager,
    local_rpc::{
        auto_route,
        error::{LocalError, LocalResult},
        jev, jev_http,
        owned::OwnedDirectory,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tauri::{AppHandle, Emitter, Manager};

#[path = "puppeteer_events.rs"]
mod events;
use events::{Agent as EventAgent, Candidate, Data as EventData, Outcome, Reason, Selection};

const SETTINGS_FILE: &str = "puppeteer-dance.json";
fn initial_threshold() -> f64 {
    0.5
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Strategy {
    #[default]
    Context,
    Rule,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Settings {
    pub schema: u8,
    pub enabled: bool,
    pub sentinel: Option<String>,
    pub strategy: Strategy,
    #[serde(default = "initial_threshold")]
    pub threshold: f64,
    #[serde(default)]
    pub conditions: BTreeMap<String, String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: 1,
            enabled: false,
            sentinel: None,
            strategy: Strategy::Context,
            threshold: initial_threshold(),
            conditions: BTreeMap::new(),
        }
    }
}
fn agent_id(id: &str) -> bool {
    id.starts_with("agent-")
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
impl Settings {
    fn validate(&self) -> LocalResult<()> {
        if self.schema != 1
            || !auto_route::valid_threshold(self.threshold)
            || self.sentinel.as_deref().is_some_and(|id| !agent_id(id))
            || self.conditions.iter().any(|(id, condition)| {
                !agent_id(id) || condition.chars().count() > 200 || condition.contains('\0')
            })
        {
            return Err(LocalError::new("dance_settings_invalid"));
        }
        if self.strategy == Strategy::Rule
            && self
                .conditions
                .values()
                .filter(|c| !c.trim().is_empty())
                .count()
                < 2
        {
            return Err(LocalError::new("dance_rules_required"));
        }
        Ok(())
    }
}
pub(crate) fn prompts() -> &'static Value {
    crate::puppeteer_prompts::builtin()
}
#[tauri::command]
pub(crate) fn puppeteer_jev_prompts() -> Value {
    let loaded = crate::puppeteer_prompts::read();
    let p = &loaded.value;
    json!({"stringFirst":p["stringFirst"],"stringLater":p["stringLater"],"danceContext":p["danceContext"],"danceRule":p["danceRule"]})
}
fn settings_path(root: &Path) -> PathBuf {
    crate::project_memory_dir(root).join(SETTINGS_FILE)
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshot {
    settings: Settings,
    source_revision: String,
    current_sentinel: Option<String>,
}
fn read_source(root: &Path) -> LocalResult<Option<Vec<u8>>> {
    let path = settings_path(root);
    let stat = match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LocalError::new("dance_settings_read_failed")),
        Ok(s) => s,
    };
    if !stat.is_file() || stat.file_type().is_symlink() || stat.len() > 1024 * 1024 {
        return Err(LocalError::new("dance_settings_invalid"));
    }
    fs::read(path).map(Some).map_err(|_| LocalError::new("dance_settings_read_failed"))
}
fn decode(source: Option<&[u8]>) -> LocalResult<Settings> {
    let Some(source) = source else { return Ok(Settings::default()); };
    let settings: Settings = serde_json::from_slice(&crate::puppeteer_setup::json_without_comments(source)?)
        .map_err(|_| LocalError::new("dance_settings_invalid"))?;
    settings.validate()?;
    Ok(settings)
}
fn read_snapshot(root: &Path) -> LocalResult<SettingsSnapshot> {
    let source = read_source(root)?;
    let settings = decode(source.as_deref())?;
    let current_sentinel = if settings.enabled { settings.sentinel.clone() } else { None };
    Ok(SettingsSnapshot { settings, source_revision: crate::puppeteer_setup::revision(source.as_deref()), current_sentinel })
}
fn read(root: &Path) -> LocalResult<Settings> { Ok(read_snapshot(root)?.settings) }
fn read_current_snapshot(root: &Path) -> LocalResult<SettingsSnapshot> {
    let lock = routing_lock(root);
    let mut routing = lock.lock().map_err(|_| LocalError::new("dance_busy"))?;
    // Read after the in-flight route; otherwise a stale file snapshot could reset its state.
    let mut snapshot = read_snapshot(root)?;
    routing.sentinel.sync(&snapshot.settings, false);
    snapshot.current_sentinel = routing.sentinel.current.clone();
    Ok(snapshot)
}
fn settings_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default).lock().unwrap().entry(root.into()).or_default().clone()
}
fn serialize(settings: &Settings) -> Vec<u8> {
    format!("{}{}\n", crate::puppeteer_setup::DANCE_COMMENTS, serde_json::to_string_pretty(settings).unwrap()).into_bytes()
}
pub(crate) fn prepare(root: &Path) -> LocalResult<PathBuf> {
    let lock = settings_lock(root);
    let _guard = lock.lock().map_err(|_| LocalError::new("dance_busy"))?;
    let path = settings_path(root);
    if read_source(root)?.is_none() {
        crate::puppeteer_setup::create_if_absent(&path, &serialize(&Settings::default()))?;
    }
    let source = read_source(root)?;
    decode(source.as_deref())?;
    let before = source.as_deref().ok_or_else(|| LocalError::new("dance_settings_read_failed"))?;
    let text = std::str::from_utf8(before).map_err(|_| LocalError::new("dance_settings_invalid"))?;
    let next = crate::puppeteer_setup::with_missing_comments(text, crate::puppeteer_setup::DANCE_COMMENTS);
    if next.as_bytes() != before {
        if read_source(root)? != source { return Err(LocalError::new("dance_settings_conflict")); }
        crate::adapter_sync::atomic_replace(&path, next.as_bytes()).map_err(|_| LocalError::new("dance_settings_write_failed"))?;
    }
    Ok(path)
}
pub(crate) fn project_root(root: &str) -> LocalResult<PathBuf> {
    let path = Path::new(root);
    if !path.is_absolute() || !path.is_dir() {
        return Err(LocalError::new("invalid_request"));
    }
    path.canonicalize()
        .map_err(|_| LocalError::new("invalid_request"))
}
#[derive(Clone, Debug)]
struct Seat {
    id: String,
    name: String,
}
fn seats(manager: &IntegrationManager, root: &str) -> LocalResult<Vec<Seat>> {
    // The layout stores the UI's root spelling, not its canonical filesystem identity.
    let layout = crate::load_project_agent_layout(manager, Some(root))
        .map_err(|_| LocalError::new("dance_roster_unavailable"))?
        .ok_or_else(|| LocalError::new("dance_roster_unavailable"))?;
    let ids: Vec<_> = layout.table_slots.into_iter().flatten().collect();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for id in ids.into_iter().take(8) {
        if !agent_id(&id) || !seen.insert(id.clone()) {
            continue;
        }
        let path = Path::new(root)
            .join(".agent-workspaces")
            .join(&id)
            .join("agent.yaml");
        let Ok(yaml) = crate::read_yaml_mapping(&path) else {
            continue;
        };
        if crate::yaml_string(&yaml, "status").is_some_and(|s| !s.eq_ignore_ascii_case("active")) {
            continue;
        }
        let name = crate::yaml_string(&yaml, "display-name")
            .or_else(|| crate::yaml_string(&yaml, "displayName"))
            .unwrap_or_else(|| id.clone());
        result.push(Seat {
            id,
            name: clip(&name, 64),
        });
    }
    Ok(result)
}
fn gate(settings: &Settings, roster: &[Seat], owned: &OwnedDirectory, starting: bool) -> LocalResult<()> {
    if settings.enabled {
        if starting && !roster
            .iter()
            .any(|s| Some(s.id.as_str()) == settings.sentinel.as_deref())
        {
            return Err(LocalError::new("dance_sentinel_required"));
        }
        jev::gate(owned)?;
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn puppeteer_dance_read(project_root: String) -> LocalResult<SettingsSnapshot> {
    tauri::async_runtime::spawn_blocking(move || read_current_snapshot(&self::project_root(&project_root)?))
        .await
        .map_err(|_| LocalError::new("dance_settings_read_failed"))?
}
fn save_checked(
    manager: &IntegrationManager,
    project_root: &str,
    settings: &Settings,
    owned: &OwnedDirectory,
    source_revision: &str,
    on_sentinel: impl Fn(Option<String>),
) -> LocalResult<SettingsSnapshot> {
    let root = self::project_root(project_root)?;
    let lock = settings_lock(&root);
    let _guard = lock.lock().map_err(|_| LocalError::new("dance_busy"))?;
    let before = read_source(&root)?;
    if crate::puppeteer_setup::revision(before.as_deref()) != source_revision {
        return Err(LocalError::new("dance_settings_conflict"));
    }
    settings.validate()?;
    // A valid disabled replacement can repair unreadable settings without roster/auth gates.
    let existing = decode(before.as_deref()).ok();
    let starting = settings.enabled && existing.as_ref().is_none_or(|s| !s.enabled);
    // Turning off must still work when a key was rejected or old Rule agents left the room.
    let turning_off_only = !settings.enabled
        && existing.is_none_or(|existing| {
            Settings {
                enabled: false,
                ..existing
            } == *settings
        });
    if !turning_off_only {
        let roster = seats(manager, project_root)?;
        if settings.strategy == Strategy::Rule
            && roster
                .iter()
                .filter(|s| {
                    settings
                        .conditions
                        .get(&s.id)
                        .is_some_and(|c| !c.trim().is_empty())
                })
                .count()
                < 2
        {
            return Err(LocalError::new("dance_rules_required"));
        }
        gate(settings, &roster, owned, starting)?;
    }
    fs::create_dir_all(crate::project_memory_dir(&root))
        .map_err(|_| LocalError::new("dance_settings_write_failed"))?;
    let bytes = serialize(settings);
    if read_source(&root)? != before { return Err(LocalError::new("dance_settings_conflict")); }
    crate::adapter_sync::atomic_replace(&settings_path(&root), &bytes)
        .map_err(|_| LocalError::new("dance_settings_write_failed"))?;
    let lock = routing_lock(&root);
    let mut routing = lock.lock().map_err(|_| LocalError::new("dance_busy"))?;
    if routing.sentinel.sync(settings, starting) { on_sentinel(routing.sentinel.current.clone()); }
    Ok(SettingsSnapshot { settings: settings.clone(), source_revision: crate::puppeteer_setup::revision(Some(&bytes)),
        current_sentinel: routing.sentinel.current.clone() })
}
#[tauri::command]
pub(crate) async fn puppeteer_dance_save(
    app: AppHandle,
    project_root: String,
    settings: Settings,
    source_revision: String,
) -> LocalResult<SettingsSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = save_checked(
            &app.state::<IntegrationManager>(),
            &project_root,
            &settings,
            &OwnedDirectory::new(&crate::kota_home_dir()),
            &source_revision,
            |current| emit_sentinel(&app, &project_root, current),
        )?;
        let _ = app.emit(
            "puppeteer-dance://changed",
            json!({"projectRoot":project_root,"settings":snapshot.settings,"sourceRevision":snapshot.source_revision,"currentSentinel":snapshot.current_sentinel}),
        );
        Ok(snapshot)
    })
    .await
    .map_err(|_| LocalError::new("dance_settings_write_failed"))?
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimedText {
    pub at: String,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RosterItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub last_reply: Option<TimedText>,
    /// Private room-order tie-break for coarse timestamps. Never part of Jev's state.
    #[serde(default)]
    pub last_reply_order: Option<u64>,
    #[serde(default)]
    pub recent_user_messages: Vec<TimedText>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Attachment {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RouteRequest {
    pub message_id: String,
    pub timestamp: String,
    pub payload: String,
    /// Clean composer draft for classification; payload is the unchanged delivery string.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub paths_mentioned: Vec<String>,
    pub roster: Vec<RosterItem>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Routed {
    pub message_id: String,
    pub target_agent_id: String,
    pub fallback: bool,
    pub recorded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared: Option<crate::temporal_context::PreparedComposerPrompt>,
}
fn clip(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}
fn timed(text: &TimedText) -> Value {
    json!({"at":clip(&text.at,64),"text":clip(&text.text,200)})
}
/// Older callers omit `text`; then use the existing composer echo rules to keep
/// attachment paths/provider decorations out of Jev's text. This never changes delivery.
fn classification_text(request: &RouteRequest) -> String {
    if let Some(text) = &request.text {
        return text.clone();
    }
    static MARKERS: OnceLock<regex::Regex> = OnceLock::new();
    let markers = MARKERS.get_or_init(|| {
        regex::Regex::new(r"\[[^\]\n#]{1,40}#[0-9]+\]|\[[^\]\n:]{1,20}: source: [^\]\n]+\]")
            .unwrap()
    });
    let mut text = markers.replace_all(&request.payload, " ").into_owned();
    for path in request
        .attachments
        .iter()
        .filter_map(|a| a.path.as_deref())
        .filter(|p| !p.is_empty())
    {
        text = text
            .replace(&serde_json::to_string(path).unwrap(), " ")
            .replace(path, " ");
    }
    text.split_whitespace()
        .filter(|token| !token.contains("project-memory/attachments/"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn request_body(
    request: &RouteRequest,
    settings: &Settings,
    roster: &[Seat],
    templates: &Value,
) -> Value {
    let attachments: Vec<_> = request.attachments.iter().take(8).map(|a| json!({"name":clip(&a.name,256),
        "path":a.path.as_ref().map(|s|clip(s,1024)),"mimeType":a.mime_type.as_ref().map(|s|clip(s,128)),"size":a.size})).collect();
    let new_message = json!({"at":request.timestamp,"text":clip(&classification_text(request),4096),"attachments":attachments,
        "links":request.links.iter().take(8).map(|s|clip(s,1024)).collect::<Vec<_>>(),
        "paths_mentioned":request.paths_mentioned.iter().take(8).map(|s|clip(s,1024)).collect::<Vec<_>>()});
    let contexts: HashMap<_, _> = request.roster.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut candidates = Vec::new();
    let mut criteria = Map::new();
    for seat in roster {
        if settings.strategy == Strategy::Rule {
            let Some(condition) = settings
                .conditions
                .get(&seat.id)
                .filter(|c| !c.trim().is_empty())
            else {
                continue;
            };
            candidates.push(json!({"id":seat.id,"name":seat.name,"condition":condition}));
            let template = templates["danceRule"]["questions"]["target"]["criteria"]["<agent_id>"]
                .as_str()
                .unwrap();
            criteria.insert(
                seat.id.clone(),
                json!(template
                    .replace("<agent_id>", &seat.id)
                    .replace("<condition>", condition)),
            );
        } else {
            let recent = contexts.get(seat.id.as_str());
            candidates.push(json!({"id":seat.id,"name":seat.name,
                "last_reply":recent.and_then(|r|r.last_reply.as_ref()).map(timed),
                "recent_user_messages":recent.map(|r|r.recent_user_messages.iter().skip(r.recent_user_messages.len().saturating_sub(2)).map(timed).collect::<Vec<_>>()).unwrap_or_default()}));
            let template = templates["danceContext"]["questions"]["target"]["criteria"]
                ["<agent_id>"]
                .as_str()
                .unwrap();
            criteria.insert(
                seat.id.clone(),
                json!(template.replace("<agent_id>", &seat.id)),
            );
        }
    }
    let key = if settings.strategy == Strategy::Context {
        "danceContext"
    } else {
        "danceRule"
    };
    let mut state = json!({"new_message":new_message,"roster":candidates});
    if let Some((id, reply)) = latest_reply_message(request, roster) {
        let mut latest = timed(reply);
        latest["agent_id"] = json!(id);
        state["latest_reply"] = latest;
    }
    json!({"state":state,"questions":{"target":{"type":"choice",
        "instructions":templates[key]["questions"]["target"]["instructions"],"criteria":criteria}}})
}
/// Only seated agents, before this input. Same-second coarse records use room order;
/// full precision / other seconds use time, even when history arrived in paged order.
/// Keep the id and content from the same record, including duplicate-id histories.
fn latest_reply_message<'a>(request: &'a RouteRequest, roster: &[Seat]) -> Option<(&'a str, &'a TimedText)> {
    let boundary = chrono::DateTime::parse_from_rfc3339(&request.timestamp).ok()?;
    let candidates = request.roster.iter().filter_map(|item| {
        if !roster.iter().any(|seat| seat.id == item.id) { return None; }
        let reply = item.last_reply.as_ref()?;
        let time = chrono::DateTime::parse_from_rfc3339(&reply.at).ok()?;
        if time >= boundary { return None; }
        let fraction = reply.at.split_once('.').map(|(_, tail)| tail.bytes().take_while(u8::is_ascii_digit).count()).unwrap_or(0);
        Some((item, time, fraction < 3))
    }).collect::<Vec<_>>();
    let coarse_seconds: HashSet<_> = candidates.iter().filter(|(_, _, coarse)| *coarse).map(|(_, time, _)| time.timestamp()).collect();
    let mut best: Option<&( &RosterItem, chrono::DateTime<chrono::FixedOffset>, bool)> = None;
    for candidate in &candidates {
        let later = best.is_none_or(|before| {
            if candidate.1.timestamp() == before.1.timestamp() && coarse_seconds.contains(&candidate.1.timestamp()) {
                if let (Some(a), Some(b)) = (candidate.0.last_reply_order, before.0.last_reply_order) { return a > b; }
            }
            candidate.1 > before.1
        });
        if later { best = Some(candidate); }
    }
    best.and_then(|(item, _, _)| item.last_reply.as_ref().map(|reply| (item.id.as_str(), reply)))
}
fn pick(reply: &Value, body: &Value) -> LocalResult<(String, f64)> {
    let answer = &reply["answers"]["target"];
    let ids = body["questions"]["target"]["criteria"].as_object().unwrap();
    let probabilities = answer["probabilities"]
        .as_object()
        .ok_or_else(|| LocalError::new("jev_unreachable"))?;
    if answer["type"] != "choice"
        || answer["choice"]
            .as_str()
            .is_none_or(|id| !ids.contains_key(id))
        || ids.keys().any(|id| {
            probabilities
                .get(id)
                .and_then(Value::as_f64)
                .is_none_or(|p| !p.is_finite() || !(0.0..=1.0).contains(&p))
        })
    {
        return Err(LocalError::new("jev_unreachable"));
    }
    let mut best: Option<(&String, f64)> = None;
    for id in ids.keys() {
        let p = probabilities[id].as_f64().unwrap();
        if best.is_none_or(|(_, before)| p > before) {
            best = Some((id, p));
        }
    }
    best.map(|(id, probability)| (id.clone(), probability))
        .ok_or_else(|| LocalError::new("jev_unreachable"))
}
fn choose_target(
    request: &RouteRequest,
    settings: &Settings,
    before: &[Seat],
    templates: &Value,
    after: impl FnOnce() -> LocalResult<Vec<Seat>>,
    classify: impl FnOnce(Value) -> LocalResult<Value>,
) -> LocalResult<Selection> {
    let mut candidate = None;
    let (selected, mut outcome, mut reason) = if before.len() == 1 {
        (Some(before[0].id.clone()), Outcome::Direct, Some(Reason::OnlySeat))
    } else if classification_text(request).trim().is_empty() {
        (None, Outcome::Sentinel, Some(Reason::EmptyText))
    } else {
        let body = request_body(request, settings, before, templates);
        let candidates = body["state"]["roster"].as_array().unwrap();
        if candidates.is_empty() {
            (None, Outcome::Sentinel, Some(Reason::NoCandidates))
        } else if candidates.len() == 1 {
            (
                Some(candidates[0]["id"].as_str().unwrap().to_owned()),
                Outcome::Direct, Some(Reason::OnlyCandidate),
            )
        } else {
            let (id, probability) = pick(&classify(body.clone())?, &body)?;
            let seat = before.iter().find(|s| s.id == id).expect("validated candidate");
            candidate = Some(Candidate { agent: EventAgent { agent_id: id.clone(), name: seat.name.clone() }, probability });
            if probability > settings.threshold {
                (Some(id), Outcome::Selected, None)
            } else {
                (None, Outcome::Sentinel, Some(Reason::BelowThreshold))
            }
        }
    };
    let after = after()?;
    let target = if let Some(seat) = selected.as_ref().and_then(|id| after.iter().find(|s| s.id == *id)) {
        seat
    } else {
        if selected.is_some() { reason = Some(Reason::TargetLeft); }
        outcome = Outcome::Sentinel;
        after.iter().find(|s| Some(&s.id) == settings.sentinel.as_ref())
            .ok_or_else(|| LocalError::new("dance_sentinel_required"))?
    };
    Ok(Selection { target: EventAgent { agent_id: target.id.clone(), name: target.name.clone() },
        outcome, candidate, threshold: settings.threshold, strategy: settings.strategy.clone(), reason })
}
struct Completed {
    message_id: String,
    timestamp: String,
    payload_hash: [u8; 32],
    routed: Routed,
}
#[derive(Default)]
struct Sentinel {
    enabled: bool,
    seed: Option<String>,
    current: Option<String>,
}
impl Sentinel {
    fn sync(&mut self, settings: &Settings, restart: bool) -> bool {
        let before = self.current.clone();
        if !settings.enabled { self.current = None; }
        else if restart || !self.enabled || self.seed != settings.sentinel { self.current = settings.sentinel.clone(); }
        self.enabled = settings.enabled;
        self.seed = settings.sentinel.clone();
        before != self.current
    }
    fn for_round(&mut self, settings: &Settings) -> bool {
        self.sync(settings, false)
    }
    fn follow(&mut self, target: &str) -> bool {
        if self.current.as_deref() == Some(target) { return false; }
        self.current = Some(target.into());
        true
    }
}
#[derive(Default)]
struct Routing {
    completed: VecDeque<Completed>,
    events: events::Log,
    sentinel: Sentinel,
}
fn emit_sentinel(app: &AppHandle, project_root: &str, current: Option<String>) {
    let _ = app.emit("puppeteer-dance://sentinel", json!({"projectRoot":project_root,"currentSentinel":current}));
}
/// Open-dialog snapshot only. No polling or disk history is introduced.
#[tauri::command]
pub(crate) async fn puppeteer_dance_events(project_root: String) -> LocalResult<Vec<events::Event>> {
    tauri::async_runtime::spawn_blocking(move || {
        let lock = routing_lock(&self::project_root(&project_root)?);
        let routing = lock.try_lock().map_err(|_| LocalError::new("dance_busy"))?;
        Ok(routing.events.snapshot())
    }).await.map_err(|_| LocalError::new("dance_failed"))?
}
fn record_event(routing: &mut Routing, data: EventData, emit: &impl Fn(events::Event)) {
    if let Some(event) = routing.events.record(data) { emit(event); }
}
impl Routing {
    fn route(&mut self, request: &RouteRequest, emit: &impl Fn(events::Event),
        fresh: impl FnOnce(&mut Self) -> LocalResult<Routed>) -> LocalResult<Routed> {
        if let Some(previous) = self.cached(request)? { return Ok(previous); }
        let result = fresh(self);
        match &result {
            Ok(routed) => self.remember(request, routed.clone()),
            Err(error) => if let Some(data) = EventData::error(&error.code) { record_event(self, data, emit); },
        }
        result
    }
    fn cached(&self, request: &RouteRequest) -> LocalResult<Option<Routed>> {
        let Some(previous) = self
            .completed
            .iter()
            .find(|r| r.message_id == request.message_id)
        else {
            return Ok(None);
        };
        let payload_hash: [u8; 32] = Sha256::digest(request.payload.as_bytes()).into();
        if previous.payload_hash != payload_hash || previous.timestamp != request.timestamp {
            return Err(LocalError::new("invalid_request"));
        }
        Ok(Some(previous.routed.clone()))
    }
    fn remember(&mut self, request: &RouteRequest, routed: Routed) {
        if self.completed.len() >= 128 {
            self.completed.pop_front();
        }
        self.completed.push_back(Completed {
            message_id: request.message_id.clone(),
            timestamp: request.timestamp.clone(),
            payload_hash: Sha256::digest(request.payload.as_bytes()).into(),
            routed,
        });
    }
}
fn routing_lock(root: &Path) -> Arc<Mutex<Routing>> {
    static ROUTING: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Routing>>>>> = OnceLock::new();
    ROUTING
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(root.into())
        .or_default()
        .clone()
}
#[tauri::command]
pub(crate) async fn puppeteer_dance_route(
    app: AppHandle,
    project_root: String,
    request: RouteRequest,
) -> LocalResult<Routed> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = self::project_root(&project_root)?;
        if request.message_id.is_empty()
            || request.message_id.len() > 256
            || request.message_id.chars().any(|c| c.is_control())
            || chrono::DateTime::parse_from_rfc3339(&request.timestamp).is_err()
            || request.payload.len() > 1024 * 1024
            || request.payload.trim().is_empty()
            || request.roster.len() > 8
            || request
                .roster
                .iter()
                .any(|r| !agent_id(&r.id) || r.name.chars().count() > 64 || r.name.contains('\0'))
        {
            return Err(LocalError::new("invalid_request"));
        }
        let lock = routing_lock(&root);
        let mut routing = lock.try_lock().map_err(|_| LocalError::new("dance_busy"))?;
        let emit = |event| { let _ = app.emit("puppeteer-dance://event", json!({"projectRoot":project_root,"event":event})); };
        let result = routing.route(&request, &emit, |routing| {
            let settings = read(&root)?;
            if !settings.enabled {
                if routing.sentinel.sync(&settings, false) { emit_sentinel(&app, &project_root, None); }
                return Err(LocalError::new("dance_disabled"));
            }
            let manager = app.state::<IntegrationManager>();
            let before = seats(&manager, &project_root)?;
            if routing.sentinel.for_round(&settings) {
                emit_sentinel(&app, &project_root, routing.sentinel.current.clone());
            }
            let effective = Settings { sentinel: routing.sentinel.current.clone(), ..settings.clone() };
            let owned = OwnedDirectory::new(&crate::kota_home_dir());
            jev::gate(&owned)?;
            let loaded = crate::puppeteer_prompts::read();
            let selection = choose_target(
                &request,
                &effective,
                &before,
                &loaded.value,
                || {
                    if read(&root)? != settings {
                        return Err(LocalError::new("dance_settings_changed"));
                    }
                    seats(&manager, &project_root)
                },
                |body| jev_http::classify(&owned, body),
            )?;
            let target = selection.target.agent_id.clone();
            let fallback = selection.outcome == Outcome::Sentinel;
            // Exactly once per fresh decision, before recording delivery. Cached Retry exits above.
            record_event(routing, EventData::Selection(selection), &emit);
            let (prepared, changed) = app
                .state::<crate::temporal_context::TemporalContextManager>()
                .prepare_puppeteer_composer(
                    &root,
                    &request.message_id,
                    &request.timestamp,
                    &target,
                    &request.payload,
                    chrono::Local::now(),
                )
                .map_err(|_| LocalError::new("dance_record_failed"))?;
            if routing.sentinel.follow(&target) {
                emit_sentinel(&app, &project_root, routing.sentinel.current.clone());
            }
            if !changed.is_empty() {
                crate::violet::emit_room_changed(&app, Path::new(&project_root), "puppeteer", changed);
            }
            let routed = Routed {
                message_id: request.message_id.clone(),
                target_agent_id: target,
                fallback,
                recorded: true,
                prepared: Some(prepared),
            };
            Ok(routed)
        });
        if let Err(error) = &result {
            if error.code == "jev_auth" {
                if let Ok(settings) = jev::read(&OwnedDirectory::new(&crate::kota_home_dir())) {
                    let _ = app.emit("local-jev://changed", settings);
                }
            }
        }
        result
    })
    .await
    .map_err(|_| LocalError::new("dance_failed"))?
}

#[cfg(test)]
#[path = "puppeteer_tests.rs"]
mod tests;
