//! Shared Local MCP configuration and a secret-free, session-scoped UI view.
//! Neither operation connects/reloads a server or reads model credentials.
use super::{error::{LocalError, LocalResult}, owned::{OwnedDirectory, read_json, write_json}};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const CODEC: &str = "kota.mcp.v1:";
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all="snake_case")]
pub(crate) enum Health { Connected, Connecting, NeedsAuth, NotConnected, Failed }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all="snake_case")]
pub(crate) enum Reason { ConnectionFailed, ConnectionRefused, CommandNotFound, ConnectionTimeout, AuthenticationFailed }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveServer {
    name: String, status: Health,
    #[serde(skip_serializing_if="Option::is_none")]
    reason: Option<Reason>,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    schema: u8, pub request_id: String, pub available: bool, pub servers: Vec<LiveServer>,
}
impl Receipt {
    pub fn data(&self) -> Value { serde_json::json!({"available":self.available,"servers":self.servers}) }
}
pub(super) fn receipt(event: &Value) -> Option<Receipt> {
    if event["type"] != "extension_ui_request" || event["method"] != "notify" { return None; }
    let text = event["message"].as_str()?.strip_prefix(CODEC)?;
    if text.len() > 64 * 1024 { return None; }
    let value: Receipt = serde_json::from_str(text).ok()?;
    let mut names = BTreeSet::new();
    if value.schema != 1 || !super::error::identifier(&value.request_id,128) || value.servers.len() > 256
        || (!value.available && !value.servers.is_empty()) || value.servers.iter().any(|row|
            !name_valid(&row.name) || !names.insert(&row.name) || (row.reason.is_some() && row.status != Health::Failed)) { return None; }
    Some(value)
}

fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b,b'_'|b'-'))
}
#[derive(Clone)]
struct Configured { name: String, enabled: bool }
fn configured(value: &Value) -> LocalResult<Vec<Configured>> {
    let root = value.as_object().ok_or_else(|| LocalError::new("mcp_config_invalid"))?;
    let Some(servers) = root.get("mcpServers") else { return Ok(vec![]); };
    let servers = servers.as_object().filter(|s| s.len() <= 256).ok_or_else(|| LocalError::new("mcp_config_invalid"))?;
    servers.iter().map(|(name,config)| {
        let config = config.as_object().filter(|_| name_valid(name)).ok_or_else(|| LocalError::new("mcp_config_invalid"))?;
        let enabled = config.get("enabled").map(|v| v.as_bool().ok_or_else(|| LocalError::new("mcp_config_invalid"))).transpose()?.unwrap_or(true);
        Ok(Configured { name:name.clone(), enabled })
    }).collect()
}
fn read(owned: &OwnedDirectory) -> LocalResult<Vec<Configured>> {
    owned.validate_existing()?;
    read_json(&owned.agent.join("mcp.json"),"mcp_config_invalid")?.map(|v| configured(&v)).transpose().map(|v| v.unwrap_or_default())
}

#[derive(Serialize)]
pub(crate) struct Server {
    name: String, status: Health, configured: bool, enabled: bool,
    #[serde(skip_serializing_if="Option::is_none")]
    reason: Option<Reason>,
}
#[derive(Serialize)]
pub(crate) struct View { schema: u8, available: bool, servers: Vec<Server> }
pub(crate) fn view(owned: &OwnedDirectory, live: Option<Value>) -> LocalResult<View> {
    let configured = read(owned)?;
    let live = live.map(|v| {
        // The runtime already validates the full receipt. Keep this boundary
        // typed as well; no transport errors or arbitrary strings reach UI.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Live { available: bool, servers: Vec<LiveServer> }
        serde_json::from_value::<Live>(v).map_err(|_| LocalError::new("mcp_status_unavailable"))
    }).transpose()?;
    if live.as_ref().is_some_and(|v| !v.available) { return Ok(View { schema:1, available:false, servers:vec![] }); }
    let mut rows: BTreeMap<_,_> = live.into_iter().flat_map(|v| v.servers).map(|row| (row.name.clone(),row)).collect();
    let mut servers = vec![];
    for config in configured {
        let row = rows.remove(&config.name);
        servers.push(Server { name:config.name, status:row.as_ref().map(|r| r.status).unwrap_or(Health::NotConnected),
            reason:row.and_then(|r| r.reason), configured:true, enabled:config.enabled });
    }
    // A removed server can still have this session's native connection. Retain
    // that fact, without pretending Remove disconnected it or reloading Pi.
    servers.extend(rows.into_values().filter(|r| matches!(r.status,Health::Connected|Health::Connecting)).map(|row|
        Server { name:row.name, status:row.status, reason:row.reason, configured:false, enabled:true }));
    Ok(View { schema:1, available:true, servers })
}

