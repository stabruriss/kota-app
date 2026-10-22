//! Ephemeral, bounded display state. Never an acknowledgement or history source.
use super::{ledger::Ledger, protocol::known_custom_type, DeliveryState, Injection, LocalScope, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TEXT_LIMIT: usize = 256 * 1024;
const BLOCK_LIMIT: usize = 16 * 1024;
const BLOCKS: usize = 200;
const QUEUE_LIMIT: usize = 100;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Block {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub text: String,
    pub active: bool,
    pub error: bool,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueItem {
    pub kota_id: String,
    pub custom_type: String,
    pub sender_id: Option<String>,
    pub created_at: String,
    pub state: DeliveryState,
    pub handed: bool,
    pub summary: String,
    pub truncated: bool,
}

/// Auto Route facts for the current run. The mode and fallback come from the router; the model
/// and thinking level are replaced by each reply's own recorded fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteTurn { pub selected_model: String, pub route_model: String, pub fallback: bool, pub provider: String, pub model: String, pub thinking_level: String }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteSelected { pub provider: String, pub model: String, pub thinking_level: String }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteCandidate { pub route_model: String, pub provider: String, pub model: String, pub probability: f64 }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RouteEventData {
    Selection { selected: RouteSelected, candidate: RouteCandidate, threshold: f64, outcome: String },
    ModelError { provider: String, model: String, message: String },
    JevError { code: String, message: String },
    StringError { code: String, message: String, #[serde(skip_serializing_if = "Option::is_none")] provider: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] model: Option<String> },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteEvent { pub id: u64, pub occurred_at: String, #[serde(flatten)] pub data: RouteEventData }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteError { pub code: String, pub event_id: Option<u64> }
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteView {
    pub current_model: Option<String>, pub turn: Option<RouteTurn>,
    pub events: Vec<RouteEvent>, pub error: Option<RouteError>,
}
const ROUTE_EVENTS: usize = 15;
const ROUTE_ERRORS: [&str; 5] = ["jev_auth", "jev_timeout", "jev_unreachable", "route_empty", "route_exhausted"];

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cursor { pub scope: LocalScope, pub revision: u64, #[serde(default)] pub queue_revision: Option<u64> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub revision: u64,
    pub run_id: u64,
    pub started_at: Option<String>,
    pub phase: String,
    pub stats_revision: u64,
    pub context_estimate: Option<f64>,
    pub dropped: usize,
    pub queue_total: usize,
    pub queue_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Block>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<QueueItem>>,
    /// Present only while `kota/auto` is the selected model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_route: Option<RouteView>,
}

#[derive(Default)]
pub(super) struct Workbench {
    revision: u64,
    run: u64,
    message: u64,
    started_at: Option<String>,
    phase: String,
    stats_revision: u64,
    // Successful Pi compaction-end estimate, scoped to this runtime's view.
    context_estimate: Option<f64>,
    dropped: usize,
    blocks: Vec<Block>,
    queue: Vec<QueueItem>,
    queue_total: usize,
    queue_revision: u64,
    route: RouteView,
    route_sequence: u64,
    route_run_start: u64,
    // File status survives agent_start ordering, but is shown only for the selected virtual model.
    prompt_warning: Option<(String, String)>,
}

/// Only text blocks cross the view boundary. Images, signatures and arbitrary
/// tool metadata (including base64) are never serialized into this projection.
pub(super) fn plain_text(value: &Value, limit: usize) -> (String, bool) {
    let mut text = String::new();
    let mut truncated = false;
    let mut append = |part: &str| {
        if !text.is_empty() && text.len() < limit { text.push('\n'); }
        let (part, cut) = clipped(part, limit.saturating_sub(text.len()));
        text.push_str(part);
        truncated |= cut;
    };
    if let Some(value) = value.as_str() { append(value); }
    else if let Some(parts) = value.as_array() {
        for part in parts {
            if part["type"] == "text" {
                if let Some(value) = part["text"].as_str() { append(value); }
            }
        }
    }
    (text, truncated)
}

/// Queue-only display projection. Never rewrite the stored or submitted input.
/// A single text block may accompany images; multiple text parts are ambiguous
/// wrapper boundaries, so keep the existing plain-text projection for those.
pub(super) fn queue_text(input: &Injection, limit: usize) -> (String, bool) {
    let text = if let Some(text) = input.content.as_str() { Some(text) }
    else if let Some(parts) = input.content.as_array() {
        let mut texts = parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str());
        let first = texts.next();
        if texts.next().is_none() { first } else { None }
    } else { None };
    let Some(text) = text.filter(|_| known_custom_type(&input.custom_type)) else {
        return plain_text(&input.content, limit);
    };
    let body = queue_body(text, input).unwrap_or(text);
    let (text, truncated) = clipped(body, limit);
    (text.into(), truncated)
}

fn queue_body<'a>(mut text: &'a str, input: &Injection) -> Option<&'a str> {
    if text.starts_with("<KOTA_HANDOFF") {
        text = crate::shell_switch::split_inline(text, &input.details.target_agent_id)?.1;
    }
    // Soul's marks envelope comes next; peeled by syntax, like the gap below.
    if text.starts_with("<KOTA_MARKS v=\"1\" revision=\"") {
        text = text.split_once("\n</KOTA_MARKS>\n")?.1;
    }
    // Composer never receives an Actor envelope. Even a complete, literal
    // KOTA_MESSAGE in user input must not be interpreted as one.
    if input.custom_type != "kota.user" { text = actor_body(text, input)?; }
    if text.starts_with("<KOTA_TEMPORAL_GAP") {
        if !crate::temporal_context::is_temporal_gap_prompt(text) { return None; }
        let mut lines = text.splitn(4, '\n');
        let header = lines.next()?;
        let time = header.strip_prefix("<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"")?.strip_suffix("\">")?;
        if !rendered_time(time) { return None; }
        lines.next()?;
        if lines.next()? != "</KOTA_TEMPORAL_GAP>" { return None; }
        text = lines.next()?;
    }
    Some(text)
}

