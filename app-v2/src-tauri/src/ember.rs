use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::agent_bus::{AgentBusManager, AgentBusSendRequest};
use crate::integrations::IntegrationManager;
use crate::pty::PtyManager;
use crate::violet::ActorMessageRecord;

const KOTA_HOME_DIR: &str = "Kota";
const STORE_SCHEMA: &str = "kota.ember.schedules.v1";
const DISPATCH_SCHEMA: &str = "kota.ember.dispatch.v1";
const CHANGED_EVENT: &str = "ember-schedules-changed";
const EMBER_ACTOR_ID: &str = "ember";
const EMBER_ACTOR_NAME: &str = "Ember";
// Must match HUMAN_TELEGRAM_TARGET_ID in src/ember-config.ts.
const HUMAN_TELEGRAM_TARGET_ID: &str = "__kota_human_telegram__";
const MAX_DRAFTS: usize = 20;
const MAX_SCHEDULES: usize = 40;
const MAX_HISTORY: usize = 120;
const DELIVERY_GRACE_SECONDS: i64 = 120;
const NOT_DELIVERED: &str = "Not Delivered";

#[derive(Debug, Default)]
struct EmberDeliveryOutcome {
    delivered: usize,
    failed: Vec<String>,
}

impl EmberDeliveryOutcome {
    fn partial_error(&self) -> Option<String> {
        if self.failed.is_empty() {
            None
        } else {
            Some(format!("Some targets failed: {}", self.failed.join("; ")))
        }
    }
}

#[derive(Default)]
pub struct EmberManager {
    dispatch_watchers: Mutex<HashMap<PathBuf, EmberDispatchWatch>>,
}

struct EmberDispatchWatch {
    _watcher: notify::RecommendedWatcher,
    watched_path: PathBuf,
}

struct EmberDispatchFileLock {
    path: PathBuf,
    token: String,
}

