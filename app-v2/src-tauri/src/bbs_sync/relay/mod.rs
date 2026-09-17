//! HTTPS content relay. The joined account's control owner explicitly starts
//! and retains the two HTTP workers. Status reads and opening a panel never
//! construct a relay owner, generate an identity or initiate network work.
mod activity;
mod announcement;
mod coalesce;
mod coordinator;
mod destination;
mod discovery;
mod envelope;
mod framing;
mod handshake;
mod http;
mod metadata;
mod proof;
mod pump;
mod reads;
mod response;
mod tls;
mod upload;
mod window;
mod wire;

pub(crate) use coordinator::Actor;
pub(crate) use http::HttpPool;
pub(crate) use tls::{PendingTls, TlsStream};
pub(crate) use wire::{SessionContext, SignedStatement};

use super::transport::{Error, Result};

pub(crate) const RELAY_VERSION: u32 = 1;
pub(crate) const PEER_VERSION: u32 = 4;
pub(crate) const MAX_BATCH: usize = 256 * 1024;
pub(super) const MAX_ENVELOPE: usize = 8 * 1024;

#[cfg(test)]
mod tests;