fn actor_body<'a>(text: &'a str, input: &Injection) -> Option<&'a str> {
    let d = &input.details;
    let prefix = format!("<KOTA_MESSAGE id=\"{}\" from=\"{}\" to=\"{}\" intent=\"{}\"",
        crate::agent_bus::escape_attr(&d.message_id), crate::agent_bus::escape_attr(d.source_actor_id.as_deref()?),
        crate::agent_bus::escape_attr(&d.target_agent_id), crate::agent_bus::escape_attr(d.source_intent.as_deref()?));
    let (timing, rest) = text.strip_prefix(&prefix)?.split_once(">\n")?;
    if !timing.is_empty() && !actor_timing(timing, input) { return None; }
    let body = rest.strip_suffix("\n</KOTA_MESSAGE>")?;
    // Find one outer envelope, without peeling or modifying quoted/nested
    // tags in its body. An early outer close means extra/multiple envelopes.
    let mut depth = 1usize;
    for line in body.lines() {
        if line == "<KOTA_MESSAGE>" || (line.starts_with("<KOTA_MESSAGE ") && line.ends_with('>')) { depth += 1; }
        else if line == "</KOTA_MESSAGE>" {
            depth -= 1;
            if depth == 0 { return None; }
        }
    }
    (depth == 1).then_some(body)
}

fn rendered_time(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).ok().is_some_and(|time|
        time.to_rfc3339_opts(chrono::SecondsFormat::Secs, false) == value)
}

fn actor_timing(timing: &str, input: &Injection) -> bool {
    // Only Ember reminders can receive these generator-owned attributes.
    if input.custom_type != "kota.ember" || input.details.source_actor_id.as_deref() != Some("ember")
        || input.details.source_intent.as_deref() != Some("reminder") { return false; }
    let Some((trigger, mut rest)) = timing.strip_prefix(" trigger=\"").and_then(|s| s.split_once('"')) else { return false; };
    match trigger {
        "scheduled" => {
            let Some((time, tail)) = rest.strip_prefix(" scheduled_for=\"").and_then(|s| s.split_once('"')) else { return false; };
            if !rendered_time(time) { return false; }
            rest = tail;
        },
        "idle" | "manual" => {},
        _ => return false,
    }
    let Some(time) = rest.strip_prefix(" current_time=\"").and_then(|s| s.strip_suffix('"')) else { return false; };
    rendered_time(time) && chrono::DateTime::parse_from_rfc3339(time).ok().zip(
        chrono::DateTime::parse_from_rfc3339(&input.details.created_at).ok()).is_some_and(|(rendered, created)|
            rendered.timestamp() == created.timestamp())
}

fn clipped(text: &str, limit: usize) -> (&str, bool) {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) { end -= 1; }
    (&text[..end], end < text.len())
}