impl Drop for EmberDispatchFileLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path)
            .ok()
            .is_some_and(|token| token == self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberProjectRequest {
    #[serde(default)]
    pub project_root: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberStateSaveRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    pub state: EmberStateFile,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberHumanReminderRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    pub event_id: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberHumanReminderResult {
    pub event_id: String,
    pub delivered: bool,
    pub room_status: String,
    pub telegram_status: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberSchedulerTickRequest {
    pub project_roots: Vec<String>,
    #[serde(default)]
    pub working_agent_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberSchedulerTickResult {
    pub checked_projects: usize,
    pub fired: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberSchedulesChangedPayload {
    pub project_root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberStateFile {
    #[serde(default = "default_schema")]
    pub schema: String,
    #[serde(default)]
    pub drafts: Vec<serde_json::Value>,
    #[serde(default)]
    pub schedules: Vec<EmberSchedule>,
    #[serde(default)]
    pub history: Vec<EmberHistoryRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmberDraft {
    id: String,
    text: String,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberActorRef {
    pub kind: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberSchedule {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub target_agent_id: String,
    #[serde(default)]
    pub target_agent_name: String,
    #[serde(default)]
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub target_agent_names: Vec<String>,
    pub mode: String,
    #[serde(default)]
    pub delay_amount: Option<u32>,
    #[serde(default)]
    pub delay_unit: Option<String>,
    #[serde(default)]
    pub at_date_time: Option<String>,
    #[serde(default)]
    pub time_of_day: Option<String>,
    #[serde(default)]
    pub interval_hours: Option<u32>,
    #[serde(default)]
    pub wait_for_idle: Option<bool>,
    #[serde(default)]
    pub repeat_enabled: Option<bool>,
    #[serde(default)]
    pub repeat_amount: Option<u32>,
    #[serde(default)]
    pub repeat_unit: Option<String>,
    #[serde(default)]
    pub repeat_kind: Option<String>,
    #[serde(default)]
    pub repeat_every_minutes: Option<u32>,
    #[serde(default)]
    pub repeat_week_days: Option<Vec<u32>>,
    #[serde(default)]
    pub repeat_every_weeks: Option<u32>,
    #[serde(default)]
    pub repeat_month_days: Option<Vec<String>>,
    #[serde(default)]
    pub repeat_every_months: Option<u32>,
    #[serde(default)]
    pub end_mode: Option<String>,
    #[serde(default)]
    pub end_after_count: Option<u32>,
    #[serde(default)]
    pub end_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub next_run_at: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub run_count: u32,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub created_by: Option<EmberActorRef>,
    #[serde(default)]
    pub updated_by: Option<EmberActorRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmberHistoryRecord {
    pub id: String,
    pub schedule_id: String,
    pub prompt: String,
    #[serde(default)]
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub target_agent_names: Vec<String>,
    pub sent_at: String,
    pub status: String,
    #[serde(default)]
    pub triggered_by: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub scheduled_for: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub missed_runs: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmberDispatchFile {
    schema: String,
    created_at: String,
    project_root: String,
    action: String,
    #[serde(default)]
    schedule_id: Option<String>,
    #[serde(default)]
    schedule: Option<EmberSchedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft: Option<EmberDraft>,
}

#[derive(Clone, Debug)]
struct CliSchedulePatch {
    targets: Option<Vec<String>>,
    text: Option<String>,
    timing: Option<CliTiming>,
    end_after: Option<u32>,
    end_at: Option<String>,
}

#[derive(Clone, Debug)]
enum CliTiming {
    Delay(String),
    At(String),
    Idle,
    Cron(String),
}

impl EmberManager {
    pub fn refresh_dispatch_watcher(&self, app: &AppHandle, project_root: &Path) -> Result<()> {
        let outbox = dispatch_outbox_dir(project_root);
        fs::create_dir_all(&outbox)?;
        self.process_dispatch_outbox(app, project_root)?;

        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut watchers = self
            .dispatch_watchers
            .lock()
            .map_err(|_| anyhow!("ember dispatch watcher lock poisoned"))?;
        if watchers
            .get(&key)
            .is_some_and(|existing| existing.watched_path == outbox)
        {
            return Ok(());
        }

        let app_handle = app.clone();
        let project_root_for_callback = project_root.to_path_buf();
        let last_process = std::sync::Arc::new(Mutex::new(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
        ));
        let last_process_for_callback = std::sync::Arc::clone(&last_process);
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else {
                    return;
                };
                if event.paths.is_empty() {
                    return;
                }
                let now = Instant::now();
                if let Ok(mut last) = last_process_for_callback.lock() {
                    if now.duration_since(*last) < Duration::from_millis(250) {
                        return;
                    }
                    *last = now;
                }
                let app_for_process = app_handle.clone();
                let project_root_for_process = project_root_for_callback.clone();
                thread::spawn(move || {
                    let ember = app_for_process.state::<EmberManager>();
                    if let Err(err) =
                        ember.process_dispatch_outbox(&app_for_process, &project_root_for_process)
                    {
                        crate::kota_debug_log(&format!(
                            "[ember] process dispatch outbox failed: {err}"
                        ));
                    }
                });
            })?;
        watcher.watch(&outbox, RecursiveMode::NonRecursive)?;
        watchers.insert(
            key,
            EmberDispatchWatch {
                _watcher: watcher,
                watched_path: outbox,
            },
        );
        Ok(())
    }

    pub fn process_dispatch_outbox(&self, app: &AppHandle, project_root: &Path) -> Result<()> {
        let outbox = dispatch_outbox_dir(project_root);
        if !outbox.is_dir() {
            return Ok(());
        }
        let processing = dispatch_processing_dir(project_root);
        let delivered = dispatch_delivered_dir(project_root);
        let failed = dispatch_failed_dir(project_root);
        fs::create_dir_all(&processing)?;
        fs::create_dir_all(&delivered)?;
        fs::create_dir_all(&failed)?;

        let mut paths = fs::read_dir(&outbox)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();

        for path in paths {
            consume_dispatch_path(
                project_root,
                &path,
                &processing,
                &delivered,
                &failed,
                || emit_changed(app, project_root),
            )?;
        }
        Ok(())
    }
}

fn consume_dispatch_path(
    project_root: &Path,
    path: &Path,
    processing: &Path,
    delivered: &Path,
    failed: &Path,
    on_changed: impl FnOnce(),
) -> Result<()> {
    with_dispatch_file_lock(project_root, || {
        let Some(file_name) = path.file_name().map(|name| name.to_owned()) else {
            return Ok(());
        };
        let processing_path = processing.join(&file_name);
        if fs::rename(path, &processing_path).is_err() {
            return Ok(());
        }
        let result = process_dispatch_file_state(project_root, &processing_path);
        if result.is_ok() {
            on_changed();
        }
        let target_dir = if result.is_ok() { delivered } else { failed };
        let target_path = unique_path(target_dir.join(&file_name));
        let _ = fs::rename(&processing_path, &target_path);
        if let Err(err) = result {
            let _ = fs::write(target_path.with_extension("error.txt"), err.to_string());
        }
        Ok(())
    })
}

fn process_dispatch_file_state(project_root: &Path, path: &Path) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let request = serde_json::from_str::<EmberDispatchFile>(&text)?;
    if request.schema != DISPATCH_SCHEMA {
        bail!("unsupported Ember dispatch schema: {}", request.schema);
    }
    let dispatch_project_root = PathBuf::from(&request.project_root);
    if !paths_same(project_root, &dispatch_project_root) {
        bail!(
            "Ember dispatch project mismatch: {} != {}",
            dispatch_project_root.display(),
            project_root.display()
        );
    }
    let mut state = load_state(project_root)?;
    apply_dispatch_to_state(project_root, &mut state, request, actor_agent())?;
    save_state(project_root, &state)?;
    Ok(())
}

pub fn load_project_state(project_root: &Path) -> Result<EmberStateFile> {
    load_state(project_root)
}

pub fn save_project_state(
    app: &AppHandle,
    project_root: &Path,
    mut state: EmberStateFile,
) -> Result<EmberStateFile> {
    state.schema = STORE_SCHEMA.into();
    state.drafts.truncate(MAX_DRAFTS);
    state.history.truncate(MAX_HISTORY);
    state.schedules.truncate(MAX_SCHEDULES);
    for schedule in &mut state.schedules {
        normalize_schedule(project_root, schedule, actor_human())?;
    }
    save_state(project_root, &state)?;
    emit_changed(app, project_root);
    Ok(state)
}

pub fn scheduler_tick(
    app: &AppHandle,
    project_roots: &[String],
    working_agent_ids: &[String],
) -> Result<EmberSchedulerTickResult> {
    let mut checked_projects = 0usize;
    let mut fired = 0usize;
    let mut failed = 0usize;
    let working = working_agent_ids
        .iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>();
    for project_root_raw in project_roots {
        let project_root = PathBuf::from(project_root_raw);
        if project_root_raw.trim().is_empty() || !project_root.join("project-memory").is_dir() {
            continue;
        }
        checked_projects += 1;
        let ember = app.state::<EmberManager>();
        let _ = ember.refresh_dispatch_watcher(app, &project_root);
        let mut state = load_state(&project_root)?;
        let now = Utc::now();
        let overdue_count = reconcile_overdue_not_delivered(&mut state, now);
        failed = failed.saturating_add(overdue_count);
        let mut changed = overdue_count > 0;
        let mut emit_change = changed;
        let due_ids = state
            .schedules
            .iter()
            .filter(|schedule| schedule.status == "scheduled")
            .filter(|schedule| {
                parse_rfc3339(&schedule.next_run_at).is_some_and(|due| due <= Utc::now())
            })
            .filter(|schedule| {
                if schedule.wait_for_idle.unwrap_or(false) {
                    !schedule_has_working_agent_target(schedule, &working)
                } else {
                    true
                }
            })
            .map(|schedule| schedule.id.clone())
            .collect::<Vec<_>>();
        for schedule_id in due_ids {
            let Some(schedule) = state
                .schedules
                .iter()
                .find(|candidate| candidate.id == schedule_id)
                .cloned()
            else {
                continue;
            };
            let result = deliver_schedule(app, &project_root, &schedule);
            let now = Utc::now();
            match result {
                Ok(outcome) => {
                    fired += 1;
                    state.history.insert(
                        0,
                        history_for_schedule(
                            &schedule,
                            "delivered",
                            now,
                            outcome.partial_error(),
                            None,
                        ),
                    );
                    mark_schedule_delivered(&mut state, &schedule, now);
                    state.history.truncate(MAX_HISTORY);
                    changed = true;
                    emit_change = true;
                }
                Err(err) => {
                    failed += 1;
                    crate::kota_debug_log(&format!(
                        "[ember] delivery attempt failed for {}: {err}",
                        schedule.id
                    ));
                    state.history.insert(
                        0,
                        history_for_schedule(
                            &schedule,
                            "failed",
                            now,
                            Some(NOT_DELIVERED.into()),
                            None,
                        ),
                    );
                    mark_schedule_failed(&mut state, &schedule.id, NOT_DELIVERED, now);
                    state.history.truncate(MAX_HISTORY);
                    changed = true;
                    emit_change = true;
                }
            }
        }
        if changed {
            save_state(&project_root, &state)?;
            if emit_change {
                emit_changed(app, &project_root);
            }
        }
    }
    Ok(EmberSchedulerTickResult {
        checked_projects,
        fired,
        failed,
    })
}

fn deliver_schedule(
    app: &AppHandle,
    project_root: &Path,
    schedule: &EmberSchedule,
) -> Result<EmberDeliveryOutcome> {
    let target_ids = target_ids(schedule);
    if target_ids.is_empty() {
        bail!("Ember schedule has no targets");
    }
    let manager = app.state::<IntegrationManager>();
    let pty = app.state::<PtyManager>();
    let agent_bus = app.state::<AgentBusManager>();
    let target_names = target_names(schedule);
    let mut outcome = EmberDeliveryOutcome::default();
    for (index, target) in target_ids.iter().enumerate() {
        let label = target_names
            .get(index)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| target.clone());
        if is_human_telegram_target(target) {
            let event_id = ember_event_id("reminder", &schedule.id, target);
            match deliver_human_reminder(
                app,
                project_root,
                EmberHumanReminderRequest {
                    project_root: Some(path_string(project_root)),
                    event_id,
                    text: schedule.text.clone(),
                },
            ) {
                Ok(result) if result.delivered => {
                    outcome.delivered += 1;
                    outcome.failed.extend(
                        result
                            .warnings
                            .into_iter()
                            .map(|warning| format!("{label}: {warning}")),
                    );
                }
                Ok(result) => outcome.failed.push(format!(
                    "{label}: {}",
                    if result.warnings.is_empty() {
                        "Could not deliver the reminder to the human target.".into()
                    } else {
                        result.warnings.join("; ")
                    }
                )),
                Err(err) => outcome.failed.push(format!("{label}: {err}")),
            }
            continue;
        }
        let launch_request =
            crate::resolve_project_agent_launch(&manager, Some(&path_string(project_root)), target)
                .ok();
        let event_id = ember_event_id("reminder", &schedule.id, target);
        let result = agent_bus.send_request(
            app,
            &pty,
            project_root,
            AgentBusSendRequest {
                project_root: Some(path_string(project_root)),
                sender_agent_id: Some(EMBER_ACTOR_ID.into()),
                sender_name: Some(EMBER_ACTOR_NAME.into()),
                target: target.clone(),
                intent: Some("reminder".into()),
                text: render_reminder_prompt(schedule),
                event_id: Some(event_id.clone()),
                dedupe_key: Some(event_id),
            },
            launch_request,
        );
        match result {
            Ok(result) if result.submitted || result.duplicate => outcome.delivered += 1,
            Ok(result) => outcome.failed.push(format!(
                "{label}: {}",
                result
                    .skipped_reason
                    .unwrap_or_else(|| format!("Could not reach {target}"))
            )),
            Err(err) => outcome.failed.push(format!("{label}: {err}")),
        }
    }
    if outcome.delivered == 0 {
        bail!(
            "{}",
            if outcome.failed.is_empty() {
                "Ember schedule could not reach any target.".into()
            } else {
                outcome.failed.join("; ")
            }
        );
    }
    Ok(outcome)
}

fn render_reminder_prompt(schedule: &EmberSchedule) -> String {
    render_reminder_text(&schedule.text)
}

fn render_reminder_text(text: &str) -> String {
    format!("Ember scheduled prompt\n\n{}", text.trim())
}

pub(crate) fn deliver_human_reminder(
    app: &AppHandle,
    project_root: &Path,
    request: EmberHumanReminderRequest,
) -> Result<EmberHumanReminderResult> {
    let event_id = request.event_id.trim().to_string();
    if event_id.is_empty() {
        bail!("Ember human reminder requires an event id");
    }
    if event_id.len() > 256
        || !event_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        bail!("Ember human reminder event id is invalid");
    }
    let text = request.text.trim().to_string();
    if text.is_empty() {
        bail!("Ember human reminder requires text");
    }

    let agent_bus = app.state::<AgentBusManager>();
    let room_result = agent_bus.record_actor_notice(
        app,
        project_root,
        ActorMessageRecord {
            actor_id: EMBER_ACTOR_ID.into(),
            actor_name: EMBER_ACTOR_NAME.into(),
            text: render_reminder_text(&text),
            target_agent_ids: vec![HUMAN_TELEGRAM_TARGET_ID.into()],
            event_id: event_id.clone(),
            actor_intent: Some("reminder".into()),
        },
        Some(&event_id),
    );
    let mut warnings = Vec::new();
    let room_status = match room_result {
        Ok(result) if result.duplicate => "duplicate",
        Ok(result) if result.recorded => "delivered",
        Ok(_) => "failed",
        Err(err) => {
            warnings.push(format!("Room delivery failed: {err}"));
            "failed"
        }
    }
    .to_string();

    let mut telegram_status = "skipped".to_string();
    if crate::laughing_man::ember_reminder_route_configured() {
        let manager = app.state::<IntegrationManager>();
        let (project_id, project_name) = project_log_info(&manager, project_root);
        match crate::laughing_man::send_ember_reminder(
            crate::laughing_man::LmEmberReminderRequest {
                event_id: event_id.clone(),
                text,
                project_id: Some(project_id),
                project_name: Some(project_name),
                project_root: Some(path_string(project_root)),
            },
        ) {
            Ok(()) => telegram_status = "delivered".into(),
            Err(err) => {
                telegram_status = "failed".into();
                warnings.push(format!("Laughing Man delivery failed: {err}"));
            }
        }
    }

    let delivered =
        matches!(room_status.as_str(), "delivered" | "duplicate") || telegram_status == "delivered";
    Ok(EmberHumanReminderResult {
        event_id,
        delivered,
        room_status,
        telegram_status,
        warnings,
    })
}

fn project_log_info(manager: &IntegrationManager, project_root: &Path) -> (String, String) {
    if let Ok(workspaces) = manager.list_workspaces() {
        for workspace in workspaces {
            if paths_same(Path::new(&workspace.local_root), project_root)
                || paths_same(Path::new(&workspace.source_dir), project_root)
            {
                let project_name =
                    crate::bbs::display_project_name_with_fallback(&workspace.project_id, None);
                return (workspace.project_id, project_name);
            }
        }
    }
    let fallback = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("current")
        .to_string();
    (fallback.clone(), fallback)
}

fn mark_schedule_delivered(
    state: &mut EmberStateFile,
    schedule: &EmberSchedule,
    now: chrono::DateTime<Utc>,
) {
    if let Some(index) = state
        .schedules
        .iter()
        .position(|candidate| candidate.id == schedule.id)
    {
        let mut next = state.schedules[index].clone();
        let completed_run_count = next.run_count.saturating_add(1);
        next.run_count = completed_run_count;
        next.last_run_at = Some(now.to_rfc3339());
        next.updated_at = now.to_rfc3339();
        next.error = None;
        if schedule_should_continue(&next, completed_run_count, now) {
            next.status = "scheduled".into();
            next.next_run_at = next_run_after(&next, now).unwrap_or_else(|| now.to_rfc3339());
            state.schedules[index] = next;
        } else {
            state.schedules.remove(index);
        }
    }
}

fn mark_schedule_failed(
    state: &mut EmberStateFile,
    schedule_id: &str,
    error: &str,
    now: chrono::DateTime<Utc>,
) {
    if let Some(schedule) = state
        .schedules
        .iter_mut()
        .find(|candidate| candidate.id == schedule_id)
    {
        schedule.updated_at = now.to_rfc3339();
        schedule.status = "failed".into();
        schedule.error = Some(error.to_string());
    }
}

fn reconcile_overdue_not_delivered(
    state: &mut EmberStateFile,
    now: chrono::DateTime<Utc>,
) -> usize {
    let overdue_before = now - chrono::Duration::seconds(DELIVERY_GRACE_SECONDS);
    let mut missed = Vec::new();
    for schedule in &mut state.schedules {
        if schedule.status != "scheduled" || schedule.wait_for_idle.unwrap_or(false) {
            continue;
        }
        let Some(next_due) = parse_rfc3339(&schedule.next_run_at) else {
            continue;
        };
        if next_due > overdue_before {
            continue;
        }
        let missed_count = count_missed_runs(schedule, overdue_before).max(1);
        missed.push((schedule.clone(), missed_count, next_due));
        if schedule_should_continue(
            schedule,
            schedule.run_count.saturating_add(missed_count),
            now,
        ) {
            schedule.run_count = schedule.run_count.saturating_add(missed_count);
            schedule.last_run_at = Some(now.to_rfc3339());
            schedule.next_run_at =
                next_run_after(schedule, now).unwrap_or_else(|| now.to_rfc3339());
            schedule.updated_at = now.to_rfc3339();
            schedule.error = Some(NOT_DELIVERED.into());
        } else {
            schedule.run_count = schedule.run_count.saturating_add(missed_count);
            schedule.last_run_at = Some(now.to_rfc3339());
            schedule.updated_at = now.to_rfc3339();
            schedule.status = "failed".into();
            schedule.error = Some(NOT_DELIVERED.into());
        }
    }
    let missed_count = missed.len();
    for (schedule, missed_runs, scheduled_for) in missed {
        state.history.insert(
            0,
            history_for_schedule(
                &schedule,
                "failed",
                now,
                Some(NOT_DELIVERED.into()),
                Some((scheduled_for.to_rfc3339(), missed_runs)),
            ),
        );
    }
    state.history.truncate(MAX_HISTORY);
    missed_count
}

fn count_missed_runs(schedule: &EmberSchedule, now: chrono::DateTime<Utc>) -> u32 {
    if !is_repeating(schedule) {
        return 1;
    }
    let mut count = 0u32;
    let mut cursor = schedule.clone();
    while parse_rfc3339(&cursor.next_run_at).is_some_and(|due| due <= now) && count < 500 {
        count = count.saturating_add(1);
        cursor.run_count = cursor.run_count.saturating_add(1);
        let Some(next) = next_run_after(&cursor, parse_rfc3339(&cursor.next_run_at).unwrap_or(now))
        else {
            break;
        };
        cursor.next_run_at = next;
    }
    count.max(1)
}

fn history_for_schedule(
    schedule: &EmberSchedule,
    status: &str,
    now: chrono::DateTime<Utc>,
    error: Option<String>,
    missed: Option<(String, u32)>,
) -> EmberHistoryRecord {
    EmberHistoryRecord {
        id: mint_id("history"),
        schedule_id: schedule.id.clone(),
        prompt: schedule.text.trim().to_string(),
        target_agent_ids: target_ids(schedule),
        target_agent_names: target_names(schedule),
        sent_at: now.to_rfc3339(),
        status: status.into(),
        triggered_by: Some("schedule".into()),
        error: error.clone(),
        scheduled_for: missed
            .as_ref()
            .map(|(scheduled_for, _)| scheduled_for.clone())
            .or_else(|| Some(schedule.next_run_at.clone())),
        started_at: if status == "delivered" {
            Some(now.to_rfc3339())
        } else {
            None
        },
        finished_at: Some(now.to_rfc3339()),
        reason: error,
        missed_runs: missed.map(|(_, count)| count),
    }
}

fn schedule_should_continue(
    schedule: &EmberSchedule,
    completed_run_count: u32,
    now: chrono::DateTime<Utc>,
) -> bool {
    if !is_repeating(schedule) {
        return false;
    }
    match schedule.end_mode.as_deref().unwrap_or("never") {
        "after" => completed_run_count < schedule.end_after_count.unwrap_or(1),
        "at" => schedule
            .end_at
            .as_deref()
            .and_then(parse_rfc3339)
            .is_none_or(|end_at| now < end_at),
        _ => true,
    }
}

fn next_run_after(schedule: &EmberSchedule, now: chrono::DateTime<Utc>) -> Option<String> {
    if schedule.repeat_enabled.unwrap_or(false) {
        match schedule.repeat_kind.as_deref().unwrap_or("fixed") {
            "weekly" => return next_weekly_run(schedule, now).map(|dt| dt.to_rfc3339()),
            "monthly" => return next_monthly_run(schedule, now).map(|dt| dt.to_rfc3339()),
            _ => {
                let minutes = schedule
                    .repeat_every_minutes
                    .or_else(|| {
                        schedule.repeat_amount.map(|amount| {
                            amount.saturating_mul(
                                match schedule.repeat_unit.as_deref().unwrap_or("days") {
                                    "minutes" => 1,
                                    "hours" => 60,
                                    _ => 1440,
                                },
                            )
                        })
                    })
                    .unwrap_or(1440)
                    .max(1);
                return Some((now + chrono::Duration::minutes(minutes as i64)).to_rfc3339());
            }
        }
    }
    match schedule.mode.as_str() {
        "daily" => next_daily_run(schedule.time_of_day.as_deref(), now).map(|dt| dt.to_rfc3339()),
        "interval" => Some(
            (now + chrono::Duration::hours(schedule.interval_hours.unwrap_or(4).max(1) as i64))
                .to_rfc3339(),
        ),
        _ => None,
    }
}

fn next_daily_run(
    time_of_day: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    let (hour, minute) = parse_time_of_day(time_of_day.unwrap_or("09:00"))?;
    let local_now = now.with_timezone(&Local);
    let date = local_now.date_naive();
    let mut candidate = local_datetime(date, hour, minute)?;
    if candidate <= local_now {
        candidate = local_datetime(date.succ_opt()?, hour, minute)?;
    }
    Some(candidate.with_timezone(&Utc))
}

fn next_weekly_run(
    schedule: &EmberSchedule,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    let days = schedule.repeat_week_days.clone().unwrap_or_else(|| vec![1]);
    let (hour, minute) = scheduled_local_time(schedule, now)?;
    let local_now = now.with_timezone(&Local);
    for offset in 0..370 {
        let date = local_now
            .date_naive()
            .checked_add_signed(chrono::Duration::days(offset))?;
        let weekday = date.weekday().num_days_from_sunday();
        if !days.contains(&weekday) {
            continue;
        }
        let candidate = local_datetime(date, hour, minute)?;
        if candidate > local_now {
            return Some(candidate.with_timezone(&Utc));
        }
    }
    None
}

fn next_monthly_run(
    schedule: &EmberSchedule,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    let days = schedule
        .repeat_month_days
        .clone()
        .unwrap_or_else(|| vec!["1".into()]);
    let every = schedule.repeat_every_months.unwrap_or(1).max(1);
    let (hour, minute) = scheduled_local_time(schedule, now)?;
    let local_now = now.with_timezone(&Local);
    for offset in 0..60 {
        let total_month = local_now.month0() as i32 + offset;
        let year = local_now.year() + total_month.div_euclid(12);
        let month = total_month.rem_euclid(12) as u32 + 1;
        let month_offset = offset as u32;
        if month_offset % every != 0 {
            continue;
        }
        let last_day = last_day_of_month(year, month)?;
        for day_token in &days {
            let day = if day_token == "last" {
                last_day
            } else {
                day_token.parse::<u32>().ok()?.min(last_day).max(1)
            };
            let date = NaiveDate::from_ymd_opt(year, month, day)?;
            let candidate = local_datetime(date, hour, minute)?;
            if candidate > local_now {
                return Some(candidate.with_timezone(&Utc));
            }
        }
    }
    None
}

fn scheduled_local_time(
    schedule: &EmberSchedule,
    now: chrono::DateTime<Utc>,
) -> Option<(u32, u32)> {
    if let Some((hour, minute)) = schedule.time_of_day.as_deref().and_then(parse_time_of_day) {
        return Some((hour, minute));
    }
    schedule
        .next_run_at
        .as_str()
        .pipe(parse_rfc3339)
        .map(|dt| dt.with_timezone(&Local))
        .map(|dt| (dt.hour(), dt.minute()))
        .or_else(|| {
            let local = now.with_timezone(&Local);
            Some((local.hour(), local.minute()))
        })
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    Some(next_month.pred_opt()?.day())
}

fn is_repeating(schedule: &EmberSchedule) -> bool {
    schedule.repeat_enabled.unwrap_or(false)
        || matches!(schedule.mode.as_str(), "daily" | "interval")
}

fn target_ids(schedule: &EmberSchedule) -> Vec<String> {
    let mut out = Vec::new();
    for id in schedule
        .target_agent_ids
        .iter()
        .chain(std::iter::once(&schedule.target_agent_id))
    {
        let id = id.trim();
        if !id.is_empty() && !out.iter().any(|existing: &String| existing == id) {
            out.push(id.to_string());
        }
    }
    out
}

fn is_human_telegram_target(target: &str) -> bool {
    target.trim() == HUMAN_TELEGRAM_TARGET_ID
}

fn schedule_has_working_agent_target(schedule: &EmberSchedule, working: &HashSet<String>) -> bool {
    target_ids(schedule)
        .iter()
        .filter(|agent_id| !is_human_telegram_target(agent_id))
        .any(|agent_id| working.contains(agent_id))
}

fn configured_account_user_display_name() -> Option<String> {
    crate::load_account_user_identity()
        .map(|identity| identity.name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty() && name != "User")
}

fn human_telegram_target_name_for(target: &str, configured_name: Option<&str>) -> Option<String> {
    let target = target.trim();
    if target == HUMAN_TELEGRAM_TARGET_ID {
        return Some(configured_name.unwrap_or("User").to_string());
    }
    let name = configured_name?.trim();
    if !name.is_empty() && target.eq_ignore_ascii_case(name) {
        return Some(name.to_string());
    }
    None
}

fn human_telegram_target_name(target: &str) -> Option<String> {
    let configured_name = configured_account_user_display_name();
    human_telegram_target_name_for(target, configured_name.as_deref())
}

fn validate_cli_schedule_targets(project_root: &Path, schedule: &EmberSchedule) -> Result<()> {
    let configured_name = configured_account_user_display_name();
    validate_cli_schedule_targets_for(project_root, schedule, configured_name.as_deref())
}

fn validate_cli_schedule_targets_for(
    project_root: &Path,
    schedule: &EmberSchedule,
    configured_human_name: Option<&str>,
) -> Result<()> {
    let agent_bus = AgentBusManager::default();
    for target in target_ids(schedule) {
        if human_telegram_target_name_for(&target, configured_human_name).is_some() {
            continue;
        }
        agent_bus
            .resolve_target_agent_id(project_root, &target)
            .map_err(|err| anyhow!("invalid Ember target {target:?}: {err}"))?;
    }
    Ok(())
}

fn target_names(schedule: &EmberSchedule) -> Vec<String> {
    let ids = target_ids(schedule);
    let mut names = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let name = schedule
            .target_agent_names
            .get(index)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .or_else(|| {
                if index == 0 && !schedule.target_agent_name.trim().is_empty() {
                    Some(schedule.target_agent_name.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| id.clone());
        names.push(name);
    }
    names
}

fn draft_id(value: &serde_json::Value) -> Option<&str> {
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

fn ember_drafts(state: &EmberStateFile) -> Vec<EmberDraft> {
    state
        .drafts
        .iter()
        .filter_map(|value| serde_json::from_value::<EmberDraft>(value.clone()).ok())
        .filter(|draft| !draft.id.trim().is_empty() && !draft.text.trim().is_empty())
        .collect()
}

fn find_draft(state: &EmberStateFile, id: &str) -> Result<Option<EmberDraft>> {
    let Some(value) = state
        .drafts
        .iter()
        .find(|value| draft_id(value) == Some(id))
    else {
        return Ok(None);
    };
    serde_json::from_value::<EmberDraft>(value.clone())
        .map(Some)
        .map_err(|err| anyhow!("invalid Ember draft {id}: {err}"))
}

fn normalize_draft(mut draft: EmberDraft) -> Result<EmberDraft> {
    draft.text = draft.text.trim().to_string();
    if draft.text.is_empty() {
        bail!("Ember draft requires note text");
    }
    if draft.id.trim().is_empty() {
        draft.id = mint_id("draft");
    }
    let now = Utc::now().to_rfc3339();
    if draft.created_at.trim().is_empty() {
        draft.created_at = now.clone();
    }
    if draft.updated_at.trim().is_empty() {
        draft.updated_at = now;
    }
    Ok(draft)
}

fn upsert_draft(state: &mut EmberStateFile, draft: EmberDraft) -> Result<()> {
    let value = serde_json::to_value(&draft)?;
    if let Some(existing) = state
        .drafts
        .iter_mut()
        .find(|candidate| draft_id(candidate) == Some(draft.id.as_str()))
    {
        *existing = value;
        return Ok(());
    }
    if state.drafts.len() >= MAX_DRAFTS {
        bail!(
            "Draft limit reached ({MAX_DRAFTS}). Delete one with kota-ember delete <draft-id> first."
        );
    }
    state.drafts.insert(0, value);
    Ok(())
}

fn upsert_schedule(state: &mut EmberStateFile, schedule: EmberSchedule) {
    if let Some(existing) = state
        .schedules
        .iter_mut()
        .find(|candidate| candidate.id == schedule.id)
    {
        *existing = schedule;
    } else {
        state.schedules.insert(0, schedule);
    }
    state.schedules.truncate(MAX_SCHEDULES);
}

fn normalize_schedule(
    project_root: &Path,
    schedule: &mut EmberSchedule,
    actor: EmberActorRef,
) -> Result<()> {
    schedule.text = schedule.text.trim().to_string();
    if schedule.text.is_empty() {
        bail!("Ember schedule requires prompt text");
    }
    if schedule.id.trim().is_empty() {
        schedule.id = mint_id("schedule");
    }
    let mut ids: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let existing_names = target_names(schedule);
    for (index, target) in target_ids(schedule).into_iter().enumerate() {
        if let Some(human_name) = human_telegram_target_name(&target) {
            if !ids.iter().any(|id| is_human_telegram_target(id)) {
                let name = existing_names
                    .get(index)
                    .filter(|name| !name.trim().is_empty() && !is_human_telegram_target(name))
                    .cloned()
                    .unwrap_or(human_name);
                ids.push(HUMAN_TELEGRAM_TARGET_ID.into());
                names.push(name);
            }
            continue;
        }
        match resolve_agent_identity(project_root, &target) {
            Ok(identity) => {
                if !ids.contains(&identity.agent_id) {
                    ids.push(identity.agent_id);
                    names.push(identity.display_name);
                }
            }
            Err(_) => {
                if !ids.contains(&target) {
                    ids.push(target.clone());
                    names.push(target);
                }
            }
        }
    }
    if ids.is_empty() {
        bail!("Ember schedule requires at least one target");
    }
    schedule.target_agent_id = ids[0].clone();
    schedule.target_agent_name = names[0].clone();
    schedule.target_agent_ids = ids;
    schedule.target_agent_names = names;
    schedule.mode = normalize_mode(&schedule.mode);
    let now = Utc::now();
    if parse_rfc3339(&schedule.next_run_at).is_none() {
        schedule.next_run_at = compute_initial_next_run(schedule, now)?;
    }
    if schedule.created_at.trim().is_empty() {
        schedule.created_at = now.to_rfc3339();
    }
    if schedule.updated_at.trim().is_empty() {
        schedule.updated_at = now.to_rfc3339();
    }
    if schedule.status.trim().is_empty() {
        schedule.status = "scheduled".into();
    }
    if schedule.created_by.is_none() {
        schedule.created_by = Some(actor.clone());
    }
    if schedule.updated_by.is_none() {
        schedule.updated_by = Some(actor);
    }
    Ok(())
}

fn normalize_mode(mode: &str) -> String {
    match mode {
        "idle" | "delay" | "at" | "daily" | "interval" => mode.into(),
        _ => "delay".into(),
    }
}

fn compute_initial_next_run(
    schedule: &EmberSchedule,
    now: chrono::DateTime<Utc>,
) -> Result<String> {
    let next = match schedule.mode.as_str() {
        "idle" => now,
        "delay" => {
            let amount = schedule.delay_amount.unwrap_or(10).max(1) as i64;
            let unit = schedule.delay_unit.as_deref().unwrap_or("minutes");
            now + match unit {
                "days" => chrono::Duration::days(amount),
                "hours" => chrono::Duration::hours(amount),
                _ => chrono::Duration::minutes(amount),
            }
        }
        "at" => parse_local_datetime_or_rfc3339(schedule.at_date_time.as_deref().unwrap_or(""))
            .ok_or_else(|| anyhow!("invalid --at datetime"))?,
        "daily" => next_daily_run(schedule.time_of_day.as_deref(), now)
            .ok_or_else(|| anyhow!("invalid daily time"))?,
        "interval" => {
            now + chrono::Duration::hours(schedule.interval_hours.unwrap_or(4).max(1) as i64)
        }
        _ => now + chrono::Duration::minutes(10),
    };
    Ok(next.to_rfc3339())
}

fn load_state(project_root: &Path) -> Result<EmberStateFile> {
    let path = state_path(project_root);
    if !path.is_file() {
        return Ok(empty_state());
    }
    let mut state = serde_json::from_slice::<EmberStateFile>(&fs::read(&path)?)?;
    state.schema = STORE_SCHEMA.into();
    state.schedules.truncate(MAX_SCHEDULES);
    state.history.truncate(MAX_HISTORY);
    for schedule in &mut state.schedules {
        normalize_legacy_not_delivered(&mut schedule.error);
    }
    for record in &mut state.history {
        normalize_legacy_not_delivered(&mut record.error);
        normalize_legacy_not_delivered(&mut record.reason);
    }
    Ok(state)
}

fn normalize_legacy_not_delivered(error: &mut Option<String>) {
    if error.as_deref() == Some("app not running") {
        *error = Some(NOT_DELIVERED.into());
    }
}

fn load_state_with_pending_dispatch(project_root: &Path) -> Result<EmberStateFile> {
    let mut state = load_state(project_root)?;
    apply_pending_dispatch_dir(project_root, &mut state, &dispatch_processing_dir(project_root))?;
    apply_pending_dispatch_dir(project_root, &mut state, &dispatch_outbox_dir(project_root))?;
    Ok(state)
}

fn apply_pending_dispatch_dir(
    project_root: &Path,
    state: &mut EmberStateFile,
    dir: &Path,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut paths = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        let request = serde_json::from_str::<EmberDispatchFile>(&text)?;
        apply_dispatch_to_state(project_root, state, request, actor_agent())?;
    }
    Ok(())
}

fn apply_dispatch_to_state(
    project_root: &Path,
    state: &mut EmberStateFile,
    request: EmberDispatchFile,
    actor: EmberActorRef,
) -> Result<()> {
    if request.schema != DISPATCH_SCHEMA {
        bail!("unsupported Ember dispatch schema: {}", request.schema);
    }
    let dispatch_project_root = PathBuf::from(&request.project_root);
    if !paths_same(project_root, &dispatch_project_root) {
        bail!(
            "Ember dispatch project mismatch: {} != {}",
            dispatch_project_root.display(),
            project_root.display()
        );
    }
    match request.action.as_str() {
        "upsert" => {
            let mut schedule = request
                .schedule
                .ok_or_else(|| anyhow!("Ember upsert dispatch missing schedule"))?;
            normalize_schedule(project_root, &mut schedule, actor)?;
            upsert_schedule(state, schedule);
        }
        "delete" => {
            let schedule_id = request
                .schedule_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Ember delete dispatch missing scheduleId"))?;
            state.schedules.retain(|schedule| schedule.id != schedule_id);
        }
        "upsert-draft" => {
            let draft = request
                .draft
                .ok_or_else(|| anyhow!("Ember draft upsert dispatch missing draft"))?;
            upsert_draft(state, normalize_draft(draft)?)?;
        }
        "delete-draft" => {
            let id = request
                .draft_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Ember draft delete dispatch missing draftId"))?;
            state.drafts.retain(|draft| draft_id(draft) != Some(id));
        }
        other => bail!("unknown Ember dispatch action: {other}"),
    }
    Ok(())
}

fn save_state(project_root: &Path, state: &EmberStateFile) -> Result<()> {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn empty_state() -> EmberStateFile {
    EmberStateFile {
        schema: STORE_SCHEMA.into(),
        drafts: Vec::new(),
        schedules: Vec::new(),
        history: Vec::new(),
    }
}

fn emit_changed(app: &AppHandle, project_root: &Path) {
    let _ = app.emit(
        CHANGED_EVENT,
        EmberSchedulesChangedPayload {
            project_root: path_string(project_root),
        },
    );
}

fn state_path(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join("ember")
        .join("schedules.json")
}

fn dispatch_outbox_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".ember")
        .join("outbox")
}

fn dispatch_processing_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".ember")
        .join("processing")
}

fn dispatch_delivered_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".ember")
        .join("delivered")
}

fn dispatch_failed_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".ember")
        .join("failed")
}

fn dispatch_lock_path(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".ember")
        .join("dispatch.lock")
}

