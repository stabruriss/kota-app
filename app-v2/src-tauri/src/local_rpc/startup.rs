//! Human-only Local startup state. Never sent to Pi, a ledger or Violet.
use super::{error::{LocalError, LocalResult}, models::ModelSelection, query::Query};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::{Arc, Condvar, Mutex, OnceLock}, time::{Duration, Instant}};

const MAX_ATTEMPTS: usize = 128;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Selection {
    #[serde(flatten)] pub model: ModelSelection,
    pub thinking_level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_route: Option<super::auto_route::Group>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Step {
    pub stage: String, pub started_at: String, pub finished_at: Option<String>,
    pub elapsed_ms: u64, pub error: Option<LocalError>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    pub project_root: String, pub agent_id: String, pub attempt_id: String,
    pub revision: u64, pub state: String, pub selection: Option<Selection>,
    pub saved: bool, pub steps: Vec<Step>, pub error: Option<LocalError>,
}
struct State { view: View, step_at: Instant, executing: bool }
pub(crate) struct Attempt {
    state: Mutex<State>,
    completed: Condvar,
    notify: Arc<dyn Fn(View) + Send + Sync>,
}
pub(crate) struct Execution { attempt: Arc<Attempt>, armed: bool }
impl Execution {
    pub fn release_for_takeover(mut self) {
        self.attempt.release_for_takeover();
        self.armed = false;
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        if self.armed { self.attempt.finish(Err(LocalError::new("local_start_failed"))); }
    }
}
pub(crate) struct Prepared { pub attempt: Arc<Attempt>, pub model: super::models::ResolvedRecruit, pub backend: super::auth::Backend }
type Attempts = BTreeMap<(String, String), Arc<Attempt>>;
fn attempts() -> &'static Mutex<Attempts> { static MAP: OnceLock<Mutex<Attempts>> = OnceLock::new(); MAP.get_or_init(Default::default) }
fn now() -> String { chrono::Utc::now().to_rfc3339() }
fn running(view: &View) -> bool { view.state == "running" }

impl Attempt {
    pub fn view(&self) -> View { self.state.lock().unwrap().view.clone() }
    pub fn check(&self) -> LocalResult<()> {
        let state = self.state.lock().unwrap();
        if state.view.state == "cancelled" { return Err(LocalError::new("startup_cancelled")); }
        if state.view.state == "failed" { return Err(state.view.error.clone().unwrap_or_else(|| LocalError::new("local_start_failed"))); }
        Ok(())
    }
    pub fn query(self: &Arc<Self>) -> Query {
        let cancel = self.clone(); let progress = self.clone();
        Query::observed(Instant::now() + super::auth::QUERY_TIMEOUT, Arc::new(move || cancel.check().is_err()), Arc::new(move |stage| progress.step(stage)))
            .with_readiness_timeout(Duration::from_secs(40))
    }
    pub fn step(&self, stage: &'static str) {
        let mut state = self.state.lock().unwrap();
        if !running(&state.view) || state.view.steps.last().is_some_and(|s| s.stage == stage) { return; }
        Self::end_step(&mut state, None);
        state.view.steps.push(Step { stage: stage.into(), started_at: now(), finished_at: None, elapsed_ms: 0, error: None });
        state.step_at = Instant::now(); state.view.revision += 1;
        let view = state.view.clone(); drop(state); (self.notify)(view);
    }
    fn end_step(state: &mut State, error: Option<LocalError>) {
        if let Some(step) = state.view.steps.last_mut() {
            if step.finished_at.is_none() {
                step.finished_at = Some(now()); step.elapsed_ms = state.step_at.elapsed().as_millis() as u64; step.error = error;
            }
        }
    }
    pub fn selection(&self, selection: Selection) {
        let mut state = self.state.lock().unwrap();
        if !running(&state.view) { return; }
        state.view.selection = Some(selection); state.view.revision += 1;
        let view = state.view.clone(); drop(state); (self.notify)(view);
    }
    /// Cancellation is serialized with this short atomic write, never with a
    /// helper query. A completed write is reported even if startup is cancelled.
    pub fn save(&self, write: impl FnOnce() -> LocalResult<()>) -> LocalResult<()> {
        self.check()?;
        let mut state = self.state.lock().unwrap();
        if !running(&state.view) { return Err(LocalError::new("startup_cancelled")); }
        write()?; state.view.saved = true; state.view.revision += 1;
        let view = state.view.clone(); drop(state); (self.notify)(view); Ok(())
    }
    pub fn finish(&self, result: LocalResult<()>) {
        // An executor may unwind during the short save section. Its guard must
        // still publish a terminal result and release concurrent joiners.
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.state.clear_poison();
        if !running(&state.view) { return; }
        let error = result.err().map(|e| {
            let stage = state.view.steps.last().map(|s| s.stage.as_str()).unwrap_or("configuration_read"); e.at(stage)
        });
        Self::end_step(&mut state, error.clone());
        state.view.state = match error.as_ref().map(|e| e.code.as_str()) {
            None => "ready", Some("startup_cancelled") => "cancelled", _ => "failed",
        }.into();
        state.view.error = error; state.view.revision += 1;
        let view = state.view.clone(); drop(state); self.completed.notify_all(); (self.notify)(view);
    }
    pub fn execution(self: &Arc<Self>) -> LocalResult<Execution> {
        self.claim()?;
        Ok(Execution { attempt: self.clone(), armed: true })
    }
    fn claim(&self) -> LocalResult<()> {
        self.check()?;
        let mut state = self.state.lock().unwrap();
        if !running(&state.view) || state.executing { return Err(LocalError::new("startup_in_progress")); }
        state.executing = true; Ok(())
    }
    /// Concurrent cold deliveries join the executing start instead of losing
    /// their input to a second spawn. Its existing query/owner deadlines own
    /// completion; this is a notification wait, not a deadline supervisor.
    pub fn join(&self) -> LocalResult<()> {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while running(&state.view) && state.executing {
            state = self.completed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        match state.view.state.as_str() {
            "ready" => Ok(()),
            "running" => Err(LocalError::new("startup_in_progress")),
            _ => Err(state.view.error.clone().unwrap_or_else(|| LocalError::new("local_start_failed"))),
        }
    }
    fn release_for_takeover(&self) {
        self.state.lock().unwrap().executing = false;
        self.completed.notify_all(); self.step("session_takeover");
    }
}

#[derive(Serialize)]
pub(crate) struct Begun { pub view: View, pub reused: bool }
pub(crate) fn begin(root: String, agent: String, selection: Option<Selection>, notify: Arc<dyn Fn(View) + Send + Sync>) -> LocalResult<Begun> {
    if let Some(selection) = &selection {
        super::models::validate_saved_selection(&format!("{}/{}", selection.model.provider_id, selection.model.model_id), Some(&selection.thinking_level))?;
    }
    let mut all = attempts().lock().unwrap();
    let key = (root.clone(), agent.clone());
    if let Some(existing) = all.get(&key).filter(|a| running(&a.view())) {
        return Ok(Begun { view: existing.view(), reused: true });
    }
    if all.len() >= MAX_ATTEMPTS {
        all.retain(|_, a| running(&a.view()));
        if all.len() >= MAX_ATTEMPTS { return Err(LocalError::new("startup_busy")); }
    }
    let view = View { project_root: root, agent_id: agent, attempt_id: uuid::Uuid::new_v4().to_string(), revision: 1,
        state: "running".into(), selection, saved: false,
        steps: vec![Step { stage: "configuration_read".into(), started_at: now(), finished_at: None, elapsed_ms: 0, error: None }], error: None };
    let attempt = Arc::new(Attempt { state: Mutex::new(State { view: view.clone(), step_at: Instant::now(), executing: false }), completed: Condvar::new(),
        notify });
    all.insert(key, attempt.clone()); drop(all);
    (attempt.notify)(view.clone()); Ok(Begun { view, reused: false })
}
pub(crate) fn read(root: &str, agent: &str) -> Option<View> { attempts().lock().unwrap().get(&(root.into(), agent.into())).map(|a| a.view()) }
pub(crate) fn get(root: &str, agent: &str, id: &str) -> LocalResult<Arc<Attempt>> {
    attempts().lock().unwrap().get(&(root.into(), agent.into())).filter(|a| a.view().attempt_id == id).cloned()
        .ok_or_else(|| LocalError::new("startup_superseded"))
}

pub(crate) fn lock<'a, T>(mutex: &'a Mutex<T>, query: &Query, stage: &'static str) -> LocalResult<std::sync::MutexGuard<'a, T>> {
    loop {
        query.check(stage)?;
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return Err(LocalError::new("local_start_failed").at(stage)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fresh() -> (Begun, Arc<Attempt>) {
        let root = format!("/fixture/start-{}", uuid::Uuid::new_v4());
        let begun = begin(root.clone(), "agent-start".into(), None, Arc::new(|_| {})).unwrap();
        let attempt = get(&root, "agent-start", &begun.view.attempt_id).unwrap();
        (begun, attempt)
    }
    #[test]
    fn concurrent_open_joins_the_same_attempt_and_only_one_executor_can_claim() {
        let (begun, attempt) = fresh();
        let again = begin(begun.view.project_root.clone(), "agent-start".into(), None, Arc::new(|_| {})).unwrap();
        assert!(again.reused);
        assert_eq!(again.view.attempt_id, begun.view.attempt_id);
        attempt.claim().unwrap();
        assert_eq!(attempt.claim().unwrap_err().code, "startup_in_progress");
        attempt.finish(Ok(()));
    }
    #[test]
    fn concurrent_delivery_waits_for_the_executor_and_observes_its_result() {
        for cancelled in [false, true] {
            let (_, attempt) = fresh(); attempt.claim().unwrap();
            std::thread::scope(|scope| {
                let waiter = scope.spawn(|| attempt.join());
                if cancelled { attempt.finish(Err(LocalError::new("startup_cancelled"))); }
                else { attempt.finish(Ok(())); }
                let result = waiter.join().unwrap();
                if cancelled { assert_eq!(result.unwrap_err().code, "startup_cancelled"); }
                else { result.unwrap(); }
            });
        }
        let (_, attempt) = fresh(); attempt.claim().unwrap();
        attempt.release_for_takeover();
        assert_eq!(attempt.join().unwrap_err().code, "startup_in_progress");
        attempt.finish(Err(LocalError::new("startup_cancelled")));
    }
    #[test]
    fn cancellation_finishes_the_visible_step_and_cannot_later_save_or_become_ready() {
        let (_, attempt) = fresh();
        attempt.step("helper_ready");
        attempt.finish(Err(LocalError::new("startup_cancelled")));
        let mut wrote = false;
        assert_eq!(attempt.save(|| { wrote = true; Ok(()) }).unwrap_err().code, "startup_cancelled");
        assert!(!wrote);
        attempt.finish(Ok(())); attempt.step("configuration_save");
        let view = attempt.view();
        assert_eq!(view.state, "cancelled");
        assert_eq!(view.error.unwrap().stage.as_deref(), Some("helper_ready"));
        assert_eq!(view.steps.last().unwrap().stage, "helper_ready");
        assert!(view.steps.last().unwrap().finished_at.is_some());
    }
    #[test]
    fn a_saved_choice_survives_startup_failure_and_late_outcomes_cannot_replace_it() {
        let (_, attempt) = fresh();
        attempt.step("configuration_save"); attempt.save(|| Ok(())).unwrap();
        attempt.step("session_ready"); attempt.finish(Err(LocalError::new("auth_timeout")));
        attempt.finish(Ok(()));
        let view = attempt.view();
        assert!(view.saved); assert_eq!(view.state, "failed");
        assert_eq!(view.error.unwrap().stage.as_deref(), Some("session_ready"));
    }
    #[test]
    fn executor_unwind_finishes_the_attempt_wakes_joiners_and_allows_a_new_start() {
        for during_save in [false, true] {
            let (begun, attempt) = fresh();
            let execution = attempt.execution().unwrap();
            let (send, receive) = std::sync::mpsc::channel();
            let waiting = attempt.clone();
            let waiter = std::thread::spawn(move || { send.send(waiting.join()).unwrap(); });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _execution = execution;
                if during_save { let _ = attempt.save(|| panic!("fixture save panic")); }
                panic!("fixture executor panic");
            }));
            assert!(result.is_err());
            assert_eq!(receive.recv_timeout(Duration::from_secs(2)).unwrap().unwrap_err().code, "local_start_failed");
            waiter.join().unwrap();
            let view = attempt.view();
            assert_eq!(view.state, "failed");
            assert!(view.steps.last().unwrap().finished_at.is_some());
            let next = begin(begun.view.project_root, begun.view.agent_id, None, Arc::new(|_| {})).unwrap();
            assert!(!next.reused);
            assert_ne!(next.view.attempt_id, begun.view.attempt_id);
            get(&next.view.project_root, &next.view.agent_id, &next.view.attempt_id).unwrap().finish(Ok(()));
        }
    }
    #[test]
    fn execution_guard_preserves_terminal_results_and_releases_only_for_takeover() {
        let (_, attempt) = fresh();
        let execution = attempt.execution().unwrap();
        attempt.finish(Ok(()));
        let revision = attempt.view().revision;
        drop(execution);
        assert_eq!(attempt.view().state, "ready");
        assert_eq!(attempt.view().revision, revision);

        let (_, attempt) = fresh();
        attempt.execution().unwrap().release_for_takeover();
        assert_eq!(attempt.view().state, "running");
        assert_eq!(attempt.join().unwrap_err().code, "startup_in_progress");
        let execution = attempt.execution().unwrap();
        attempt.finish(Err(LocalError::new("startup_cancelled")));
        drop(execution);
        assert_eq!(attempt.view().state, "cancelled");
    }
}
