//! Pair-scoped, rebuildable catalog hints. Completion only stages a candidate;
//! a later Open/Answer must explicitly acknowledge its exact revision before
//! it can omit any item. No cache operation is content authority.
use super::{Catalog, Snapshot};
use crate::bbs_sync::{
    atomic_write,
    transport::{LocalIo, PeerIdentity, Result},
    FileStamp, StateStore,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const FULL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
// Cache limits are optimizations, never history limits. Oversize/evicted entries
// use a full manifest. One current candidate per member (Worker membership <=32).
const PAIR_BYTES: usize = 8 * 1024 * 1024;
const TOTAL_BYTES: usize = 32 * 1024 * 1024;
const ENVELOPE_BYTES: usize = 32 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Scope {
    group: String,
    local: String,
    remote: String,
    local_membership: String,
    remote_membership: String,
}
impl From<&PeerIdentity> for Scope {
    fn from(p: &PeerIdentity) -> Self {
        Self {
            group: p.group_id.clone(),
            local: p.local_device_id.clone(),
            remote: p.remote_device_id.clone(),
            local_membership: p.local_membership_id.clone(),
            remote_membership: p.remote_membership_id.clone(),
        }
    }
}
#[derive(Clone)]
struct Candidate {
    snapshot: Stored,
    order: u64,
}
#[derive(Clone)]
struct Stored {
    catalog: Arc<Catalog>,
    revision: String,
    roster_version: Option<String>,
    bytes: usize,
}
struct Peer {
    scope: Scope,
    candidate: Option<Candidate>,
    full_at: Option<Instant>,
    force: u64,
    satisfied: u64,
}
#[derive(Clone)]
pub(super) struct Start {
    scope: Scope,
    force: u64,
    pub checkpoint: Option<String>,
    base: Option<Stored>,
}
impl Start {
    pub(super) fn select(
        &self,
        snapshot: &Snapshot,
        remote_checkpoint: Option<&str>,
    ) -> (Arc<Catalog>, bool) {
        if self.checkpoint.is_some() && snapshot.cache_bytes.is_some() {
            if let Some(base) = &self.base {
                if remote_checkpoint == Some(base.revision.as_str()) {
                    return (Arc::new(snapshot.catalog.since(&base.catalog)), true);
                }
            }
        }
        (snapshot.catalog.clone(), false)
    }
}
pub(super) struct Cache {
    peers: BTreeMap<String, Peer>,
    generation: u64,
    dirty: bool,
    stamp: Option<FileStamp>,
    boot: String,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            peers: BTreeMap::new(),
            generation: 0,
            dirty: false,
            stamp: None,
            boot: uuid::Uuid::new_v4().to_string(),
        }
    }
}
impl Cache {
    pub(super) fn members(&mut self, peers: impl IntoIterator<Item = PeerIdentity>) {
        let mut previous = std::mem::take(&mut self.peers);
        let mut replaced = false;
        for p in peers.into_iter().take(31) {
            let scope = Scope::from(&p);
            let old = previous.remove(&scope.remote);
            replaced |= old.as_ref().is_some_and(|old| old.scope != scope);
            let value = old.filter(|old| old.scope == scope).unwrap_or(Peer {
                scope: scope.clone(),
                candidate: None,
                full_at: None,
                force: 0,
                satisfied: 0,
            });
            self.peers.insert(scope.remote.clone(), value);
        }
        // Even unchanged membership may refresh authority, but does not rewrite
        // the cache. Only removed/replaced candidates require a new disk image.
        if replaced || !previous.is_empty() {
            self.changed();
        }
    }
    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }
    pub(super) fn forget(&mut self, remote: &str) {
        if let Some(p) = self.peers.get_mut(remote) {
            p.candidate = None;
            p.full_at = None;
            self.changed();
        }
    }
    pub(super) fn force_full(&mut self) {
        for p in self.peers.values_mut() {
            p.force = p.force.wrapping_add(1);
        }
    }
    pub(super) fn due(&self, remote: &str, now: Instant) -> bool {
        self.peers.get(remote).is_some_and(|p| {
            p.force != p.satisfied
                || p.full_at
                    .is_none_or(|t| now.saturating_duration_since(t) >= FULL_INTERVAL)
        })
    }
    pub(super) fn start(
        &self,
        remote: &str,
        checkpoint: Option<String>,
        now: Instant,
    ) -> Option<Start> {
        let p = self.peers.get(remote)?;
        // Until the out-of-round write finishes (or if it is lost), use full.
        let base = if !self.dirty && self.stamp.is_some() && !self.due(remote, now) {
            p.candidate.as_ref().map(|c| c.snapshot.clone())
        } else {
            None
        };
        Some(Start {
            scope: p.scope.clone(),
            force: p.force,
            checkpoint: base.as_ref().and(checkpoint),
            base,
        })
    }
    pub(super) fn complete(&mut self, start: &Start, snapshot: Snapshot, full: bool, now: Instant) {
        let Some(p) = self
            .peers
            .get_mut(&start.scope.remote)
            .filter(|p| p.scope == start.scope)
        else {
            return;
        };
        if full {
            p.full_at = Some(now);
            p.satisfied = start.force;
        }
        p.candidate = snapshot.cache_bytes.map(|bytes| Candidate {
            snapshot: Stored {
                catalog: snapshot.catalog,
                revision: snapshot.revision,
                roster_version: snapshot.roster.as_ref().map(|r| r.version.clone()),
                bytes,
            },
            order: self.generation,
        });
        self.changed();
        while self
            .peers
            .values()
            .filter_map(|p| p.candidate.as_ref())
            .map(|c| c.snapshot.bytes)
            .sum::<usize>()
            > TOTAL_BYTES - ENVELOPE_BYTES
        {
            let oldest = self
                .peers
                .iter()
                .filter_map(|(id, p)| p.candidate.as_ref().map(|c| (id.clone(), c.order)))
                .min_by_key(|(_, order)| *order)
                .map(|(id, _)| id)
                .unwrap();
            self.peers.get_mut(&oldest).unwrap().candidate = None;
        }
    }
    pub(super) fn flush(&self, store: &StateStore) -> Flush {
        Flush {
            generation: self.generation,
            path: store.root.join("relay-catalogs.json"),
            boot: self.boot.clone(),
            stamp: self.stamp.clone(),
            dirty: self.dirty,
            entries: self
                .peers
                .values()
                .filter_map(|p| {
                    p.candidate
                        .as_ref()
                        .map(|c| (p.scope.clone(), c.snapshot.clone()))
                })
                .collect(),
        }
    }
    pub(super) fn flushed(&mut self, generation: u64, result: Option<FileStamp>) {
        if self.generation != generation {
            return;
        }
        self.stamp = result;
        if self.stamp.is_none() {
            for p in self.peers.values_mut() {
                p.candidate = None;
            }
        }
        self.dirty = false;
    }
}