fn acquire_dispatch_file_lock(project_root: &Path) -> Result<EmberDispatchFileLock> {
    let path = dispatch_lock_path(project_root);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid Ember dispatch lock path: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let token = Uuid::new_v4().to_string();
    for attempt in 0..=100 {
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(err) = file.write_all(token.as_bytes()) {
                    let _ = fs::remove_file(&path);
                    return Err(err.into());
                }
                return Ok(EmberDispatchFileLock { path, token });
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(10 * 60));
                if stale {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                if attempt == 100 {
                    bail!("Ember dispatch is busy; retry the command");
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!()
}

fn with_dispatch_file_lock<T>(project_root: &Path, op: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = acquire_dispatch_file_lock(project_root)?;
    op()
}

pub fn install_cli_shim() -> Result<PathBuf> {
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
        .join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join("kota-ember");
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("kota-ember"));
            candidates.push(parent.join("../Resources/kota-ember"));
            if let Some(triple) = current_target_triple_guess() {
                candidates.push(parent.join(format!("kota-ember-{triple}")));
                candidates.push(parent.join(format!("../Resources/kota-ember-{triple}")));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/kota-ember"));
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/kota-ember"));

    let mut script = String::from("#!/bin/sh\nset -eu\n");
    script.push_str("if [ -n \"${KOTA_EMBER_BIN:-}\" ] && [ -x \"$KOTA_EMBER_BIN\" ]; then exec \"$KOTA_EMBER_BIN\" \"$@\"; fi\n");
    for candidate in candidates {
        script.push_str("if [ -x ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" ]; then exec ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" \"$@\"; fi\n");
    }
    script.push_str("echo 'kota-ember binary is not installed. Build it with: cargo build --bin kota-ember' >&2\nexit 127\n");
    fs::write(&shim, script)?;
    make_executable(&shim)?;
    Ok(shim)
}

pub fn run_cli() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty()
        || matches!(
            args.first().map(String::as_str),
            Some("help" | "--help" | "-h")
        )
    {
        print_cli_usage();
        return Ok(());
    }
    let command = args.remove(0);
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print_cli_usage();
        return Ok(());
    }
    match command.as_str() {
        "add" => cli_add(args),
        "list" => cli_list(args),
        "show" => cli_show(args),
        "update" => cli_update(args),
        "delete" => cli_delete(args),
        "install-shim" => {
            let path = install_cli_shim()?;
            println!("{}", path.display());
            Ok(())
        }
        _ => {
            print_cli_usage();
            bail!("unknown kota-ember command: {command}");
        }
    }
}

