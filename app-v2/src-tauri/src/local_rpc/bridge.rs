use super::{LocalNotice, Sink};
use anyhow::{anyhow, Result};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

type Factory = fn(AppHandle) -> Arc<dyn Sink>;
static FACTORY: OnceLock<Factory> = OnceLock::new();

/// Installed once by Violet R3. The six terminal providers never call it.
pub(crate) fn register_sink_factory(factory: Factory) -> Result<()> {
    FACTORY.set(factory).map_err(|_| anyhow!("Local projection factory already registered"))
}

pub(crate) fn sink(app: &AppHandle) -> Result<Arc<dyn Sink>> {
    let factory = FACTORY.get().ok_or_else(|| anyhow!("Local room projection is not available"))?;
    Ok(Arc::new(Projection { app: app.clone(), inner: factory(app.clone()) }))
}

struct Projection { app: AppHandle, inner: Arc<dyn Sink> }
impl Sink for Projection {
    fn notify(&self, notice: &LocalNotice) -> std::result::Result<(), String> {
        let scope = match notice {
            LocalNotice::Event { scope, .. } | LocalNotice::Entries { scope, .. } |
            LocalNotice::Delivery { scope, .. } | LocalNotice::Status { scope, .. } => scope,
        };
        if matches!(notice, LocalNotice::Status { .. } | LocalNotice::Delivery { .. }) {
            // Raw stream events stay in Rust; the visible workbench reads its bounded
            // revision snapshot. The inner Violet sink still receives every notice.
            // Live runtime subscriptions have no terminal grid. R3 separately
            // emits room changes after its durable projection succeeds.
            let _ = self.app.emit(&format!("local-rpc://agent/{}/notice", scope.agent_id), notice);
        }
        // Soul: hand a held delivery to its confirmation worker; never wait here.
        if let LocalNotice::Delivery { kota_id, state, .. } = notice {
            if let Some(service) = self.app.try_state::<Arc<crate::soul_push::service::Service>>() {
                let app = self.app.clone();
                service.local_delivery(kota_id, *state, move |root, paths| crate::violet::emit_room_changed(&app, root, "soul-context", paths));
            }
        }
        self.inner.notify(notice)
    }
}
