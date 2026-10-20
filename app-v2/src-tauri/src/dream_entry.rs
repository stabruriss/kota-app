//! Agent submissions for Ember. Inbox files are atomic, account-scoped and distinct
//! from the conversation. Only successful consolidation consumes their contents.
use crate::soul::{self, cli::Env};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub(crate) const HELP: &str = r#"kota-dream — record your dream entry for Ember to consolidate.

  kota-dream -- "<insight>" ["<insight>"]   Record 0 to 2 insights, up to 240 characters each.
  kota-dream --none                         Record that nothing qualifies tonight."#;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Entry {
    pub id: String,
    pub ts: String,
    pub agent_id: String,
    pub agent_display_name: Option<String>,
    pub project_root: PathBuf,
    pub project_id: String,
    pub items: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub none: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

pub(crate) fn inbox(kota_home: &Path) -> PathBuf {
    kota_home.join("dreams/inbox")
}
fn inbox_lock(kota_home: &Path) -> PathBuf {
    kota_home.join("dreams/.inbox.lock")
}

fn normalized_items(items: &[String], none: bool) -> Result<Vec<String>, String> {
    if none && !items.is_empty() {
        return Err("pass insights or --none, not both.".into());
    }
    if items.len() > 2 {
        return Err("at most 2 insights.".into());
    }
    if items.is_empty() && !none {
        return Err("nothing to record. Pass insights or --none.".into());
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let text = item.split_whitespace().collect::<Vec<_>>().join(" ");
            let chars = text.chars().count();
            if chars == 0 {
                return Err(format!("insight {} is empty.", index + 1));
            }
            if chars > 240 {
                return Err(format!(
                    "insight {} is too long: {chars} of 240 characters.",
                    index + 1
                ));
            }
            Ok(text)
        })
        .collect()
}

fn entry_id(agent: &str, project: &str, items: &[String], ts: &str) -> Result<String, String> {
    let date = DateTime::parse_from_rfc3339(ts)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc)
        .format("%Y-%m-%d")
        .to_string();
    // JSON preserves item boundaries (including empty lists) without ambiguous joins.
    let encoded = serde_json::to_string(items).map_err(|e| e.to_string())?;
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{agent}\0{project}\0{encoded}").as_bytes())
    );
    Ok(format!("de-{date}-{}", &digest[..8]))
}

impl Entry {
    pub(crate) fn new(
        env: &Env,
        project_root: &Path,
        display_name: Option<String>,
        items: &[String],
        none: bool,
        ts: &str,
    ) -> Result<Self, String> {
        let items = normalized_items(items, none)?;
        let project_root = project_root
            .canonicalize()
            .map_err(|e| format!("read {}: {e}", project_root.display()))?;
        let project_id = soul::project_slug(env.project.as_deref(), Some(&project_root))
            .ok_or_else(|| format!("invalid project root: {}", project_root.display()))?;
        Ok(Self {
            id: entry_id(&env.agent_id, &project_id, &items, ts)?,
            ts: ts.into(),
            agent_id: env.agent_id.clone(),
            agent_display_name: display_name,
            project_root,
            project_id,
            items,
            none,
        })
    }

    fn validate(&self, path: &Path) -> Result<(), String> {
        if self.agent_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || !self.project_root.is_absolute()
        {
            return Err("invalid Dream entry identity".into());
        }
        if normalized_items(&self.items, self.none)? != self.items {
            return Err("Dream entry items are not normalized".into());
        }
        if self.id != entry_id(&self.agent_id, &self.project_id, &self.items, &self.ts)?
            || path.file_stem().and_then(|s| s.to_str()) != Some(self.id.as_str())
        {
            return Err("Dream entry id does not match its contents or filename".into());
        }
        Ok(())
    }