fn cli_add(args: Vec<String>) -> Result<()> {
    let project_root = cli_project_root(&args)?;
    if args.iter().any(|arg| arg == "--draft") {
        return cli_add_draft(&project_root, &args);
    }
    let patch = parse_cli_patch(args, true)?;
    let text = patch
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("kota-ember add requires prompt body on stdin"))?;
    let targets = patch
        .targets
        .clone()
        .ok_or_else(|| anyhow!("kota-ember add requires --to"))?;
    let now = Utc::now();
    let mut schedule = EmberSchedule {
        id: mint_id("schedule"),
        text: text.into(),
        target_agent_id: targets[0].clone(),
        target_agent_name: targets[0].clone(),
        target_agent_ids: targets,
        target_agent_names: Vec::new(),
        mode: "delay".into(),
        delay_amount: None,
        delay_unit: None,
        at_date_time: None,
        time_of_day: None,
        interval_hours: None,
        wait_for_idle: None,
        repeat_enabled: None,
        repeat_amount: None,
        repeat_unit: None,
        repeat_kind: None,
        repeat_every_minutes: None,
        repeat_week_days: None,
        repeat_every_weeks: None,
        repeat_month_days: None,
        repeat_every_months: None,
        end_mode: None,
        end_after_count: patch.end_after,
        end_at: patch.end_at,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        next_run_at: String::new(),
        last_run_at: None,
        run_count: 0,
        status: "scheduled".into(),
        error: None,
        created_by: Some(actor_agent()),
        updated_by: Some(actor_agent()),
    };
    apply_cli_timing(&mut schedule, patch.timing, now)?;
    validate_cli_schedule_targets(&project_root, &schedule)?;
    normalize_schedule(&project_root, &mut schedule, actor_agent())?;
    enqueue_dispatch(&project_root, "upsert", Some(schedule.clone()), None)?;
    println!("{}", schedule.id);
    println!("interpreted as: {}", schedule_interpretation(&schedule));
    Ok(())
}

fn cli_add_draft(project_root: &Path, args: &[String]) -> Result<()> {
    validate_draft_add_args(args)?;
    let text = read_stdin_body(true)?
        .ok_or_else(|| anyhow!("kota-ember add --draft requires note text on stdin"))?;
    let draft = enqueue_new_draft(project_root, text)?;
    println!("{}", draft.id);
    Ok(())
}

fn enqueue_new_draft(project_root: &Path, text: String) -> Result<EmberDraft> {
    with_dispatch_file_lock(project_root, || {
        let state = load_state_with_pending_dispatch(project_root)?;
        if state.drafts.len() >= MAX_DRAFTS {
            bail!(
                "Draft limit reached ({MAX_DRAFTS}). Delete one with kota-ember delete <draft-id> first."
            );
        }
        let created_at = Utc::now();
        let timestamp = created_at.to_rfc3339();
        let draft = normalize_draft(EmberDraft {
            id: mint_id("draft"),
            text,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        })?;
        enqueue_draft_dispatch_at_unlocked(
            project_root,
            "upsert-draft",
            Some(draft.clone()),
            None,
            created_at,
        )?;
        Ok(draft)
    })
}