// IDs/selection metadata must be valid as a whole; only error prose may be clipped.
fn route_text(value: &Value, key: &str, limit: usize) -> Option<String> {
    value[key].as_str().filter(|s| !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control)).map(str::to_owned)
}
fn route_selection(fact: &Value) -> Option<RouteEventData> {
    let selected = &fact["selected"];
    let candidate = &fact["candidate"];
    let outcome = fact["outcome"].as_str().filter(|s| matches!(*s, "selected" | "retained" | "default" | "manual"))?.to_owned();
    let threshold = fact["threshold"].as_f64().filter(|n| n.is_finite() && (0.0..=1.0).contains(n))?;
    let probability = candidate["probability"].as_f64().filter(|n| n.is_finite() && (0.0..=1.0).contains(n))?;
    let route_model = route_text(candidate, "routeModel", 16).filter(|m| matches!(m.as_str(), "model-1"|"model-2"|"model-3"|"model-4"|"model-5"|"model-6"))?;
    Some(RouteEventData::Selection {
        selected: RouteSelected { provider: route_text(selected, "provider", 128)?, model: route_text(selected, "model", 1024)?, thinking_level: route_text(selected, "thinkingLevel", 16)? },
        candidate: RouteCandidate { route_model, provider: route_text(candidate, "provider", 128)?, model: route_text(candidate, "model", 1024)?, probability },
        threshold, outcome,
    })
}

impl Workbench {
    pub fn read(&self, snapshot: Snapshot, since: Option<&Cursor>) -> View {
        let changed = since.is_none_or(|s| s.scope != snapshot.scope || s.revision != self.revision);
        let snapshot_scope = snapshot.scope.clone();
        let auto = snapshot.model.as_ref().is_some_and(|m| m["provider"] == super::auto_route::PROVIDER && m["id"] == super::auto_route::MODEL);
        View { snapshot, revision: self.revision, run_id: self.run, started_at: self.started_at.clone(),
            phase: self.phase.clone(), stats_revision: self.stats_revision, context_estimate: self.context_estimate, dropped: self.dropped,
            queue_total: self.queue_total, queue_revision: self.queue_revision, blocks: changed.then(|| {
                let mut blocks = self.blocks.clone();
                if auto {
                    if let Some((path, warning)) = &self.prompt_warning {
                        blocks.push(Block { id: "puppeteer-prompt".into(), kind: "status", label: "Puppeteer".into(),
                            text: format!("{warning}\n{path}"), active: false, error: false, truncated: false, started_at: None });
                        while blocks.len() > BLOCKS || blocks.iter().map(|b| b.text.len()).sum::<usize>() > TEXT_LIMIT {
                            blocks.remove(0);
                        }
                    }
                }
                blocks
            }),
            queue: since.is_none_or(|s| s.scope != snapshot_scope || s.queue_revision != Some(self.queue_revision)).then(|| self.queue.clone()),
            auto_route: (auto && changed).then(|| self.route.clone()) }
    }

    fn route_event(&mut self, data: RouteEventData) -> u64 {
        self.route_sequence += 1;
        let id = self.route_sequence;
        self.route.events.insert(0, RouteEvent { id, occurred_at: chrono::Utc::now().to_rfc3339(), data });
        self.route.events.truncate(ROUTE_EVENTS);
        id
    }

