//! `$KOTA_HOME/dreams/usage.jsonl`: the account-level Dream ledger (SOUL-PLAN v4 §18.1,
//! sample §7). The host appends `push` when Jev pushed a Dream to an incarnation;
//! clear-heat appends `clear`. Every incarnation's process writes the same file, so each
//! append holds the ledger's flock. Dream consolidation reads `summaries`.
use super::{append_lines, date_of, dreams_usage_lock, dreams_usage_path, lock, read_jsonl};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Line {
    pub ts: String,
    pub op: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// How much one active Dream has been surfaced over its whole life (a clear does not
/// reset it): the consolidation prompt's `usage` field.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct DreamUsage {
    pub pushed: u64,
    pub agents: u64,
    pub last: Option<String>,
}

pub(crate) fn append_pushes(
    kota_home: &Path,
    agent_id: &str,
    message: &str,
    ts: &str,
    pushes: &[(&str, f64)],
) -> Result<(), String> {
    if pushes.is_empty() {
        return Ok(());
    }
    let lines = pushes
        .iter()
        .map(|(id, p)| {
            serde_json::to_string(&Line {
                ts: ts.into(),
                op: "push".into(),
                id: (*id).into(),
                p: Some(*p),
                agent: Some(agent_id.into()),
                message: Some(message.into()),
            })
            .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let _lock = lock(&dreams_usage_lock(kota_home))?;
    append_lines(&dreams_usage_path(kota_home), &lines)
}

pub(crate) fn append_clear(kota_home: &Path, agent_id: &str, id: &str, ts: &str) -> Result<(), String> {
    let line = serde_json::to_string(&Line {
        ts: ts.into(),
        op: "clear".into(),
        id: id.into(),
        p: None,
        agent: Some(agent_id.into()),
        message: None,
    })
    .map_err(|err| err.to_string())?;
    let _lock = lock(&dreams_usage_lock(kota_home))?;
    append_lines(&dreams_usage_path(kota_home), &[line])
}

pub(crate) fn read_all(kota_home: &Path) -> Result<Vec<Line>, String> {
    read_jsonl(&dreams_usage_path(kota_home))
}

pub(crate) fn has_record(kota_home: &Path, id: &str) -> Result<bool, String> {
    Ok(read_all(kota_home)?.iter().any(|line| line.id == id))
}

/// This incarnation's pushes of `id` after its last clear, by ledger order: a clear and
/// a push in the same second are told apart by which line came first.
pub(crate) fn pushed_since_clear(kota_home: &Path, agent_id: &str, id: &str) -> Result<u64, String> {
    let mut count = 0;
    for line in read_all(kota_home)? {
        if line.id != id || line.agent.as_deref() != Some(agent_id) {
            continue;
        }
        match line.op.as_str() {
            "clear" => count = 0,
            "push" => count += 1,
            _ => {}
        }
    }
    Ok(count)
}

/// Only `push` counts; `clear` belongs to each incarnation's reminder loop. A missing
/// file is an empty map; a file that cannot be read is an error, never a silent zero.
#[allow(dead_code)] // host-side API: Dream consolidation reads it for each active item's `usage`.
pub(crate) fn summaries(kota_home: &Path) -> Result<HashMap<String, DreamUsage>, String> {
    let mut tally: HashMap<String, (DreamUsage, HashSet<String>)> = HashMap::new();
    for line in read_all(kota_home)? {
        if line.op != "push" {
            continue;
        }
        let entry = tally.entry(line.id).or_default();
        entry.0.pushed += 1;
        if let Some(agent) = line.agent {
            entry.1.insert(agent);
        }
        let day = date_of(&line.ts).to_string();
        if entry.0.last.as_deref().map_or(true, |last| day.as_str() >= last) {
            entry.0.last = Some(day);
        }
    }
    Ok(tally
        .into_iter()
        .map(|(id, (mut usage, agents))| {
            usage.agents = agents.len() as u64;
            (id, usage)
        })
        .collect())
}