fn cli_list(args: Vec<String>) -> Result<()> {
    let json = args.iter().any(|arg| arg == "--json");
    let project_root = cli_project_root(&args)?;
    if args.iter().any(|arg| arg == "--draft") {
        validate_draft_list_args(&args)?;
        let state = with_dispatch_file_lock(&project_root, || {
            load_state_with_pending_dispatch(&project_root)
        })?;
        let drafts = ember_drafts(&state);
        if json {
            println!("{}", serde_json::to_string_pretty(&drafts)?);
            return Ok(());
        }
        if drafts.is_empty() {
            println!("No Ember drafts.");
            return Ok(());
        }
        for draft in drafts {
            println!(
                "{}  {}",
                draft.id,
                draft
                    .text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(());
    }
    let state = load_state_with_pending_dispatch(&project_root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&state.schedules)?);
        return Ok(());
    }
    if state.schedules.is_empty() {
        println!("No Ember schedules.");
        return Ok(());
    }
    for schedule in state.schedules {
        println!(
            "{}  {}  {}",
            schedule.id,
            schedule_interpretation(&schedule),
            target_names(&schedule).join(", ")
        );
    }
    Ok(())
}

fn cli_show(args: Vec<String>) -> Result<()> {
    let project_root = cli_project_root(&args)?;
    let id = cli_item_id(&args)
        .ok_or_else(|| anyhow!("usage: kota-ember show <schedule-or-draft-id>"))?;
    if is_draft_id(id) {
        let state = with_dispatch_file_lock(&project_root, || {
            load_state_with_pending_dispatch(&project_root)
        })?;
        let draft =
            find_draft(&state, id)?.ok_or_else(|| anyhow!("Ember draft not found: {id}"))?;
        println!("{}", serde_json::to_string_pretty(&draft)?);
        return Ok(());
    }
    let state = load_state_with_pending_dispatch(&project_root)?;
    let schedule = state
        .schedules
        .iter()
        .find(|schedule| schedule.id == *id)
        .ok_or_else(|| anyhow!("Ember schedule not found: {id}"))?;
    println!("{}", serde_json::to_string_pretty(schedule)?);
    Ok(())
}

fn cli_update(args: Vec<String>) -> Result<()> {
    let project_root = cli_project_root(&args)?;
    let id = cli_item_id(&args)
        .cloned()
        .ok_or_else(|| anyhow!("usage: kota-ember update <schedule-or-draft-id> [options]"))?;
    if is_draft_id(&id) {
        return cli_update_draft(&project_root, &id, &args);
    }
    let patch = parse_cli_patch(args.into_iter().filter(|arg| arg != &id).collect(), false)?;
    let state = load_state_with_pending_dispatch(&project_root)?;
    let mut schedule = state
        .schedules
        .into_iter()
        .find(|schedule| schedule.id == id)
        .ok_or_else(|| anyhow!("Ember schedule not found: {id}"))?;
    if let Some(text) = patch.text {
        if !text.trim().is_empty() {
            schedule.text = text.trim().into();
        }
    }
    if let Some(targets) = patch.targets {
        schedule.target_agent_ids = targets;
    }
    if patch.timing.is_some() {
        apply_cli_timing(&mut schedule, patch.timing, Utc::now())?;
    }
    if let Some(end_after) = patch.end_after {
        schedule.end_mode = Some("after".into());
        schedule.end_after_count = Some(end_after);
    }
    if let Some(end_at) = patch.end_at {
        schedule.end_mode = Some("at".into());
        schedule.end_at = Some(
            parse_local_datetime_or_rfc3339(&end_at)
                .ok_or_else(|| anyhow!("invalid --end-at datetime"))?
                .to_rfc3339(),
        );
    }
    validate_cli_schedule_targets(&project_root, &schedule)?;
    schedule.updated_at = Utc::now().to_rfc3339();
    schedule.updated_by = Some(actor_agent());
    normalize_schedule(&project_root, &mut schedule, actor_agent())?;
    enqueue_dispatch(&project_root, "upsert", Some(schedule.clone()), None)?;
    println!("{}", schedule.id);
    println!("interpreted as: {}", schedule_interpretation(&schedule));
    Ok(())
}

fn cli_update_draft(project_root: &Path, id: &str, args: &[String]) -> Result<()> {
    validate_draft_update_args(args, id)?;
    let text = read_stdin_body(true)?
        .ok_or_else(|| anyhow!("kota-ember update <draft-id> requires note text on stdin"))?;
    with_dispatch_file_lock(project_root, || {
        let state = load_state_with_pending_dispatch(project_root)?;
        let mut draft =
            find_draft(&state, id)?.ok_or_else(|| anyhow!("Ember draft not found: {id}"))?;
        let updated_at = Utc::now();
        draft.text = text;
        draft.updated_at = updated_at.to_rfc3339();
        let draft = normalize_draft(draft)?;
        enqueue_draft_dispatch_at_unlocked(
            project_root,
            "upsert-draft",
            Some(draft),
            None,
            updated_at,
        )?;
        Ok(())
    })?;
    println!("{id}");
    Ok(())
}

fn cli_delete(args: Vec<String>) -> Result<()> {
    let project_root = cli_project_root(&args)?;
    let id = cli_item_id(&args)
        .ok_or_else(|| anyhow!("usage: kota-ember delete <schedule-or-draft-id>"))?;
    if is_draft_id(id) {
        enqueue_draft_dispatch(&project_root, "delete-draft", None, Some(id.clone()))?;
        println!("{id}");
        return Ok(());
    }
    enqueue_dispatch(&project_root, "delete", None, Some(id.clone()))?;
    println!("{id}");
    Ok(())
}

fn is_draft_id(id: &str) -> bool {
    id.starts_with("draft-")
}

fn cli_item_id(args: &[String]) -> Option<&String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--project-root" {
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") {
            return args.get(i);
        }
        i += 1;
    }
    None
}

fn is_schedule_only_argument(arg: &str) -> bool {
    matches!(
        arg,
        "--to" | "--in" | "--at" | "--idle" | "--cron" | "--end-after" | "--end-at"
    )
}

fn validate_draft_add_args(args: &[String]) -> Result<()> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--draft" => i += 1,
            "--project-root" => {
                i += 1;
                if args.get(i).is_none() {
                    bail!("--project-root requires a directory");
                }
                i += 1;
            }
            value if is_schedule_only_argument(value) => {
                bail!("kota-ember add --draft does not accept schedule argument: {value}");
            }
            value if value.starts_with("--") => {
                bail!("unknown kota-ember add --draft argument: {value}");
            }
            value => bail!("unexpected kota-ember add --draft argument: {value}"),
        }
    }
    Ok(())
}

fn validate_draft_list_args(args: &[String]) -> Result<()> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--draft" | "--json" => i += 1,
            "--project-root" => {
                i += 1;
                if args.get(i).is_none() {
                    bail!("--project-root requires a directory");
                }
                i += 1;
            }
            value if value.starts_with("--") => {
                bail!("unknown kota-ember list --draft argument: {value}");
            }
            value => bail!("unexpected kota-ember list --draft argument: {value}"),
        }
    }
    Ok(())
}

fn validate_draft_update_args(args: &[String], id: &str) -> Result<()> {
    let mut i = 0;
    let mut saw_id = false;
    while i < args.len() {
        match args[i].as_str() {
            "--project-root" => {
                i += 1;
                if args.get(i).is_none() {
                    bail!("--project-root requires a directory");
                }
                i += 1;
            }
            value if value == id && !saw_id => {
                saw_id = true;
                i += 1;
            }
            value if is_schedule_only_argument(value) => {
                bail!("Ember draft update does not accept schedule argument: {value}");
            }
            value if value.starts_with("--") => {
                bail!("unknown kota-ember draft update argument: {value}");
            }
            value => bail!("unexpected kota-ember draft update argument: {value}"),
        }
    }
    Ok(())
}

fn parse_cli_patch(args: Vec<String>, require_body: bool) -> Result<CliSchedulePatch> {
    let mut targets = Vec::new();
    let mut timing = None;
    let mut end_after = None;
    let mut end_at = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--to" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| anyhow!("--to requires a target"))?;
                targets.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(ToOwned::to_owned),
                );
                i += 1;
            }
            "--in" => {
                i += 1;
                timing = Some(CliTiming::Delay(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--in requires a duration"))?
                        .clone(),
                ));
                i += 1;
            }
            "--at" => {
                i += 1;
                timing = Some(CliTiming::At(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--at requires a datetime"))?
                        .clone(),
                ));
                i += 1;
            }
            "--idle" => {
                timing = Some(CliTiming::Idle);
                i += 1;
            }
            "--cron" => {
                i += 1;
                timing = Some(CliTiming::Cron(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--cron requires an expression"))?
                        .clone(),
                ));
                i += 1;
            }
            "--end-after" => {
                i += 1;
                end_after = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--end-after requires a count"))?
                        .parse::<u32>()?
                        .max(1),
                );
                i += 1;
            }
            "--end-at" => {
                i += 1;
                end_at = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--end-at requires a datetime"))?
                        .clone(),
                );
                i += 1;
            }
            "--project-root" => {
                i += 2;
            }
            value if value.starts_with("--") => bail!("unknown kota-ember argument: {value}"),
            _ => {
                i += 1;
            }
        }
    }
    let text = read_stdin_body(require_body)?;
    Ok(CliSchedulePatch {
        targets: (!targets.is_empty()).then_some(targets),
        text,
        timing,
        end_after,
        end_at,
    })
}

fn apply_cli_timing(
    schedule: &mut EmberSchedule,
    timing: Option<CliTiming>,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    match timing.unwrap_or_else(|| CliTiming::Delay("10m".into())) {
        CliTiming::Delay(value) => {
            let minutes = parse_duration_minutes(&value)?;
            schedule.mode = "delay".into();
            schedule.delay_amount = Some(minutes.max(1));
            schedule.delay_unit = Some("minutes".into());
            schedule.repeat_enabled = Some(false);
            schedule.next_run_at = (now + chrono::Duration::minutes(minutes as i64)).to_rfc3339();
        }
        CliTiming::At(value) => {
            let at = parse_local_datetime_or_rfc3339(&value)
                .ok_or_else(|| anyhow!("invalid --at datetime"))?;
            schedule.mode = "at".into();
            schedule.at_date_time = Some(at.to_rfc3339());
            schedule.repeat_enabled = Some(false);
            schedule.next_run_at = at.to_rfc3339();
        }
        CliTiming::Idle => {
            schedule.mode = "idle".into();
            schedule.wait_for_idle = Some(true);
            schedule.repeat_enabled = Some(false);
            schedule.next_run_at = now.to_rfc3339();
        }
        CliTiming::Cron(expr) => apply_cron(schedule, &expr, now)?,
    }
    if schedule.end_after_count.is_some() && schedule.repeat_enabled.unwrap_or(false) {
        schedule.end_mode = Some("after".into());
    }
    if let Some(end_at) = schedule.end_at.clone() {
        schedule.end_mode = Some("at".into());
        schedule.end_at = Some(
            parse_local_datetime_or_rfc3339(&end_at)
                .ok_or_else(|| anyhow!("invalid --end-at datetime"))?
                .to_rfc3339(),
        );
    }
    Ok(())
}

