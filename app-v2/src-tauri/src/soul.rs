//! kota-my-soul: an incarnation's marks, pushes and episodes under `under-table/`,
//! plus the account-level Dream usage ledger in `$KOTA_HOME/dreams/`.
//! Files are the source of truth (SOUL-PLAN v4 §13–§18, UNDER-TABLE-SAMPLE.md): JSON
//! documents are replaced atomically, ledgers are append-only JSONL, and every writer
//! holds a short flock. Model-visible copy lives in `cli.rs` and is used verbatim.
pub(crate) mod carry;
pub(crate) mod cli;
pub(crate) mod dream_usage;
pub(crate) mod episodes;
pub(crate) mod export;
pub(crate) mod marks;
pub(crate) mod pushes;
pub(crate) mod search;
#[cfg(test)]
mod tests;

pub use cli::{install_cli_shim, run_cli};

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where one incarnation keeps its soul: everything under `<agent CWD>/under-table/`.
#[derive(Clone, Debug)]
pub(crate) struct SoulPaths {
    agent_cwd: PathBuf,
}

impl SoulPaths {
    pub(crate) fn new(agent_cwd: &Path) -> Self {
        Self { agent_cwd: agent_cwd.to_path_buf() }
    }
    pub(crate) fn under_table(&self) -> PathBuf {
        self.agent_cwd.join("under-table")
    }
    pub(crate) fn soul_dir(&self) -> PathBuf {
        self.under_table().join("soul")
    }
    pub(crate) fn marks(&self) -> PathBuf {
        self.soul_dir().join("marks.json")
    }
    pub(crate) fn pushes(&self) -> PathBuf {
        self.soul_dir().join("pushes.jsonl")
    }
    pub(crate) fn soul_lock(&self) -> PathBuf {
        self.soul_dir().join(".lock")
    }
    pub(crate) fn episodes_dir(&self) -> PathBuf {
        self.under_table().join("episodes")
    }
    pub(crate) fn episodes(&self) -> PathBuf {
        self.episodes_dir().join("episodes.jsonl")
    }
    pub(crate) fn episodes_usage(&self) -> PathBuf {
        self.episodes_dir().join("usage.jsonl")
    }
    pub(crate) fn episodes_state(&self) -> PathBuf {
        self.episodes_dir().join("state.json")
    }
    pub(crate) fn episodes_index(&self) -> PathBuf {
        self.episodes_dir().join("index.sqlite")
    }
    pub(crate) fn episodes_lock(&self) -> PathBuf {
        self.episodes_dir().join(".lock")
    }
    /// Held for a whole export, which takes `episodes_lock` inside it to append.
    pub(crate) fn export_lock(&self) -> PathBuf {
        self.episodes_dir().join("export.lock")
    }
}

/// The account-level Dream usage ledger: one file for every incarnation on this machine.
pub(crate) fn dreams_usage_path(kota_home: &Path) -> PathBuf {
    kota_home.join("dreams").join("usage.jsonl")
}

pub(crate) fn dreams_usage_lock(kota_home: &Path) -> PathBuf {
    kota_home.join("dreams").join(".usage.lock")
}

/// An exclusive flock on a stable, never-unlinked lock file. Kernel ownership ends on
/// close or crash, so a dead writer never leaves a stale lock behind.
pub(crate) struct Lock(#[allow(dead_code)] File);

const LOCK_WAIT: Duration = Duration::from_secs(5);

pub(crate) fn lock(path: &Path) -> Result<Lock, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let started = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Lock(file));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock && error.kind() != io::ErrorKind::Interrupted {
                return Err(format!("lock {}: {error}", path.display()));
            }
            if started.elapsed() >= LOCK_WAIT {
                return Err("kota-my-soul is busy; retry".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err("kota-my-soul requires an OS file lock on this platform".into())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// Reads an append-only ledger. A missing file is empty; an I/O failure is an error; a
/// torn last line (a writer mid-append) or an unparsable line is skipped, never fatal.
pub(crate) fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read {}: {err}", path.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<T>(line.trim()).ok())
        .collect())
}

/// Appends whole lines in one write so a reader never sees half a record.
pub(crate) fn append_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    if lines.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let mut buffer = String::new();
    for line in lines {
        buffer.push_str(line);
        buffer.push('\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("open {}: {err}", path.display()))?;
    file.write_all(buffer.as_bytes())
        .map_err(|err| format!("write {}: {err}", path.display()))
}

/// Pretty JSON, replaced atomically; the parent directory is made first (a fresh
/// under-table has none yet).
pub(crate) fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let mut content = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    content.push('\n');
    crate::adapter_sync::atomic_replace(path, content.as_bytes())
}

/// UTC, RFC 3339 at second precision, as every sample timestamp is written.
pub(crate) fn now_ts() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The calendar date of a timestamp (`2026-10-14` of `2026-10-14T07:02:11Z`).
pub(crate) fn date_of(ts: &str) -> &str {
    ts.get(..10).unwrap_or(ts)
}

/// The project a record was written in. The registered project id when there is one
/// (unique, survives moving the directory); otherwise `<dir name>-<sha256(root)[..6]>`.
pub(crate) fn project_slug(project_id: Option<&str>, project_root: Option<&Path>) -> Option<String> {
    if let Some(id) = project_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Some(id.to_string());
    }
    let root = project_root?;
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let name = canonical.file_name()?.to_string_lossy().to_string();
    let digest = format!("{:x}", Sha256::digest(canonical.to_string_lossy().as_bytes()));
    Some(format!("{name}-{}", &digest[..6]))
}
