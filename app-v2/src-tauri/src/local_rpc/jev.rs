//! Shared Jev channel and keys (Auto Route, and Puppeteer later). Kota writes a key only after a
//! save-time check; a Jev user that receives an authentication failure writes a per-key marker.
//! The two writers never share a file. A key leaves this module only as one request option.
use super::{
    auth::Backend,
    credentials::validate_literal_key,
    error::{LocalError, LocalResult},
    owned::{read_json, write_json, FileLock, OwnedDirectory},
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

/// Link order. Each channel keeps its own key; one channel is selected for every Jev user.
pub(crate) const PROVIDERS: [&str; 4] = ["openrouter", "vercel-ai-gateway", "opencode", "typesafe"];
const DEFAULT_PROVIDER: &str = "openrouter";

/// Pi 1.0.0 catalog classifier per channel (PLAN v5 §3).
pub(crate) fn model_id(provider: &str) -> Option<&'static str> {
    match provider {
        "openrouter" => Some("~typesafe/jev-latest"),
        "vercel-ai-gateway" => Some("typesafe-ai/jev"),
        "opencode" => Some("jev-1.13"),
        "typesafe" => Some("jev-latest"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyState { pub provider_id: String, pub status: &'static str }
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JevSettings { pub schema: u8, pub provider: String, pub keys: Vec<KeyState>, pub ready: bool }

/// The selected channel's usable key. Deliberately neither Debug nor Serialize.
pub(crate) struct JevCredential { pub provider: String, pub model_id: &'static str, pub key: String, pub key_id: String }

struct Stored { provider: String, keys: Map<String, Value> }

fn directory(owned: &OwnedDirectory) -> PathBuf { owned.root.join("jev") }
fn keys_path(owned: &OwnedDirectory) -> PathBuf { directory(owned).join("keys.json") }
fn rejected_path(owned: &OwnedDirectory, provider: &str) -> PathBuf { directory(owned).join(format!("rejected-{provider}.json")) }

fn load(owned: &OwnedDirectory) -> LocalResult<Stored> {
    let invalid = || LocalError::new("jev_store_invalid");
    let Some(value) = read_json(&keys_path(owned), "jev_store_invalid")? else {
        return Ok(Stored { provider: DEFAULT_PROVIDER.into(), keys: Map::new() });
    };
    let provider = value["provider"].as_str().filter(|p| PROVIDERS.contains(p)).ok_or_else(invalid)?;
    let keys = value["keys"].as_object().cloned().ok_or_else(invalid)?;
    if value["schema"] != 1 || keys.iter().any(|(id, entry)| !PROVIDERS.contains(&id.as_str())
        || !entry["key"].as_str().is_some_and(|k| !k.is_empty()) || !entry["keyId"].as_str().is_some_and(|k| !k.is_empty())) {
        return Err(invalid());
    }
    Ok(Stored { provider: provider.into(), keys })
}
fn save(owned: &OwnedDirectory, stored: &Stored) -> LocalResult<()> {
    super::owned::directory_private(&directory(owned))?;
    write_json(&keys_path(owned), &json!({"schema":1,"provider":stored.provider,"keys":stored.keys}), "jev_store_write_failed")
}
fn status(owned: &OwnedDirectory, stored: &Stored, provider: &str) -> LocalResult<&'static str> {
    let Some(key_id) = stored.keys.get(provider).and_then(|entry| entry["keyId"].as_str()) else { return Ok("none") };
    let marker = read_json(&rejected_path(owned, provider), "jev_store_invalid")?;
    Ok(if marker.is_some_and(|m| m["keyId"] == key_id) { "rejected" } else { "verified" })
}

pub(crate) fn read(owned: &OwnedDirectory) -> LocalResult<JevSettings> {
    let stored = load(owned)?;
    let keys = PROVIDERS.iter().map(|p| Ok(KeyState { provider_id: (*p).into(), status: status(owned, &stored, p)? }))
        .collect::<LocalResult<Vec<_>>>()?;
    let ready = keys.iter().any(|k| k.provider_id == stored.provider && k.status == "verified");
    Ok(JevSettings { schema: 1, provider: stored.provider, keys, ready })
}

/// Auto Route and Puppeteer gate: the selected channel has a verified, unrejected key.
pub(crate) fn gate(owned: &OwnedDirectory) -> LocalResult<()> {
    let stored = load(owned)?;
    match status(owned, &stored, &stored.provider)? {
        "verified" => Ok(()),
        "rejected" => Err(LocalError::new("jev_key_rejected")),
        _ => Err(LocalError::new("jev_key_required")),
    }
}

/// For a Jev user outside Pi (Puppeteer). `None` when the selected channel is not ready.
pub(crate) fn current(owned: &OwnedDirectory) -> LocalResult<Option<JevCredential>> {
    let stored = load(owned)?;
    if status(owned, &stored, &stored.provider)? != "verified" { return Ok(None); }
    let entry = &stored.keys[&stored.provider];
    Ok(Some(JevCredential { model_id: model_id(&stored.provider).unwrap(), key: entry["key"].as_str().unwrap().into(),
        key_id: entry["keyId"].as_str().unwrap().into(), provider: stored.provider }))
}
/// Marks exactly the key that Jev rejected. A newer saved key has another keyId.
pub(crate) fn mark_rejected(owned: &OwnedDirectory, credential: &JevCredential) -> LocalResult<()> {
    write_json(&rejected_path(owned, &credential.provider), &json!({"schema":1,"keyId":credential.key_id}), "jev_store_write_failed")
}

/// One short lock for Jev configuration writes. Pi helpers never take it.
pub(crate) fn configuration_lock(owned: &OwnedDirectory) -> LocalResult<FileLock> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match FileLock::acquire(&owned.root.join(".jev.lock"), "local_config_busy") {
            Err(error) if error.code == "local_config_busy" && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            result => return result,
        }
    }
}

pub(crate) fn select(backend: &Backend, provider: &str) -> LocalResult<JevSettings> {
    if !PROVIDERS.contains(&provider) { return Err(LocalError::new("invalid_request")); }
    backend.owned.initialize()?;
    let _lock = configuration_lock(&backend.owned)?;
    let mut stored = load(&backend.owned)?;
    if stored.provider != provider { stored.provider = provider.into(); save(&backend.owned, &stored)?; }
    read(&backend.owned)
}

struct Verification { request_id: String, cancelled: Arc<AtomicBool> }
/// One save-time check at a time per Kota Local directory.
fn in_flight() -> &'static Mutex<BTreeMap<PathBuf, Verification>> {
    static SLOTS: OnceLock<Mutex<BTreeMap<PathBuf, Verification>>> = OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

/// Saves a key only after Jev accepts it. A failed or unavailable check keeps the old key; a
/// result that returns after cancellation is discarded.
pub(crate) fn save_key(backend: &Backend, request_id: &str, provider: &str, key: &str) -> LocalResult<JevSettings> {
    save_key_with(backend, request_id, provider, key, super::auth::jev_verify)
}
pub(crate) fn save_key_with(backend: &Backend, request_id: &str, provider: &str, key: &str,
    verify: impl FnOnce(&Backend, &str, &str, &dyn Fn() -> bool) -> LocalResult<&'static str>) -> LocalResult<JevSettings> {
    if !PROVIDERS.contains(&provider) || !super::error::identifier(request_id, 128) { return Err(LocalError::new("invalid_request")); }
    if key.is_empty() { return Err(LocalError::new("jev_key_empty")); }
    if validate_literal_key(key, provider).is_err() { return Err(LocalError::new("jev_verification_failed")); }
    let cancelled = Arc::new(AtomicBool::new(false));
    let slot = backend.owned.root.clone();
    {
        let mut slots = in_flight().lock().unwrap();
        if slots.contains_key(&slot) { return Err(LocalError::new("busy-auth")); }
        slots.insert(slot.clone(), Verification { request_id: request_id.into(), cancelled: cancelled.clone() });
    }
    let result = (|| {
        backend.owned.initialize()?;
        let observed = cancelled.clone();
        let outcome = verify(backend, provider, key, &move || observed.load(Ordering::SeqCst));
        if cancelled.load(Ordering::SeqCst) { return Err(LocalError::new("jev_verification_cancelled")); }
        match outcome {
            Ok("verified") => {},
            Err(error) if error.code == "jev_verification_cancelled" => return Err(error),
            _ => return Err(LocalError::new("jev_verification_failed")),
        }
        let _lock = configuration_lock(&backend.owned)?;
        // cancel() takes the same slot lock, so a cancel either precedes this check or follows the write.
        let _slots = in_flight().lock().unwrap();
        if cancelled.load(Ordering::SeqCst) { return Err(LocalError::new("jev_verification_cancelled")); }
        let mut stored = load(&backend.owned)?;
        stored.keys.insert(provider.into(), json!({"key":key,"keyId":uuid::Uuid::new_v4().to_string()}));
        save(&backend.owned, &stored)?;
        let _ = std::fs::remove_file(rejected_path(&backend.owned, provider));
        read(&backend.owned)
    })();
    in_flight().lock().unwrap().remove(&slot);
    result
}
pub(crate) fn cancel(request_id: &str) -> LocalResult<()> {
    for active in in_flight().lock().unwrap().values().filter(|v| v.request_id == request_id) {
        active.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
#[path = "jev_tests.rs"]
mod tests;