fn apply_cron(schedule: &mut EmberSchedule, expr: &str, now: chrono::DateTime<Utc>) -> Result<()> {
    let parts = expr.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        bail!("unsupported cron: expected 5 fields");
    }
    if parts
        .iter()
        .any(|part| part.contains('/') || part.contains('-'))
    {
        bail!("unsupported cron: stepped and ranged cron fields are not representable by Ember UI yet");
    }
    let minute = parse_cron_number(parts[0], 0, 59, "minute")?;
    let hour = parse_cron_number(parts[1], 0, 23, "hour")?;
    let dom = parts[2];
    let month = parts[3];
    let dow = parts[4];
    if month != "*" {
        bail!("unsupported cron: month constraints are not representable by Ember UI yet");
    }
    if dom != "*" && dow != "*" {
        bail!("unsupported cron: day-of-month and day-of-week cannot both be constrained");
    }
    let time = format!("{hour:02}:{minute:02}");
    if dom == "*" && dow == "*" {
        schedule.mode = "daily".into();
        schedule.time_of_day = Some(time);
        schedule.repeat_enabled = Some(false);
        schedule.next_run_at = next_daily_run(schedule.time_of_day.as_deref(), now)
            .ok_or_else(|| anyhow!("invalid cron time"))?
            .to_rfc3339();
        return Ok(());
    }
    if dow != "*" {
        let days = parse_week_days(dow)?;
        schedule.mode = "at".into();
        schedule.repeat_enabled = Some(true);
        schedule.repeat_kind = Some("weekly".into());
        schedule.repeat_week_days = Some(days);
        schedule.repeat_every_weeks = Some(1);
        schedule.time_of_day = Some(time);
        schedule.next_run_at = next_weekly_run(schedule, now)
            .ok_or_else(|| anyhow!("invalid weekly cron"))?
            .to_rfc3339();
        return Ok(());
    }
    let days = parse_month_days(dom)?;
    schedule.mode = "at".into();
    schedule.repeat_enabled = Some(true);
    schedule.repeat_kind = Some("monthly".into());
    schedule.repeat_month_days = Some(days);
    schedule.repeat_every_months = Some(1);
    schedule.time_of_day = Some(time);
    schedule.next_run_at = next_monthly_run(schedule, now)
        .ok_or_else(|| anyhow!("invalid monthly cron"))?
        .to_rfc3339();
    Ok(())
}

fn parse_cron_number(value: &str, min: u32, max: u32, label: &str) -> Result<u32> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| anyhow!("unsupported cron {label}: {value}"))?;
    if parsed < min || parsed > max {
        bail!("unsupported cron {label}: {value}");
    }
    Ok(parsed)
}

fn parse_week_days(value: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for token in value.split(',') {
        let day = match token.to_ascii_uppercase().as_str() {
            "SUN" | "0" | "7" => 0,
            "MON" | "1" => 1,
            "TUE" | "2" => 2,
            "WED" | "3" => 3,
            "THU" | "4" => 4,
            "FRI" | "5" => 5,
            "SAT" | "6" => 6,
            other => bail!("unsupported cron weekday: {other}"),
        };
        if !out.contains(&day) {
            out.push(day);
        }
    }
    if out.is_empty() {
        bail!("cron weekday list is empty");
    }
    Ok(out)
}

fn parse_month_days(value: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for token in value.split(',') {
        let token = token.trim();
        if token.eq_ignore_ascii_case("L") || token.eq_ignore_ascii_case("last") {
            out.push("last".into());
            continue;
        }
        let day = token
            .parse::<u32>()
            .map_err(|_| anyhow!("unsupported cron month day: {token}"))?;
        if !(1..=31).contains(&day) {
            bail!("unsupported cron month day: {token}");
        }
        let value = day.to_string();
        if !out.contains(&value) {
            out.push(value);
        }
    }
    if out.is_empty() {
        bail!("cron month day list is empty");
    }
    Ok(out)
}

fn schedule_interpretation(schedule: &EmberSchedule) -> String {
    match schedule.mode.as_str() {
        "idle" => "when targets are idle".into(),
        "delay" => format!(
            "after {} {}",
            schedule.delay_amount.unwrap_or(10),
            schedule.delay_unit.as_deref().unwrap_or("minutes")
        ),
        "daily" => format!(
            "daily {} local",
            schedule.time_of_day.as_deref().unwrap_or("09:00")
        ),
        "interval" => format!("every {} hours", schedule.interval_hours.unwrap_or(4)),
        _ if schedule.repeat_kind.as_deref() == Some("weekly") => {
            format!(
                "weekly {:?} {} local",
                schedule.repeat_week_days.clone().unwrap_or_default(),
                schedule.time_of_day.as_deref().unwrap_or("09:00")
            )
        }
        _ if schedule.repeat_kind.as_deref() == Some("monthly") => {
            format!(
                "monthly {:?} {} local",
                schedule.repeat_month_days.clone().unwrap_or_default(),
                schedule.time_of_day.as_deref().unwrap_or("09:00")
            )
        }
        _ => format!("at {}", schedule.next_run_at),
    }
}

fn cli_project_root(args: &[String]) -> Result<PathBuf> {
    let mut project_root = std::env::var("KOTA_PROJECT_ROOT").ok();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--project-root" {
            i += 1;
            project_root = args.get(i).cloned();
            i += 1;
            continue;
        }
        i += 1;
    }
    project_root
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(find_project_root_from)
        })
        .ok_or_else(|| anyhow!("kota-ember requires KOTA_PROJECT_ROOT or --project-root"))
}

fn enqueue_dispatch(
    project_root: &Path,
    action: &str,
    schedule: Option<EmberSchedule>,
    schedule_id: Option<String>,
) -> Result<PathBuf> {
    let outbox = dispatch_outbox_dir(project_root);
    fs::create_dir_all(&outbox)?;
    let id = schedule_id
        .clone()
        .or_else(|| schedule.as_ref().map(|schedule| schedule.id.clone()))
        .unwrap_or_else(|| mint_id("dispatch"));
    let request = EmberDispatchFile {
        schema: DISPATCH_SCHEMA.into(),
        created_at: Utc::now().to_rfc3339(),
        project_root: path_string(project_root),
        action: action.into(),
        schedule_id,
        schedule,
        draft_id: None,
        draft: None,
    };
    let path = outbox.join(format!("{}.json", sanitize_id(&id)));
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&request)?)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn enqueue_draft_dispatch(
    project_root: &Path,
    action: &str,
    draft: Option<EmberDraft>,
    draft_id: Option<String>,
) -> Result<PathBuf> {
    with_dispatch_file_lock(project_root, || {
        let created_at = Utc::now();
        enqueue_draft_dispatch_at_unlocked(project_root, action, draft, draft_id, created_at)
    })
}

#[cfg(test)]
fn enqueue_draft_dispatch_at(
    project_root: &Path,
    action: &str,
    draft: Option<EmberDraft>,
    draft_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
) -> Result<PathBuf> {
    with_dispatch_file_lock(project_root, || {
        enqueue_draft_dispatch_at_unlocked(project_root, action, draft, draft_id, created_at)
    })
}

fn enqueue_draft_dispatch_at_unlocked(
    project_root: &Path,
    action: &str,
    draft: Option<EmberDraft>,
    draft_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
) -> Result<PathBuf> {
    let outbox = dispatch_outbox_dir(project_root);
    fs::create_dir_all(&outbox)?;
    let id = draft_id
        .clone()
        .or_else(|| draft.as_ref().map(|draft| draft.id.clone()))
        .unwrap_or_else(|| mint_id("dispatch"));
    let request = EmberDispatchFile {
        schema: DISPATCH_SCHEMA.into(),
        created_at: created_at.to_rfc3339(),
        project_root: path_string(project_root),
        action: action.into(),
        schedule_id: None,
        schedule: None,
        draft_id,
        draft,
    };
    let order = created_at
        .timestamp_nanos_opt()
        .unwrap_or_else(|| created_at.timestamp_micros().saturating_mul(1_000));
    let nonce = &Uuid::new_v4().simple().to_string()[..12];
    let path = outbox.join(format!(
        "draft-{order:020}-{}-{nonce}.json",
        sanitize_id(&id)
    ));
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&request)?)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn read_stdin_body(required: bool) -> Result<Option<String>> {
    if !required && io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut body = String::new();
    io::stdin().read_to_string(&mut body)?;
    let body = body.trim().to_string();
    if body.is_empty() {
        if required {
            bail!("kota-ember requires a non-empty prompt body on stdin");
        }
        return Ok(None);
    }
    Ok(Some(body))
}

fn print_cli_usage() {
    eprintln!("Kota Ember CLI");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  kota-ember add --to <agent-or-human[,agent-or-human...]> (--in <duration> | --at <local time> | --idle | --cron <expr>) <<'EOF'");
    eprintln!("  kota-ember add --draft <<'EOF'");
    eprintln!("  kota-ember list [--json]");
    eprintln!("  kota-ember list --draft [--json]");
    eprintln!("  kota-ember show <schedule-or-draft-id>");
    eprintln!("  kota-ember update <schedule-or-draft-id> [options] [<<'EOF']");
    eprintln!("  kota-ember delete <schedule-or-draft-id>");
    eprintln!("  kota-ember install-shim");
    eprintln!();
    eprintln!("Targets:");
    eprintln!("  Agent targets accept an AKA, display name, or agent id.");
    eprintln!("  Human targets accept the configured account name (shown as <your-name>), or __kota_human_telegram__.");
    eprintln!("  Human reminders appear in the room and also use Laughing Man when configured.");
    eprintln!("  Multiple targets can be comma-separated: --to 'Gem,agent-123,<your-name>'");
    eprintln!();
    eprintln!("Timing:");
    eprintln!("  --in <duration>       Delay such as 10m, 2h, or 1d.");
    eprintln!("  --at <local time>     Local datetime such as \"2026-06-18 14:00\".");
    eprintln!("  --idle                Deliver when the target is idle.");
    eprintln!("  --cron <expr>         Supported cron expression.");
    eprintln!("  --end-after <count>   Stop a repeating schedule after count runs.");
    eprintln!("  --end-at <time>       Stop a repeating schedule at a local/RFC3339 datetime.");
    eprintln!("  --project-root <dir>  Override project root detection.");
    eprintln!();
    eprintln!("Draft notes:");
    eprintln!(
        "  add --draft creates a note without targets or timing and never schedules delivery."
    );
    eprintln!("  list --draft lists notes only; combine it with --json for structured output.");
    eprintln!("  show, update, and delete accept draft-* ids as well as schedule-* ids.");
    eprintln!(
        "  Draft updates replace note text from stdin; drafts cannot be converted to schedules."
    );
    eprintln!(
        "  Draft notes are limited to {MAX_DRAFTS}; delete one before adding another at the limit."
    );
    eprintln!("  Schedule-only arguments are rejected when creating or updating a draft.");
    eprintln!();
    eprintln!("Prompt body:");
    eprintln!("  add requires prompt text on stdin.");
    eprintln!("  add --draft and update <draft-id> require non-empty note text on stdin.");
    eprintln!(
        "  update replaces prompt text when stdin is non-empty; otherwise it only patches options."
    );
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  printf 'Review the release notes' | kota-ember add --to Gem --in 2h");
    eprintln!("  kota-ember add --to '<your-name>' --at \"2026-06-18 14:00\" <<'EOF'");
    eprintln!("  Discuss whether human Ember reminders should use Cloudflare.");
    eprintln!("  EOF");
    eprintln!("  printf 'Updated reminder text' | kota-ember update schedule-abc --at \"2026-06-18 15:00\"");
    eprintln!("  kota-ember list --json");
    eprintln!("  printf 'Investigate Ember history cleanup' | kota-ember add --draft");
    eprintln!("  kota-ember list --draft --json");
}

#[derive(Clone, Debug)]
struct AgentIdentity {
    agent_id: String,
    display_name: String,
    aka: String,
}

