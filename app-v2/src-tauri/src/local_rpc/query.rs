//! Per-stage deadlines and cancellation, never a model-visible event.
use super::error::{LocalError, LocalResult};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct Query {
    pub deadline: Instant,
    timeout: Duration,
    readiness_timeout: Duration,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    progress: Arc<dyn Fn(&'static str) + Send + Sync>,
}
impl Query {
    pub fn new(timeout: Duration) -> Self {
        Self { deadline: Instant::now() + timeout, timeout, readiness_timeout: timeout, cancelled: Arc::new(|| false), progress: Arc::new(|_| {}) }
    }
    pub fn standard() -> Self { Self::new(Duration::from_secs(30)).with_readiness_timeout(Duration::from_secs(40)) }
    pub fn with_readiness_timeout(mut self, timeout: Duration) -> Self { self.readiness_timeout = timeout; self }
    pub fn readiness(&self) -> Self { Self { deadline: Instant::now() + self.readiness_timeout, ..self.clone() } }
    pub fn observed(deadline: Instant, cancelled: Arc<dyn Fn() -> bool + Send + Sync>, progress: Arc<dyn Fn(&'static str) + Send + Sync>) -> Self {
        let timeout = deadline.saturating_duration_since(Instant::now());
        Self { deadline, timeout, readiness_timeout: timeout, cancelled, progress }
    }
    pub fn check(&self, stage: &'static str) -> LocalResult<()> {
        self.check_cancelled(stage)?;
        if Instant::now() >= self.deadline { return Err(LocalError::new("auth_timeout").at(stage)); }
        Ok(())
    }
    pub fn check_cancelled(&self, stage: &'static str) -> LocalResult<()> {
        if (self.cancelled)() { Err(LocalError::new("startup_cancelled").at(stage)) } else { Ok(()) }
    }
    pub fn stage(&self, stage: &'static str) -> LocalResult<()> {
        self.check(stage)?;
        self.report(stage);
        Ok(())
    }
    pub fn report(&self, stage: &'static str) { (self.progress)(stage); }
    pub fn observe(&self, observer: Arc<dyn Fn(&'static str) + Send + Sync>) -> Self {
        let progress = self.progress.clone();
        Self { progress: Arc::new(move |stage| { progress(stage); observer(stage); }), ..self.clone() }
    }
    pub fn renewed(&self) -> Self {
        Self { deadline: Instant::now() + self.timeout, ..self.clone() }
    }
}