pub(super) struct Flush {
    pub generation: u64,
    path: PathBuf,
    boot: String,
    stamp: Option<FileStamp>,
    dirty: bool,
    entries: Vec<(Scope, Stored)>,
}
impl Flush {
    /// Called only by Engine::refresh while holding the round permit. Old boot
    /// files are deliberately never trusted: startup must reconcile in full.
    pub(super) fn run(self, io: &mut LocalIo) -> Result<Option<FileStamp>> {
        io.check()?;
        let observed = || -> io::Result<FileStamp> {
            let parent =
                fs::symlink_metadata(self.path.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
            if !parent.is_dir() || parent.file_type().is_symlink() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&self.path)?;
            let m = file.metadata()?;
            if !m.is_file() || m.len() > TOTAL_BYTES as u64 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(FileStamp::of(&m))
        };
        if let Some(expected) = &self.stamp {
            if observed().ok().as_ref() != Some(expected) {
                return Ok(None);
            }
        }
        if !self.dirty {
            return Ok(self.stamp);
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Entry<'a> {
            scope: &'a Scope,
            revision: &'a str,
            roster_version: Option<&'a str>,
            catalog: &'a Catalog,
        }
        #[derive(Serialize)]
        struct Disk<'a> {
            schema: u32,
            boot: &'a str,
            peers: Vec<Entry<'a>>,
        }
        let disk = Disk {
            schema: 1,
            boot: &self.boot,
            peers: self
                .entries
                .iter()
                .map(|(scope, s)| Entry {
                    scope,
                    revision: &s.revision,
                    roster_version: s.roster_version.as_deref(),
                    catalog: &s.catalog,
                })
                .collect(),
        };
        struct Charged<'a, W> {
            io: &'a mut LocalIo,
            output: W,
            total: usize,
        }
        impl<W: Write> Write for Charged<'_, W> {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.total = self
                    .total
                    .checked_add(b.len())
                    .filter(|n| *n <= TOTAL_BYTES)
                    .ok_or(io::ErrorKind::InvalidData)?;
                self.io
                    .charge(b.len())
                    // Interrupted is automatically retried by Write helpers;
                    // cancellation must instead unwind and remove the temp.
                    .map_err(io::Error::other)?;
                self.output.write_all(b)?;
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.output.flush()
            }
        }
        let written = atomic_write(&self.path, |file| {
            let output = Charged {
                io,
                output: file,
                total: 0,
            };
            let mut output = BufWriter::with_capacity(16 * 1024, output);
            serde_json::to_writer(&mut output, &disk).map_err(io::Error::other)?;
            output.flush()
        });
        io.check()?;
        // Cache failure costs another full scan, not a failed content round.
        Ok(written.ok().and_then(|_| observed().ok()))
    }
}

pub(super) fn cache_bytes(catalog: &Catalog) -> Option<usize> {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(b.len())
                .filter(|n| *n <= PAIR_BYTES)
                .ok_or(io::ErrorKind::InvalidData)?;
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, catalog).ok()?;
    Some(count.0)
}

#[cfg(test)]
mod tests;
