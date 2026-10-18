//! User-managed Magpie gateway, exposed through Pi's ordinary models.json provider.
//! No agent wiring, process management, model calls, or background discovery.
use super::{auth::{self, Backend}, credentials::{AuthStore, validate_literal_key},
    error::{LocalError, LocalResult}, owned::{read_json, write_json, OwnedDirectory}, query::Query};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(crate) const DEFAULT_KEY: &str = "magpie-kota";
const TIMEOUT: Duration = Duration::from_secs(10);
const PROVIDER: &str = "magpie";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode { #[default] Local, Lan }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings { pub mode: Mode, pub ip: String, pub port: u16 }
impl Default for Settings {
    fn default() -> Self { Self { mode: Mode::Local, ip: String::new(), port: 3425 } }
}
impl Settings {
    fn address(&self) -> LocalResult<String> {
        let host = if self.mode == Mode::Local { "127.0.0.1" } else { self.ip.trim() };
        let mut url = url::Url::parse("http://127.0.0.1").unwrap();
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            url.set_ip_host(ip).map_err(|_| error("unreachable"))?;
        } else { url.set_host(Some(host)).map_err(|_| error("unreachable"))?; }
        url.set_port(Some(self.port)).map_err(|_| error("unreachable"))?;
        Ok(url.as_str().trim_end_matches('/').into())
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct State {
    pub settings: Settings, pub linked: bool, pub key_saved: bool, pub model_count: usize,
    pub default_key: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
// Deliberately not Debug/Serialize: the optional key is an IPC input, never a status value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Save { pub settings: Settings, pub key: Option<String> }
fn error(code: &str) -> LocalError { LocalError::provider(code, PROVIDER) }
fn config(owned: &OwnedDirectory) -> LocalResult<Value> {
    let value = read_json(&owned.agent.join("models.json"), "save_failed")?
        .unwrap_or_else(|| json!({"providers":{}}));
    if !value.is_object() || value.get("providers").is_some_and(|v| !v.is_object()) { return Err(error("save_failed")); }
    Ok(value)
}
fn settings(config: &Value) -> LocalResult<Settings> {
    config.get("kotaMagpie").map(|v| serde_json::from_value(v.clone()).map_err(|_| error("save_failed")))
        .transpose().map(|s| s.unwrap_or_default())
}
fn credential(owned: &OwnedDirectory) -> LocalResult<Option<String>> {
    let store = AuthStore::load(owned).map_err(|_| error("save_failed"))?;
    store.preflight().map_err(|_| error("save_failed"))?;
    store.api_key(PROVIDER).map(|v| v.map(str::to_owned)).map_err(|_| error("save_failed"))
}
pub(crate) fn read(owned: &OwnedDirectory) -> LocalResult<State> {
    let config = config(owned)?;
    Ok(State { settings: settings(&config)?, linked: false, key_saved: credential(owned)?.is_some(),
        model_count: config["providers"][PROVIDER]["models"].as_array().map_or(0, Vec::len),
        default_key: DEFAULT_KEY, error: None })
}
fn write_provider(owned: &OwnedDirectory, settings: &Settings, models: Option<Vec<Value>>) -> LocalResult<()> {
    let _lock = owned.configuration_lock_observed(&Query::standard()).map_err(|_| error("save_failed"))?;
    let mut config = config(owned)?;
    let before = config.clone();
    let base = settings.address()?;
    let models = models.unwrap_or_else(|| config["providers"][PROVIDER]["models"].as_array().cloned().unwrap_or_default());
    let models: Vec<_> = models.into_iter().map(|mut m| {
        if m["api"] == "anthropic-messages" { m["baseUrl"] = json!(base); }
        else if let Some(m) = m.as_object_mut() { m.remove("baseUrl"); }
        m
    }).collect();
    config["kotaMagpie"] = serde_json::to_value(settings).unwrap();
    if config.get("providers").is_none() { config["providers"] = json!({}); }
    config["providers"][PROVIDER] = json!({"name":"Magpie","baseUrl":format!("{base}/v1"),
        "api":"openai-completions","models":models});
    if config != before { write_json(&owned.agent.join("models.json"), &config, "save_failed")?; }
    Ok(())
}

pub(crate) fn probe(backend: &Backend) -> LocalResult<State> { run(backend, None, auth::save_magpie_key) }
pub(crate) fn save(backend: &Backend, request: Save) -> LocalResult<State> { run(backend, Some(request), auth::save_magpie_key) }
fn run(backend: &Backend, request: Option<Save>, save_key: impl Fn(&Backend, &str) -> LocalResult<()>) -> LocalResult<State> {
    backend.owned.initialize().map_err(|_| error("save_failed"))?;
    // Reuse the provider's existing lock, including across Apps and native auth operations.
    let _lock = backend.owned.provider_lock(PROVIDER).map_err(|_| error("save_failed"))?;
    let mut state = read(&backend.owned)?;
    let stored = credential(&backend.owned)?;
    let mut credential_saved = false;
    let outcome: LocalResult<()> = (|| {
        let key = if let Some(request) = request {
            let key = request.key.filter(|v| !v.is_empty()).or_else(|| {
                if request.settings.mode == Mode::Local { Some(DEFAULT_KEY.into()) }
                else if state.settings.mode == Mode::Lan && state.settings.ip == request.settings.ip && state.settings.port == request.settings.port { stored.clone() }
                else { None }
            }).ok_or_else(|| error("key_rejected"))?;
            validate_literal_key(&key, PROVIDER).map_err(|_| error("key_rejected"))?;
            write_provider(&backend.owned, &request.settings, None)?;
            state.settings = request.settings;
            if stored.as_deref() != Some(&key) {
                save_key(backend, &key).map_err(|_| error("save_failed"))?;
                credential_saved = true;
            }
            key
        } else {
            stored.clone().or_else(|| (state.settings.mode == Mode::Local).then(|| DEFAULT_KEY.into()))
                .ok_or_else(|| error("key_rejected"))?
        };
        let models = fetch_models(&state.settings, &key)?;
        write_provider(&backend.owned, &state.settings, Some(models))?;
        if stored.is_none() && !credential_saved && state.settings.mode == Mode::Local {
            save_key(backend, &key).map_err(|_| error("save_failed"))?;
        }
        Ok(())
    })();
    let mut result = read(&backend.owned)?;
    match outcome { Ok(()) => result.linked = true, Err(e) => result.error = Some(e.code) }
    Ok(result)
}
fn fetch_models(settings: &Settings, key: &str) -> LocalResult<Vec<Value>> {
    let base = settings.address()?;
    let deadline = Instant::now() + TIMEOUT;
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let get = |path: &str, invalid: &str| -> LocalResult<Value> {
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or_else(|| error("unreachable"))?;
        let response = agent.get(&format!("{base}{path}")).timeout(remaining)
            .set("Authorization", &format!("Bearer {key}")).call().map_err(|e| match e {
                ureq::Error::Status(401 | 403, _) => error("key_rejected"),
                ureq::Error::Status(_, _) if path == "/v1/models" => error("catalog_failed"),
                ureq::Error::Status(_, _) => error("not_magpie"),
                _ => error("unreachable"),
            })?;
        response.into_json().map_err(|_| error(invalid))
    };
    if get("/api/hello", "not_magpie")?["name"] != "magpie" { return Err(error("not_magpie")); }
    let list: ModelList = serde_json::from_value(get("/v1/models", "catalog_failed")?).map_err(|_| error("catalog_failed"))?;
    Ok(list.data.into_iter().filter(|m| !m.id.starts_with("group/")).map(model).collect())
}

#[derive(Deserialize)]
struct ModelList { data: Vec<GatewayModel> }
#[derive(Deserialize)]
struct Effort { effort: String }
#[derive(Default, Deserialize)]
struct Modalities { #[serde(default)] input: Vec<String> }
#[derive(Deserialize)]
struct GatewayModel {
    id: String, display_name: Option<String>, context_window: Option<u64>, max_output_tokens: Option<u64>,
    #[serde(default)] supported_reasoning_levels: Vec<Effort>,
    #[serde(default)] native_endpoints: Vec<String>,
    #[serde(default)] modalities: Modalities,
}
// Port of Magpie e4c292f agents.go piModelJSON/piThinkingLevels/piPromptCache (MIT).
// /v1/models does not publish Price, so cost is omitted and Pi supplies its defaults.
fn model(m: GatewayModel) -> Value {
    let (claude, adaptive) = claude_flags(&m.id);
    let mut v = json!({"id":m.id,"name":m.display_name.as_deref().unwrap_or(&m.id),"reasoning":!m.supported_reasoning_levels.is_empty()});
    let responses = m.native_endpoints.iter().any(|a| a == "/v1/responses");
    let messages = !responses && m.native_endpoints.iter().any(|a| a == "/v1/messages");
    if responses { v["api"] = json!("openai-responses"); }
    else if messages { v["api"] = json!("anthropic-messages"); if adaptive { v["compat"] = json!({"forceAdaptiveThinking":true}); } }
    if m.modalities.input.iter().any(|s| s == "image") { v["input"] = json!(["text","image"]); }
    if !m.supported_reasoning_levels.is_empty() {
        let mut levels = serde_json::Map::new();
        for level in super::auth_protocol::THINKING_LEVELS {
            let effort = if *level == "off" { "none" } else { level };
            if m.supported_reasoning_levels.iter().any(|e| e.effort == effort) { levels.insert((*level).into(), json!(effort)); }
            else if !(*level == "off" && messages && claude) { levels.insert((*level).into(), Value::Null); }
        }
        v["thinkingLevelMap"] = Value::Object(levels);
    }
    if let Some(window) = m.context_window.filter(|n| *n > 0) { v["contextWindow"] = json!(window); }
    if let Some(output) = m.max_output_tokens.filter(|n| *n > 0) {
        v["maxTokens"] = json!(m.context_window.filter(|n| *n > 0).map_or(output, |window| window.min(output)));
    }
    let name = m.id.rsplit('/').next().unwrap_or(&m.id).to_ascii_lowercase();
    if messages && name.starts_with("claude") { v["promptCache"] = json!({"short":300,"long":3600}); }
    else if responses && name.starts_with("gpt") { v["promptCache"] = json!({"short":300}); }
    v
}
// The upstream Claude compatibility rule, not a model eligibility filter. Use string
// boundaries/version digits rather than introducing regular-expression validation.
fn claude_flags(id: &str) -> (bool, bool) {
    let id = id.to_ascii_lowercase();
    let mut claude = id.starts_with("claude-") || ["/claude-", ".claude-", ":claude-", "-claude-"].iter().any(|s| id.contains(s));
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut versions = Vec::new();
    for family in ["opus-", "sonnet-", "haiku-"] {
        for (at, _) in id.match_indices(family) {
            if at > 0 && id.as_bytes()[at - 1].is_ascii_alphanumeric() { continue; }
            let tail = &id[at + family.len()..]; let n = digits(tail);
            if n == 0 || n > 2 { continue; }
            let major: u32 = tail[..n].parse().unwrap_or(0);
            let rest = &tail[n..];
            let minor = rest.strip_prefix(['-', '.']).and_then(|s| { let n = digits(s); (n > 0 && n <= 2).then(|| s[..n].parse::<u32>().unwrap_or(0)) }).unwrap_or(0);
            versions.push((at, major, minor));
        }
    }
    for (at, _) in id.match_indices("claude-") {
        let tail = &id[at + 7..]; let n = digits(tail); if n == 0 { continue; }
        let major = tail[..n].parse::<u32>().unwrap_or(0); let mut rest = &tail[n..]; let mut minor = 0;
        if let Some(s) = rest.strip_prefix(['-', '.']) {
            if s.as_bytes().first().is_some_and(u8::is_ascii_digit) { minor = (s.as_bytes()[0] - b'0') as u32; rest = &s[1..]; }
        }
        if ["-opus", "-sonnet", "-haiku"].iter().any(|s| rest.starts_with(s)) { versions.push((at, major, minor)); }
    }
    versions.sort_by_key(|v| v.0);
    if let Some((_, major, minor)) = versions.first() { claude = true; (claude, *major > 4 || *major == 4 && *minor >= 6) }
    else { (claude, false) }
}

#[cfg(test)]
#[path = "magpie_tests.rs"]
mod tests;
