//! `under-table/soul/pushes.jsonl`: every mark or Dream the host pushed to this
//! incarnation (SOUL-PLAN v4 §13, §18.1; sample §3). Cooldown, `used` and the Dream
//! reminder count read from here; a Dream push is mirrored into the account ledger.
use super::{append_lines, dream_usage, lock, marks, read_jsonl, SoulPaths};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Push {
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream: Option<String>,
    pub p: f64,
    pub message: String,
    pub session: String,
    pub project: Option<String>,
    /// This agent's product Turns when the delivery was confirmed; cooldown counts
    /// replies from here. None when unavailable, and on rows written before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
}

impl Push {
    pub(crate) fn id(&self) -> &str {
        self.mark.as_deref().or(self.dream.as_deref()).unwrap_or("")
    }
}

/// One item the host pushed: its id (`m-` mark or `d-` Dream) and Jev's probability.
#[derive(Clone, Debug)]
pub(crate) struct Pushed {
    pub id: String,
    pub p: f64,
}

/// The message the items were attached to.
#[derive(Clone, Debug)]
pub(crate) struct Delivery<'a> {
    pub agent_id: &'a str,
    pub message: &'a str,
    pub session: &'a str,
    pub project: Option<&'a str>,
    pub ts: &'a str,
    pub turns: Option<u64>,
}

pub(crate) fn is_dream_id(id: &str) -> bool {
    id.starts_with("d-")
}

/// Records one delivery: the agent's lines and the marks' `used` under the soul lock,
/// then the Dream pushes in the account ledger under its own lock. A crash between the
/// two leaves the agent's own ledger (cooldown, used, reminders) complete.
///
/// Idempotent on (message, id): a line already in pushes.jsonl is not written again,
/// not counted in `used` and not mirrored, so a redelivered or retried message counts
/// once across restarts. A partial earlier run is never back-filled or retried here.
#[allow(dead_code)] // host-side API: the Jev push line records through here.
pub(crate) fn record(
    paths: &SoulPaths,
    kota_home: &Path,
    delivery: &Delivery<'_>,
    pushed: &[Pushed],
) -> Result<(), String> {
    if pushed.is_empty() {
        return Ok(());
    }
    let _lock = lock(&paths.soul_lock())?;
    let recorded = read_all(paths)?
        .into_iter()
        .filter(|push| push.message == delivery.message)
        .map(|push| push.id().to_string())
        .collect::<std::collections::HashSet<_>>();
    let pushed = pushed.iter().filter(|push| !recorded.contains(&push.id)).cloned().collect::<Vec<_>>();
    if pushed.is_empty() {
        return Ok(());
    }
    let lines = pushed
        .iter()
        .map(|push| {
            let (mark, dream) = if is_dream_id(&push.id) {
                (None, Some(push.id.clone()))
            } else {
                (Some(push.id.clone()), None)
            };
            serde_json::to_string(&Push {
                ts: delivery.ts.into(),
                mark,
                dream,
                p: push.p,
                message: delivery.message.into(),
                session: delivery.session.into(),
                project: delivery.project.map(str::to_string),
                turns: delivery.turns,
            })
            .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    append_lines(&paths.pushes(), &lines)?;
    let mark_ids = pushed
        .iter()
        .filter(|push| !is_dream_id(&push.id))
        .map(|push| push.id.as_str())
        .collect::<Vec<_>>();
    marks::bump_used_locked(paths, &mark_ids)?;
    drop(_lock);
    let dreams = pushed
        .iter()
        .filter(|push| is_dream_id(&push.id))
        .map(|push| (push.id.as_str(), push.p))
        .collect::<Vec<_>>();
    dream_usage::append_pushes(kota_home, delivery.agent_id, delivery.message, delivery.ts, &dreams)
}

pub(crate) fn read_all(paths: &SoulPaths) -> Result<Vec<Push>, String> {
    read_jsonl(&paths.pushes())
}

/// The last push of `id` to this incarnation, within one session when given. The host
/// counts end-turns and the day boundary from its `ts` and `session`.
pub(crate) fn last_push(paths: &SoulPaths, id: &str, session: Option<&str>) -> Result<Option<Push>, String> {
    Ok(read_all(paths)?
        .into_iter()
        .rev()
        .find(|push| push.id() == id && session.map_or(true, |session| push.session == session)))
}

/// Pushes of `id` to this incarnation since its last clear-heat. A Dream's pushes and
/// clears both sit in the account ledger (`record` mirrors every Dream push there), so
/// the count is by ledger order, never by second-precision timestamps. A mark is never
/// cleared, so every push counts.
#[allow(dead_code)] // host-side API: the Dream reminder threshold (TEXTS §K.4).
pub(crate) fn pushed_since_clear(
    paths: &SoulPaths,
    kota_home: &Path,
    agent_id: &str,
    id: &str,
) -> Result<u64, String> {
    if is_dream_id(id) {
        return dream_usage::pushed_since_clear(kota_home, agent_id, id);
    }
    Ok(read_all(paths)?.iter().filter(|push| push.id() == id).count() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clear and a push in the same second: order in the ledger decides, not the clock.
    #[test]
    fn same_second_clear_and_push_count_by_ledger_order() {
        let root = std::env::temp_dir().join(format!("kota-soul-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = SoulPaths::new(&root.join("agent"));
        let home = root.join("home");
        let ts = "2026-10-15T10:00:00Z";
        // Three messages in one second: each is its own delivery, so each counts.
        let delivery = |message: &'static str| Delivery { agent_id: "agent-fixture", message, session: "s", project: None, ts, turns: None };
        let push = |p: f64| vec![Pushed { id: "d-1".into(), p }];
        record(&paths, &home, &delivery("violet-m1"), &push(0.9)).unwrap();
        dream_usage::append_clear(&home, "agent-fixture", "d-1", ts).unwrap();
        record(&paths, &home, &delivery("violet-m2"), &push(0.91)).unwrap();
        assert_eq!(pushed_since_clear(&paths, &home, "agent-fixture", "d-1").unwrap(), 1);
        dream_usage::append_clear(&home, "agent-fixture", "d-1", ts).unwrap();
        assert_eq!(pushed_since_clear(&paths, &home, "agent-fixture", "d-1").unwrap(), 0);
        dream_usage::append_clear(&home, "agent-other", "d-1", ts).unwrap();
        record(&paths, &home, &delivery("violet-m3"), &push(0.92)).unwrap();
        assert_eq!(pushed_since_clear(&paths, &home, "agent-fixture", "d-1").unwrap(), 1);
        assert_eq!(pushed_since_clear(&paths, &home, "agent-other", "d-1").unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