    /// A private `kota.route.v1:` fact from the router. Unknown or malformed facts are ignored.
    pub fn route_fact(&mut self, fact: &Value) {
        let text = |key: &str, limit: usize| fact[key].as_str().map(|s| clipped(s, limit).0.to_owned());
        let route_model = text("routeModel", 16).filter(|m| matches!(m.as_str(), "model-1"|"model-2"|"model-3"|"model-4"|"model-5"|"model-6"));
        match fact["event"].as_str() {
            Some("prompt_status") => {
                let Some(path) = text("path", 4096).filter(|p| !p.chars().any(char::is_control)) else { return; };
                let Some(status) = fact.get("warning") else { return; };
                self.prompt_warning = if status.is_null() { None } else {
                    let Some(warning) = fact["warning"].as_str().filter(|s| [crate::puppeteer_prompts::INVALID, crate::puppeteer_prompts::UNREADABLE].contains(s)) else { return; };
                    Some((path, warning.to_owned()))
                };
            },
            Some("decision") => {
                let selected_model = text("selectedModel", 16).filter(|m| matches!(m.as_str(), "model-1"|"model-2"|"model-3"|"model-4"|"model-5"|"model-6"));
                let (Some(selected_model), Some(route_model)) = (selected_model, route_model) else { return };
                self.route.current_model = Some(selected_model.clone());
                self.route.turn = Some(RouteTurn { fallback: fact["fallback"] == true && route_model != selected_model, selected_model, route_model,
                    provider: text("provider", 128).unwrap_or_default(), model: text("model", 1024).unwrap_or_default(),
                    thinking_level: text("thinkingLevel", 16).unwrap_or_default() });
                self.route.error = None;
            },
            Some("selection") => {
                let Some(data) = route_selection(fact) else { return; };
                self.route_event(data);
            },
            Some("model_error") => {
                let (Some(provider), Some(model), Some(message)) = (route_text(fact, "provider", 128), route_text(fact, "model", 1024), text("message", BLOCK_LIMIT)) else { return; };
                self.route_event(RouteEventData::ModelError { provider, model, message });
            },
            Some("error") => {
                let Some(code) = route_text(fact, "code", 32).filter(|c| ROUTE_ERRORS.contains(&c.as_str())) else { return };
                let event_id = if fact.get("kind").is_some() {
                    let Some(message) = text("message", BLOCK_LIMIT) else { return; };
                    let data = match fact["kind"].as_str() {
                        Some("jev_error") if code.starts_with("jev_") && fact.get("provider").is_none() && fact.get("model").is_none() => RouteEventData::JevError { code: code.clone(), message },
                        Some("string_error") if !code.starts_with("jev_") => {
                            let (provider, model) = match (fact.get("provider"), fact.get("model")) {
                                (None, None) => (None, None),
                                (Some(_), Some(_)) => {
                                    let (Some(provider), Some(model)) = (route_text(fact, "provider", 128), route_text(fact, "model", 1024)) else { return; };
                                    (Some(provider), Some(model))
                                },
                                _ => return,
                            };
                            RouteEventData::StringError { code: code.clone(), message, provider, model }
                        },
                        _ => return,
                    };
                    Some(self.route_event(data))
                } else if code == "route_exhausted" && fact.get("message").is_none() {
                    self.route.events.iter().find(|e| e.id > self.route_run_start && matches!(e.data, RouteEventData::ModelError { .. })).map(|e| e.id)
                } else { return; };
                self.route.error = Some(RouteError { code, event_id });
            },
            _ => return,
        }
        self.revision += 1;
    }

    pub fn clear_run(&mut self) {
        self.blocks.clear(); self.started_at = None; self.phase.clear(); self.dropped = 0;
        self.revision += 1;
    }

    pub fn refresh_queue(&mut self, ledger: &Ledger, session: &str) {
        let (count, entries) = ledger.queued_entries(session, QUEUE_LIMIT);
        self.queue_revision += 1;
        self.queue_total = count;
        self.queue = entries.into_iter().take(QUEUE_LIMIT).map(|e| {
            let (summary, truncated) = queue_text(&e.injection, 384);
            QueueItem { kota_id: e.injection.kota_id.clone(), custom_type: e.injection.custom_type.clone(),
                sender_id: e.injection.details.source_actor_id.clone(), created_at: e.injection.details.created_at.clone(),
                state: e.state, handed: self.queue.iter().any(|q| q.kota_id == e.injection.kota_id && q.handed), summary, truncated }
        }).collect();
        self.revision += 1;
    }

    pub fn mark_handed(&mut self, id: &str) {
        if let Some(item) = self.queue.iter_mut().find(|q| q.kota_id == id && !q.handed) {
            item.handed = true;
            self.queue_revision += 1; self.revision += 1;
        }
    }

    fn put(&mut self, id: String, kind: &'static str, label: &str, text: &str, active: bool, error: bool, append: bool) {
        let index = self.blocks.iter().position(|b| b.id == id).unwrap_or_else(|| {
            self.blocks.push(Block { id, kind, label: String::new(), text: String::new(), active, error, truncated: false,
                started_at: (kind == "thinking").then(|| chrono::Utc::now().to_rfc3339()) });
            self.blocks.len() - 1
        });
        let b = &mut self.blocks[index];
        b.kind = kind; b.label = clipped(label, 128).0.into(); b.active = active; b.error = error;
        if !append { b.text.clear(); b.truncated = false; }
        let (text, truncated) = clipped(text, BLOCK_LIMIT.saturating_sub(b.text.len()));
        b.text.push_str(text); b.truncated |= truncated;
        while self.blocks.len() > BLOCKS || self.blocks.iter().map(|b| b.text.len()).sum::<usize>() > TEXT_LIMIT {
            self.blocks.remove(0); self.dropped += 1;
        }
    }