fn resolve_agent_identity(project_root: &Path, raw: &str) -> Result<AgentIdentity> {
    let wanted = normalize_agent_ref(raw).to_lowercase();
    if wanted.is_empty() {
        bail!("empty agent reference");
    }
    let mut matches = list_agent_identities(project_root)?
        .into_iter()
        .filter(|identity| {
            identity.agent_id.to_lowercase() == wanted
                || identity.aka.to_lowercase() == wanted
                || identity.display_name.to_lowercase() == wanted
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    matches.dedup_by(|a, b| a.agent_id == b.agent_id);
    if matches.is_empty() {
        bail!("agent not found for reference: {raw}");
    }
    if matches.len() > 1 {
        bail!("agent reference is ambiguous: {raw}");
    }
    Ok(matches.remove(0))
}

fn list_agent_identities(project_root: &Path) -> Result<Vec<AgentIdentity>> {
    let agents_root = project_root.join(".agent-workspaces");
    if !agents_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut identities = Vec::new();
    for entry in fs::read_dir(&agents_root)? {
        let entry = entry?;
        let cwd = entry.path();
        if !cwd.is_dir() {
            continue;
        }
        let Some(agent_id) = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let yaml_path = cwd.join("agent.yaml");
        let yaml = read_yaml_value(&yaml_path).unwrap_or(serde_yaml::Value::Null);
        let status = yaml_string(&yaml, "status").unwrap_or_else(|| "active".into());
        if matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "archived" | "deleted" | "dismissed" | "removed"
        ) {
            continue;
        }
        let display_name = yaml_string(&yaml, "display-name")
            .or_else(|| yaml_string(&yaml, "displayName"))
            .unwrap_or_else(|| agent_id.clone());
        identities.push(AgentIdentity {
            aka: agent_aka_from_display_name(&display_name),
            agent_id,
            display_name,
        });
    }
    Ok(identities)
}

fn normalize_agent_ref(raw: &str) -> String {
    raw.trim().trim_start_matches('@').trim().to_string()
}

fn agent_aka_from_display_name(display_name: &str) -> String {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return "Agent".into();
    }
    trimmed
        .split_once(" v. ")
        .map(|(head, _)| head)
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn read_yaml_value(path: &Path) -> Result<serde_yaml::Value> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

fn yaml_string(value: &serde_yaml::Value, key: &str) -> Option<String> {
    let serde_yaml::Value::Mapping(map) = value else {
        return None;
    };
    map.get(&serde_yaml::Value::String(key.to_string()))
        .and_then(|value| match value {
            serde_yaml::Value::String(value) => Some(value.clone()),
            serde_yaml::Value::Number(value) => Some(value.to_string()),
            serde_yaml::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
}

fn find_project_root_from(mut path: PathBuf) -> Option<PathBuf> {
    loop {
        if path.join(".agent-workspaces").is_dir() && path.join("project-memory").is_dir() {
            return Some(path);
        }
        if !path.pop() {
            return None;
        }
    }
}

fn parse_duration_minutes(value: &str) -> Result<u32> {
    let mut total = 0u32;
    let mut number = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        let amount = number
            .parse::<u32>()
            .map_err(|_| anyhow!("invalid duration: {value}"))?;
        number.clear();
        total = total.saturating_add(match ch {
            'd' | 'D' => amount.saturating_mul(1440),
            'h' | 'H' => amount.saturating_mul(60),
            'm' | 'M' => amount,
            _ => bail!("invalid duration unit in {value}; use d, h, or m"),
        });
    }
    if !number.is_empty() {
        total = total.saturating_add(number.parse::<u32>()?);
    }
    if total == 0 {
        bail!("duration must be at least one minute");
    }
    Ok(total)
}

fn parse_local_datetime_or_rfc3339(value: &str) -> Option<chrono::DateTime<Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }
    let formats = [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ];
    for format in formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            if let Some(local) = Local.from_local_datetime(&naive).single() {
                return Some(local.with_timezone(&Utc));
            }
        }
    }
    None
}

