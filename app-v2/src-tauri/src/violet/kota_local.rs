//! Kota Local (Pi RPC) source for Violet: R3 of the frozen Kota Local plan
//! (`KOTA-LOCAL-FEATURES.md` v6.2, `CONTRACT-INJECT.md` §5).
//!
//! The Local runtime owns the `pi --mode rpc` child, its stdio and the
//! delivery ledger. Violet only consumes the ordered notices it is handed:
//! durable session entries read through `get_entries`, a few live Pi events,
//! delivery facts and runtime status. Nothing here discovers, watches or scans
//! Pi session files, and the file-backed source/cursor paths of the six
//! terminal providers stay untouched. Room records still pass through the same
//! envelope, privacy, dedupe, sequence and turn-credit rules as
//! `sync_project`; raw session content never leaves the Local runtime.
use super::*;

pub(crate) const KOTA_LOCAL_SHELL: &str = "kota-local";
const KOTA_LOCAL_SOURCE_KIND: &str = "kota-local-rpc";
const KOTA_LOCAL_STATE_VERSION: u32 = 1;
const KOTA_LOCAL_RECEIPT_LIMIT: usize = 256;
const KOTA_LOCAL_FAULT_KEY_LIMIT: usize = 256;
const KOTA_LOCAL_SETTLED_DELIVERY_LIMIT: usize = 512;
const KOTA_LOCAL_PATH_TAIL_LIMIT: usize = 512;
const KOTA_LOCAL_FAULT_DETAIL_LIMIT: usize = 400;
const KOTA_USER_CUSTOM_TYPE: &str = "kota.user";
const KOTA_INPUT_CUSTOM_TYPES: [&str; 7] = [
    "kota.user",
    "kota.lm",
    "kota.bus",
    "kota.bbs",
    "kota.ember",
    "kota.dream",
    "kota.bartender",
];

static KOTA_LOCAL_STATE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();
// Child epochs restart with every App process, so the fence is the highest
// epoch seen by this process, never a persisted value.
static KOTA_LOCAL_PROCESS_EPOCHS: OnceLock<Mutex<HashMap<(PathBuf, String), u64>>> =
    OnceLock::new();
static KOTA_LOCAL_PROCESS_START: OnceLock<String> = OnceLock::new();

/// Room-side view of one delivery the Local runtime still owns. The room uses
/// it only to hold back the legacy 180 second "unconfirmed" verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPendingDelivery {
    pub message_id: String,
    pub target_agent_id: String,
}

/// Every notice is bound to one project / agent / session / child epoch.
pub(crate) type KotaLocalScope = crate::local_rpc::LocalScope;

/// One `get_entries` read, passed through verbatim.
pub(crate) struct KotaLocalEntriesRead<'a> {
    pub(crate) entries: &'a [JsonValue],
    pub(crate) leaf_id: Option<&'a str>,
    /// `None` for a full read.
    pub(crate) since: Option<&'a str>,
    pub(crate) recovery: bool,
}

/// The runtime ledger's state of one kotaId.
pub(crate) type KotaLocalDeliveryState = crate::local_rpc::DeliveryState;

impl KotaLocalDeliveryState {
    fn pending(self) -> bool {
        matches!(
            self,
            Self::Held | Self::Sent | Self::Accepted | Self::ClearRequested
        )
    }

    fn confirmed(self) -> bool {
        matches!(self, Self::Consumed | Self::Durable)
    }
}

/// One delivery notice: the ledger state of `kota_id` after the fact.
pub(crate) struct KotaLocalDelivery<'a> {
    pub(crate) kota_id: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) state: KotaLocalDeliveryState,
    pub(crate) at: &'a str,
    pub(crate) custom_type: &'a str,
}

