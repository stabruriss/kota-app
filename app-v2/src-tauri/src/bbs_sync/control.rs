//! Synchronous, durable control client. Call on the single background control worker.
//! Construction and `status_memory` do no network I/O. Mutating methods require &mut self.
use super::{
    load, raw_sha256, safe_id, valid_hash, write, DeviceIdentity, StateStore, SCHEMA_VERSION,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fmt,
    io::Read,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const PROTOCOL_VERSION: u32 = 1;
const BBS_MIN_RELAY_VERSION: (u64, u64, u64) = (0, 1, 3);
const REQUEST_BUDGET: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 768 * 1024;
const MAX_HEALTH_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Member,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub group_id: String,
    pub worker_url: String,
    pub role: Role,
    pub membership_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub device_id: String,
    pub public_key: String,
    pub name: String,
    pub role: Role,
    pub membership_id: String,
    pub last_seen_at: u64,
    pub online: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteStatus {
    pub gen: u64,
    pub has_code: bool,
    pub expires_at: u64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteResponse {
    pub protocol_version: u32,
    pub ok: bool,
    pub error: Option<String>,
    pub group_id: Option<String>,
    pub role: Option<Role>,
    pub membership_id: Option<String>,
    pub members: Option<Vec<Member>>,
    pub member_version: Option<u64>,
    pub invite: Option<InviteStatus>,
    #[serde(default)]
    pub signals: Vec<Signal>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signal {
    pub id: String,
    pub from: String,
    pub to: String,
    pub payload: String,
    pub expires_at: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitationResult {
    pub group_id: String,
    pub generation: String,
    pub invitation: String,
}
impl fmt::Debug for InvitationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvitationResult")
            .field("group_id", &self.group_id)
            .field("generation", &self.generation)
            .field("invitation", &"<redacted>")
            .finish()
    }
}
#[derive(Clone)]
pub struct OwnerConnection {
    worker_url: String,
    desktop_secret: String,
}
impl fmt::Debug for OwnerConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerConnection")
            .field("worker_url", &self.worker_url)
            .field("desktop_secret", &"<redacted>")
            .finish()
    }
}
impl OwnerConnection {
    pub fn from_paired(config: &crate::laughing_man::LmStandbyConfig) -> Result<Self, String> {
        if config.paired_at.is_empty() || config.desktop_secret.is_empty() {
            return Err("worker_not_paired".into());
        }
        Ok(Self {
            worker_url: worker_origin(&config.worker_url)?,
            desktop_secret: config.desktop_secret.clone(),
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalInvitation {
    group_id: String,
    gen: u64,
    token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Pending {
    Create {
        group_id: String,
        worker_url: String,
        request_id: String,
        name: String,
    },
    Invitation {
        group_id: String,
        worker_url: String,
        gen: u64,
        token: String,
        request_id: String,
    },
    Join {
        worker_url: String,
        token: String,
        request_id: String,
        name: String,
        revoke_own: Option<Membership>,
    },
    Mutation {
        membership: Membership,
        action: Mutation,
        request_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Mutation {
    Disconnect,
    Rename { name: String },
    Remove { device_id: String },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlState {
    schema_version: u32,
    device_name: String,
    current: Option<Membership>,
    pending: Option<Pending>,
    invitation: Option<LocalInvitation>,
}
impl ControlState {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("unsupported_control_schema".into());
        }
        name(&self.device_name)?;
        if let Some(member) = &self.current {
            validate_membership(member)?;
        }
        if let Some(invite) = &self.invitation {
            safe_id(&invite.group_id)?;
            valid_hash(&invite.token)?;
        }
        if let Some(pending) = &self.pending {
            let (worker, request) = match pending {
                Pending::Create {
                    group_id,
                    worker_url,
                    request_id,
                    name: device_name,
                } => {
                    safe_id(group_id)?;
                    name(device_name)?;
                    (worker_url, request_id)
                }
                Pending::Invitation {
                    group_id,
                    worker_url,
                    token,
                    request_id,
                    ..
                } => {
                    safe_id(group_id)?;
                    valid_hash(token)?;
                    (worker_url, request_id)
                }
                Pending::Join {
                    worker_url,
                    token,
                    request_id,
                    name: device_name,
                    revoke_own,
                } => {
                    valid_hash(token)?;
                    name(device_name)?;
                    if let Some(member) = revoke_own {
                        validate_membership(member)?;
                    }
                    (worker_url, request_id)
                }
                Pending::Mutation {
                    membership,
                    request_id,
                    ..
                } => {
                    validate_membership(membership)?;
                    (&membership.worker_url, request_id)
                }
            };
            worker_origin(worker)?;
            safe_id(request)?;
        }
        Ok(())
    }
}
fn validate_membership(member: &Membership) -> Result<(), String> {
    safe_id(&member.group_id)?;
    safe_id(&member.membership_id)?;
    worker_origin(&member.worker_url)?;
    Ok(())
}

/// Transport failures deliberately omit URLs, payloads and response bodies (invites are secrets).
pub trait Transport {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<RemoteResponse, String>;
}
impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<RemoteResponse, String> {
        (**self).send(method, url, headers, body)
    }
}
pub struct HttpsTransport {
    agent: ureq::Agent,
}
impl Default for HttpsTransport {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(REQUEST_BUDGET)
                .redirects(0)
                .build(),
        }
    }
}
impl Transport for HttpsTransport {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<RemoteResponse, String> {
        let deadline = Instant::now() + REQUEST_BUDGET;
        let parsed = Url::parse(url).map_err(|_| "invalid_worker_url")?;
        if parsed.scheme() != "https" {
            return Err("https_required".into());
        }
        let mut request = self
            .agent
            .request(method, url)
            .set("content-type", "application/json");
        for (key, value) in headers {
            request = request.set(key, value);
        }
        let (status, bytes) = read_http_response(
            request
                .timeout(remaining_budget(deadline)?)
                .send_string(body),
            MAX_RESPONSE_BYTES,
        )?;
        decode_control_response(status, &bytes, &parsed, deadline, |health, remaining| {
            // No owner secret, device proof, invitation, body or redirect is
            // forwarded to this same-origin GET. Both requests share a deadline.
            read_http_response(
                self.agent.get(health).timeout(remaining).call(),
                MAX_HEALTH_BYTES,
            )
        })
    }
}

fn remaining_budget(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "worker_unreachable".into())
}

fn read_http_response(
    response: Result<ureq::Response, ureq::Error>,
    limit: usize,
) -> Result<(u16, Vec<u8>), String> {
    let response = match response {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err("worker_unreachable".into()),
    };
    let status = response.status();
    if (300..400).contains(&status) {
        return Err("worker_redirect_rejected".into());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "worker_response_failed")?;
    if bytes.len() > limit {
        return Err("worker_response_too_large".into());
    }
    Ok((status, bytes))
}

fn decode_control_response(
    status: u16,
    bytes: &[u8],
    request_url: &Url,
    deadline: Instant,
    health: impl FnOnce(&str, Duration) -> Result<(u16, Vec<u8>), String>,
) -> Result<RemoteResponse, String> {
    // A missing BBS route alone is insufficient: a proxy, bad endpoint or a
    // newer incompatible Worker must never be labelled an old LM release.
    let legacy = status == 404
        && serde_json::from_slice::<Value>(bytes).ok()
            == Some(json!({"ok":false,"error":"not found"}));
    if legacy {
        let health_url = format!("{}/health", request_url.origin().ascii_serialization());
        let (health_status, bytes) = health(&health_url, remaining_budget(deadline)?)?;
        remaining_budget(deadline)?;
        if health_status == 200 {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                let old_release = value["relayVersion"]
                    .as_str()
                    .and_then(relay_version)
                    .is_some_and(|version| version < BBS_MIN_RELAY_VERSION);
                if value["ok"] == true
                    && value["protocolVersion"] == crate::laughing_man::STANDBY_PROTOCOL
                    && old_release
                {
                    return Err("worker_update_required".into());
                }
            }
        }
    }
    serde_json::from_slice(bytes).map_err(|_| "invalid_worker_response".into())
}

fn relay_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let mut part = || {
        let text = parts.next()?;
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        text.parse::<u64>().ok()
    };
    let version = (part()?, part()?, part()?);
    parts.next().is_none().then_some(version)
}
pub struct ControlClient<T: Transport> {
    store: StateStore,
    identity: DeviceIdentity,
    data: ControlState,
    owner: Option<OwnerConnection>,
    transport: T,
    cached: Option<RemoteResponse>,
    _lease: super::ControlLease,
}
/// Read-only projection for publication and content-commit fences. This neither
/// creates a device nor touches Worker. The caller owns the short content lock
/// when using the value to authorize a mutation.
pub(crate) fn read_membership(store: &StateStore) -> Result<Option<Membership>, String> {
    load(&store.control_path(), |state: ControlState| {
        state.validate()?;
        Ok(state)
    })
    .map(|state| state.and_then(|s| s.current))
}