    pub fn event(&mut self, event: &Value) {
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            "agent_start" => {
                // Retry/compaction may start another model request before settled.
                if self.started_at.is_none() {
                    self.clear_run();
                    self.run += 1; self.message = 0;
                    self.started_at = Some(chrono::Utc::now().to_rfc3339());
                    self.route.turn = None; self.route.error = None; self.route_run_start = self.route_sequence;
                }
                self.phase = "Working".into();
            },
            "agent_settled" => {
                // Keep one bounded completed run so the visible panel cannot
                // miss a final message between polls. The next run clears it.
                for block in &mut self.blocks { block.active = false; }
                self.started_at = None; self.phase.clear(); self.stats_revision += 1;
            },
            "message_start" if event["message"]["role"] == "assistant" => { self.message += 1; },
            "message_update" => {
                let e = &event["assistantMessageEvent"];
                let index = e["contentIndex"].as_u64().unwrap_or(0);
                let id = format!("{}:{}:{index}", self.run, self.message);
                let t = e["type"].as_str().unwrap_or_default();
                let (block_kind, label) = match t {
                    "thinking_start" | "thinking_delta" | "thinking_end" => ("thinking", "Thinking"),
                    "text_start" | "text_delta" | "text_end" => ("progress", "Output"),
                    _ => return,
                };
                self.phase = label.into();
                let append = t.ends_with("_delta");
                let text = e[if append { "delta" } else { "content" }].as_str().unwrap_or_default();
                self.put(id, block_kind, label, text, !t.ends_with("_end"), false, append);
            },
            "message_end" if event["message"]["role"] == "assistant" => {
                let message = &event["message"];
                // The reply's own record names the physical model that answered.
                if let (Some(turn), Some(provider), Some(model)) = (self.route.turn.as_mut(), message["provider"].as_str(), message["model"].as_str()) {
                    if provider != super::auto_route::PROVIDER && message["stopReason"] != "error" {
                        turn.provider = clipped(provider, 128).0.into(); turn.model = clipped(model, 1024).0.into();
                        if let Some(level) = message["thinkingLevel"].as_str() { turn.thinking_level = clipped(level, 16).0.into(); }
                    }
                }
                let end = !matches!(message["stopReason"].as_str(), Some("toolUse" | "tool_calls"));
                if let Some(content) = message["content"].as_array() {
                    for (i, part) in content.iter().enumerate() {
                        let (kind, label, text) = match part["type"].as_str() {
                            Some("text") => (if end { "end" } else { "progress" }, if end { "Answer" } else { "Progress" }, part["text"].as_str()),
                            Some("thinking") => ("thinking", "Thinking", part["thinking"].as_str()),
                            _ => continue,
                        };
                        self.put(format!("{}:{}:{i}", self.run, self.message), kind, label, text.unwrap_or_default(), false,
                            message["stopReason"] == "error", false);
                    }
                }
            },
            "tool_execution_start" | "tool_execution_update" | "tool_execution_end" => {
                let Some(id) = event["toolCallId"].as_str() else { return; };
                let label = event["toolName"].as_str().unwrap_or("Tool");
                self.phase = format!("Tool · {}", clipped(label, 80).0);
                let result = if kind == "tool_execution_update" { &event["partialResult"] } else { &event["result"] };
                let (text, cut) = plain_text(&result["content"], BLOCK_LIMIT);
                self.put(format!("tool:{}", clipped(id, 256).0), "tool", label, &text, kind != "tool_execution_end", event["isError"] == true, false);
                if cut { if let Some(b) = self.blocks.iter_mut().find(|b| b.id == format!("tool:{}", clipped(id, 256).0)) { b.truncated = true; } }
            },
            "turn_end" => { self.stats_revision += 1; },
            "compaction_start" => { self.phase = "Compacting".into(); self.context_estimate = None; },
            "compaction_end" => {
                self.phase = "Working".into(); self.stats_revision += 1;
                self.context_estimate = if event["aborted"] == false && event["errorMessage"].is_null() {
                    event["result"]["estimatedTokensAfter"].as_f64().filter(|n| n.is_finite() && *n >= 0.0)
                } else { None };
            },
            "auto_retry_start" => { self.phase = "Retrying".into(); },
            "auto_retry_end" => { self.phase = "Working".into(); },
            _ => return,
        }
        self.revision += 1;
    }
}

#[cfg(test)]
#[path = "workbench_tests.rs"]
mod tests;
