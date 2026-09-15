//! A small credit protocol over reliable ordered channels, not a second retry
//! protocol. Any impossible offset/ACK aborts; there is no partial replay.
use super::{file_io::CHUNK_BYTES, Error, Limits, Resource, Result, DATA_WINDOW, MAX_FRAME};
use bytes::BytesMut;
use std::collections::VecDeque;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferId(pub(crate) [u8; 16]);
impl TransferId {
    pub(crate) fn new() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}
pub(crate) struct Frame {
    pub(crate) bytes: BytesMut,
    _permit: OwnedSemaphorePermit,
}
impl Frame {
    pub(crate) fn allocate(limits: &Limits) -> Result<Self> {
        let permit = limits.reserve(MAX_FRAME)?;
        Ok(Self {
            bytes: BytesMut::with_capacity(MAX_FRAME),
            _permit: permit,
        })
    }
    pub(crate) fn received(bytes: BytesMut, limits: &Limits) -> Result<Self> {
        if bytes.len() > MAX_FRAME || bytes.capacity() > 2 * MAX_FRAME {
            return Err(Error::Protocol);
        }
        let permit = limits.reserve(bytes.capacity())?;
        Ok(Self {
            bytes,
            _permit: permit,
        })
    }
}

pub(crate) enum Data<'a> {
    Begin {
        id: TransferId,
        resource: Resource,
    },
    Ready(TransferId),
    Chunk {
        id: TransferId,
        offset: u64,
        bytes: &'a [u8],
    },
    Ack {
        id: TransferId,
        offset: u64,
    },
    Finish {
        id: TransferId,
        hash: String,
    },
    Complete {
        id: TransferId,
        offset: u64,
        hash: String,
    },
}
impl Data<'_> {
    pub(crate) fn encode(&self, limits: &Limits) -> Result<Frame> {
        let mut frame = Frame::allocate(limits)?;
        let (tag, id) = match self {
            Self::Begin { id, .. } => (1, id),
            Self::Ready(id) => (2, id),
            Self::Chunk { id, .. } => (3, id),
            Self::Ack { id, .. } => (4, id),
            Self::Finish { id, .. } => (5, id),
            Self::Complete { id, .. } => (6, id),
        };
        frame.bytes.extend_from_slice(&[tag]);
        frame.bytes.extend_from_slice(&id.0);
        match self {
            Self::Begin { resource, .. } => {
                resource.validate()?;
                let json = serde_json::to_vec(resource).map_err(|_| Error::InvalidResource)?;
                if json.len() > MAX_FRAME - 17 {
                    return Err(Error::Protocol);
                }
                frame.bytes.extend_from_slice(&json);
            }
            Self::Chunk { offset, bytes, .. } => {
                if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
                    return Err(Error::Protocol);
                }
                frame.bytes.extend_from_slice(&offset.to_be_bytes());
                frame.bytes.extend_from_slice(bytes);
            }
            Self::Ack { offset, .. } => frame.bytes.extend_from_slice(&offset.to_be_bytes()),
            Self::Finish { hash, .. } => frame.bytes.extend_from_slice(&hash_bytes(hash)?),
            Self::Complete { offset, hash, .. } => {
                frame.bytes.extend_from_slice(&offset.to_be_bytes());
                frame.bytes.extend_from_slice(&hash_bytes(hash)?);
            }
            Self::Ready(_) => {}
        }
        Ok(frame)
    }
}
pub(crate) fn decode_data(bytes: &[u8]) -> Result<Data<'_>> {
    if bytes.len() < 17 || bytes.len() > MAX_FRAME {
        return Err(Error::Protocol);
    }
    let id = TransferId(bytes[1..17].try_into().map_err(|_| Error::Protocol)?);
    let tail = &bytes[17..];
    match bytes[0] {
        1 => {
            let resource: Resource =
                serde_json::from_slice(tail).map_err(|_| Error::InvalidResource)?;
            resource.validate()?;
            Ok(Data::Begin { id, resource })
        }
        2 if tail.is_empty() => Ok(Data::Ready(id)),
        3 if tail.len() > 8 => Ok(Data::Chunk {
            id,
            offset: number(&tail[..8])?,
            bytes: &tail[8..],
        }),
        4 if tail.len() == 8 => Ok(Data::Ack {
            id,
            offset: number(tail)?,
        }),
        5 if tail.len() == 32 => Ok(Data::Finish {
            id,
            hash: hex(tail),
        }),
        6 if tail.len() == 40 => Ok(Data::Complete {
            id,
            offset: number(&tail[..8])?,
            hash: hex(&tail[8..]),
        }),
        _ => Err(Error::Protocol),
    }
}
pub(crate) enum Control<'a> {
    Message { seq: u64, bytes: &'a [u8] },
    Credit(u64),
}
pub(crate) fn encode_control(value: Control<'_>, limits: &Limits) -> Result<Frame> {
    let mut frame = Frame::allocate(limits)?;
    match value {
        Control::Message { seq, bytes } => {
            if bytes.len() > MAX_FRAME - 9 {
                return Err(Error::Protocol);
            }
            frame.bytes.extend_from_slice(&[1]);
            frame.bytes.extend_from_slice(&seq.to_be_bytes());
            frame.bytes.extend_from_slice(bytes);
        }
        Control::Credit(seq) => {
            frame.bytes.extend_from_slice(&[2]);
            frame.bytes.extend_from_slice(&seq.to_be_bytes());
        }
    }
    Ok(frame)
}
pub(crate) fn decode_control(bytes: &[u8]) -> Result<Control<'_>> {
    if bytes.len() < 9 || bytes.len() > MAX_FRAME {
        return Err(Error::Protocol);
    }
    let seq = number(&bytes[1..9])?;
    match bytes[0] {
        1 => Ok(Control::Message {
            seq,
            bytes: &bytes[9..],
        }),
        2 if bytes.len() == 9 => Ok(Control::Credit(seq)),
        _ => Err(Error::Protocol),
    }
}
fn number(bytes: &[u8]) -> Result<u64> {
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| Error::Protocol)?,
    ))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash_bytes(hash: &str) -> Result<[u8; 32]> {
    crate::bbs_sync::valid_hash(hash).map_err(|_| Error::Protocol)?;
    let mut bytes = [0; 32];
    for (i, pair) in hash.as_bytes().chunks_exact(2).enumerate() {
        fn digit(byte: u8) -> u8 {
            if byte <= b'9' {
                byte - b'0'
            } else {
                byte - b'a' + 10
            }
        }
        bytes[i] = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(bytes)
}

pub(crate) struct SendWindow {
    pub(crate) sent: u64,
    pub(crate) acked: u64,
    ends: VecDeque<u64>,
}
impl SendWindow {
    pub(crate) fn new() -> Self {
        Self {
            sent: 0,
            acked: 0,
            ends: VecDeque::with_capacity(DATA_WINDOW),
        }
    }
    pub(crate) fn writable(&self) -> bool {
        self.ends.len() < DATA_WINDOW
    }
    pub(crate) fn sent_chunk(&mut self, offset: u64, size: usize) -> Result<()> {
        if offset != self.sent || size == 0 || size > CHUNK_BYTES || !self.writable() {
            return Err(Error::Protocol);
        }
        self.sent = self.sent.checked_add(size as u64).ok_or(Error::Protocol)?;
        self.ends.push_back(self.sent);
        Ok(())
    }
    pub(crate) fn acknowledge(&mut self, offset: u64) -> Result<()> {
        // Cumulative ACKs must land on a boundary actually sent. Duplicates and
        // backwards movement are not silently accepted on an ordered channel.
        if offset <= self.acked || !self.ends.contains(&offset) {
            return Err(Error::Protocol);
        }
        while self.ends.front().is_some_and(|n| *n <= offset) {
            self.ends.pop_front();
        }
        self.acked = offset;
        Ok(())
    }
    pub(crate) fn drained(&self) -> bool {
        self.ends.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credit_window_stops_until_written_boundary_ack_and_rejects_false_or_repeated_ack() {
        let mut window = SendWindow::new();
        for i in 0..DATA_WINDOW {
            assert!(window.writable());
            window
                .sent_chunk((i * CHUNK_BYTES) as u64, CHUNK_BYTES)
                .unwrap();
        }
        assert!(!window.writable());
        assert!(window.sent_chunk(window.sent, 1).is_err());
        assert!(window.acknowledge(1).is_err());
        assert!(window.acknowledge(window.sent + 1).is_err());
        window.acknowledge(CHUNK_BYTES as u64).unwrap();
        assert!(window.writable());
        assert!(window.acknowledge(CHUNK_BYTES as u64).is_err());
        window.acknowledge(window.sent).unwrap();
        assert!(window.drained());
    }
    #[test]
    fn codec_enforces_header_chunk_hash_and_frame_bounds() {
        let limits = Limits::default();
        let id = TransferId::new();
        let bytes = vec![5; CHUNK_BYTES];
        let f = Data::Chunk {
            id,
            offset: 42,
            bytes: &bytes,
        }
        .encode(&limits)
        .unwrap();
        assert_eq!(f.bytes.len(), MAX_FRAME);
        match decode_data(&f.bytes).unwrap() {
            Data::Chunk {
                id: decoded,
                offset,
                bytes: payload,
            } => {
                assert_eq!(decoded, id);
                assert_eq!(offset, 42);
                assert_eq!(payload, bytes);
            }
            _ => panic!("wrong frame"),
        }
        assert!(Data::Chunk {
            id,
            offset: 0,
            bytes: &vec![0; CHUNK_BYTES + 1]
        }
        .encode(&limits)
        .is_err());
        assert!(decode_data(&f.bytes[..24]).is_err());
        assert!(decode_data(&[0; MAX_FRAME + 1]).is_err());
        assert!(Data::Finish {
            id,
            hash: "wrong".into()
        }
        .encode(&limits)
        .is_err());
        let credit = encode_control(Control::Credit(12), &limits).unwrap();
        assert!(matches!(
            decode_control(&credit.bytes),
            Ok(Control::Credit(12))
        ));
        let mut bad = credit.bytes.to_vec();
        bad.push(0);
        assert!(decode_control(&bad).is_err());
    }
    #[test]
    fn allocation_budget_is_account_wide_and_released_on_drop() {
        let limits = Limits::default();
        let other = limits.clone();
        let mut frames = Vec::new();
        for _ in 0..super::super::APP_QUEUE_BYTES / MAX_FRAME {
            frames.push(Frame::allocate(&limits).unwrap());
        }
        assert!(matches!(Frame::allocate(&other), Err(Error::Busy)));
        frames.pop();
        assert!(Frame::allocate(&other).is_ok());
    }
}
