//! Bounded in-memory Pi metadata. Credentials and readiness are never cached.
use super::{auth::Backend, error::{LocalError, LocalResult}, query::Query};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path, sync::{Arc, Condvar, Mutex, OnceLock}, time::{Duration, SystemTime}};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FileIdentity { path: std::path::PathBuf, length: u64, modified: Option<SystemTime> }
fn identity(path: &Path, required: bool) -> LocalResult<FileIdentity> {
    match fs::metadata(path) {
        Ok(m) if m.is_file() => Ok(FileIdentity { path: path.into(), length: m.len(), modified: m.modified().ok() }),
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Ok(FileIdentity { path: path.into(), length: 0, modified: None }),
        _ => Err(LocalError::new("local_metadata_read_failed")),
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key { version: &'static str, files: Vec<FileIdentity>, request: String }
fn key(backend: &Backend, request: &Value) -> LocalResult<Key> {
    let mut files = vec![identity(&backend.bundled.executable, true)?];
    let directory = backend.bundled.extension.parent().ok_or_else(|| LocalError::new("local_extension_unavailable"))?;
    let mut extensions = fs::read_dir(directory).map_err(|_| LocalError::new("local_extension_unavailable"))?
        .filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|ext| ext == "ts")).collect::<Vec<_>>();
    extensions.sort();
    for path in extensions { files.push(identity(&path, true)?); }
    for name in ["models.json", "models-store.json"] { files.push(identity(&backend.owned.agent.join(name), false)?); }
    Ok(Key { version: super::bundle::PI_VERSION, files, request: request.to_string() })
}
struct Entry { value: Mutex<Option<LocalResult<Value>>>, stage: Mutex<Option<&'static str>>, ready: Condvar }
type Cache = BTreeMap<Key, Arc<Entry>>;
fn cache() -> &'static Mutex<Cache> { static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new(); CACHE.get_or_init(Default::default) }

/// Same-key callers share one query. Failed/cancelled reads are not retained;
/// waiters still observe that attempt's result rather than stacking retries.
pub(crate) fn query(backend: &Backend, request: Value, control: &Query, validate: impl Fn(&Value) -> LocalResult<()>) -> LocalResult<Value> {
    let cache_key = key(backend, &request)?;
    let (entry, leader) = {
        let mut cache = cache().lock().unwrap();
        if let Some(entry) = cache.get(&cache_key) { (entry.clone(), false) }
        else {
            // No unbounded per-model cache or detached helper process.
            if cache.len() >= 64 {
                cache.retain(|_, entry| entry.value.lock().unwrap().is_none());
                if cache.len() >= 64 { return Err(LocalError::new("local_metadata_busy")); }
            }
            let entry = Arc::new(Entry { value: Mutex::new(None), stage: Mutex::new(None), ready: Condvar::new() });
            cache.insert(cache_key.clone(), entry.clone()); (entry, true)
        }
    };
    if leader {
        let observed = entry.clone();
        let control = control.observe(Arc::new(move |stage| {
            let mut previous = observed.stage.lock().unwrap();
            if *previous != Some(stage) { *previous = Some(stage); observed.ready.notify_all(); }
        }));
        let result = backend.query_observed(request, &control).and_then(|value| { validate(&value)?; Ok(value) });
        *entry.value.lock().unwrap() = Some(result.clone());
        entry.ready.notify_all();
        if result.is_err() { cache().lock().unwrap().remove(&cache_key); }
        result
    } else {
        control.stage("model_metadata")?;
        let mut value = entry.value.lock().unwrap();
        let mut reported = None;
        loop {
            // The leader owns the 40 s readiness / 30 s command deadlines.
            // A waiter must not impose a second, shorter total deadline while
            // Pi is legitimately recovering its 30 s credential-store lock.
            control.check_cancelled("model_metadata")?;
            if let Some(result) = value.as_ref() { return result.clone(); }
            // A workbench joining an availability/catalog read still sees the
            // real helper phase, including Pi's credential-store wait.
            let stage = *entry.stage.lock().unwrap();
            if stage != reported {
                if let Some(stage) = stage { control.report(stage); }
                reported = stage;
            }
            value = entry.ready.wait_timeout(value, Duration::from_millis(20)).unwrap().0;
        }
    }
}