    pub(crate) fn text(&self) -> String {
        if self.none {
            "__KOTA_DREAM_NONE__".into()
        } else {
            self.items
                .iter()
                .map(|text| format!("- {text}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

pub(crate) fn submit(kota_home: &Path, entry: &Entry) -> Result<(), String> {
    let path = inbox(kota_home).join(format!("{}.json", entry.id));
    entry.validate(&path)?;
    let _lock = soul::lock(&inbox_lock(kota_home))?;
    // A repeated submission is still one file, with a fresh timestamp for this run.
    soul::write_json_atomic(&path, entry)
}

fn read_entry(path: &Path) -> Result<Entry, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let entry: Entry =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
    entry.validate(path)?;
    Ok(entry)
}

/// Only ingestion quarantines malformed files. Status queries are read-only.
pub(crate) fn read_inbox(kota_home: &Path, quarantine: bool) -> Result<Vec<Entry>, String> {
    let directory = inbox(kota_home);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let _lock = soul::lock(&inbox_lock(kota_home))?;
    let mut entries = Vec::new();
    for file in
        fs::read_dir(&directory).map_err(|e| format!("read {}: {e}", directory.display()))?
    {
        let file = file.map_err(|e| e.to_string())?;
        if !file.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let path = file.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match read_entry(&path) {
            Ok(entry) => entries.push(entry),
            Err(error) if quarantine => {
                let rejected = directory.join("rejected");
                fs::create_dir_all(&rejected).map_err(|e| e.to_string())?;
                let target = rejected.join(format!(
                    "{}-{}",
                    Utc::now().timestamp_millis(),
                    file.file_name().to_string_lossy()
                ));
                fs::rename(&path, &target)
                    .map_err(|e| format!("reject {}: {e}", path.display()))?;
                crate::kota_debug_log(&format!(
                    "[ember] rejected Dream inbox file {}: {error}",
                    path.display()
                ));
            }
            Err(_) => {}
        }
    }
    if quarantine {
        clean_rejected(&directory.join("rejected"), Utc::now())?;
    }
    entries.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.id.cmp(&b.id)));
    Ok(entries)
}

fn clean_rejected(directory: &Path, now: DateTime<Utc>) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    let cutoff = (now - chrono::Duration::days(30)).timestamp_millis();
    for file in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let file = file.map_err(|e| e.to_string())?;
        if !file.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let name = file.file_name();
        let name = name.to_string_lossy();
        if name
            .split_once('-')
            .and_then(|(ts, _)| ts.parse::<i64>().ok())
            .is_some_and(|ts| ts < cutoff)
        {
            fs::remove_file(file.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// A run can still observe a submission after another project's consolidation has
// consumed it. One latest timestamp per incarnation; no second persistent ledger.
type SubmissionKey = (PathBuf, PathBuf, String);
static CONSUMED_SUBMISSIONS: OnceLock<Mutex<HashMap<SubmissionKey, DateTime<Utc>>>> =
    OnceLock::new();

pub(crate) fn remove_acknowledged(kota_home: &Path, ids: &HashSet<String>) -> Result<(), String> {
    let _lock = soul::lock(&inbox_lock(kota_home))?;
    for id in ids {
        // Only IDs previously parsed from an inbox record or the processed ledger.
        if !id.starts_with("de-") || id.contains(['/', '\\']) {
            continue;
        }
        let path = inbox(kota_home).join(format!("{id}.json"));
        if !path.exists() {
            continue;
        }
        let entry = read_entry(&path)?;
        let ts = DateTime::parse_from_rfc3339(&entry.ts)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);
        fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
        let mut seen = CONSUMED_SUBMISSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let key = (kota_home.to_path_buf(), entry.project_root, entry.agent_id);
        seen.entry(key)
            .and_modify(|old| *old = (*old).max(ts))
            .or_insert(ts);
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Participant {
    pub agent_id: String,
    pub project_root: PathBuf,
    pub delivered_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStatusRequest {
    pub participants: Vec<Participant>,
    // Terminal work signals already maintained by agentRuntime in the host UI.
    #[serde(default)]
    pub working_agent_ids: HashSet<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStatus {
    pub agent_id: String,
    pub project_root: PathBuf,
    pub submitted: bool,
    pub working: bool,
}

pub(crate) fn local_is_working(snapshot: &crate::local_rpc::Snapshot, root: &Path) -> bool {
    use crate::local_rpc::OnlineState;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if snapshot.scope.project_root != root {
        return false;
    }
    match snapshot.state {
        OnlineState::Starting | OnlineState::Busy => true,
        OnlineState::Ready => snapshot.compacting || snapshot.queued > 0,
        OnlineState::Offline | OnlineState::Error => false,
    }
}

pub(crate) fn has_submitted(
    kota_home: &Path,
    participant: &Participant,
    entries: &[Entry],
) -> bool {
    let Ok(since) = DateTime::parse_from_rfc3339(&participant.delivered_at) else {
        return false;
    };
    let root = participant
        .project_root
        .canonicalize()
        .unwrap_or_else(|_| participant.project_root.clone());
    if entries.iter().any(|entry| {
        entry.agent_id == participant.agent_id
            && entry.project_root == root
            && DateTime::parse_from_rfc3339(&entry.ts).is_ok_and(|ts| ts >= since)
    }) {
        return true;
    }
    let seen = CONSUMED_SUBMISSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    seen.get(&(kota_home.to_path_buf(), root, participant.agent_id.clone()))
        .is_some_and(|ts| *ts >= since)
}

pub fn run_cli() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = (|| {
        let (flags, rest) = soul::cli::split_flags(&args).map_err(|e| e.to_string())?;
        if matches!(
            rest.first().map(String::as_str),
            None | Some("--help" | "-h" | "help")
        ) {
            return Ok(HELP.to_string());
        }
        let (items, none) = parse_items(&rest)?;
        let env = Env::from_process(&flags).map_err(|e| {
            e.to_string()
                .trim_start_matches("kota-my-soul ")
                .to_string()
        })?;
        let root = flags
            .get("--project-root")
            .cloned()
            .or_else(|| std::env::var("KOTA_PROJECT_ROOT").ok())
            .map(PathBuf::from)
            .or_else(|| env.agent_cwd.parent()?.parent().map(Path::to_path_buf))
            .ok_or_else(|| "requires KOTA_AGENT_CWD or --agent-cwd".to_string())?;
        let display_name = fs::read_to_string(env.agent_cwd.join("agent.yaml"))
            .ok()
            .and_then(|text| serde_yaml::from_str::<serde_yaml::Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("display-name")
                    .or_else(|| value.get("displayName"))
                    .and_then(|value| value.as_str())
                    .map(crate::agent_aka_from_display_name)
            });
        let entry = Entry::new(
            &env,
            &root,
            display_name,
            &items,
            none,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        )?;
        submit(&env.kota_home, &entry)?;
        Ok(success_reply(entry.items.len()))
    })();
    match result {
        Ok(output) => {
            println!("{output}");
            Ok(())
        }
        Err(error) if error == HELP => Err(error),
        Err(error) => Err(format!("{error}\n\n{HELP}")),
    }
}

fn parse_items(args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut literal = false;
    let mut none = false;
    let mut items = Vec::new();
    for arg in args {
        if !literal && arg == "--" {
            literal = true;
        } else if !literal && arg == "--none" {
            none = true;
        } else if !literal && arg.starts_with('-') {
            return Err(HELP.to_string());
        } else {
            items.push(arg.clone());
        }
    }
    Ok((normalized_items(&items, none)?, none))
}

fn success_reply(count: usize) -> String {
    match count {
        0 => "Dream entry recorded (none).".into(),
        1 => "Dream entry recorded (1 item).".into(),
        _ => format!("Dream entry recorded ({count} items)."),
    }
}

pub fn install_cli_shim() -> anyhow::Result<PathBuf> {
    crate::soul::cli::install_named_cli_shim("kota-dream", "KOTA_DREAM_BIN")
}

#[cfg(test)]
mod tests;
