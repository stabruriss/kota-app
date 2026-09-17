//! KBR1 finite receive envelope. Metadata has no delivery authority: ciphertext
//! still enters TLS in order, and a forwarded ACK still needs peer verification.
use super::{
    proof::{compact_json, decimal, ReceiveMode, ReceiveRequest, SignedAck},
    window::{Batch, Payload},
    wire::{hash, token},
    Error, Result, MAX_BATCH, MAX_ENVELOPE,
};
use crate::bbs_sync::transport::BytePermit;
use serde::{de::SeqAccess, Deserialize, Deserializer};
use std::{fmt, marker::PhantomData};

pub(super) const CONTENT_TYPE: &str = "application/octet-stream";
const PREFIX: usize = 8;
const MAGIC: &[u8; 4] = b"KBR1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    boot: String,
    #[serde(deserialize_with = "bounded::<_, _, 4>")]
    items: Vec<WireItem>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireItem {
    session: String,
    next: String,
    consumed: String,
    closed: bool,
    #[serde(deserialize_with = "bounded::<_, _, 2>")]
    batches: Vec<WireBatch>,
    // A missing nullable field is not the canonical explicit-null wire.
    #[serde(deserialize_with = "Option::deserialize")]
    ack: Option<SignedAck>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBatch {
    sequence: String,
    length: usize,
}
fn bounded<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "at most {N} items")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut items = Vec::with_capacity(N);
            while let Some(item) = seq.next_element()? {
                if items.len() == N {
                    return Err(serde::de::Error::custom("too many relay items"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Visitor::<T, N>(PhantomData))
}

pub(super) struct Metadata {
    start: usize,
    items: Vec<WireItem>,
}
impl Metadata {
    // Called in the existing fixed HTTP worker, before returning a successful
    // response. Any invalid envelope causes that worker to discard its Agent.
    pub(super) fn parse(expected: &ReceiveRequest, body: &Payload) -> Result<Self> {
        let bytes = body.bytes();
        let cap = if expected.mode == ReceiveMode::Data {
            MAX_BATCH + MAX_ENVELOPE
        } else {
            MAX_ENVELOPE
        };
        if bytes.len() < PREFIX || bytes.len() > cap || &bytes[..4] != MAGIC {
            return Err(Error::Protocol);
        }
        let json_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if json_len == 0 || json_len > MAX_ENVELOPE - PREFIX || PREFIX + json_len > bytes.len() {
            return Err(Error::Protocol);
        }
        let start = PREFIX + json_len;
        compact_json(&bytes[PREFIX..start], MAX_ENVELOPE - PREFIX)?;
        let header: Header =
            serde_json::from_slice(&bytes[PREFIX..start]).map_err(|_| Error::Protocol)?;
        token(&header.boot)?;
        if header.boot != expected.boot
            || header.items.len() != expected.cursors.len()
            || header.items.is_empty()
        {
            return Err(Error::Protocol);
        }
        let mut payload_len = 0usize;
        for (item, (session, cursor)) in header.items.iter().zip(&expected.cursors) {
            hash(&item.session)?;
            let next = decimal(&item.next)?;
            let consumed = decimal(&item.consumed)?;
            if &item.session != session
                || consumed > *cursor
                || *cursor > next
                || (expected.mode != ReceiveMode::Data && !item.batches.is_empty())
                || (expected.mode == ReceiveMode::Probe && item.ack.is_some())
            {
                return Err(Error::Protocol);
            }
            if let Some(ack) = &item.ack {
                ack.envelope_shape(&header.boot, session)?;
            }
            for (i, batch) in item.batches.iter().enumerate() {
                let seq = decimal(&batch.sequence)?;
                if seq != cursor + i as u64
                    || seq >= next
                    || batch.length == 0
                    || batch.length > MAX_BATCH
                {
                    return Err(Error::Protocol);
                }
                payload_len = payload_len
                    .checked_add(batch.length)
                    .ok_or(Error::Protocol)?;
                if payload_len > MAX_BATCH {
                    return Err(Error::Protocol);
                }
            }
        }
        if start + payload_len != bytes.len() {
            return Err(Error::Protocol);
        }
        Ok(Self {
            start,
            items: header.items,
        })
    }
    pub(super) fn into_packet(self, body: Payload, charge: BytePermit) -> Result<Packet> {
        let mut at = self.start;
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            let mut batches = Vec::with_capacity(item.batches.len());
            for batch in item.batches {
                let end = at.checked_add(batch.length).ok_or(Error::Protocol)?;
                batches.push(Batch {
                    sequence: decimal(&batch.sequence)?,
                    payload: body.slice(at..end)?,
                });
                at = end;
            }
            items.push(Item {
                session: item.session,
                next: decimal(&item.next)?,
                consumed: decimal(&item.consumed)?,
                closed: item.closed,
                batches,
                ack: item.ack,
            });
        }
        Ok(Packet {
            items,
            _metadata: charge,
        })
    }
}

pub(super) struct Packet {
    items: Vec<Item>,
    // Reuses the original 16 KiB HTTP header reservation after headers drop.
    // Counts/strings are bounded; no additional general permit is required to
    // decode a small receipt while the general payload budget is exhausted.
    _metadata: BytePermit,
}
impl Packet {
    pub(super) fn items(&self) -> &[Item] {
        &self.items
    }
}
pub(super) struct Item {
    session: String,
    next: u64,
    consumed: u64,
    closed: bool,
    batches: Vec<Batch>,
    ack: Option<SignedAck>,
}
impl Item {
    pub(super) fn session(&self) -> &str {
        &self.session
    }
    /// Server state is descriptive only: never advance local consumption from it.
    pub(super) fn window(&self) -> (u64, u64, bool) {
        (self.next, self.consumed, self.closed)
    }
    pub(super) fn batches(&self) -> &[Batch] {
        &self.batches
    }
    pub(super) fn unverified_ack(&self) -> Option<&SignedAck> {
        self.ack.as_ref()
    }
}

#[cfg(test)]
pub(super) mod tests;