pub(crate) fn remove(owned: &OwnedDirectory, name: &str) -> LocalResult<bool> {
    if !name_valid(name) { return Err(LocalError::new("invalid_request")); }
    owned.validate_existing()?;
    let path = owned.agent.join("mcp.json");
    if read_json(&path,"mcp_config_invalid")?.is_none() { return Ok(false); }
    let query = super::query::Query::new(super::auth::QUERY_TIMEOUT);
    let _lock = owned.configuration_lock_observed(&query)?;
    let Some(mut value) = read_json(&path,"mcp_config_invalid")? else { return Ok(false); };
    configured(&value)?;
    let removed = value.get_mut("mcpServers").and_then(Value::as_object_mut).is_some_and(|servers| servers.remove(name).is_some());
    if removed { write_json(&path,&value,"mcp_config_write_failed")?; }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(std::path::PathBuf);
    impl Drop for Directory { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.0); } }
    fn owned() -> (Directory, OwnedDirectory) {
        let temp = Directory(std::env::temp_dir().join(format!("kota-mcp-fixture-{}",uuid::Uuid::new_v4())));
        let owned = OwnedDirectory::new(&temp.0);
        std::fs::create_dir_all(&owned.agent).unwrap(); (temp,owned)
    }
    #[test]
    fn remove_preserves_other_servers_auth_and_unknown_configuration() {
        let (_tmp,owned) = owned();
        let config = serde_json::json!({"mcpServers":{"a":{"command":"a","env":{"TOKEN":"fake-a"}},"b":{"url":"http://example.invalid"}},"autoEnableCodemode":false,"future":{"keep":true}});
        write_json(&owned.agent.join("mcp.json"),&config,"fixture").unwrap();
        std::fs::write(owned.agent.join("auth.json"),b"fake model credentials").unwrap();
        std::fs::write(owned.agent.join("mcp-auth.json"),b"fake MCP credentials").unwrap();
        assert!(remove(&owned,"a").unwrap());
        let mut expected = config; expected["mcpServers"].as_object_mut().unwrap().remove("a");
        assert_eq!(read_json(&owned.agent.join("mcp.json"),"fixture").unwrap(),Some(expected));
        assert_eq!(std::fs::read(owned.auth_path()).unwrap(),b"fake model credentials");
        assert_eq!(std::fs::read(owned.agent.join("mcp-auth.json")).unwrap(),b"fake MCP credentials");
        assert!(!remove(&owned,"a").unwrap());
    }
    #[test]
    fn offline_read_and_removed_live_connection_are_distinct() {
        let (_tmp,owned) = owned();
        write_json(&owned.agent.join("mcp.json"),&serde_json::json!({"mcpServers":{"new":{"command":"new"}}}),"fixture").unwrap();
        let offline = view(&owned,None).unwrap(); assert_eq!(offline.servers[0].status,Health::NotConnected);
        let live = view(&owned,Some(serde_json::json!({"available":true,"servers":[{"name":"old","status":"connected"},{"name":"gone","status":"failed","reason":"connection_failed"}]}))).unwrap();
        assert_eq!(live.servers.len(),2); assert!(!live.servers[1].configured); assert_eq!(live.servers[1].status,Health::Connected);
        assert!(!view(&owned,Some(serde_json::json!({"available":false,"servers":[]}))).unwrap().available);
    }
    #[test]
    fn bad_config_is_not_an_empty_list_and_private_receipt_rejects_arbitrary_errors() {
        let (_tmp,owned) = owned(); std::fs::write(owned.agent.join("mcp.json"),b"broken").unwrap();
        assert_eq!(view(&owned,None).err().unwrap().code,"mcp_config_invalid");
        let event = |data:Value| serde_json::json!({"type":"extension_ui_request","method":"notify","message":format!("{CODEC}{data}")});
        let data=serde_json::json!({"schema":1,"requestId":"local-1-2","available":true,"servers":[{"name":"a","status":"failed","reason":"connection_failed"}]});
        assert!(receipt(&event(data.clone())).is_some());
        let mut bad=data.clone();bad["servers"][0]["reason"]=serde_json::json!("https://secret.invalid/token");assert!(receipt(&event(bad)).is_none());
        let mut bad=data;bad["servers"][0]["secret"]=serde_json::json!("fake");assert!(receipt(&event(bad)).is_none());
    }
}