/// Startup inspection only: no lease, creation, rewrite or recovery side effect.
pub(crate) fn inspect_existing(
    store: &StateStore,
) -> Result<Option<(DeviceIdentity, String, Option<Membership>, bool)>, String> {
    let state = load(&store.control_path(), |state: ControlState| {
        state.validate()?;
        Ok(state)
    })?;
    let identity = store.load_identity()?;
    match (state, identity) {
        (None, None) => Ok(None),
        (Some(state), Some(identity)) => Ok(Some((
            identity,
            state.device_name,
            state.current,
            state.pending.is_some(),
        ))),
        _ => Err("incomplete_sync_identity".into()),
    }
}
impl<T: Transport> ControlClient<T> {
    pub fn open(
        store: StateStore,
        transport: T,
        owner: Option<OwnerConnection>,
        device_name: &str,
    ) -> Result<Self, String> {
        Self::open_mode(store, transport, owner, device_name, true)
    }
    pub(crate) fn open_existing(
        store: StateStore,
        transport: T,
        owner: Option<OwnerConnection>,
    ) -> Result<Self, String> {
        Self::open_mode(store, transport, owner, "This device", false)
    }
    fn open_mode(
        store: StateStore,
        transport: T,
        owner: Option<OwnerConnection>,
        device_name: &str,
        initialize: bool,
    ) -> Result<Self, String> {
        let lease = store.control_lease()?;
        // Check both existing files before generating anything; corrupt/unknown state is not reset.
        let control = load(&store.control_path(), |state: ControlState| {
            state.validate()?;
            Ok(state)
        })?;
        let existing_identity = store.load_identity()?;
        if !initialize && (control.is_none() || existing_identity.is_none()) {
            return Err("incomplete_sync_identity".into());
        }
        if control.is_some() && existing_identity.is_none() {
            return Err("missing_device_identity".into());
        }
        if control.is_none() && existing_identity.is_some() {
            return Err("incomplete_sync_identity".into());
        }
        let identity = match existing_identity {
            Some(identity) => identity,
            None => {
                let identity = DeviceIdentity::generate()?;
                store.save_identity(&identity)?;
                identity
            }
        };
        let data = control.unwrap_or(ControlState {
            schema_version: SCHEMA_VERSION,
            device_name: name(device_name)?,
            current: None,
            pending: None,
            invitation: None,
        });
        {
            let _content_lock = store.content_lock()?;
            write(&store.control_path(), &data)?;
        }
        Ok(Self {
            store,
            identity,
            data,
            owner,
            transport,
            cached: None,
            _lease: lease,
        })
    }
    pub fn device_id(&self) -> Result<String, String> {
        self.identity.device_id()
    }
    pub fn device_name(&self) -> &str {
        &self.data.device_name
    }
    pub(crate) fn identity(&self) -> DeviceIdentity {
        self.identity.clone()
    }
    pub(crate) fn update_owner(&mut self, owner: Option<OwnerConnection>) {
        self.owner = owner;
    }
    pub(crate) fn leaving(&self) -> bool {
        matches!(
            self.data.pending,
            Some(Pending::Mutation {
                action: Mutation::Disconnect,
                ..
            }) | Some(Pending::Join {
                revoke_own: Some(_),
                ..
            })
        )
    }
    pub(crate) fn invitation_generation(&self, now: u64) -> Option<String> {
        let member = self
            .data
            .current
            .as_ref()
            .filter(|m| m.role == Role::Owner)?;
        let local = self.data.invitation.as_ref()?;
        let remote = self.cached.as_ref()?.invite.as_ref()?;
        (local.group_id == member.group_id
            && local.gen == remote.gen
            && remote.has_code
            && remote.expires_at > now)
            .then(|| local.gen.to_string())
    }
    pub(crate) fn invitation_expiry(&self) -> Option<u64> {
        self.cached.as_ref()?.invite.as_ref().map(|v| v.expires_at)
    }
    pub(crate) fn take_signals(&mut self) -> Vec<Signal> {
        self.cached
            .as_mut()
            .map(|c| std::mem::take(&mut c.signals))
            .unwrap_or_default()
    }
    pub fn membership(&self) -> Option<&Membership> {
        self.data.current.as_ref()
    }
    pub fn has_pending(&self) -> bool {
        self.data.pending.is_some()
    }
    pub fn status_memory(&self) -> Option<&RemoteResponse> {
        self.cached.as_ref()
    }
    fn fence(&self, expected: Option<&str>) -> Result<(), String> {
        if self
            .data
            .current
            .as_ref()
            .map(|member| member.group_id.as_str())
            != expected
        {
            return Err("group_context_changed".into());
        }
        Ok(())
    }
    fn commit(&mut self, next: ControlState) -> Result<(), String> {
        next.validate()?;
        let _content_lock = self.store.content_lock()?;
        write(&self.store.control_path(), &next)?;
        self.data = next;
        Ok(())
    }
    fn begin(&mut self, pending: Pending) -> Result<(), String> {
        if self.data.pending.is_some() {
            return Err("control_busy".into());
        }
        let mut next = self.data.clone();
        next.pending = Some(pending);
        self.commit(next)
    }
    fn owner_secret(&self, worker: &str) -> Result<&str, String> {
        let owner = self.owner.as_ref().ok_or("worker_not_paired")?;
        if owner.worker_url != worker {
            return Err("owner_worker_mismatch".into());
        }
        Ok(&owner.desktop_secret)
    }
    fn request(
        &self,
        worker: &str,
        path: &str,
        group: &str,
        fields: Value,
        owner: bool,
    ) -> Result<RemoteResponse, String> {
        let worker = worker_origin(worker)?;
        let mut fields = fields
            .as_object()
            .cloned()
            .ok_or("invalid_control_request")?;
        fields.insert("protocolVersion".into(), json!(PROTOCOL_VERSION));
        let body = serde_json::to_string(&fields).map_err(|_| "invalid_control_request")?;
        let method = if path == "/bbs/owner/invitation" {
            "PUT"
        } else {
            "POST"
        };
        let headers = if owner {
            BTreeMap::from([(
                "x-kota-standby-secret".into(),
                self.owner_secret(&worker)?.into(),
            )])
        } else {
            signed_headers(
                &self.identity,
                &worker,
                method,
                path,
                group,
                &body,
                now_ms()?,
                &request_id(),
            )?
        };
        let response = self
            .transport
            .send(method, &format!("{worker}{path}"), &headers, &body)?;
        if response.protocol_version != PROTOCOL_VERSION {
            return Err("protocol_mismatch".into());
        }
        if !response.ok {
            let code = response.error.as_deref().unwrap_or("control_rejected");
            // Only the local legacy-route + health proof may issue this code;
            // an arbitrary remote error string cannot request an upgrade.
            if code == "worker_update_required" {
                return Err("control_rejected".into());
            }
            return Err(
                if !code.is_empty()
                    && code.len() <= 64
                    && code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                {
                    code.into()
                } else {
                    "control_rejected".into()
                },
            );
        }
        if !group.is_empty() && response.group_id.as_deref().is_some_and(|id| id != group) {
            return Err("invalid_worker_response".into());
        }
        Ok(response)
    }
    pub fn resume_pending(&mut self) -> Result<(), String> {
        let pending = match self.data.pending.clone() {
            Some(pending) => pending,
            None => return Ok(()),
        };
        let result = self.resume(pending);
        // Definite rejections discard the intent. Transport/protocol/persistence errors retain it.
        if let Err(error) = &result {
            if matches!(
                error.as_str(),
                "removed"
                    | "generation_conflict"
                    | "request_conflict"
                    | "invalid_invitation"
                    | "invitation_expired"
                    | "invitation_used"
                    | "group_full"
            ) {
                let mut next = self.data.clone();
                next.pending = None;
                next.invitation = None;
                if error == "removed" {
                    next.current = None;
                    self.cached = None;
                }
                self.commit(next)?;
            }
        }
        result
    }
    fn resume(&mut self, pending: Pending) -> Result<(), String> {
        let mut next = self.data.clone();
        match pending {
            Pending::Create {
                group_id,
                worker_url,
                request_id,
                name,
            } => {
                let reply = self.request(&worker_url, "/bbs/owner/create", "", json!({"expectedGroupId":null,"groupId":group_id,"requestId":request_id,"name":name,"deviceId":self.device_id()?,"publicKey":self.identity.public_key}), true)?;
                if reply.group_id.as_deref() != Some(&group_id)
                    || reply.role != Some(Role::Owner)
                    || reply.membership_id.as_deref() != Some(&request_id)
                {
                    return Err("invalid_worker_response".into());
                }
                next.current = Some(Membership {
                    group_id,
                    worker_url,
                    role: Role::Owner,
                    membership_id: request_id,
                });
            }
            Pending::Invitation {
                group_id,
                worker_url,
                gen,
                token,
                request_id,
            } => {
                self.request(&worker_url, "/bbs/owner/invitation", &group_id, json!({"expectedGroupId":group_id,"hash":raw_sha256(token.as_bytes()),"gen":gen,"requestId":request_id}), true)?;
                // A replayed registration receipt may predate a later refresh; check authority again.
                let status = self.request(
                    &worker_url,
                    "/bbs/owner/status",
                    &group_id,
                    json!({"expectedGroupId":group_id}),
                    true,
                )?;
                if !status
                    .invite
                    .as_ref()
                    .is_some_and(|invite| invite.gen == gen && invite.has_code)
                {
                    next.pending = None;
                    next.invitation = None;
                    self.commit(next)?;
                    return Err("invitation_changed".into());
                }
                next.invitation = Some(LocalInvitation {
                    group_id,
                    gen,
                    token,
                });
                self.cached = Some(status);
            }
            Pending::Join {
                worker_url,
                token,
                request_id,
                name,
                revoke_own,
            } => {
                if let Some(own) = revoke_own {
                    self.request(
                        &own.worker_url,
                        "/bbs/owner/dissolve",
                        &own.group_id,
                        json!({"expectedGroupId":own.group_id,"requestId":request_id}),
                        true,
                    )?;
                    next.current = None;
                    next.invitation = None;
                    next.pending = Some(Pending::Join {
                        worker_url: worker_url.clone(),
                        token: token.clone(),
                        request_id: request_id.clone(),
                        name: name.clone(),
                        revoke_own: None,
                    });
                    self.commit(next.clone())?;
                    self.cached = None;
                }
                let reply = self.request(
                    &worker_url,
                    "/bbs/join",
                    "",
                    json!({"token":token,"requestId":request_id,"name":name}),
                    false,
                )?;
                let group_id = reply.group_id.ok_or("invalid_worker_response")?;
                safe_id(&group_id)?;
                if reply.role != Some(Role::Member)
                    || reply.membership_id.as_deref() != Some(&request_id)
                {
                    return Err("invalid_worker_response".into());
                }
                next.current = Some(Membership {
                    group_id,
                    worker_url,
                    role: Role::Member,
                    membership_id: request_id,
                });
                next.invitation = None;
            }
            Pending::Mutation {
                membership,
                action,
                request_id,
            } => {
                let (path, fields, owner) = match &action {
                    Mutation::Disconnect if membership.role == Role::Owner => (
                        "/bbs/owner/dissolve".into(),
                        json!({"expectedGroupId":membership.group_id,"requestId":request_id}),
                        true,
                    ),
                    Mutation::Disconnect => (
                        format!("/bbs/groups/{}/leave", membership.group_id),
                        json!({"expectedGroupId":membership.group_id,"requestId":request_id}),
                        false,
                    ),
                    Mutation::Rename { name } => (
                        format!("/bbs/groups/{}/rename", membership.group_id),
                        json!({"expectedGroupId":membership.group_id,"requestId":request_id,"name":name}),
                        false,
                    ),
                    Mutation::Remove { device_id } => (
                        "/bbs/owner/remove".into(),
                        json!({"expectedGroupId":membership.group_id,"requestId":request_id,"deviceId":device_id}),
                        true,
                    ),
                };
                let result = self.request(
                    &membership.worker_url,
                    &path,
                    &membership.group_id,
                    fields,
                    owner,
                );
                if let Err(error) = &result {
                    if action != Mutation::Disconnect
                        || !(error == "removed"
                            || (membership.role == Role::Member && error == "unauthorized"))
                    {
                        return Err(error.clone());
                    }
                }
                match action {
                    Mutation::Disconnect => {
                        next.current = None;
                        next.invitation = None;
                        self.cached = None;
                    }
                    Mutation::Rename { name } => next.device_name = name,
                    Mutation::Remove { .. } => {}
                }
            }
        }
        next.pending = None;
        self.commit(next)
    }
    pub fn invitation(
        &mut self,
        expected: Option<&str>,
        refresh: bool,
    ) -> Result<InvitationResult, String> {
        self.fence(expected)?;
        if !refresh && !self.has_pending() && self.invitation_generation(now_ms()?).is_some() {
            return self.local_invitation_result();
        }
        let resumed_invitation = matches!(self.data.pending, Some(Pending::Invitation { .. }));
        if let Some(pending) = &self.data.pending {
            if !matches!(pending, Pending::Create { .. } | Pending::Invitation { .. }) {
                return Err("control_busy".into());
            }
            self.resume_pending()?;
        }
        if self.data.current.is_none() {
            let worker_url = self
                .owner
                .as_ref()
                .ok_or("worker_not_paired")?
                .worker_url
                .clone();
            self.begin(Pending::Create {
                group_id: request_id(),
                worker_url,
                request_id: request_id(),
                name: self.data.device_name.clone(),
            })?;
            self.resume_pending()?;
        }
        let membership = self.data.current.clone().ok_or("not_joined")?;
        if membership.role != Role::Owner {
            return Err("owner_required".into());
        }
        let status = self.request(
            &membership.worker_url,
            "/bbs/owner/status",
            &membership.group_id,
            json!({"expectedGroupId":membership.group_id}),
            true,
        )?;
        let remote = status.invite.as_ref().ok_or("invalid_worker_response")?;
        let valid = self.data.invitation.as_ref().is_some_and(|local| {
            local.group_id == membership.group_id
                && local.gen == remote.gen
                && remote.has_code
                && remote.expires_at > now_ms().unwrap_or(u64::MAX)
        });
        if (refresh && !resumed_invitation) || !valid {
            let mut next = self.data.clone();
            next.invitation = None;
            next.pending = Some(Pending::Invitation {
                group_id: membership.group_id.clone(),
                worker_url: membership.worker_url.clone(),
                gen: remote.gen.checked_add(1).ok_or("generation_overflow")?,
                token: random_token()?,
                request_id: request_id(),
            });
            self.commit(next)?;
            self.resume_pending()?;
        } else {
            self.cached = Some(status);
        }
        self.local_invitation_result()
    }
    fn local_invitation_result(&self) -> Result<InvitationResult, String> {
        let membership = self.data.current.as_ref().ok_or("not_joined")?;
        let local = self
            .data
            .invitation
            .as_ref()
            .ok_or("invitation_preparing")?;
        Ok(InvitationResult {
            group_id: membership.group_id.clone(),
            generation: local.gen.to_string(),
            invitation: invitation_string(&membership.worker_url, &local.token)?,
        })
    }
    pub fn join(&mut self, expected: Option<&str>, invitation: &str) -> Result<(), String> {
        self.fence(expected)?;
        if self.data.pending.is_some() {
            return Err("control_busy".into());
        }
        let (worker_url, token) = parse_invitation(invitation)?;
        let revoke_own = if let Some(current) = &self.data.current {
            if current.role != Role::Owner {
                return Err("already_joined".into());
            }
            let status = self.request(
                &current.worker_url,
                "/bbs/owner/status",
                &current.group_id,
                json!({"expectedGroupId":current.group_id}),
                true,
            )?;
            if status.members.as_ref().map(Vec::len) != Some(1) {
                return Err("owner_group_not_empty".into());
            }
            if current.worker_url == worker_url {
                return Err("already_joined".into());
            }
            Some(current.clone())
        } else {
            None
        };
        self.begin(Pending::Join {
            worker_url,
            token,
            request_id: request_id(),
            name: self.data.device_name.clone(),
            revoke_own,
        })?;
        self.resume_pending()
    }
    pub fn disconnect(&mut self, expected: Option<&str>) -> Result<(), String> {
        self.mutate(expected, Mutation::Disconnect)
    }
    pub fn rename(&mut self, expected: Option<&str>, next_name: &str) -> Result<(), String> {
        self.fence(expected)?;
        let name = name(next_name)?;
        if self.data.current.is_none() {
            if self.has_pending() {
                return Err("control_busy".into());
            }
            let mut next = self.data.clone();
            next.device_name = name;
            return self.commit(next);
        }
        self.mutate(expected, Mutation::Rename { name })
    }
    pub fn remove(&mut self, expected: Option<&str>, device_id: &str) -> Result<(), String> {
        valid_hash(device_id)?;
        self.mutate(
            expected,
            Mutation::Remove {
                device_id: device_id.into(),
            },
        )?;
        if let Some(members) = self.cached.as_mut().and_then(|c| c.members.as_mut()) {
            members.retain(|m| m.device_id != device_id);
        }
        Ok(())
    }
    fn mutate(&mut self, expected: Option<&str>, action: Mutation) -> Result<(), String> {
        self.fence(expected)?;
        let membership = self.data.current.clone().ok_or("not_joined")?;
        if matches!(action, Mutation::Remove { .. }) && membership.role != Role::Owner {
            return Err("owner_required".into());
        }
        self.begin(Pending::Mutation {
            membership,
            action,
            request_id: request_id(),
        })?;
        self.resume_pending()
    }
    /// Explicit background heartbeat. The public UI status reader must never call this.
    pub fn refresh_members(&mut self, expected: Option<&str>) -> Result<(), String> {
        self.fence(expected)?;
        let member = self.data.current.clone().ok_or("not_joined")?;
        let status = self.request(
            &member.worker_url,
            &format!("/bbs/groups/{}/heartbeat", member.group_id),
            &member.group_id,
            json!({"expectedGroupId":member.group_id}),
            false,
        );
        match status {
            Ok(status) => {
                self.cached = Some(status);
            }
            Err(error) if matches!(error.as_str(), "removed" | "unauthorized") => {
                let mut next = self.data.clone();
                next.current = None;
                next.pending = None;
                next.invitation = None;
                self.commit(next)?;
                self.cached = None;
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        if member.role == Role::Owner && !self.has_pending() {
            // A normal heartbeat contains everything needed to retain the
            // locally held code. Only consumption/expiry needs a new mutation.
            if self.invitation_generation(now_ms()?).is_none() {
                let remote = self
                    .cached
                    .as_ref()
                    .and_then(|c| c.invite.as_ref())
                    .cloned()
                    .ok_or("invalid_worker_response")?;
                let mut next = self.data.clone();
                next.invitation = None;
                next.pending = Some(Pending::Invitation {
                    group_id: member.group_id.clone(),
                    worker_url: member.worker_url.clone(),
                    gen: remote.gen.checked_add(1).ok_or("generation_overflow")?,
                    token: random_token()?,
                    request_id: request_id(),
                });
                let signals = self.take_signals();
                self.commit(next)?;
                let result = self.resume_pending();
                if let Some(cached) = self.cached.as_mut() {
                    cached.signals = signals;
                }
                result?;
            }
        }
        Ok(())
    }
    pub fn send_signal(&self, expected: &str, to: &str, payload: &str) -> Result<(), String> {
        self.fence(Some(expected))?;
        valid_hash(to)?;
        if payload.is_empty() || payload.len() > 64 * 1024 {
            return Err("invalid_signal".into());
        }
        let member = self.data.current.as_ref().ok_or("not_joined")?;
        self.request(
            &member.worker_url,
            &format!("/bbs/groups/{expected}/signal"),
            expected,
            json!({"expectedGroupId":expected,"requestId":request_id(),"to":to,"payload":payload}),
            false,
        )?;
        Ok(())
    }
    pub fn pull_signals(&self, expected: &str) -> Result<Vec<Signal>, String> {
        self.fence(Some(expected))?;
        let member = self.data.current.as_ref().ok_or("not_joined")?;
        Ok(self
            .request(
                &member.worker_url,
                &format!("/bbs/groups/{expected}/signal/pull"),
                expected,
                json!({"expectedGroupId":expected}),
                false,
            )?
            .signals)
    }
}
fn name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() || name.encode_utf16().count() > 80 || name.chars().any(char::is_control) {
        return Err("invalid_device_name".into());
    }
    Ok(name.into())
}
fn now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .map_err(|_| "invalid_system_clock".into())
}
fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn random_token() -> Result<String, String> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "random_generation_failed")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub fn worker_origin(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "invalid_worker_url")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_worker_url".into());
    }
    Ok(url.origin().ascii_serialization())
}
pub fn parse_invitation(value: &str) -> Result<(String, String), String> {
    if value.len() > 1024 {
        return Err("invalid_invitation".into());
    }
    let url = Url::parse(value.trim()).map_err(|_| "invalid_invitation")?;
    if url.scheme() != "kota-bbs"
        || url.path() != "/join"
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("invalid_invitation".into());
    }
    let host = url.host_str().ok_or("invalid_invitation")?;
    let token = url.fragment().ok_or("invalid_invitation")?;
    valid_hash(token).map_err(|_| "invalid_invitation")?;
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.into(),
    };
    let worker = worker_origin(&format!("https://{authority}"))?;
    Ok((worker, token.into()))
}
fn invitation_string(worker: &str, token: &str) -> Result<String, String> {
    let worker = worker_origin(worker)?;
    valid_hash(token)?;
    Ok(format!(
        "kota-bbs://{}/join#{token}",
        worker.trim_start_matches("https://")
    ))
}
pub fn signed_headers(
    identity: &DeviceIdentity,
    origin: &str,
    method: &str,
    path: &str,
    group: &str,
    body: &str,
    timestamp: u64,
    nonce: &str,
) -> Result<BTreeMap<String, String>, String> {
    let origin = worker_origin(origin)?;
    safe_id(nonce)?;
    let device_id = identity.device_id()?;
    if path.contains('\n') || group.contains('\n') {
        return Err("invalid_signing_context".into());
    }
    let message = [
        "kota-bbs-control.v1",
        &origin,
        method,
        path,
        group,
        &device_id,
        &timestamp.to_string(),
        nonce,
        &raw_sha256(body.as_bytes()),
    ]
    .join("\n");
    Ok(BTreeMap::from([
        ("x-kota-bbs-version".into(), "1".into()),
        ("x-kota-bbs-device".into(), device_id),
        ("x-kota-bbs-public-key".into(), identity.public_key.clone()),
        ("x-kota-bbs-time".into(), timestamp.to_string()),
        ("x-kota-bbs-nonce".into(), nonce.into()),
        (
            "x-kota-bbs-signature".into(),
            identity.sign(message.as_bytes())?,
        ),
    ]))
}

#[cfg(test)]
mod tests;
