//! The App is the only BBS-to-Bus bridge. Publishers (including the CLI) only
//! persist intent before publishing bytes; no UI, network epoch or control
//! lease owns these receipts.
use super::*;
use crate::bbs_sync::{self, reconcile, transport::LocalIo, FileStamp, SyncState};
use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};

mod delivery;
mod runtime;
mod store;
pub(super) use runtime::attachments_saved;
pub(crate) use runtime::Service;
pub(super) use store::{prepare, published};

const SCHEMA: u32 = 1;
const BODY_BUDGET: u64 = 16 * 1024;
const ATTACHMENT_WAIT_MS: i64 = 10_000;
const ORPHAN_WAIT_MS: i64 = 600_000;
const ATTACHMENT_WARNING: &str =
    "Some attachments in this thread have not synced yet. Check first.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Destination {
    thread_id: String,
    post_id: String,
    target: mentions::Mention,
}
impl Destination {
    fn key(&self) -> String {
        // One canonical five-tuple, never a path assembled from long IDs.
        bbs_sync::raw_sha256(
            &serde_json::to_vec(&(
                &self.thread_id,
                &self.post_id,
                &self.target.device_id,
                &self.target.project_id,
                &self.target.agent_id,
            ))
            .expect("serialize notification identity"),
        )
    }
    fn validate(&self) -> Result<()> {
        if !safe_component(&self.thread_id) || !safe_component(&self.post_id) {
            bail!("invalid_notification_identity");
        }
        self.target.validate()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Author {
    project_id: String,
    project_name: String,
    agent_id: String,
    agent_name: String,
    // Forwarded BBS bytes do not attest the original author's device. Never
    // mislabel the authenticated forwarding peer as the author or as a human.
    local: bool,
    local_device_id: Option<String>,
    received_via: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WaitingAttachment {
    post_id: String,
    version_id: String,
    attachment_id: String,
    sha256: String,
    size_bytes: u64,
}
impl WaitingAttachment {
    fn verified(&self, state: &SyncState) -> bool {
        bbs_sync::LedgerEntry::version_key(&self.post_id, &self.version_id)
            .ok()
            .and_then(|key| state.ledger.get(&key))
            .and_then(|row| row.attachments.get(&self.attachment_id))
            .is_some_and(|a| {
                a.verified && a.sha256 == self.sha256 && a.size_bytes == self.size_bytes
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    destination: Destination,
    version_id: String,
    author: Author,
    body_offset: u64,
    file_size: u64,
    created_at_ms: i64,
    // Captured before commit, never extended by a duplicate or by new posts.
    deadline_ms: i64,
    waiting: Vec<WaitingAttachment>,
    ready: bool,
    proof: Option<FileStamp>,
    owner_run: Option<String>,
    result: Option<String>,
}
impl Record {
    fn validate(&self, key: &str) -> Result<()> {
        self.destination.validate()?;
        bbs_sync::valid_hash(&self.version_id).map_err(anyhow::Error::msg)?;
        if self.schema_version != SCHEMA
            || self.destination.key() != key
            || self.body_offset > self.file_size
            || self.created_at_ms < 0
            || self.deadline_ms < self.created_at_ms
            || (self.ready && self.proof.is_none())
        {
            bail!("invalid_notification_record");
        }
        for a in &self.waiting {
            if !safe_component(&a.post_id) || !safe_component(&a.attachment_id) {
                bail!("invalid_notification_attachment");
            }
            bbs_sync::valid_hash(&a.version_id).map_err(anyhow::Error::msg)?;
            bbs_sync::valid_hash(&a.sha256).map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
    fn missing(&self, state: &SyncState) -> bool {
        self.waiting.iter().any(|a| !a.verified(state))
    }
    fn event_id(&self) -> String {
        format!("bbs-mention:{}", self.destination.key())
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
fn diagnostic(code: &str) {
    crate::kota_debug_log(&format!("[bbs-notify] {code}"));
    #[cfg(test)]
    eprintln!("[bbs-notify] {code}");
}

#[cfg(test)]
mod tests;
