//! One registered file at a time. There is one poller, and receipt of a frame
//! never acknowledges it. Only the file worker's completed write grants credit.
use super::{
    channel::Channel,
    protocol::{self, Data, Frame, SendWindow, TransferId},
    Cancellation, Error, FileIo, FileWriter, Limits, MembershipCheck, Resource, Result,
    VerifiedFile, DATA_WINDOW, PROGRESS_TIMEOUT,
};
use bytes::BytesMut;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, Mutex, OwnedMutexGuard};
use webrtc::data_channel::DataChannel;

pub(crate) struct DataPipe {
    channel: Channel,
    incoming: Mutex<mpsc::Receiver<Frame>>,
    operation: Arc<Mutex<()>>,
    active: Arc<AtomicBool>,
    limits: Limits,
    io: FileIo,
    cancel: Cancellation,
    authorized: MembershipCheck,
    #[cfg(test)]
    pub(crate) pause_after_ready: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Flip one received byte of the next resumable slice before it is written.
    #[cfg(test)]
    pub(crate) corrupt_next_slice: AtomicBool,
    #[cfg(test)]
    pub(crate) pause_after_write: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<u64>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
struct Active {
    _lock: OwnedMutexGuard<()>,
    active: Arc<AtomicBool>,
    cancel: Cancellation,
    completed: bool,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if !self.completed {
            self.cancel.cancel();
        }
    }
}
impl DataPipe {
    #[cfg(test)]
    pub(crate) async fn debug_queued(&self) -> usize {
        self.incoming.lock().await.len()
    }
    pub(crate) fn start(
        channel: Arc<dyn DataChannel>,
        limits: Limits,
        io: FileIo,
        cancel: Cancellation,
        authorized: MembershipCheck,
    ) -> Arc<Self> {
        Self::start_on(
            Channel::Rtc(channel, limits.clone()),
            limits,
            io,
            cancel,
            authorized,
        )
    }
    pub(crate) fn start_on(
        channel: Channel,
        limits: Limits,
        io: FileIo,
        cancel: Cancellation,
        authorized: MembershipCheck,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(DATA_WINDOW + 2);
        let pipe = Arc::new(Self {
            channel,
            incoming: Mutex::new(rx),
            operation: Arc::new(Mutex::new(())),
            active: Arc::new(AtomicBool::new(false)),
            limits,
            io,
            cancel,
            authorized,
            #[cfg(test)]
            pause_after_ready: Mutex::new(None),
            #[cfg(test)]
            pause_after_write: Mutex::new(None),
            #[cfg(test)]
            corrupt_next_slice: AtomicBool::new(false),
        });
        let task = pipe.clone();
        tokio::spawn(async move {
            let _ = task.poll(tx).await;
            task.cancel.cancel();
        });
        pipe
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(self.authorized)() {
            self.cancel.cancel();
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn acquire(&self) -> Result<Active> {
        self.check()?;
        let lock = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| Error::Busy)?;
        self.active.store(true, Ordering::Release);
        Ok(Active {
            _lock: lock,
            active: self.active.clone(),
            cancel: self.cancel.clone(),
            completed: false,
        })
    }
    async fn poll(&self, tx: mpsc::Sender<Frame>) -> Result<()> {
        loop {
            self.check()?;
            let frame = tokio::select! {_ = self.cancel.cancelled() => return Err(Error::Cancelled), frame = self.channel.receive() => frame?};
            self.check()?;
            if !self.active.load(Ordering::Acquire) {
                return Err(Error::Protocol);
            }
            tx.try_send(frame).map_err(|_| Error::Protocol)?;
        }
    }
    async fn next(&self) -> Result<Frame> {
        self.check()?;
        tokio::select! {
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(PROGRESS_TIMEOUT, async {self.incoming.lock().await.recv().await}) => {
                self.check()?;
                result.map_err(|_| Error::Timeout)?.ok_or(Error::Closed)
            }
        }
    }
    async fn send(&self, message: Data<'_>) -> Result<()> {
        self.check()?;
        let capacity = self.channel.encoding_capacity(message.compact_capacity());
        self.channel
            .send_encoded(&self.limits, &self.cancel, capacity, || {
                if capacity == super::MAX_FRAME {
                    message.encode(&self.limits)
                } else {
                    message.encode_sized(&self.limits, capacity)
                }
            })
            .await
    }
    async fn ack(&self, id: TransferId, window: &mut SendWindow, frame: Frame) -> Result<()> {
        match protocol::decode_data(&frame.bytes)? {
            Data::Ack { id: actual, offset } if actual == id => window.acknowledge(offset),
            _ => Err(Error::Protocol),
        }
    }
    /// Begin only after the receiver has registered this exact resource via
    /// expect_file. A source path comes from the local BBS resolver, never wire.
    pub(crate) async fn send_file(&self, resource: Resource, source: &Path) -> Result<()> {
        let end = resource.size_bytes;
        self.send_range(resource, source, 0, end).await
    }
    /// One v5 attachment slice uses the same frames as a whole file. Offsets
    /// are absolute; Finish/Complete mark the slice end, not the file end.
    pub(crate) async fn send_range(
        &self,
        resource: Resource,
        source: &Path,
        start: u64,
        end: u64,
    ) -> Result<()> {
        resource.validate()?;
        let mut file = tokio::time::timeout(
            PROGRESS_TIMEOUT,
            self.io.read_range(resource.clone(), source, start, end, &self.cancel),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        let mut active = self.acquire()?;
        let id = TransferId::new();
        self.send(Data::Begin {
            id,
            resource: resource.clone(),
        })
        .await?;
        let ready = self.next().await?;
        if !matches!(protocol::decode_data(&ready.bytes)?, Data::Ready(actual) if actual == id) {
            return Err(Error::Protocol);
        }
        drop(ready);
        let mut window = SendWindow::starting_at(start);
        let mut burst = 0;
        loop {
            while !window.writable() {
                self.ack(id, &mut window, self.next().await?).await?;
            }
            // Do not cancel and recreate next(): the worker may have read the
            // chunk already. Timeout aborts this entire file and drops its handle.
            let next = tokio::time::timeout(PROGRESS_TIMEOUT, file.next())
                .await
                .map_err(|_| Error::Timeout)??;
            let Some(chunk) = next else {
                break;
            };
            window.sent_chunk(chunk.offset, chunk.bytes.len())?;
            self.send(Data::Chunk {
                id,
                offset: chunk.offset,
                bytes: &chunk.bytes,
            })
            .await?;
            drop(chunk);
            loop {
                let frame = self.incoming.lock().await.try_recv();
                match frame {
                    Ok(frame) => self.ack(id, &mut window, frame).await?,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(_) => return Err(Error::Closed),
                }
            }
            burst += 1;
            if burst == 4 {
                tokio::task::yield_now().await;
                burst = 0;
            }
        }
        while !window.drained() {
            self.ack(id, &mut window, self.next().await?).await?;
        }
        if window.sent != end {
            return Err(Error::Integrity);
        }
        self.send(Data::Finish {
            id,
            hash: resource.sha256.clone(),
        })
        .await?;
        let complete = self.next().await?;
        if !matches!(protocol::decode_data(&complete.bytes)?, Data::Complete{id:actual, offset, hash} if actual == id && offset == end && hash == resource.sha256)
        {
            return Err(Error::Integrity);
        }
        active.completed = true;
        Ok(())
    }
    /// This await prepares a private output and arms reception before the
    /// coordinator sends the peer a file request on the independent control DC.
    pub(crate) async fn expect_file(
        self: &Arc<Self>,
        resource: Resource,
        staging: &Path,
    ) -> Result<IncomingFile> {
        resource.validate()?;
        let writer = tokio::time::timeout(
            PROGRESS_TIMEOUT,
            self.io.receive(resource.clone(), staging, &self.cancel),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        let active = self.acquire()?;
        Ok(IncomingFile {
            pipe: self.clone(),
            writer: Some(writer),
            resource,
            active,
        })
    }
}
pub(crate) struct IncomingFile {
    pipe: Arc<DataPipe>,
    writer: Option<FileWriter>,
    resource: Resource,
    active: Active,
}
impl IncomingFile {
    /// Only after an explicit control rejection, before any Begin was accepted.
    pub(crate) fn declined(mut self) {
        self.active.completed = true;
    }

    pub(crate) async fn finish(mut self) -> Result<VerifiedFile> {
        let first = self.pipe.next().await?;
        let id = match protocol::decode_data(&first.bytes)? {
            Data::Begin { id, resource } if resource == self.resource => id,
            _ => return Err(Error::InvalidResource),
        };
        drop(first);
        self.pipe.send(Data::Ready(id)).await?;
        #[cfg(test)]
        if let Some(gate) = self.pipe.pause_after_ready.lock().await.take() {
            tokio::select! {_=gate=>{},_=self.pipe.cancel.cancelled()=>return Err(Error::Cancelled)}
        }
        let mut writer = self.writer.take().ok_or(Error::Closed)?;
        let mut written = 0;
        loop {
            let frame = self.pipe.next().await?;
            match protocol::decode_data(&frame.bytes)? {
                Data::Chunk {
                    id: actual,
                    offset,
                    bytes,
                } if actual == id && offset == written => {
                    let bytes = BytesMut::from(bytes);
                    drop(frame);
                    written = tokio::time::timeout(PROGRESS_TIMEOUT, writer.write(offset, bytes))
                        .await
                        .map_err(|_| Error::Timeout)??;
                    #[cfg(test)]
                    if let Some((started, release)) =
                        self.pipe.pause_after_write.lock().await.take()
                    {
                        let _ = started.send(written);
                        tokio::select! {_=release=>{},_=self.pipe.cancel.cancelled()=>return Err(Error::Cancelled)}
                    }
                    self.pipe
                        .send(Data::Ack {
                            id,
                            offset: written,
                        })
                        .await?;
                }
                Data::Finish { id: actual, hash }
                    if actual == id
                        && written == self.resource.size_bytes
                        && hash == self.resource.sha256 =>
                {
                    drop(frame);
                    let verified =
                        tokio::time::timeout(PROGRESS_TIMEOUT, writer.finish(hash.clone()))
                            .await
                            .map_err(|_| Error::Timeout)??;
                    self.pipe
                        .send(Data::Complete {
                            id,
                            offset: written,
                            hash,
                        })
                        .await?;
                    self.active.completed = true;
                    return Ok(verified);
                }
                _ => return Err(Error::Protocol),
            }
        }
    }
}

/// A v5 attachment receive. The deterministic partial outlives a lost link;
/// each slice is one control Get and only a slice end is fsynced.
pub(crate) struct ResumableFile {
    pipe: Arc<DataPipe>,
    writer: Option<FileWriter>,
    resource: Resource,
    offset: u64,
}
pub(crate) enum Slice {
    Partial(u64),
    Verified(VerifiedFile),
}
impl DataPipe {
    /// No PROGRESS_TIMEOUT here: after a restart, restoring a long prefix hash
    /// is paced local work. The link watchdog observes it as file work.
    pub(crate) async fn resume_file(
        self: &Arc<Self>,
        resource: Resource,
        staging: &Path,
    ) -> Result<ResumableFile> {
        resource.validate()?;
        let writer = self.io.resume(resource.clone(), staging, &self.cancel).await?;
        Ok(ResumableFile {
            pipe: self.clone(),
            offset: writer.start(),
            writer: Some(writer),
            resource,
        })
    }
}
impl ResumableFile {
    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }
    /// Arm one slice before its Get is sent, as expect_file does for a file.
    pub(crate) fn expect(&mut self, length: u64) -> Result<IncomingSlice<'_>> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| length > 0 && *end <= self.resource.size_bytes)
            .ok_or(Error::Protocol)?;
        let active = self.pipe.acquire()?;
        Ok(IncomingSlice {
            file: self,
            end,
            active,
        })
    }
}
pub(crate) struct IncomingSlice<'a> {
    file: &'a mut ResumableFile,
    end: u64,
    active: Active,
}
impl IncomingSlice<'_> {
    /// Only after an explicit control rejection, before any Begin was accepted.
    pub(crate) fn declined(mut self) {
        self.active.completed = true;
    }
    /// A local write failure or a final hash mismatch fails this item only:
    /// the slice is still drained and completed, so the link stays usable.
    pub(crate) async fn finish(mut self) -> Result<Slice> {
        let pipe = self.file.pipe.clone();
        let first = pipe.next().await?;
        let id = match protocol::decode_data(&first.bytes)? {
            Data::Begin { id, resource } if resource == self.file.resource => id,
            _ => return Err(Error::InvalidResource),
        };
        drop(first);
        pipe.send(Data::Ready(id)).await?;
        #[cfg(test)]
        if let Some(gate) = pipe.pause_after_ready.lock().await.take() {
            tokio::select! {_=gate=>{},_=pipe.cancel.cancelled()=>return Err(Error::Cancelled)}
        }
        let mut written = self.file.offset;
        let mut failed = None;
        loop {
            let frame = pipe.next().await?;
            match protocol::decode_data(&frame.bytes)? {
                Data::Chunk {
                    id: actual,
                    offset,
                    bytes,
                } if actual == id
                    && offset == written
                    && offset + bytes.len() as u64 <= self.end =>
                {
                    let next = offset + bytes.len() as u64;
                    #[allow(unused_mut)]
                    let mut bytes = BytesMut::from(bytes);
                    drop(frame);
                    #[cfg(test)]
                    if pipe.corrupt_next_slice.swap(false, Ordering::Relaxed) {
                        bytes[0] ^= 0xff;
                    }
                    if failed.is_none() {
                        let writer = self.file.writer.as_mut().ok_or(Error::Closed)?;
                        match tokio::time::timeout(PROGRESS_TIMEOUT, writer.write(offset, bytes))
                            .await
                            .map_err(|_| Error::Timeout)?
                        {
                            Ok(done) if done == next => {}
                            Ok(_) => return Err(Error::Protocol),
                            Err(Error::Io) => failed = Some(Error::Io),
                            Err(error) => return Err(error),
                        }
                    }
                    written = next;
                    pipe.send(Data::Ack { id, offset: written }).await?;
                }
                Data::Finish { id: actual, hash }
                    if actual == id && written == self.end && hash == self.file.resource.sha256 =>
                {
                    drop(frame);
                    let slice = match failed {
                        Some(error) => Err(error),
                        None if written == self.file.resource.size_bytes => {
                            let writer = self.file.writer.take().ok_or(Error::Closed)?;
                            match tokio::time::timeout(PROGRESS_TIMEOUT, writer.finish(hash.clone()))
                                .await
                                .map_err(|_| Error::Timeout)?
                            {
                                Ok(verified) => Ok(Slice::Verified(verified)),
                                Err(error @ (Error::Integrity | Error::Io)) => Err(error),
                                Err(error) => return Err(error),
                            }
                        }
                        None => {
                            let writer = self.file.writer.as_mut().ok_or(Error::Closed)?;
                            match tokio::time::timeout(PROGRESS_TIMEOUT, writer.checkpoint())
                                .await
                                .map_err(|_| Error::Timeout)?
                            {
                                Ok(_) => Ok(Slice::Partial(written)),
                                Err(Error::Io) => Err(Error::Io),
                                Err(error) => return Err(error),
                            }
                        }
                    };
                    pipe.send(Data::Complete {
                        id,
                        offset: written,
                        hash,
                    })
                    .await?;
                    self.active.completed = true;
                    if slice.is_ok() {
                        self.file.offset = written;
                    }
                    return slice;
                }
                _ => return Err(Error::Protocol),
            }
        }
    }
}
