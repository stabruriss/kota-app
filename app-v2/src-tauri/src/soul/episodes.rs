//! `under-table/episodes/`: the pool (`episodes.jsonl`) and its usage ledger
//! (`usage.jsonl`). This stage writes retired marks and clear-heat lines; room-message
//! export, the FTS5 index and search follow (SOUL-PLAN v4 §2/§9/§18).
use super::{append_lines, lock, read_jsonl, SoulPaths};
use serde::{Deserialize, Serialize};

pub(crate) const RETIRED_MARK_W0: f64 = 0.5;

/// One line of episodes.jsonl (UNDER-TABLE-SAMPLE.md §4); field order is the sample's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Episode {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub project: Option<String>,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    pub text: String,
    pub indexed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w0: Option<f64>,
    #[serde(default, skip_serializing)]
    pub replaced_by: Option<String>,
}

impl Episode {
    pub(crate) fn retired_mark(
        id: String,
        project: Option<String>,
        ts: &str,
        text: String,
    ) -> Self {
        Self {
            id,
            kind: "retired_mark".into(),
            role: None,
            origin: None,
            project,
            ts: ts.into(),
            from: None,
            prev: None,
            text,
            indexed: true,
            w0: Some(RETIRED_MARK_W0),
            replaced_by: None,
        }
    }
}

pub(crate) fn append(paths: &SoulPaths, episodes: &[Episode]) -> Result<(), String> {
    let lines = episodes
        .iter()
        .map(|episode| serde_json::to_string(episode).map_err(|err| err.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let _lock = lock(&paths.episodes_lock())?;
    append_lines(&paths.episodes(), &lines)
}

/// One line of usage.jsonl (sample §5): `return` (w 0.5, with the query), `expand`
/// (w 1.0) or `clear`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Usage {
    pub ts: String,
    pub op: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

impl Usage {
    pub(crate) fn clear(id: &str, ts: &str) -> Self {
        Self { ts: ts.into(), op: "clear".into(), id: id.into(), w: None, q: None }
    }
}

pub(crate) fn append_usage(paths: &SoulPaths, entries: &[Usage]) -> Result<(), String> {
    let lines = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).map_err(|err| err.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let _lock = lock(&paths.episodes_lock())?;
    append_lines(&paths.episodes_usage(), &lines)
}

pub(crate) fn read_usage(paths: &SoulPaths) -> Result<Vec<Usage>, String> {
    read_jsonl(&paths.episodes_usage())
}

pub(crate) fn has_usage(paths: &SoulPaths, id: &str) -> Result<bool, String> {
    Ok(read_usage(paths)?.iter().any(|usage| usage.id == id))
}
