use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use tauri::AppHandle;
use uuid::Uuid;

use super::agent::{AgentRegistry, AgentRoute, AgentSpawnRequest, AgentSummary};
use super::smart::{MagiProvider, SmartPtySummary, SmartTerminalPty, TranslateResult};

#[derive(Default)]
struct SmartPool {
    default_id: Option<String>,
    ptys: HashMap<String, SmartTerminalPty>,
}

#[derive(Default)]
pub struct PtyManager {
    smart: Mutex<SmartPool>,
    agents: AgentRegistry,
}

impl PtyManager {
    pub fn smart_init(&self, app: &AppHandle) -> Result<String> {
        let smart = self.resolve_smart(None)?;
        smart.init(app)?;
        Ok(smart.pty_id())
    }

    pub fn smart_spawn(
        &self,
        app: &AppHandle,
        cwd: Option<String>,
        cli: Option<String>,
    ) -> Result<String> {
        let pty_id = new_smart_pty_id();
        let smart = SmartTerminalPty::new(pty_id.clone(), cwd, cli)?;

        {
            let mut pool = self.smart.lock().expect("smart pool poisoned");
            if pool.default_id.is_none() {
                pool.default_id = Some(pty_id.clone());
            }
            pool.ptys.insert(pty_id.clone(), smart.clone());
        }

        if let Err(err) = smart.init(app) {
            let mut pool = self.smart.lock().expect("smart pool poisoned");
            pool.ptys.remove(&pty_id);
            if pool.default_id.as_deref() == Some(pty_id.as_str()) {
                pool.default_id = None;
            }
            return Err(err);
        }

        Ok(pty_id)
    }

    pub fn smart_close(&self, app: &AppHandle, pty_id: String) -> Result<()> {
        let smart = {
            let mut pool = self.smart.lock().expect("smart pool poisoned");
            if pool.default_id.as_deref() == Some(pty_id.as_str()) {
                pool.default_id = None;
            }
            pool.ptys
                .remove(&pty_id)
                .ok_or_else(|| anyhow!("unknown smart terminal PTY: {pty_id}"))?
        };

        smart.close(app)
    }

    pub fn smart_write(
        &self,
        app: &AppHandle,
        pty_id: Option<String>,
        input: String,
    ) -> Result<()> {
        self.resolve_smart(pty_id)?.write(app, input)
    }

    pub fn smart_resize(&self, pty_id: Option<String>, cols: u16, rows: u16) -> Result<()> {
        self.resolve_smart(pty_id)?.resize(cols, rows)
    }

    pub fn smart_scroll(&self, app: &AppHandle, pty_id: Option<String>, lines: i32) -> Result<()> {
        self.resolve_smart(pty_id)?.scroll(app, lines)
    }

    pub fn smart_interrupt(&self, pty_id: Option<String>) -> Result<()> {
        self.resolve_smart(pty_id)?.interrupt()
    }

    pub fn smart_clear(&self, app: &AppHandle, pty_id: Option<String>) -> Result<()> {
        self.resolve_smart(pty_id)?.clear(app)
    }

    pub fn smart_restart(&self, app: &AppHandle, pty_id: Option<String>) -> Result<()> {
        self.resolve_smart(pty_id)?.restart(app)
    }

    pub fn smart_list(&self) -> Vec<SmartPtySummary> {
        let pool = self.smart.lock().expect("smart pool poisoned");
        let default_id = pool.default_id.clone();
        let mut summaries = pool
            .ptys
            .values()
            .map(SmartTerminalPty::summary)
            .collect::<Vec<_>>();

        summaries.sort_by_key(|summary| {
            (
                default_id.as_deref() != Some(summary.pty_id.as_str()),
                summary.pty_id.clone(),
            )
        });

        summaries
    }

    pub fn nl_translate(&self, ask: String, provider: MagiProvider) -> Result<TranslateResult> {
        self.resolve_smart(None)?.translate(ask, provider)
    }

    // ─── Agent PTY (CC / Codex / etc.) ────────────────────────────────

    pub fn agent_spawn(&self, app: &AppHandle, request: AgentSpawnRequest) -> Result<AgentRoute> {
        self.agents.spawn(app, request)
    }

    pub fn agent_write(&self, app: &AppHandle, agent_id: String, input: String) -> Result<()> {
        self.agents.write(app, &agent_id, input)
    }

    pub fn agent_submit_prompt(
        &self,
        app: &AppHandle,
        agent_id: String,
        input: String,
    ) -> Result<()> {
        self.agents.submit_prompt(app, &agent_id, input)
    }

    pub fn agent_resize(&self, agent_id: String, cols: u16, rows: u16) -> Result<()> {
        self.agents.resize(&agent_id, cols, rows)
    }

    pub fn agent_scroll(&self, app: &AppHandle, agent_id: String, lines: i32) -> Result<()> {
        self.agents.scroll(app, &agent_id, lines)
    }

    pub fn agent_interrupt(&self, app: &AppHandle, agent_id: String) -> Result<()> {
        self.agents.interrupt(app, &agent_id)
    }

    pub fn agent_close(&self, app: &AppHandle, agent_id: String) -> Result<()> {
        self.agents.close(app, &agent_id)
    }

    pub fn agent_list(&self) -> Vec<AgentSummary> {
        self.agents.list()
    }

    pub fn agent_route(&self, agent_id: String) -> Option<AgentRoute> {
        self.agents.route_for(&agent_id)
    }

    fn resolve_smart(&self, pty_id: Option<String>) -> Result<SmartTerminalPty> {
        if let Some(pty_id) = pty_id {
            return self
                .smart
                .lock()
                .expect("smart pool poisoned")
                .ptys
                .get(&pty_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown smart terminal PTY: {pty_id}"));
        }

        let mut pool = self.smart.lock().expect("smart pool poisoned");
        if let Some(default_id) = pool.default_id.clone() {
            if let Some(smart) = pool.ptys.get(&default_id).cloned() {
                return Ok(smart);
            }
            pool.default_id = None;
        }

        let pty_id = new_smart_pty_id();
        let smart = SmartTerminalPty::new(pty_id.clone(), None, None)?;
        pool.default_id = Some(pty_id.clone());
        pool.ptys.insert(pty_id, smart.clone());
        Ok(smart)
    }
}

fn new_smart_pty_id() -> String {
    format!("smart-{}", Uuid::new_v4().simple())
}
