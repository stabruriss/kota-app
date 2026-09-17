//! One finite upload assembled from charged fixed blocks. No body-sized copy,
//! per-frame HTTP operation, encryption retry or private uncharged scratch.
use super::{
    window::{Buffer, Payload},
    Error, Result, TlsStream, MAX_BATCH,
};
use crate::bbs_sync::transport::{Limits, MAX_FRAME};
use std::{
    io::{self, Read},
    sync::{Arc, OnceLock},
};

const BLOCKS: usize = MAX_BATCH / MAX_FRAME;

#[derive(Clone)]
pub(super) struct Upload {
    parts: Arc<Parts>,
}
struct Parts {
    chunks: Vec<Payload>,
    len: usize,
    digest: OnceLock<String>,
}
impl Upload {
    pub(super) fn single(payload: Payload) -> Result<Self> {
        let len = payload.bytes().len();
        if len == 0 || len > MAX_BATCH {
            return Err(Error::Protocol);
        }
        Ok(Self::from_parts(vec![payload], len))
    }
    fn from_parts(chunks: Vec<Payload>, len: usize) -> Self {
        Self {
            parts: Arc::new(Parts {
                chunks,
                len,
                digest: OnceLock::new(),
            }),
        }
    }
    pub(super) fn len(&self) -> usize {
        self.parts.len
    }
    /// Computed on first signing, then shared by every immutable retry owner.
    pub(super) fn digest(&self) -> &str {
        self.parts
            .digest
            .get_or_init(|| digest(self.parts.chunks.iter().map(Payload::bytes)))
    }
    #[cfg(test)]
    pub(super) fn shares(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.parts, &other.parts)
    }
}

pub(super) struct Builder {
    limits: Limits,
    parts: Vec<Payload>,
    current: Option<Buffer>,
    len: usize,
}
impl Builder {
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            limits,
            parts: Vec::with_capacity(BLOCKS),
            current: None,
            len: 0,
        }
    }
    pub(super) fn len(&self) -> usize {
        self.len
    }
    pub(super) fn full(&self) -> bool {
        self.len == MAX_BATCH
    }
    /// Retains partial assembly on Busy. Empty TLS output allocates nothing.
    /// The caller chooses the bounded flush deadline; this owns no timer.
    pub(super) fn fill(&mut self, tls: &mut TlsStream) -> Result<usize> {
        let before = self.len;
        while tls.wants_write()? && !self.full() {
            if self.current.is_none() {
                self.current = Some(match Buffer::new(&self.limits, MAX_FRAME) {
                    Err(Error::Busy) if self.len > before => break,
                    other => other?,
                });
            }
            let current = self.current.as_mut().ok_or(Error::Protocol)?;
            let n = tls.drain_tls(current.spare_mut())?;
            if n == 0 {
                break;
            }
            current.advance(n)?;
            self.len += n;
            if current.len() == MAX_FRAME {
                self.parts
                    .push(self.current.take().ok_or(Error::Protocol)?.freeze()?);
            }
        }
        Ok(self.len - before)
    }
    pub(super) fn finish(mut self) -> Result<Upload> {
        if self.len == 0 {
            return Err(Error::Protocol);
        }
        if let Some(current) = self.current.take() {
            if current.len() > 0 {
                self.parts.push(current.freeze()?);
            }
        }
        if self.parts.len() > BLOCKS || self.len > MAX_BATCH {
            return Err(Error::Protocol);
        }
        Ok(Upload::from_parts(self.parts, self.len))
    }
}

/// Metadata remains a single existing bounded payload. Only opaque send bytes
/// may be assembled; the proof encoder does not accept multipart JSON aliases.
#[derive(Clone)]
pub(super) enum Body {
    Single(Payload),
    Ciphertext(Upload),
}
impl From<Payload> for Body {
    fn from(p: Payload) -> Self {
        Self::Single(p)
    }
}
impl From<Upload> for Body {
    fn from(p: Upload) -> Self {
        Self::Ciphertext(p)
    }
}
impl Body {
    fn parts(&self) -> &[Payload] {
        match self {
            Self::Single(p) => std::slice::from_ref(p),
            Self::Ciphertext(p) => &p.parts.chunks,
        }
    }
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Single(p) => p.bytes().len(),
            Self::Ciphertext(p) => p.len(),
        }
    }
    pub(super) fn digest(&self) -> String {
        match self {
            Self::Single(p) => digest(std::iter::once(p.bytes())),
            Self::Ciphertext(p) => p.digest().into(),
        }
    }
    pub(super) fn reader(&self) -> Reader<'_> {
        Reader {
            parts: self.parts(),
            part: 0,
            offset: 0,
        }
    }
}

pub(super) struct Reader<'a> {
    parts: &'a [Payload],
    part: usize,
    offset: usize,
}
impl Read for Reader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < output.len() && self.part < self.parts.len() {
            let bytes = self.parts[self.part].bytes();
            let count = (bytes.len() - self.offset).min(output.len() - written);
            output[written..written + count]
                .copy_from_slice(&bytes[self.offset..self.offset + count]);
            written += count;
            self.offset += count;
            if self.offset == bytes.len() {
                self.offset = 0;
                self.part += 1;
            }
        }
        Ok(written)
    }
}

fn digest<'a>(parts: impl Iterator<Item = &'a [u8]>) -> String {
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    for bytes in parts {
        hash.update(bytes);
    }
    hash.finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
pub(super) mod tests;
