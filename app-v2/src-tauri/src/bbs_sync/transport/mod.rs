//! Account-scoped, explicitly started transport. Importing/constructing policy
//! starts no runtime, worker, timer or socket. The coordinator owns membership.
#[cfg(test)]
mod availability_tests;
pub(crate) mod channel;
pub(crate) mod control_channel;
pub(crate) mod data_channel;
mod file_io;
mod network_policy;
pub(crate) mod protocol;
mod runtime_host;
mod session;
mod signaling;
#[cfg(test)]
mod tests;

pub(crate) use control_channel::Delivery;
pub(crate) use file_io::{FileIo, FileWriter, LocalIo, Resource, ResourceKind, VerifiedFile};
pub(crate) use runtime_host::{Context, NetworkHost};
pub(crate) use session::{Connection, NetworkMode, PendingConnection};
pub(crate) use signaling::{PeerIdentity, SessionRole, SignedDescription};

use std::{fmt, sync::Arc};
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};

pub(crate) const MAX_FRAME: usize = 16 * 1024;
pub(crate) const DATA_WINDOW: usize = 32;
// Smaller than the approved ceiling; leave room for frame handoffs and headers.
pub(crate) const CONTROL_WINDOW: usize = 4;
pub(crate) const APP_QUEUE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_CONNECTIONS: usize = 4;
pub(crate) const BYTES_PER_SECOND: u64 = 8 * 1024 * 1024;
pub(crate) const PROGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidSignal,
    Unauthorized,
    StaleSignature,
    InvalidResource,
    Protocol,
    ProtocolVersion,
    RelaySessionLost,
    CloudflareResourceLimit,
    Integrity,
    Io,
    Busy,
    Closed,
    Cancelled,
    Timeout,
    Runtime,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No raw SDP, keys, file paths or upstream exception text in diagnostics.
        f.write_str(match self {
            Self::InvalidSignal => "invalid_sync_signal",
            Self::Unauthorized => "sync_peer_unauthorized",
            Self::StaleSignature => "stale_signature",
            Self::InvalidResource => "invalid_sync_resource",
            Self::Protocol => "sync_protocol_error",
            Self::ProtocolVersion => "protocol_mismatch",
            Self::RelaySessionLost => "relay_session_lost",
            Self::CloudflareResourceLimit => "cloudflare_resource_limit",
            Self::Integrity => "sync_integrity_error",
            Self::Io => "sync_file_io_failed",
            Self::Busy => "sync_busy",
            Self::Closed => "sync_connection_closed",
            Self::Cancelled => "sync_cancelled",
            Self::Timeout => "sync_timeout",
            Self::Runtime => "sync_runtime_failed",
        })
    }
}
impl std::error::Error for Error {}
pub(crate) type Result<T> = std::result::Result<T, Error>;
pub(crate) type MembershipCheck = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Cancellation(watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}
impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Limits {
    bytes: Arc<Semaphore>,
    capacity: usize,
    released: Arc<Notify>,
    backing: Option<Arc<BytePermit>>,
    files: Arc<Semaphore>,
    connections: Arc<Semaphore>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Semaphore::new(APP_QUEUE_BYTES)),
            capacity: APP_QUEUE_BYTES,
            released: Arc::new(Notify::new()),
            backing: None,
            files: Arc::new(Semaphore::new(1)),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        }
    }
}
impl Limits {
    pub(crate) fn reserve(&self, size: usize) -> Result<BytePermit> {
        if size > APP_QUEUE_BYTES {
            return Err(Error::Protocol);
        }
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned(size as u32)
            .map_err(|_| Error::Busy)?;
        Ok(BytePermit {
            permit: Some(permit),
            backing: self.backing.clone(),
            released: self.released.clone(),
        })
    }
    /// A readiness hint, not admission: waiting never reserves bytes or joins
    /// the semaphore's fair acquire queue ahead of small frames. The caller
    /// still uses reserve(), and owns cancellation/deadlines around this await.
    pub(crate) async fn wait_for_bytes(&self, minimum: usize) -> Result<()> {
        if minimum == 0 || minimum > self.capacity {
            return Err(Error::Protocol);
        }
        loop {
            let changed = self.released.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.bytes.is_closed() {
                return Err(Error::Closed);
            }
            if self.bytes.available_permits() >= minimum {
                return Ok(());
            }
            changed.await;
        }
    }
    /// Carves out, never adds to, the account budget. Outstanding child buffers
    /// retain the parent charge even after the pool/partition has been dropped.
    pub(crate) fn partition(&self, size: usize) -> Result<Self> {
        if size == 0 {
            return Err(Error::Protocol);
        }
        Ok(Self {
            backing: Some(Arc::new(self.reserve(size)?)),
            bytes: Arc::new(Semaphore::new(size)),
            capacity: size,
            released: self.released.clone(),
            files: self.files.clone(),
            connections: self.connections.clone(),
        })
    }
    fn file(&self) -> Result<OwnedSemaphorePermit> {
        self.files
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)
    }
    pub(crate) fn connection(&self) -> Result<OwnedSemaphorePermit> {
        self.connections
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)
    }
}

pub(crate) struct BytePermit {
    permit: Option<OwnedSemaphorePermit>,
    backing: Option<Arc<BytePermit>>,
    released: Arc<Notify>,
}
impl Drop for BytePermit {
    fn drop(&mut self) {
        // Publish the hint only after both this allocation and any final parent
        // backing have returned credit. Shared payload views still own their
        // original permit, so their intermediate drops do not announce credit.
        let released = self.permit.as_ref().is_some_and(|p| p.num_permits() != 0);
        drop(self.permit.take());
        drop(self.backing.take());
        if released {
            self.released.notify_waiters();
        }
    }
}

// Called on the owned threads, never on Tauri/IPC or another caller's thread.
pub(crate) fn background_thread() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        if unsafe { pthread_set_qos_class_self_np(0x09, 0) } != 0 {
            return Err(Error::Runtime);
        }
    }
    Ok(())
}