fn parse_rfc3339(value: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn parse_time_of_day(value: &str) -> Option<(u32, u32)> {
    let (hour, minute) = value.split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn local_datetime(date: NaiveDate, hour: u32, minute: u32) -> Option<chrono::DateTime<Local>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    Local.from_local_datetime(&naive).single()
}

fn actor_human() -> EmberActorRef {
    EmberActorRef {
        kind: "human".into(),
        label: "Human".into(),
    }
}

fn actor_agent() -> EmberActorRef {
    EmberActorRef {
        kind: "agent".into(),
        label: "Agent".into(),
    }
}

fn ember_event_id(kind: &str, schedule_id: &str, agent_id: &str) -> String {
    format!(
        "ember-{}-{}-{}-{}",
        sanitize_id(kind),
        sanitize_id(schedule_id),
        sanitize_id(agent_id),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn mint_id(prefix: &str) -> String {
    format!("{}-{}", prefix, Uuid::new_v4())
}

fn default_schema() -> String {
    STORE_SCHEMA.into()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("dispatch");
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    for index in 1.. {
        let name = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '+'))
    {
        return value.into();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn current_target_triple_guess() -> Option<String> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match (arch, os) {
        ("aarch64", "macos") => Some("aarch64-apple-darwin".into()),
        ("x86_64", "macos") => Some("x86_64-apple-darwin".into()),
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu".into()),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu".into()),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc".into()),
        _ => None,
    }
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, hour, minute, second)
            .unwrap()
    }

    fn schedule_with_targets(targets: Vec<&str>, names: Vec<&str>) -> EmberSchedule {
        let target_agent_id = targets.first().copied().unwrap_or("").to_string();
        let target_agent_name = names.first().copied().unwrap_or("").to_string();
        let now = Utc::now().to_rfc3339();
        EmberSchedule {
            id: "schedule-test".into(),
            text: "ping".into(),
            target_agent_id,
            target_agent_name,
            target_agent_ids: targets.into_iter().map(str::to_string).collect(),
            target_agent_names: names.into_iter().map(str::to_string).collect(),
            mode: "delay".into(),
            delay_amount: Some(10),
            delay_unit: Some("minutes".into()),
            at_date_time: None,
            time_of_day: None,
            interval_hours: None,
            wait_for_idle: Some(true),
            repeat_enabled: None,
            repeat_amount: None,
            repeat_unit: None,
            repeat_kind: None,
            repeat_every_minutes: None,
            repeat_week_days: None,
            repeat_every_weeks: None,
            repeat_month_days: None,
            repeat_every_months: None,
            end_mode: None,
            end_after_count: None,
            end_at: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            next_run_at: now,
            last_run_at: None,
            run_count: 0,
            status: "scheduled".into(),
            error: None,
            created_by: None,
            updated_by: None,
        }
    }

    fn ember_test_project(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("kota-ember-{label}-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_test_agent(project_root: &Path, agent_id: &str, display_name: &str) {
        let cwd = project_root.join(".agent-workspaces").join(agent_id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            format!("id: {agent_id}\ndisplay-name: {display_name}\nstatus: active\n"),
        )
        .unwrap();
    }

    fn draft_note(id: &str, text: &str) -> EmberDraft {
        EmberDraft {
            id: id.into(),
            text: text.into(),
            created_at: "2026-07-30T12:00:00Z".into(),
            updated_at: "2026-07-30T12:00:00Z".into(),
        }
    }

    fn schedule_for_reconcile(
        id: &str,
        mode: &str,
        next_run_at: chrono::DateTime<Utc>,
        changed_at: chrono::DateTime<Utc>,
    ) -> EmberSchedule {
        EmberSchedule {
            id: id.into(),
            text: "Test reminder".into(),
            target_agent_id: "agent-one".into(),
            target_agent_name: "Agent One".into(),
            target_agent_ids: vec!["agent-one".into()],
            target_agent_names: vec!["Agent One".into()],
            mode: mode.into(),
            delay_amount: None,
            delay_unit: None,
            at_date_time: None,
            time_of_day: None,
            interval_hours: None,
            wait_for_idle: Some(mode == "idle"),
            repeat_enabled: Some(false),
            repeat_amount: None,
            repeat_unit: None,
            repeat_kind: None,
            repeat_every_minutes: None,
            repeat_week_days: None,
            repeat_every_weeks: None,
            repeat_month_days: None,
            repeat_every_months: None,
            end_mode: None,
            end_after_count: None,
            end_at: None,
            created_at: changed_at.to_rfc3339(),
            updated_at: changed_at.to_rfc3339(),
            next_run_at: next_run_at.to_rfc3339(),
            last_run_at: None,
            run_count: 0,
            status: "scheduled".into(),
            error: None,
            created_by: Some(actor_human()),
            updated_by: Some(actor_human()),
        }
    }

    #[test]
    fn normalize_schedule_preserves_human_telegram_target_name() {
        let mut schedule = schedule_with_targets(vec![HUMAN_TELEGRAM_TARGET_ID], vec!["human_name"]);
        normalize_schedule(&std::env::temp_dir(), &mut schedule, actor_human()).unwrap();

        assert_eq!(schedule.target_agent_id, HUMAN_TELEGRAM_TARGET_ID);
        assert_eq!(schedule.target_agent_name, "human_name");
        assert_eq!(schedule.target_agent_ids, vec![HUMAN_TELEGRAM_TARGET_ID]);
        assert_eq!(schedule.target_agent_names, vec!["human_name"]);
    }

    #[test]
    fn human_telegram_target_name_accepts_configured_human_name() {
        assert_eq!(
            human_telegram_target_name_for("human_name", Some("human_name")),
            Some("human_name".into())
        );
        assert_eq!(
            human_telegram_target_name_for("HUMAN_NAME", Some("human_name")),
            Some("human_name".into())
        );
        assert_eq!(
            human_telegram_target_name_for(HUMAN_TELEGRAM_TARGET_ID, Some("human_name")),
            Some("human_name".into())
        );
        assert_eq!(human_telegram_target_name_for("human_name", None), None);
    }

    #[test]
    fn cli_add_target_validation_rejects_any_unresolvable_target() {
        let root = ember_test_project("invalid-add-target");
        write_test_agent(&root, "agent-alice", "Gem v. kota");
        let schedule =
            schedule_with_targets(vec!["Gem", "human_name"], vec!["Gem v. kota", "human_name"]);

        let error = validate_cli_schedule_targets_for(&root, &schedule, Some("老无"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("invalid Ember target \"human_name\""));
        assert!(error.contains("agent not found for reference: human_name"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_update_target_validation_rejects_unresolvable_final_target() {
        let root = ember_test_project("invalid-update-target");
        write_test_agent(&root, "agent-alice", "Gem v. kota");
        let mut schedule = schedule_with_targets(vec!["agent-alice"], vec!["Gem v. kota"]);
        schedule.target_agent_ids = vec!["missing-agent".into()];

        let error = validate_cli_schedule_targets_for(&root, &schedule, Some("老无"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("invalid Ember target \"missing-agent\""));
        assert!(error.contains("agent not found for reference: missing-agent"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_target_validation_accepts_aka_agent_id_and_configured_human_name() {
        let root = ember_test_project("valid-targets");
        write_test_agent(&root, "agent-alice", "Gem v. kota");
        let schedule = schedule_with_targets(
            vec!["Gem", "@agent-alice", "老无", HUMAN_TELEGRAM_TARGET_ID],
            vec!["Gem v. kota", "Gem v. kota", "老无", "老无"],
        );

        validate_cli_schedule_targets_for(&root, &schedule, Some("老无")).unwrap();

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wait_for_idle_ignores_human_telegram_target() {
        let human_only = schedule_with_targets(vec![HUMAN_TELEGRAM_TARGET_ID], vec!["User"]);
        let working = HashSet::from([HUMAN_TELEGRAM_TARGET_ID.to_string()]);
        assert!(!schedule_has_working_agent_target(&human_only, &working));

        let mixed = schedule_with_targets(
            vec![HUMAN_TELEGRAM_TARGET_ID, "agent-1"],
            vec!["User", "A1"],
        );
        let working = HashSet::from(["agent-1".to_string()]);
        assert!(schedule_has_working_agent_target(&mixed, &working));
    }

    #[test]
    fn pending_dispatch_upsert_is_visible_to_cli_state_reads() {
        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut schedule = schedule_with_targets(vec![HUMAN_TELEGRAM_TARGET_ID], vec!["human_name"]);
        schedule.id = "schedule-pending".into();

        let path = enqueue_dispatch(&root, "upsert", Some(schedule), None).unwrap();
        let dispatch: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(dispatch.get("draftId").is_none());
        assert!(dispatch.get("draft").is_none());

        assert!(load_state(&root).unwrap().schedules.is_empty());
        let state = load_state_with_pending_dispatch(&root).unwrap();
        assert_eq!(state.schedules.len(), 1);
        assert_eq!(state.schedules[0].id, "schedule-pending");
        assert_eq!(state.schedules[0].target_agent_names, vec!["human_name"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_dispatch_delete_is_visible_to_cli_state_reads() {
        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut state = empty_state();
        let mut schedule = schedule_with_targets(vec!["agent-one"], vec!["Agent One"]);
        schedule.id = "schedule-delete".into();
        state.schedules.push(schedule);
        save_state(&root, &state).unwrap();

        enqueue_dispatch(&root, "delete", None, Some("schedule-delete".into())).unwrap();

        assert_eq!(load_state(&root).unwrap().schedules.len(), 1);
        assert!(load_state_with_pending_dispatch(&root)
            .unwrap()
            .schedules
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_draft_upsert_is_visible_to_cli_state_reads() {
        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let draft = draft_note("draft-pending", "Review the launch note");

        enqueue_draft_dispatch(&root, "upsert-draft", Some(draft), None).unwrap();

        assert!(load_state(&root).unwrap().drafts.is_empty());
        let state = load_state_with_pending_dispatch(&root).unwrap();
        let drafts = ember_drafts(&state);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].id, "draft-pending");
        assert_eq!(drafts[0].text, "Review the launch note");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_draft_delete_is_visible_to_cli_state_reads() {
        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut state = empty_state();
        state
            .drafts
            .push(serde_json::to_value(draft_note("draft-delete", "Old note")).unwrap());
        save_state(&root, &state).unwrap();

        enqueue_draft_dispatch(&root, "delete-draft", None, Some("draft-delete".into())).unwrap();

        assert_eq!(load_state(&root).unwrap().drafts.len(), 1);
        assert!(load_state_with_pending_dispatch(&root)
            .unwrap()
            .drafts
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_limit_rejects_new_note_without_dropping_existing_data() {
        let mut state = empty_state();
        for index in 0..MAX_DRAFTS {
            state.drafts.push(
                serde_json::to_value(draft_note(
                    &format!("draft-{index}"),
                    &format!("Note {index}"),
                ))
                .unwrap(),
            );
        }
        let first_id = draft_id(&state.drafts[0]).unwrap().to_string();

        let error = upsert_draft(&mut state, draft_note("draft-overflow", "One too many"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("Draft limit reached (20)"));
        assert_eq!(state.drafts.len(), MAX_DRAFTS);
        assert_eq!(draft_id(&state.drafts[0]), Some(first_id.as_str()));
        assert!(find_draft(&state, "draft-overflow").unwrap().is_none());
    }

    #[test]
    fn pending_draft_actions_preserve_delete_then_add_order_at_limit() {
        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut state = empty_state();
        for index in 0..(MAX_DRAFTS - 1) {
            state.drafts.push(
                serde_json::to_value(draft_note(
                    &format!("draft-existing-{index}"),
                    &format!("Note {index}"),
                ))
                .unwrap(),
            );
        }
        state
            .drafts
            .push(serde_json::to_value(draft_note("draft-zzz-old", "Old note")).unwrap());
        save_state(&root, &state).unwrap();

        let delete_at = utc(12, 0, 0);
        enqueue_draft_dispatch_at(
            &root,
            "delete-draft",
            None,
            Some("draft-zzz-old".into()),
            delete_at,
        )
        .unwrap();
        enqueue_draft_dispatch_at(
            &root,
            "upsert-draft",
            Some(draft_note("draft-aaa-new", "New note")),
            None,
            delete_at + chrono::Duration::seconds(1),
        )
        .unwrap();

        let projected = load_state_with_pending_dispatch(&root).unwrap();
        assert_eq!(projected.drafts.len(), MAX_DRAFTS);
        assert!(find_draft(&projected, "draft-zzz-old").unwrap().is_none());
        assert!(find_draft(&projected, "draft-aaa-new").unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_draft_adds_reserve_the_last_slot_once() {
        use std::sync::{Arc, Barrier};

        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut state = empty_state();
        for index in 0..(MAX_DRAFTS - 1) {
            state.drafts.push(
                serde_json::to_value(draft_note(
                    &format!("draft-existing-{index}"),
                    &format!("Note {index}"),
                ))
                .unwrap(),
            );
        }
        save_state(&root, &state).unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let first = {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                enqueue_new_draft(&root, "First concurrent note".into())
            })
        };
        let second = {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                enqueue_new_draft(&root, "Second concurrent note".into())
            })
        };
        barrier.wait();

        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .any(|error| error.to_string().contains("Draft limit reached (20)")));
        assert_eq!(
            load_state_with_pending_dispatch(&root)
                .unwrap()
                .drafts
                .len(),
            MAX_DRAFTS
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_schedule_and_draft_dispatches_preserve_both_updates() {
        use std::sync::{Arc, Barrier};

        let root =
            std::env::temp_dir().join(format!("kota-ember-test-{}", Uuid::new_v4().simple()));
        let mut schedule =
            schedule_with_targets(vec![HUMAN_TELEGRAM_TARGET_ID], vec!["human_name"]);
        schedule.id = "schedule-concurrent".into();
        let schedule_path = enqueue_dispatch(&root, "upsert", Some(schedule), None).unwrap();
        let draft_path = enqueue_draft_dispatch(
            &root,
            "upsert-draft",
            Some(draft_note("draft-concurrent", "Keep this note")),
            None,
        )
        .unwrap();
        let processing = dispatch_processing_dir(&root);
        let delivered = dispatch_delivered_dir(&root);
        let failed = dispatch_failed_dir(&root);
        fs::create_dir_all(&processing).unwrap();
        fs::create_dir_all(&delivered).unwrap();
        fs::create_dir_all(&failed).unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let schedule_consumer = {
            let root = root.clone();
            let path = schedule_path.clone();
            let processing = processing.clone();
            let delivered = delivered.clone();
            let failed = failed.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                consume_dispatch_path(&root, &path, &processing, &delivered, &failed, || {})
            })
        };
        let draft_consumer = {
            let root = root.clone();
            let path = draft_path.clone();
            let processing = processing.clone();
            let delivered = delivered.clone();
            let failed = failed.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                consume_dispatch_path(&root, &path, &processing, &delivered, &failed, || {})
            })
        };
        barrier.wait();
        schedule_consumer.join().unwrap().unwrap();
        draft_consumer.join().unwrap().unwrap();

        let state = load_state(&root).unwrap();
        assert!(state
            .schedules
            .iter()
            .any(|schedule| schedule.id == "schedule-concurrent"));
        assert!(find_draft(&state, "draft-concurrent").unwrap().is_some());
        assert_eq!(fs::read_dir(failed).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_cli_arguments_reject_schedule_options() {
        assert!(validate_draft_add_args(&["--draft".into()]).is_ok());
        assert!(validate_draft_list_args(&["--draft".into(), "--json".into()]).is_ok());
        let item_args = vec![
            "--project-root".into(),
            "/tmp/project".into(),
            "draft-one".into(),
        ];
        assert_eq!(
            cli_item_id(&item_args).map(String::as_str),
            Some("draft-one")
        );
        assert!(
            validate_draft_add_args(&["--draft".into(), "--to".into(), "agent-one".into()])
                .unwrap_err()
                .to_string()
                .contains("does not accept schedule argument: --to")
        );
        assert!(validate_draft_update_args(
            &["draft-one".into(), "--at".into(), "tomorrow".into()],
            "draft-one"
        )
        .unwrap_err()
        .to_string()
        .contains("does not accept schedule argument: --at"));
    }

    #[test]
    fn overdue_reconcile_keeps_idle_schedule_pending() {
        let changed_at = utc(9, 50, 0);
        let due_at = utc(10, 0, 0);
        let now = utc(10, 3, 0);
        let mut state = empty_state();
        state.schedules.push(schedule_for_reconcile(
            "schedule-idle",
            "idle",
            due_at,
            changed_at,
        ));

        let reconciled = reconcile_overdue_not_delivered(&mut state, now);

        assert_eq!(reconciled, 0);
        assert_eq!(state.schedules[0].status, "scheduled");
        assert_eq!(state.schedules[0].error, None);
        assert!(state.history.is_empty());
    }

    #[test]
    fn overdue_reconcile_honors_full_two_minute_grace() {
        let changed_at = utc(9, 50, 0);
        let due_at = utc(10, 1, 1);
        let now = utc(10, 3, 0);
        let mut state = empty_state();
        state.schedules.push(schedule_for_reconcile(
            "schedule-in-grace",
            "delay",
            due_at,
            changed_at,
        ));

        let reconciled = reconcile_overdue_not_delivered(&mut state, now);

        assert_eq!(reconciled, 0);
        assert_eq!(state.schedules[0].status, "scheduled");
        assert_eq!(state.schedules[0].error, None);
        assert!(state.history.is_empty());
    }

    #[test]
    fn overdue_reconcile_marks_schedule_not_delivered_at_two_minutes() {
        let changed_at = utc(9, 50, 0);
        let due_at = utc(10, 1, 0);
        let now = utc(10, 3, 0);
        let mut state = empty_state();
        state.schedules.push(schedule_for_reconcile(
            "schedule-overdue",
            "delay",
            due_at,
            changed_at,
        ));

        let reconciled = reconcile_overdue_not_delivered(&mut state, now);

        assert_eq!(reconciled, 1);
        assert_eq!(state.schedules[0].status, "failed");
        assert_eq!(state.schedules[0].error.as_deref(), Some(NOT_DELIVERED));
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].status, "failed");
        assert_eq!(state.history[0].error.as_deref(), Some(NOT_DELIVERED));
        assert_eq!(state.history[0].missed_runs, Some(1));
        let due_at_text = due_at.to_rfc3339();
        assert_eq!(
            state.history[0].scheduled_for.as_deref(),
            Some(due_at_text.as_str())
        );
    }

    #[test]
    fn overdue_reconcile_keeps_repeating_schedule_active_and_flagged() {
        let changed_at = utc(9, 50, 0);
        let due_at = utc(10, 0, 0);
        let now = utc(10, 3, 0);
        let mut schedule = schedule_for_reconcile("schedule-repeat", "delay", due_at, changed_at);
        schedule.repeat_enabled = Some(true);
        schedule.repeat_kind = Some("fixed".into());
        schedule.repeat_every_minutes = Some(60);
        schedule.end_mode = Some("never".into());
        let mut state = empty_state();
        state.schedules.push(schedule);

        let reconciled = reconcile_overdue_not_delivered(&mut state, now);

        assert_eq!(reconciled, 1);
        assert_eq!(state.schedules[0].status, "scheduled");
        assert_eq!(state.schedules[0].error.as_deref(), Some(NOT_DELIVERED));
        assert!(parse_rfc3339(&state.schedules[0].next_run_at).is_some_and(|next| next > now));
        assert_eq!(state.history[0].status, "failed");
        assert_eq!(state.history[0].error.as_deref(), Some(NOT_DELIVERED));
    }

    #[test]
    fn delivery_outcome_keeps_partial_success_as_warning() {
        let outcome = EmberDeliveryOutcome {
            delivered: 1,
            failed: vec!["Agent Two: unavailable".into()],
        };

        assert_eq!(
            outcome.partial_error().as_deref(),
            Some("Some targets failed: Agent Two: unavailable")
        );
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
