//! Rebuildable announcement outbox, separate from identity/control/content.
//! Only the control owner's coordinator uses it. Construction is memory-only;
//! all bounded reads and durable writes run on the existing account FileIo.
use super::{
    proof::{compact_json, Fields, Route, SignedRequest, Target},
    wire::{canonical_origin, hash, token, MAX_SAFE_INTEGER},
    Error, Result, PEER_VERSION,
};
use crate::bbs_sync::{
    atomic_write,
    control::Membership,
    transport::{Cancellation, FileIo, LocalIo, MembershipCheck},
    DeviceIdentity, StateStore,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const CACHE_BYTES: usize = 8 * 1024;
const BODY_BYTES: usize = 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Scope {
    origin: String,
    group: String,
    device: String,
    membership: String,
}
impl Scope {
    pub(super) fn new(membership: &Membership, identity: &DeviceIdentity) -> Result<Self> {
        let scope = Self {
            origin: membership.worker_url.clone(),
            group: membership.group_id.clone(),
            device: identity.device_id().map_err(|_| Error::Unauthorized)?,
            membership: membership.membership_id.clone(),
        };
        scope.validate()?;
        Ok(scope)
    }
    fn validate(&self) -> Result<()> {
        canonical_origin(&self.origin)?;
        token(&self.group)?;
        hash(&self.device)?;
        token(&self.membership)
    }
}

/// The immutable mutation body, not its short-lived HTTP proof. A new nonce
/// and proof time can sign this exact body after an outage or process restart.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Body {
    request_id: String,
    #[serde(deserialize_with = "required_nullable")]
    previous: Option<String>,
    created_at: u64,
    instance: String,
    revision: String,
    peer_version: u32,
}
impl Body {
    fn validate(&self) -> Result<()> {
        token(&self.request_id)?;
        if let Some(previous) = &self.previous {
            token(previous)?;
            if previous == &self.request_id {
                return Err(Error::Protocol);
            }
        }
        token(&self.instance)?;
        hash(&self.revision)?;
        if self.created_at > MAX_SAFE_INTEGER || self.peer_version != PEER_VERSION {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    fn new(revision: String, instance: String, previous: Option<String>, now: u64) -> Result<Self> {
        let value = Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            previous,
            created_at: now,
            instance,
            revision,
            peer_version: PEER_VERSION,
        };
        value.validate()?;
        Ok(value)
    }
    fn matches(&self, revision: &str, instance: &str) -> bool {
        self.revision == revision && self.instance == instance
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Intent {
    scope: Scope,
    body: Body,
}
impl Intent {
    pub(super) fn in_scope(&self, scope: &Scope) -> bool {
        &self.scope == scope
    }
    pub(super) fn revision(&self) -> &str {
        &self.body.revision
    }
    pub(super) fn instance(&self) -> &str {
        &self.body.instance
    }
    pub(super) fn request_id(&self) -> &str {
        &self.body.request_id
    }

    pub(super) fn signed(
        &self,
        identity: &DeviceIdentity,
        now: u64,
    ) -> Result<(SignedRequest, Vec<u8>)> {
        if identity.device_id().map_err(|_| Error::Unauthorized)? != self.scope.device {
            return Err(Error::Unauthorized);
        }
        let bytes = serde_json::to_vec(&self.body).map_err(|_| Error::Protocol)?;
        if bytes.len() > BODY_BYTES {
            return Err(Error::Protocol);
        }
        let request = SignedRequest::sign(
            identity,
            &self.scope.membership,
            Target::post(&self.scope.origin, &self.scope.group, Route::Announce)?,
            Fields::Mutation {
                nonce: &uuid::Uuid::new_v4().to_string(),
            },
            &bytes,
            now,
        )?;
        Ok((request, bytes))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    ok: bool,
    #[serde(deserialize_with = "required_nullable")]
    current: Option<String>,
    changed: bool,
}
fn required_nullable<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
impl Receipt {
    /// Only call for an HTTP success body; platform/error classification stays
    /// with the common response decoder, never this durable outbox.
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        compact_json(bytes, BODY_BYTES)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        if let Some(current) = &value.current {
            token(current)?;
        }
        if (value.ok && value.current.is_none()) || (!value.ok && value.changed) {
            return Err(Error::Protocol);
        }
        Ok(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cache {
    schema: u32,
    scope: Scope,
    #[serde(deserialize_with = "required_nullable")]
    announced: Option<Body>,
    #[serde(deserialize_with = "required_nullable")]
    pending: Option<Body>,
}
impl Cache {
    fn empty(scope: Scope) -> Self {
        Self {
            schema: 1,
            scope,
            announced: None,
            pending: None,
        }
    }
    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::Protocol);
        }
        self.scope.validate()?;
        for body in self.announced.iter().chain(self.pending.iter()) {
            body.validate()?;
        }
        Ok(())
    }
}

pub(super) enum Completion {
    Ignored,
    Confirmed,
    /// A CAS miss keeps the desired revision, with a new ID/body. The caller
    /// schedules it normally; this module never spins or starts a request.
    Rebased(Intent),
}

#[derive(Clone)]
pub(super) struct Outbox {
    path: PathBuf,
    scope: Scope,
    files: FileIo,
    current: MembershipCheck,
}
impl Outbox {
    /// `current` is the coordinator's origin/group/local-member fence. Calls
    /// belong outside accepted exchange rounds, never on a public status read.
    pub(super) fn new(
        store: &StateStore,
        scope: Scope,
        files: FileIo,
        current: MembershipCheck,
    ) -> Self {
        Self {
            path: store.root.join("relay-announcement.json"),
            scope,
            files,
            current,
        }
    }

    pub(super) async fn pending(&self, cancel: &Cancellation) -> Result<Option<Intent>> {
        let this = self.clone();
        self.files
            .run_when_available(cancel, move |io| {
                this.check(io)?;
                let cache = read(&this.path, io)?;
                this.check(io)?;
                Ok(cache
                    .filter(|c| c.scope == this.scope)
                    .and_then(|c| c.pending)
                    .map(|body| Intent {
                        scope: this.scope,
                        body,
                    }))
            })
            .await?
    }

    /// Returns a sendable intent only after its private cache is fsynced.
    /// A pending target takes precedence over an equal old confirmed target:
    /// the unconfirmed request may already have reached Worker.
    pub(super) async fn prepare(
        &self,
        revision: &str,
        instance: &str,
        now: u64,
        cancel: &Cancellation,
    ) -> Result<Option<Intent>> {
        hash(revision)?;
        token(instance)?;
        if now > MAX_SAFE_INTEGER {
            return Err(Error::Protocol);
        }
        let this = self.clone();
        let revision = revision.to_owned();
        let instance = instance.to_owned();
        self.files
            .run_when_available(cancel, move |io| {
                this.check(io)?;
                let mut cache = read(&this.path, io)?
                    .filter(|c| c.scope == this.scope)
                    .unwrap_or_else(|| Cache::empty(this.scope.clone()));
                if let Some(body) = &cache.pending {
                    // A new process instance is a new publication target too.
                    // Replace atomically instead of advertising a dead instance
                    // first; CAS recovers any previously accepted pending ID.
                    if body.matches(&revision, &instance) {
                        this.check(io)?;
                        return Ok(Some(Intent {
                            scope: this.scope,
                            body: body.clone(),
                        }));
                    }
                } else if cache
                    .announced
                    .as_ref()
                    .is_some_and(|b| b.matches(&revision, &instance))
                {
                    this.check(io)?;
                    return Ok(None);
                }
                let previous = cache
                    .pending
                    .as_ref()
                    .map(|b| b.previous.clone())
                    .unwrap_or_else(|| cache.announced.as_ref().map(|b| b.request_id.clone()));
                let body = Body::new(revision, instance, previous, now)?;
                cache.pending = Some(body.clone());
                this.persist(io, &cache)?;
                Ok(Some(Intent {
                    scope: this.scope,
                    body,
                }))
            })
            .await?
    }

    pub(super) async fn complete(
        &self,
        sent: &Intent,
        receipt: Receipt,
        now: u64,
        cancel: &Cancellation,
    ) -> Result<Completion> {
        if sent.scope != self.scope {
            return Ok(Completion::Ignored);
        }
        let this = self.clone();
        let sent = sent.clone();
        self.files
            .run_when_available(cancel, move |io| {
                this.check(io)?;
                let Some(mut cache) = read(&this.path, io)? else {
                    return Ok(Completion::Ignored);
                };
                if cache.scope != this.scope || cache.pending.as_ref() != Some(&sent.body) {
                    return Ok(Completion::Ignored);
                }
                let result = if receipt.ok {
                    if receipt.current.as_deref() != Some(sent.request_id()) {
                        return Err(Error::Protocol);
                    }
                    cache.announced = cache.pending.take();
                    Completion::Confirmed
                } else {
                    if receipt.current == sent.body.previous
                        || receipt.current.as_deref() == Some(sent.request_id())
                    {
                        return Err(Error::Protocol);
                    }
                    let body =
                        Body::new(sent.body.revision, sent.body.instance, receipt.current, now)?;
                    cache.pending = Some(body.clone());
                    Completion::Rebased(Intent {
                        scope: this.scope.clone(),
                        body,
                    })
                };
                this.persist(io, &cache)?;
                Ok(result)
            })
            .await?
    }

    fn check(&self, io: &LocalIo) -> Result<()> {
        io.check()?;
        if !(self.current)() {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn persist(&self, io: &mut LocalIo, cache: &Cache) -> Result<()> {
        cache.validate()?;
        let bytes = serde_json::to_vec(cache).map_err(|_| Error::Protocol)?;
        if bytes.len() > CACHE_BYTES {
            return Err(Error::Protocol);
        }
        io.charge(bytes.len())?;
        self.check(io)?;
        atomic_write(&self.path, |file| file.write_all(&bytes)).map_err(|_| Error::Io)
    }
}

fn read(path: &Path, io: &mut LocalIo) -> Result<Option<Cache>> {
    if let Some(parent) = path.parent() {
        match fs::symlink_metadata(parent) {
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err(Error::Io),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Io),
            Ok(_) => {}
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Io),
    };
    let meta = file.metadata().map_err(|_| Error::Io)?;
    if !meta.is_file() || meta.len() > CACHE_BYTES as u64 {
        return Err(Error::Protocol);
    }
    io.charge(meta.len() as usize)?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(CACHE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.len() > CACHE_BYTES {
        return Err(Error::Protocol);
    }
    let cache: Cache = serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)?;
    cache.validate()?;
    Ok(Some(cache))
}

#[cfg(test)]
mod tests;