pub(crate) struct KotaLocalRuntimeStatus<'a> {
    pub(crate) state: &'a str,
    pub(crate) queued: usize,
    pub(crate) compacting: bool,
    pub(crate) error: Option<&'a str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KotaLocalIngestOutcome {
    pub(crate) examined: usize,
    pub(crate) on_path: usize,
    pub(crate) off_path: usize,
    pub(crate) written: usize,
    pub(crate) skipped_private: usize,
    pub(crate) receipts: usize,
    /// Last entry id of this read in append order; the runtime's next `since`.
    pub(crate) cursor: Option<String>,
    pub(crate) changed_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalState {
    #[serde(default)]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start: Option<String>,
    #[serde(default)]
    agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default)]
    sessions: BTreeMap<String, LocalSessionState>,
    #[serde(default)]
    deliveries: Vec<LocalDeliveryRecord>,
    #[serde(default)]
    receipts: Vec<LocalReceipt>,
    #[serde(default)]
    fault_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_ingest: Option<LocalIngestRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalSessionState {
    #[serde(default)]
    route_selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leaf_id: Option<String>,
    #[serde(default)]
    path_tail: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    /// Error line of a failed assistant message that was the leaf of a live
    /// read. Pi may still retry that attempt and drop it with a context_edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    held_error: Option<HeldErrorLine>,
    /// Set by `agent_settled`, cleared by `agent_start` / `auto_retry_start`.
    /// Only a settled run proves a failed leaf was final; an empty read during
    /// a retry delay proves nothing.
    #[serde(default)]
    settled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeldErrorLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    entry_id: String,
    timestamp: String,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalDeliveryRecord {
    session_id: String,
    kota_id: String,
    message_id: String,
    target_agent_id: String,
    state: KotaLocalDeliveryState,
    at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalReceipt {
    event_id: String,
    agent_id: String,
    timestamp: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalIngestRecord {
    at: String,
    session_id: String,
    recovery: bool,
    examined: usize,
    written: usize,
    skipped_private: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// A validated Kota input as Pi recorded it (custom_message entry, legacy
/// custom message entry, or a live `message_end`).
struct KotaInput {
    kota_id: String,
    custom_type: String,
    message_id: String,
    target_agent_id: String,
    created_at: String,
    private: bool,
    content: JsonValue,
}

struct LocalEntry<'a> {
    id: String,
    parent_id: Option<String>,
    timestamp: String,
    kind: String,
    json: &'a JsonValue,
}

/// The shared event builders take a `NativeSource` as a provenance carrier
/// only. Local never locates, watches or cursors it.
struct LocalContext {
    agent: ProjectAgent,
    source: NativeSource,
}

impl LocalContext {
    fn new(scope: &KotaLocalScope) -> Self {
        Self {
            agent: ProjectAgent {
                agent_id: scope.agent_id.clone(),
                shell: KOTA_LOCAL_SHELL.into(),
                cwd: scope.project_root.clone(),
                session_id: Some(scope.session_id.clone()),
            },
            source: NativeSource {
                kind: KOTA_LOCAL_SOURCE_KIND.into(),
                session_id: scope.session_id.clone(),
                path: local_session_dir(scope),
                aux_path: None,
            },
        }
    }
}

struct Projected {
    written: usize,
    skipped_private: usize,
    changed_paths: Vec<PathBuf>,
}

/// Persist completed session entries as room records. Idempotent by
/// `<sessionId>/<entryId>`: repeated, recovery and overlapping reads are safe.
/// An `Err` means nothing past the previous cursor is guaranteed persisted, so
/// the runtime must keep its `since`.
pub(crate) fn ingest_entries(
    scope: &KotaLocalScope,
    read: &KotaLocalEntriesRead<'_>,
) -> Result<KotaLocalIngestOutcome, String> {
    check_scope(scope)?;
    let spans = load_privacy_spans(&scope.project_root)?;
    with_state(&scope.project_root, &scope.agent_id, |state| {
        let current = adopt_scope(state, scope);
        let deliveries_before = state.deliveries.clone();
        if read.recovery && current {
            // A recovered owner re-reports every non-terminal delivery of the
            // session right after this read; rebuild the pending set from it.
            state.deliveries.retain(|record| {
                record.session_id != scope.session_id || !record.state.pending()
            });
        }
        let result = ingest_into(state, scope, read, &spans);
        if result.is_err() {
            // The runtime treats a failed recovery read as "not rebuilt" and
            // keeps the child closed; keep the old pending set until it retries.
            state.deliveries = deliveries_before;
        }
        let (examined, written, skipped_private) = result.as_ref().map_or((0, 0, 0), |outcome| {
            (outcome.examined, outcome.written, outcome.skipped_private)
        });
        state.last_ingest = Some(LocalIngestRecord {
            at: now_iso(),
            session_id: scope.session_id.clone(),
            recovery: read.recovery,
            examined,
            written,
            skipped_private,
            error: result.as_ref().err().cloned(),
        });
        Ok(result)
    })?
}

/// Live Pi events. Only a consumed `kota.user` input, runtime faults and run
/// boundaries matter; streaming deltas are ignored without touching disk. A
/// model failure is not taken from here: Pi persists every failed attempt, so
/// the final one arrives as a durable entry (`auto_retry_end` would repeat
/// it). `agent_settled` releases a failed leaf held back by `ingest_entries`.
pub(crate) fn record_live_event(
    scope: &KotaLocalScope,
    event: &JsonValue,
) -> Result<Vec<PathBuf>, String> {
    check_scope(scope)?;
    let event_type = json_string(event, &["type"]).unwrap_or_default();
    let relevant = match event_type.as_str() {
        "message_end" => kota_input_from(event)
            .is_some_and(|input| input.custom_type == KOTA_USER_CUSTOM_TYPE),
        "compaction_end" => compaction_fault_text(event).is_some(),
        "extension_error" => !is_kota_queue_command_error(event),
        "agent_start" | "auto_retry_start" | "agent_settled" => true,
        _ => false,
    };
    if !relevant {
        return Ok(Vec::new());
    }
    let spans = load_privacy_spans(&scope.project_root)?;
    with_state(&scope.project_root, &scope.agent_id, |state| {
        if !adopt_scope(state, scope) {
            return Ok(Vec::new());
        }
        let context = LocalContext::new(scope);
        let mut events = Vec::new();
        match event_type.as_str() {
            "message_end" => {
                if let Some(input) = kota_input_from(event) {
                    let fallback = event
                        .get("message")
                        .and_then(message_millis_timestamp)
                        .unwrap_or_else(now_iso);
                    let mut unused = KotaLocalIngestOutcome::default();
                    events.extend(user_input_events(
                        &context,
                        scope,
                        &input,
                        &fallback,
                        &mut unused,
                    ));
                }
            }
            "compaction_end" => {
                if let Some(text) = compaction_fault_text(event) {
                    events.push(fault_event(&context, scope, text));
                }
            }
            "extension_error" => {
                events.push(fault_event(&context, scope, extension_fault_text(event)))
            }
            "agent_start" | "auto_retry_start" | "agent_settled" => {
                let settled = event_type == "agent_settled";
                let session = state.sessions.entry(scope.session_id.clone()).or_default();
                session.settled = settled;
                if settled {
                    if let Some(held) = session.held_error.take() {
                        events.push(held_error_event(&context, scope, &held));
                    }
                }
            }
            _ => {}
        }
        Ok(project_events(scope, events, &spans)?.changed_paths)
    })
}

/// Delivery facts. Consumed / durable bus-path inputs become Agent Bus
/// receipts; held / sent / accepted / clear_requested stay pending for the
/// room; a failed delivery writes one content-free error line.
pub(crate) fn record_delivery(
    scope: &KotaLocalScope,
    delivery: &KotaLocalDelivery<'_>,
) -> Result<Vec<PathBuf>, String> {
    check_scope(scope)?;
    if delivery.custom_type == crate::local_rpc::shadow_clone::REPORT_TYPE { return Ok(Vec::new()); }
    if !KOTA_INPUT_CUSTOM_TYPES.contains(&delivery.custom_type)
        || delivery.kota_id != format!("{}#{}", delivery.message_id, scope.agent_id)
    {
        kota_debug_log(&format!(
            "[violet] kota-local ignored delivery {} ({}) for {}",
            delivery.kota_id, delivery.custom_type, scope.agent_id
        ));
        return Ok(Vec::new());
    }
    let spans = if delivery.state == KotaLocalDeliveryState::Failed {
        load_privacy_spans(&scope.project_root)?
    } else {
        Vec::new()
    };
    with_state(&scope.project_root, &scope.agent_id, |state| {
        if !adopt_scope(state, scope) || !apply_delivery(state, scope, delivery) {
            return Ok(Vec::new());
        }
        if delivery.state.confirmed() && delivery.custom_type != KOTA_USER_CUSTOM_TYPE {
            push_receipt(
                state,
                LocalReceipt {
                    event_id: delivery.message_id.to_string(),
                    agent_id: scope.agent_id.clone(),
                    timestamp: normalize_timestamp(delivery.at),
                },
            );
        }
        let mut paths = vec![local_state_path(&scope.project_root, &scope.agent_id)];
        // Keyed by the fact time: a repeated notice is one line, a failure
        // after a manual retry of the same kotaId is another.
        if delivery.state == KotaLocalDeliveryState::Failed
            && remember_fault(state, format!("delivery/{}/{}", delivery.kota_id, delivery.at))
        {
            let context = LocalContext::new(scope);
            let line = fault_event(&context, scope, delivery_fault_text(delivery));
            paths.extend(project_events(scope, vec![line], &spans)?.changed_paths);
        }
        Ok(paths)
    })
}

/// Runtime status. Only `error` reaches the room, once per epoch and message.
pub(crate) fn record_runtime_status(
    scope: &KotaLocalScope,
    status: &KotaLocalRuntimeStatus<'_>,
) -> Result<Vec<PathBuf>, String> {
    check_scope(scope)?;
    if status.state != "error" {
        return Ok(Vec::new());
    }
    let detail = status
        .error
        .map(one_line)
        .filter(|detail| !detail.is_empty())
        .unwrap_or_else(|| "the Pi process stopped unexpectedly".into());
    kota_debug_log(&format!(
        "[violet] kota-local runtime error for {} (queued {}, compacting {})",
        scope.agent_id, status.queued, status.compacting
    ));
    let spans = load_privacy_spans(&scope.project_root)?;
    with_state(&scope.project_root, &scope.agent_id, |state| {
        if !adopt_scope(state, scope)
            || !remember_fault(
                state,
                format!(
                    "status/{}/{}/{}",
                    KOTA_LOCAL_PROCESS_START.get_or_init(now_iso),
                    scope.epoch,
                    short_hash(&detail)
                ),
            )
        {
            return Ok(Vec::new());
        }
        let context = LocalContext::new(scope);
        let detail = truncate_chars(&detail, KOTA_LOCAL_FAULT_DETAIL_LIMIT);
        // The runtime authors some errors as finished room sentences (wording
        // approved with the contract); show those verbatim, prefix the rest.
        let text = if detail.starts_with("Kota Local ") {
            sentence(detail)
        } else {
            sentence(format!("Kota Local runtime error: {detail}"))
        };
        Ok(project_events(scope, vec![fault_event(&context, scope, text)], &spans)?.changed_paths)
    })
}

/// `sync_project` arm for a Kota Local agent: status, receipts and pending
/// deliveries come from Violet's own Local state, never from file discovery.
pub(super) fn sync_agent(
    project_root: &Path,
    agent: &ProjectAgent,
    sources: &mut Vec<VioletSourceStatus>,
    receipts: &mut Vec<AgentBusReceipt>,
    pending: &mut Vec<LocalPendingDelivery>,
) -> bool {
    if agent.shell != KOTA_LOCAL_SHELL {
        return false;
    }
    let state = read_state(&local_state_path(project_root, &agent.agent_id));
    sources.push(source_status(agent, &state));
    receipts.extend(state.receipts.iter().map(|receipt| AgentBusReceipt {
        event_id: receipt.event_id.clone(),
        agent_id: receipt.agent_id.clone(),
        timestamp: receipt.timestamp.clone(),
    }));
    pending.extend(pending_deliveries(&state));
    true
}

/// Pending deliveries for cache reads, which do not run agent discovery.
pub(super) fn cached_pending_deliveries(
    project_root: &Path,
    agent_ids: Option<&[String]>,
) -> Vec<LocalPendingDelivery> {
    let Ok(entries) = fs::read_dir(local_state_dir(project_root)) else {
        return Vec::new();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Vec::new();
    }
    // Serve exactly the agents the sync arm serves: active and currently on
    // Kota Local. An agent switched back to a terminal provider keeps its
    // Local state file, but its old pending items must not hold the room.
    let local_agents = discover_project_agents(project_root)
        .map(|agents| {
            agents
                .into_iter()
                .filter(|agent| agent.shell == KOTA_LOCAL_SHELL)
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut pending = Vec::new();
    for path in paths {
        let state = read_state(&path);
        if !local_agents.contains(&state.agent_id)
            || agent_ids.is_some_and(|ids| !ids.iter().any(|id| id == &state.agent_id))
        {
            continue;
        }
        pending.extend(pending_deliveries(&state));
    }
    pending
}

fn ingest_into(
    state: &mut LocalState,
    scope: &KotaLocalScope,
    read: &KotaLocalEntriesRead<'_>,
    spans: &[PrivacySpan],
) -> Result<KotaLocalIngestOutcome, String> {
    let entries = parse_entries(read.entries);
    let mut session = state
        .sessions
        .get(&scope.session_id)
        .cloned()
        .unwrap_or_default();
    let full = read.since.is_none();
    let on_path = if full {
        read.leaf_id
            .map(|leaf| entry_ancestors(&entries, leaf))
            .unwrap_or_default()
    } else {
        delta_active_path(&entries, read.leaf_id, &session.path_tail)
    };
    // A full read starts after the last entry already processed for this
    // session; ids are append-ordered, so everything before it is done.
    let start = if full {
        session
            .last_entry_id
            .as_deref()
            .and_then(|id| entries.iter().position(|entry| entry.id == id))
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    // Hydrate only selection metadata before the cursor; never reproject old assistant entries.
    if full {
        session.route_selected = false;
        for entry in entries.iter().take(start).filter(|e| on_path.contains(e.id.as_str())) {
            if entry.kind == "model_change" { session.route_selected = is_auto_selection(entry.json); }
        }
    }
    let context = LocalContext::new(scope);
    let mut outcome = KotaLocalIngestOutcome::default();
    let mut events = Vec::new();
    let new_entries = &entries[start..];
    let retried = retried_entry_ids(new_entries);
    if let Some(held) = session.held_error.take() {
        if retried.contains(held.entry_id.as_str()) {
            // Pi retried that attempt; its error was not the outcome.
        } else if full || session.settled || new_entries.iter().any(continues_conversation) {
            events.push(held_error_event(&context, scope, &held));
        } else {
            session.held_error = Some(held);
        }
    }
    for entry in new_entries {
        outcome.examined += 1;
        if let Some(input) = entry_kota_input(entry) {
            if input.custom_type != KOTA_USER_CUSTOM_TYPE
                && input.target_agent_id == scope.agent_id
                && push_receipt(
                    state,
                    LocalReceipt {
                        event_id: input.message_id.clone(),
                        agent_id: input.target_agent_id.clone(),
                        timestamp: normalize_timestamp(&entry.timestamp),
                    },
                )
            {
                outcome.receipts += 1;
            }
        }
        if !on_path.contains(entry.id.as_str()) {
            outcome.off_path += 1;
            continue;
        }
        outcome.on_path += 1;
        match entry.kind.as_str() {
            "message" => {
                let (message_events, error_line) =
                    message_entry_events(&context, scope, entry, &session.effort, session.route_selected, &mut outcome);
                events.extend(message_events);
                if let Some(line) = error_line {
                    if retried.contains(entry.id.as_str()) {
                        // A retried attempt; the retry decides the outcome.
                    } else if !full && !session.settled && read.leaf_id == Some(entry.id.as_str()) {
                        session.held_error = Some(HeldErrorLine {
                            entry_id: entry.id.clone(),
                            timestamp: line.timestamp.clone(),
                            text: line.text.clone(),
                            model: line.model.clone(),
                            effort: line.effort.clone(),
                            route: line.route.clone(),
                        });
                    } else {
                        events.push(line);
                    }
                }
            }
            "custom_message" => {
                if let Some(input) = entry_kota_input(entry) {
                    events.extend(user_input_events(
                        &context,
                        scope,
                        &input,
                        &entry.timestamp,
                        &mut outcome,
                    ));
                }
            }
            "compaction" => events.push(compaction_event(&context, scope, entry)),
            "thinking_level_change" => {
                session.effort = json_string(entry.json, &["thinkingLevel"]);
            }
            "model_change" => { session.route_selected = is_auto_selection(entry.json); }
            _ => {}
        }
    }

    let projected = project_events(scope, events, spans)?;
    outcome.written = projected.written;
    outcome.skipped_private += projected.skipped_private;
    outcome.changed_paths = projected.changed_paths;

    if let Some(last) = entries.last() {
        session.last_entry_id = Some(last.id.clone());
    }
    if let Some(leaf) = read.leaf_id {
        session.leaf_id = Some(leaf.to_string());
    }
    let mut tail = if full {
        Vec::new()
    } else {
        std::mem::take(&mut session.path_tail)
    };
    tail.extend(
        entries
            .iter()
            .filter(|entry| on_path.contains(entry.id.as_str()))
            .map(|entry| entry.id.clone()),
    );
    if tail.len() > KOTA_LOCAL_PATH_TAIL_LIMIT {
        tail.drain(..tail.len() - KOTA_LOCAL_PATH_TAIL_LIMIT);
    }
    session.path_tail = tail;
    outcome.cursor = session.last_entry_id.clone();
    state.sessions.insert(scope.session_id.clone(), session);
    if outcome.receipts > 0 {
        outcome
            .changed_paths
            .push(local_state_path(&scope.project_root, &scope.agent_id));
    }
    Ok(outcome)
}

fn is_auto_selection(entry: &JsonValue) -> bool {
    entry["provider"] == crate::local_rpc::auto_route::PROVIDER && entry["modelId"] == crate::local_rpc::auto_route::MODEL
}

fn message_entry_events(
    context: &LocalContext,
    scope: &KotaLocalScope,
    entry: &LocalEntry<'_>,
    effort: &Option<String>,
    route_selected: bool,
    outcome: &mut KotaLocalIngestOutcome,
) -> (Vec<NativeEvent>, Option<NativeEvent>) {
    let null = JsonValue::Null;
    let message = entry.json.get("message").unwrap_or(&null);
    let native_id = format!("{}/{}", scope.session_id, entry.id);
    match json_string(message, &["role"]).as_deref() {
        Some("user") => {
            let content = message.get("content").unwrap_or(&null);
            // Kota never prompts Pi with user text; a `/kota-inject` command
            // that reached the session is plumbing, never a room message.
            if is_kota_inject_command(content) {
                return (Vec::new(), None);
            }
            let events = content_blocks_to_events(
                &context.agent,
                &context.source,
                "user",
                &entry.timestamp,
                &native_id,
                content,
            );
            (events, None)
        }
        Some("assistant") => assistant_events(context, entry, message, &native_id, effort, route_selected),
        Some("custom") => (
            entry_kota_input(entry)
                .map(|input| user_input_events(context, scope, &input, &entry.timestamp, outcome))
                .unwrap_or_default(),
            None,
        ),
        // toolResult / bashExecution / system stay out of the room until the
        // console phase; thinking and tool blocks are dropped by room_event_for.
        _ => (Vec::new(), None),
    }
}

fn assistant_events(
    context: &LocalContext,
    entry: &LocalEntry<'_>,
    message: &JsonValue,
    native_id: &str,
    effort: &Option<String>,
    route_selected: bool,
) -> (Vec<NativeEvent>, Option<NativeEvent>) {
    let stop_reason = json_string(message, &["stopReason"]);
    let mut events = content_blocks_to_events(
        &context.agent,
        &context.source,
        "assistant",
        &entry.timestamp,
        native_id,
        message.get("content").unwrap_or(&JsonValue::Null),
    );
    // Same final-stopReason classification as the Pi file parser.
    if stop_reason.as_deref().is_some_and(|reason| {
        reason.eq_ignore_ascii_case("toolUse") || reason.eq_ignore_ascii_case("tool_use")
    }) {
        for event in &mut events {
            if event.kind == "message" {
                event.kind = "commentary".into();
            }
        }
    }
    if let Some(reason) = stop_reason.as_deref() {
        let signal = work_signal_for_stop_reason(reason);
        for event in &mut events {
            event.work_signal = Some(signal.to_string());
            event.stop_reason = Some(reason.to_string());
        }
    }
    // Pi 1.0 stamps each assistant message with its thinking level.
    let caption = MessageModel {
        route: route_selected.then(|| "auto".into()),
        model: json_string(message, &["model"]),
        effort: json_string(message, &["thinkingLevel"]).or_else(|| effort.clone()),
        ..MessageModel::default()
    };
    for event in &mut events {
        caption.apply(event);
    }
    // Pi persists every failed attempt, so the error line comes from the
    // durable entry and dedupes with it.
    let error_line = (stop_reason.as_deref() == Some("error")).then(|| {
        let mut line = event(
            &context.agent,
            &context.source,
            "assistant",
            "message",
            &entry.timestamp,
            &format!("{native_id}:error"),
            model_error_text(message, route_selected),
        );
        line.work_signal = Some("failed".into());
        line.stop_reason = Some("error".into());
        caption.apply(&mut line);
        line
    });
    (events, error_line)
}

fn held_error_event(
    context: &LocalContext,
    scope: &KotaLocalScope,
    held: &HeldErrorLine,
) -> NativeEvent {
    let mut line = event(
        &context.agent,
        &context.source,
        "assistant",
        "message",
        &held.timestamp,
        &format!("{}/{}:error", scope.session_id, held.entry_id),
        held.text.clone(),
    );
    line.work_signal = Some("failed".into());
    line.stop_reason = Some("error".into());
    line.model = held.model.clone();
    line.effort = held.effort.clone();
    line.route = held.route.clone();
    line
}

/// Pi removes a failed attempt it is about to retry from the context with
/// `context_edit { targetId, replacement: null }`.
fn retried_entry_ids(entries: &[LocalEntry<'_>]) -> HashSet<String> {
    entries
        .iter()
        .filter(|entry| {
            entry.kind == "context_edit"
                && entry.json.get("replacement").is_some_and(JsonValue::is_null)
        })
        .filter_map(|entry| json_string(entry.json, &["targetId"]))
        .collect()
}

/// A later user / assistant / Kota input means a held failure was final.
fn continues_conversation(entry: &LocalEntry<'_>) -> bool {
    match entry.kind.as_str() {
        "custom_message" => true,
        "message" => matches!(
            json_string(entry.json, &["message", "role"]).as_deref(),
            Some("user" | "assistant" | "custom")
        ),
        _ => false,
    }
}

/// A consumed `kota.user` input becomes a room user record shaped like a PTY
/// native user prompt, stamped with the original input time. Other Kota
/// inputs already have their actor bubble and only confirm receipts.
fn user_input_events(
    context: &LocalContext,
    scope: &KotaLocalScope,
    input: &KotaInput,
    fallback_timestamp: &str,
    outcome: &mut KotaLocalIngestOutcome,
) -> Vec<NativeEvent> {
    if input.custom_type != KOTA_USER_CUSTOM_TYPE || input.target_agent_id != scope.agent_id {
        return Vec::new();
    }
    if input.private {
        outcome.skipped_private += 1;
        return Vec::new();
    }
    let timestamp = if input.created_at.is_empty() {
        fallback_timestamp
    } else {
        input.created_at.as_str()
    };
    content_blocks_to_events(
        &context.agent,
        &context.source,
        "user",
        timestamp,
        &input.kota_id,
        &input.content,
    )
}

fn compaction_event(
    context: &LocalContext,
    scope: &KotaLocalScope,
    entry: &LocalEntry<'_>,
) -> NativeEvent {
    let mut event = control_event(
        &context.agent,
        &context.source,
        &entry.timestamp,
        &format!("{}/{}", scope.session_id, entry.id),
        "completed",
        Some("compacted".into()),
        None,
    );
    event.kind = "compaction".into();
    event.text = json_string(entry.json, &["summary"])
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| "Conversation compacted".into());
    event
}

fn fault_event(context: &LocalContext, scope: &KotaLocalScope, text: String) -> NativeEvent {
    let timestamp = now_iso();
    let id = format!(
        "{}/fault/{}",
        scope.session_id,
        short_hash(&format!("{timestamp}\u{1f}{text}"))
    );
    let mut line = event(
        &context.agent,
        &context.source,
        "assistant",
        "message",
        &timestamp,
        &id,
        text,
    );
    line.work_signal = Some("failed".into());
    line
}

/// Local records carry exact identities (`<sessionId>/<entryId>`, kotaId,
/// fault ids). The provider pipeline's text-and-time bucketing would merge
/// distinct replies that read the same and make the result depend on how the
/// runtime batched its reads, so only exact repeats are dropped here.
fn dedupe_local_events(events: Vec<NativeEvent>) -> Vec<NativeEvent> {
    let mut seen = HashSet::new();
    events
        .into_iter()
        .filter(|event| {
            event
                .native_event_id
                .as_ref()
                .is_none_or(|id| seen.insert(id.clone()))
        })
        .collect()
}

/// The room pipeline `sync_project` applies to parsed native events, with
/// exact-identity dedupe in place of text bucketing.
fn project_events(
    scope: &KotaLocalScope,
    events: Vec<NativeEvent>,
    spans: &[PrivacySpan],
) -> Result<Projected, String> {
    let mut projected = Projected {
        written: 0,
        skipped_private: 0,
        changed_paths: Vec::new(),
    };
    if events.is_empty() {
        return Ok(projected);
    }
    let events = expand_shell_handoffs(events);
    let events = filter_internal_agent_bus_envelopes(filter_bootstrap_noise(events));
    let (visible, skipped) = partition_private(events, spans);
    projected.skipped_private = skipped;
    let mut visible = dedupe_local_events(visible);
    overflow::finalize_events(&scope.project_root, &mut visible, spans);
    let (room_events, _shared_events) = split_for_violet_outputs(visible, &scope.project_root);
    let messages = room_events
        .into_iter()
        .map(event_to_message)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Ok(projected);
    }
    projected.changed_paths = write_chathistory_messages(&scope.project_root, &messages)?;
    if let Err(err) = write_turn_credit_events_for_messages(&scope.project_root, &messages) {
        eprintln!(
            "Kota Violet turn credit sync failed for {}: {err}",
            scope.project_root.display()
        );
    }
    projected.written = messages.len();
    Ok(projected)
}

fn parse_entries(values: &[JsonValue]) -> Vec<LocalEntry<'_>> {
    values
        .iter()
        .filter_map(|json| {
            let id = json_string(json, &["id"])?;
            let kind = json_string(json, &["type"]).unwrap_or_default();
            if kind == "session" {
                return None;
            }
            Some(LocalEntry {
                id,
                parent_id: json_string(json, &["parentId"]),
                timestamp: json_string(json, &["timestamp"])
                    .or_else(|| pi_message_timestamp(json))
                    .unwrap_or_else(now_iso),
                kind,
                json,
            })
        })
        .collect()
}

fn entry_ancestors(entries: &[LocalEntry<'_>], leaf: &str) -> HashSet<String> {
    let by_id = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut path = HashSet::new();
    let mut current = Some(leaf);
    while let Some(id) = current {
        let Some(entry) = by_id.get(id) else {
            break;
        };
        if !path.insert(entry.id.clone()) {
            break;
        }
        current = entry.parent_id.as_deref();
    }
    path
}

/// Active-path membership for a `since` read. get_entries also returns
/// abandoned branches; only the leaf's ancestry is projected, matching the Pi
/// file parser. A leaf that is neither in the batch nor on the known path is
/// inconsistent, and dropping the batch would lose content, so project it all.
fn delta_active_path(
    entries: &[LocalEntry<'_>],
    leaf_id: Option<&str>,
    known_tail: &[String],
) -> HashSet<String> {
    let Some(leaf) = leaf_id else {
        return HashSet::new();
    };
    let path = entry_ancestors(entries, leaf);
    if path.is_empty() && !entries.is_empty() && !known_tail.iter().any(|id| id == leaf) {
        kota_debug_log(&format!(
            "[violet] kota-local leaf {leaf} is outside the read and the known path"
        ));
        return entries.iter().map(|entry| entry.id.clone()).collect();
    }
    path
}

fn entry_kota_input(entry: &LocalEntry<'_>) -> Option<KotaInput> {
    match entry.kind.as_str() {
        "custom_message" | "message" => kota_input_from(entry.json),
        _ => None,
    }
}

/// Kota inputs are recognised only through the runtime's single parser,
/// `local_rpc::custom_receipt` (CONTRACT-INJECT §2): custom_message entries,
/// live `message_end` events, and legacy session entries holding a custom
/// message. The projected content is taken from that same message.
fn kota_input_from(value: &JsonValue) -> Option<KotaInput> {
    let message = match json_string(value, &["type"]).as_deref() {
        Some("message") => value.get("message")?,
        _ => value,
    };
    let receipt = crate::local_rpc::custom_receipt(message)?;
    if receipt.custom_type == crate::local_rpc::shadow_clone::REPORT_TYPE { return None; }
    let content = if json_string(message, &["type"]).as_deref() == Some("message_end") {
        message.get("message").and_then(|inner| inner.get("content"))
    } else {
        message.get("content")
    };
    Some(KotaInput {
        kota_id: receipt.kota_id,
        custom_type: receipt.custom_type,
        message_id: receipt.details.message_id,
        target_agent_id: receipt.details.target_agent_id,
        created_at: receipt.details.created_at,
        private: receipt.details.private,
        content: content.cloned().unwrap_or(JsonValue::Null),
    })
}

fn valid_kota_id_part(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn is_kota_inject_command(content: &JsonValue) -> bool {
    text_from_json(content).is_some_and(|text| {
        text.trim_start()
            .strip_prefix("/kota-inject")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

/// Queue command errors are handled by the runtime, never rendered twice.
fn is_kota_queue_command_error(event: &JsonValue) -> bool {
    json_string(event, &["extensionPath"]).as_deref() == Some("command:kota-inject")
        || matches!(json_string(event, &["extensionPath"]).as_deref(),
            Some("command:kota-cancel" | "command:kota-clear-held" | "command:kota-hold-pause" | "command:kota-hold-resume" | "command:kota-reorder"
                | "command:kota-auto-route" | "command:kota-mcp"))
}

fn message_millis_timestamp(message: &JsonValue) -> Option<String> {
    let millis = message.get("timestamp").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|millis| i64::try_from(millis).ok()))
    })?;
    millis_to_iso(millis)
}

fn model_error_text(message: &JsonValue, route_selected: bool) -> String {
    if route_selected {
        if let Some(raw) = json_string(message, &["errorMessage"]) {
            if matches!(raw.as_str(), "Jev authentication failed." | "Jev did not respond within 3 seconds."
                | "Could not reach Jev." | "No models are configured for this Auto String."
                // Preserve old final-error copy when reading existing sessions; never rename history.
                | "No models are configured for this Auto Route."
                | "All configured models failed for this turn.") { return raw; }
        }
    }
    let model = match (
        json_string(message, &["provider"]),
        json_string(message, &["model"]),
    ) {
        (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
        (None, Some(model)) => Some(model),
        _ => None,
    };
    let detail = fault_detail(
        json_string(message, &["errorMessage"]).map(|raw| provider_error_detail(&raw)),
    );
    sentence(match (model, detail) {
        (Some(model), Some(detail)) => format!("Kota Local error from {model}: {detail}"),
        (None, Some(detail)) => format!("Kota Local error: {detail}"),
        (Some(model), None) => format!("Kota Local error from {model}"),
        (None, None) => "Kota Local model request failed".into(),
    })
}

/// Pi reports provider failures as `<status>: <body>`; show the body's
/// message with the HTTP status, like the OpenCode error line.
fn provider_error_detail(raw: &str) -> String {
    let raw = raw.trim();
    let Some((status, body)) = raw.split_once(": ") else {
        return raw.to_string();
    };
    if status.len() != 3 || !status.bytes().all(|byte| byte.is_ascii_digit()) {
        return raw.to_string();
    }
    let Ok(json) = serde_json::from_str::<JsonValue>(body) else {
        return raw.to_string();
    };
    json_string(&json, &["message"])
        .or_else(|| json_string(&json, &["error", "message"]))
        .map(|message| message.trim().trim_end_matches('.').to_string())
        .filter(|message| !message.is_empty())
        .map_or_else(|| raw.to_string(), |message| format!("{message} (HTTP {status})"))
}

fn compaction_fault_text(event: &JsonValue) -> Option<String> {
    if json_bool(event, &["aborted"]) == Some(true) || json_bool(event, &["willRetry"]) == Some(true)
    {
        return None;
    }
    let detail = fault_detail(json_string(event, &["errorMessage"]))?;
    Some(sentence(format!("Kota Local compaction failed: {detail}")))
}

/// Extension errors can carry arbitrary exception text and paths; the room
/// only names the failing hook.
fn extension_fault_text(event: &JsonValue) -> String {
    let hook = json_string(event, &["event"])
        .filter(|hook| valid_kota_id_part(hook, 64))
        .unwrap_or_else(|| "extension".into());
    format!("Kota Local extension error in the {hook} handler.")
}

fn delivery_fault_text(delivery: &KotaLocalDelivery<'_>) -> String {
    let label = match delivery.custom_type {
        "kota.lm" => "Telegram message",
        "kota.bus" => "Agent Bus message",
        "kota.bbs" => "BBS notice",
        "kota.ember" => "Ember reminder",
        "kota.dream" => "Dream prompt",
        "kota.bartender" => "Bartender notice",
        _ => "message",
    };
    format!(
        "Kota Local could not deliver {label} {}.",
        delivery.message_id
    )
}

fn fault_detail(detail: Option<String>) -> Option<String> {
    detail
        .map(|detail| truncate_chars(&one_line(&detail), KOTA_LOCAL_FAULT_DETAIL_LIMIT))
        .filter(|detail| !detail.is_empty())
}

fn sentence(text: String) -> String {
    let trimmed = text.trim_end();
    if trimmed.ends_with(|ch: char| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？')) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

fn short_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

/// Accept live notices only from the newest child epoch of this App process.
/// Entries are durable facts and are ingested regardless, but never move the
/// current session back. The runtime's own fence is primary; this is a guard.
fn adopt_scope(state: &mut LocalState, scope: &KotaLocalScope) -> bool {
    let epochs = KOTA_LOCAL_PROCESS_EPOCHS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut epochs = epochs
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let highest = epochs
        .entry((scope.project_root.clone(), scope.agent_id.clone()))
        .or_insert(scope.epoch);
    if scope.epoch < *highest {
        return false;
    }
    *highest = scope.epoch;
    state.session_id = Some(scope.session_id.clone());
    state.process_start = Some(KOTA_LOCAL_PROCESS_START.get_or_init(now_iso).clone());
    true
}

/// Test hook: a new App process starts with no epoch history.
#[cfg(test)]
fn forget_process_epochs(project_root: &Path, agent_id: &str) {
    if let Some(epochs) = KOTA_LOCAL_PROCESS_EPOCHS.get() {
        epochs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(project_root.to_path_buf(), agent_id.to_string()));
    }
}

/// Applies the ledger state reported for one kotaId. Notices arrive in order;
/// a consumed or durable item is never shown as pending again.
fn apply_delivery(
    state: &mut LocalState,
    scope: &KotaLocalScope,
    delivery: &KotaLocalDelivery<'_>,
) -> bool {
    let at = normalize_timestamp(delivery.at);
    if let Some(record) = state
        .deliveries
        .iter_mut()
        .find(|record| record.session_id == scope.session_id && record.kota_id == delivery.kota_id)
    {
        if record.state == delivery.state
            || (record.state.confirmed() && !delivery.state.confirmed())
        {
            return false;
        }
        record.state = delivery.state;
        record.at = at;
    } else {
        state.deliveries.push(LocalDeliveryRecord {
            session_id: scope.session_id.clone(),
            kota_id: delivery.kota_id.to_string(),
            message_id: delivery.message_id.to_string(),
            target_agent_id: scope.agent_id.clone(),
            state: delivery.state,
            at,
        });
    }
    let settled = state
        .deliveries
        .iter()
        .filter(|record| !record.state.pending())
        .count();
    if settled > KOTA_LOCAL_SETTLED_DELIVERY_LIMIT {
        let mut excess = settled - KOTA_LOCAL_SETTLED_DELIVERY_LIMIT;
        state.deliveries.retain(|record| {
            if excess > 0 && !record.state.pending() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    true
}

fn pending_deliveries(state: &LocalState) -> Vec<LocalPendingDelivery> {
    if state.process_start.as_deref() != Some(KOTA_LOCAL_PROCESS_START.get_or_init(now_iso).as_str()) {
        return Vec::new();
    }
    let Some(session_id) = state.session_id.as_deref() else {
        return Vec::new();
    };
    let mut pending: Vec<LocalPendingDelivery> = Vec::new();
    for record in state
        .deliveries
        .iter()
        .filter(|record| record.session_id == session_id && record.state.pending())
    {
        let item = LocalPendingDelivery {
            message_id: record.message_id.clone(),
            target_agent_id: record.target_agent_id.clone(),
        };
        if !pending.contains(&item) {
            pending.push(item);
        }
    }
    pending
}

fn push_receipt(state: &mut LocalState, receipt: LocalReceipt) -> bool {
    if state.receipts.iter().any(|existing| {
        existing.event_id == receipt.event_id && existing.agent_id == receipt.agent_id
    }) {
        return false;
    }
    state.receipts.push(receipt);
    if state.receipts.len() > KOTA_LOCAL_RECEIPT_LIMIT {
        let excess = state.receipts.len() - KOTA_LOCAL_RECEIPT_LIMIT;
        state.receipts.drain(..excess);
    }
    true
}

fn remember_fault(state: &mut LocalState, key: String) -> bool {
    if state.fault_keys.contains(&key) {
        return false;
    }
    state.fault_keys.push(key);
    if state.fault_keys.len() > KOTA_LOCAL_FAULT_KEY_LIMIT {
        let excess = state.fault_keys.len() - KOTA_LOCAL_FAULT_KEY_LIMIT;
        state.fault_keys.drain(..excess);
    }
    true
}

fn source_status(agent: &ProjectAgent, state: &LocalState) -> VioletSourceStatus {
    let mut status = VioletSourceStatus {
        agent_id: agent.agent_id.clone(),
        shell: agent.shell.clone(),
        session_id: state.session_id.clone().or_else(|| agent.session_id.clone()),
        source_kind: KOTA_LOCAL_SOURCE_KIND.into(),
        source_path: None,
        status: "missing".into(),
        parsed: 0,
        written: 0,
        skipped_private: 0,
        error: None,
    };
    if let Some(last) = &state.last_ingest {
        status.parsed = last.examined;
        status.written = last.written;
        status.skipped_private = last.skipped_private;
        status.status = if last.error.is_some() { "error" } else { "synced" }.into();
        status.error = last.error.clone();
    }
    status
}

fn check_scope(scope: &KotaLocalScope) -> Result<(), String> {
    if !scope.project_root.is_absolute() {
        return Err(format!(
            "Kota Local scope needs an absolute project root: {}",
            scope.project_root.display()
        ));
    }
    if scope.agent_id.trim().is_empty() || scope.session_id.trim().is_empty() {
        return Err("Kota Local scope needs an agent id and a session id.".into());
    }
    Ok(())
}

fn local_session_dir(scope: &KotaLocalScope) -> PathBuf {
    let project = scope
        .project_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    crate::kota_home_dir()
        .join("kota-local")
        .join(project)
        .join(&scope.agent_id)
        .join("sessions")
}

fn local_state_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("kota-local")
}

fn local_state_path(project_root: &Path, agent_id: &str) -> PathBuf {
    local_state_dir(project_root).join(format!("{}.json", sanitize_path_segment(agent_id)))
}

fn read_state(path: &Path) -> LocalState {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            kota_debug_log(&format!(
                "[violet] kota-local state {} unreadable: {err}",
                path.display()
            ));
            LocalState::default()
        }),
        Err(_) => LocalState::default(),
    }
}

/// Serializes read-modify-write of one agent's Local state. State is saved
/// only when `apply` succeeds, after any room writes it made.
fn with_state<T>(
    project_root: &Path,
    agent_id: &str,
    apply: impl FnOnce(&mut LocalState) -> Result<T, String>,
) -> Result<T, String> {
    let path = local_state_path(project_root, agent_id);
    let lock = {
        let locks = KOTA_LOCAL_STATE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut locks = locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(path.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state(&path);
    state.version = KOTA_LOCAL_STATE_VERSION;
    state.agent_id = agent_id.to_string();
    let result = apply(&mut state)?;
    let mut bytes = serde_json::to_vec_pretty(&state)
        .map_err(|err| format!("serialize {}: {err}", path.display()))?;
    bytes.push(b'\n');
    write_if_changed(&path, &bytes)?;
    Ok(result)
}

fn kota_debug_log(message: &str) {
    crate::kota_debug_log(message);
}

/// Violet's projection of one Local runtime, installed once through
/// `local_rpc::register_sink_factory`. The runtime calls it on its serialized
/// notifier thread; a room change is emitted after each successful write.
struct VioletLocalSink {
    app: Option<AppHandle>,
}

pub(crate) fn kota_local_sink(app: AppHandle) -> Arc<dyn crate::local_rpc::Sink> {
    Arc::new(VioletLocalSink { app: Some(app) })
}

impl crate::local_rpc::Sink for VioletLocalSink {
    fn notify(&self, notice: &crate::local_rpc::LocalNotice) -> Result<(), String> {
        use crate::local_rpc::{LocalNotice, OnlineState};
        let (scope, paths) = match notice {
            LocalNotice::Entries { scope, entries, leaf_id, since, recovery } => {
                let read = KotaLocalEntriesRead {
                    entries,
                    leaf_id: leaf_id.as_deref(),
                    since: since.as_deref(),
                    recovery: *recovery,
                };
                (scope, ingest_entries(scope, &read)?.changed_paths)
            }
            LocalNotice::Event { scope, event } => (scope, record_live_event(scope, event)?),
            LocalNotice::Delivery { scope, kota_id, message_id, state, at, custom_type } => {
                let delivery = KotaLocalDelivery {
                    kota_id,
                    message_id,
                    state: *state,
                    at,
                    custom_type,
                };
                (scope, record_delivery(scope, &delivery)?)
            }
            LocalNotice::Status { scope, state, queued, compacting, error } => {
                let state = match state {
                    OnlineState::Offline => "offline",
                    OnlineState::Starting => "starting",
                    OnlineState::Ready => "ready",
                    OnlineState::Busy => "busy",
                    OnlineState::Error => "error",
                };
                let status = KotaLocalRuntimeStatus {
                    state,
                    queued: *queued,
                    compacting: *compacting,
                    error: error.as_deref(),
                };
                (scope, record_runtime_status(scope, &status)?)
            }
        };
        if let Some(app) = self.app.as_ref().filter(|_| !paths.is_empty()) {
            emit_room_changed(app, &scope.project_root, "kota-local", paths);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "kota_local_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "kota_local_pi_tests.rs"]
mod pi_tests;
