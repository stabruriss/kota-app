//! Optional marks/Dream context for one delivery. The first choice per (agent,
//! message) lives in memory for this App's lifetime; pushes are accounted in the
//! existing `soul::pushes` ledger only after the transport's delivery evidence.
use super::{core, input::{Input, Scope}, settings};
use crate::{
    local_rpc::{jev_http, owned::OwnedDirectory, DeliveryState},
    soul::{self, marks, pushes, SoulPaths},
    violet::ActiveDream,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{mpsc, Arc, Mutex}, thread, time::Duration};

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;

/// The previous confirmation normally records within milliseconds. A stuck one
/// may delay this agent's next message by about one Jev deadline, no more.
const LEDGER_WAIT: Duration = Duration::from_secs(3);

type ReadSettings = dyn Fn(&Path) -> Result<settings::Snapshot, String> + Send + Sync;
type ReadDreams = dyn Fn(&Path) -> Result<Vec<ActiveDream>, String> + Send + Sync;
type Classify = dyn Fn(&OwnedDirectory, Value) -> Result<Value, String> + Send + Sync;

pub(crate) struct Service {
    home: PathBuf,
    settings: Box<ReadSettings>,
    dreams: Box<ReadDreams>,
    classify: Box<Classify>,
    /// (agent, message) → its first choice. A same-ID retry reuses it, never asks Jev.
    frozen: Mutex<HashMap<(String, String), Arc<Choice>>>,
    /// Per agent: disconnects once its latest confirmation is in the push ledger.
    recording: Mutex<HashMap<String, mpsc::Receiver<()>>>,
    /// Local: kota_id → a submitted delivery awaiting the runtime's notice.
    held: Mutex<HashMap<String, Delivery>>,
}

pub(crate) struct Choice {
    pub envelope: String,
    display_body: String,
    pushed: Vec<pushes::Pushed>,
}

pub(crate) struct Delivery {
    scope: Scope,
    message_id: String,
    pub choice: Arc<Choice>,
}

impl Service {
    pub(crate) fn new(home: PathBuf) -> Self {
        Self::with(home, Box::new(|home| settings::read(home).map_err(|error| error.code)),
            Box::new(|_| crate::violet::read_active_dreams()),
            Box::new(|owned, body| jev_http::classify(owned, body).map_err(|error| error.code)))
    }

    fn with(home: PathBuf, settings: Box<ReadSettings>, dreams: Box<ReadDreams>, classify: Box<Classify>) -> Self {
        Self { home, settings, dreams, classify, frozen: Mutex::default(), recording: Mutex::default(), held: Mutex::default() }
    }

    /// None: deliver the original bytes and record nothing. That covers ineligible
    /// input, no selection, and any settings/Soul/Jev failure (logged, not retried).
    pub(crate) fn prepare(&self, scope: Scope, input: &Input) -> Option<Delivery> {
        if !input.valid_for(&scope.agent_id) { return None; }
        let key = (scope.agent_id.clone(), input.message_id.clone());
        let frozen = self.frozen.lock().ok()?.get(&key).cloned();
        let choice = match frozen {
            Some(choice) => choice,
            None => {
                self.wait_for_ledger(&scope.agent_id);
                let choice = match self.select(&scope, input) {
                    Ok(Some(choice)) => Arc::new(choice),
                    Ok(None) => return None,
                    Err(error) => {
                        crate::kota_debug_log(&format!("soul: context omitted: {error}"));
                        return None;
                    }
                };
                self.frozen.lock().ok()?.entry(key).or_insert(choice).clone()
            }
        };
        Some(Delivery { scope, message_id: input.message_id.clone(), choice })
    }

    /// Cooldown reads the push ledger, so this agent's previous confirmation lands
    /// first (arrival order, SOUL-PLAN §17). Past the bound, select without it.
    fn wait_for_ledger(&self, agent_id: &str) {
        let Some(previous) = self.recording.lock().ok().and_then(|mut recording| recording.remove(agent_id))
        else { return };
        if previous.recv_timeout(LEDGER_WAIT) == Err(mpsc::RecvTimeoutError::Timeout) {
            crate::kota_debug_log("soul: previous push still unrecorded; selecting without it");
        }
    }

    fn select(&self, scope: &Scope, input: &Input) -> Result<Option<Choice>, String> {
        let snapshot = (self.settings)(&self.home)?;
        if !snapshot.jev_ready || !snapshot.settings.permits(input.origin, input.meaningful_chars()) {
            return Ok(None);
        }
        let paths = SoulPaths::new(&scope.agent_cwd);
        let marks = marks::snapshot(&paths)?;
        let dreams = if snapshot.settings.max_dreams == 0 { Vec::new() } else { (self.dreams)(&self.home)? };
        let pushed = pushes::read_all(&paths)?;
        let cooldown = u64::from(snapshot.settings.cooldown_replies);
        let today = Utc::now().date_naive();
        let mut turns = None;
        let mut candidates = Vec::new();
        for note in core::notes(&marks, &dreams).map_err(str::to_owned)? {
            // A cooldown lasts N replies within this launch and UTC day.
            let last = pushed.iter().rev().find(|push| push.id() == note.id && push.session == scope.key)
                .filter(|push| DateTime::parse_from_rfc3339(&push.ts).is_ok_and(|ts| ts.with_timezone(&Utc).date_naive() == today));
            if let Some(push) = last.filter(|_| cooldown > 0) {
                let now = *turns.get_or_insert_with(|| crate::project_agent_turns(&scope.project_root, &scope.agent_id));
                // Without either count, the launch and day alone keep it cooling.
                if push.turns.zip(now).is_none_or(|(then, now)| now.saturating_sub(then) < cooldown) { continue; }
            }
            candidates.push(note);
        }
        if candidates.is_empty() { return Ok(None); }
        let previous = crate::violet::latest_reply(&scope.project_root, &scope.agent_id);
        let Some(request) = core::build_request(&input.original_text, previous.as_deref(),
            marks.revision, candidates, &snapshot.settings) else { return Ok(None) };
        let reply = (self.classify)(&OwnedDirectory::new(&self.home), request.body.clone())?;
        let selected = core::select(&request, &reply, &snapshot.settings).map_err(str::to_owned)?;
        let mut counts = HashMap::new();
        for hit in selected.iter().filter(|hit| hit.note.kind == core::Kind::Dream) {
            let count = pushes::pushed_since_clear(&paths, &self.home, &scope.agent_id, &hit.note.id)?;
            counts.insert(hit.note.id.clone(), count + 1);
        }
        let (Some(envelope), Some(display_body)) =
            (core::envelope(request.revision, &selected, &counts), core::body(&selected, &counts)) else { return Ok(None) };
        let pushed = selected.into_iter().map(|hit| pushes::Pushed { id: hit.note.id, p: hit.probability }).collect();
        Ok(Some(Choice { envelope, display_body, pushed }))
    }

    /// Local: register before submitting, so the first notice cannot be missed.
    pub(crate) fn hold(&self, kota_id: String, delivery: Delivery) {
        if let Ok(mut held) = self.held.lock() { held.insert(kota_id, delivery); }
    }

    /// Local notices: the first Accepted/Consumed confirms; a closed session's
    /// Abandoned drops it. Cleared/Failed keep it: an edited input may be requeued.
    pub(crate) fn local_delivery(&self, kota_id: &str, state: DeliveryState,
        written: impl FnOnce(&Path, Vec<PathBuf>) + Send + 'static) {
        if !matches!(state, DeliveryState::Accepted | DeliveryState::Consumed | DeliveryState::Abandoned) { return; }
        let Some(delivery) = self.held.lock().ok().and_then(|mut held| held.remove(kota_id)) else { return };
        if state != DeliveryState::Abandoned { self.confirm(delivery, written); }
    }

    /// Call only after the transport's own delivery evidence. Records in the
    /// background; a failure is logged, never retried, never a redelivery.
    pub(crate) fn confirm(&self, delivery: Delivery, written: impl FnOnce(&Path, Vec<PathBuf>) + Send + 'static) {
        let (recorded, wait) = mpsc::channel::<()>();
        if let Ok(mut recording) = self.recording.lock() {
            recording.insert(delivery.scope.agent_id.clone(), wait);
        }
        let home = self.home.clone();
        let worker = thread::Builder::new().name("soul-confirm".into()).spawn(move || {
            let Delivery { scope, message_id, choice } = delivery;
            let ledger = pushes::record(&SoulPaths::new(&scope.agent_cwd), &home, &pushes::Delivery {
                agent_id: &scope.agent_id, message: &message_id, session: &scope.key,
                project: soul::project_slug(scope.project_id.as_deref(), Some(&scope.project_root)).as_deref(),
                ts: &soul::now_ts(), turns: crate::project_agent_turns(&scope.project_root, &scope.agent_id),
            }, &choice.pushed);
            drop(recorded);
            if let Err(error) = ledger {
                crate::kota_debug_log(&format!("soul: push ledger not recorded: {error}"));
            }
            match crate::violet::record_soul_context(&scope.project_root, &scope.agent_id, &message_id, &choice.display_body) {
                Ok(paths) => written(&scope.project_root, paths),
                Err(error) => crate::kota_debug_log(&format!("soul: context row not recorded: {error}")),
            }
        });
        // Delivery already happened: never panic the submit path over bookkeeping.
        if worker.is_err() { crate::kota_debug_log("soul: confirmation worker unavailable"); }
    }
}
