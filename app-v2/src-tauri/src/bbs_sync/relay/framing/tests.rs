use super::*;
use crate::bbs_sync::{
    raw_sha256,
    relay::{tests::fixture, window::Buffer, PendingTls},
    transport::{FileIo, Resource, ResourceKind, APP_QUEUE_BYTES},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn check() -> MembershipCheck {
    Arc::new(|| true)
}

#[tokio::test]
async fn connection_adoption_keeps_the_original_member_fence_and_cancellation_owner() {
    for revoke_before in [true, false] {
        let ctx = context();
        let host = crate::bbs_sync::transport::NetworkHost::start_with_files(
            ctx.io.clone(),
            ctx.limits.clone(),
        )
        .await
        .unwrap();
        let io = ctx.io.clone();
        let result = host
            .execute(move |_| async move {
                let current = Arc::new(AtomicBool::new(true));
                let member = current.clone();
                let auth: MembershipCheck = Arc::new(move || member.load(Ordering::Acquire));
                let cancels = [Cancellation::default(), Cancellation::default()];
                let (mut tls, _remote) = tls_pair(&cancels, auth.clone());
                let (channels, framing) =
                    Framing::start(&mut tls, &ctx, cancels[0].clone(), auth).unwrap();
                if revoke_before {
                    current.store(false, Ordering::Release);
                }
                let adopted = channels.into_connection();
                if revoke_before {
                    assert!(matches!(adopted, Err(Error::Unauthorized)));
                } else {
                    let connection = adopted.unwrap();
                    assert!(!cancels[0].is_cancelled());
                    assert!(connection.diagnostics().await?.is_none());
                    current.store(false, Ordering::Release);
                    assert_eq!(connection.recheck_membership(), Err(Error::Unauthorized));
                    assert_eq!(
                        connection.send_control(b"must not escape").await,
                        Err(Error::Unauthorized)
                    );
                    drop(connection);
                }
                assert!(
                    cancels[0].is_cancelled(),
                    "failed adoption or revocation retains the original stop token"
                );
                drop(framing);
                Ok(())
            })
            .await;
        host.stop_and_wait().await;
        io.stop_and_wait().await;
        result.unwrap();
    }
}
fn context() -> Context {
    let limits = Limits::default();
    Context {
        io: FileIo::start(limits.clone()).unwrap(),
        limits,
        shutdown: Cancellation::default(),
    }
}
fn tls_pair(cancels: &[Cancellation; 2], auth: MembershipCheck) -> (TlsStream, TlsStream) {
    let f = fixture();
    let c = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        auth.clone(),
        cancels[0].clone(),
        f.now,
    )
    .unwrap();
    let s = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        auth,
        cancels[1].clone(),
        c.statement(),
        f.now,
    )
    .unwrap();
    let cs = c.statement().clone();
    let ss = s.statement().clone();
    let mut c = c.accept(&ss, f.now).unwrap();
    let mut s = s.accept(&cs, f.now).unwrap();
    for _ in 0..32 {
        let send = |from: &mut TlsStream, to: &mut TlsStream| {
            let mut buffer = [0; 4096];
            let n = from.drain_tls(&mut buffer).unwrap();
            if n > 0 {
                assert_eq!(to.receive(&buffer[..n]).unwrap(), n);
            }
        };
        send(&mut c, &mut s);
        send(&mut s, &mut c);
        if c.handshake_complete().unwrap() && s.handshake_complete().unwrap() {
            return (c, s);
        }
    }
    panic!("handshake did not complete");
}

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("bbs-relay-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("staging")).unwrap();
        Self(root)
    }
    fn staging(&self) -> PathBuf {
        self.0.join("staging")
    }
    fn source(&self, size: usize) -> (Resource, PathBuf) {
        let bytes: Vec<_> = (0..size).map(|i| (i % 239) as u8).collect();
        let hash = raw_sha256(&bytes);
        let path = self.0.join("source");
        fs::write(&path, bytes).unwrap();
        (
            Resource {
                identity: ResourceKind::Attachment {
                    thread_id: "thread-test".into(),
                    post_id: "post-test".into(),
                    attachment_id: "att-test".into(),
                    version_id: hash.clone(),
                },
                sha256: hash,
                size_bytes: size as u64,
            },
            path,
        )
    }
    async fn clean(&self) {
        bounded(async {
            loop {
                if fs::read_dir(self.staging()).unwrap().next().is_none() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
async fn bounded<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), f)
        .await
        .expect("bounded adapter transition")
}

/// Test-only delivery of real TLS ciphertext. This is deliberately not an HTTP
/// relay/DO/4-second scheduler claim. The already-reviewed batch/HTTP tests are
/// separate; this harness exercises the new framing and actual FileIo paths.
struct Link {
    a: Channels,
    b: Channels,
    contexts: [Context; 2],
    cancels: [Cancellation; 2],
    driver: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Link {
    fn new(fragment: usize) -> Self {
        let contexts = [context(), context()];
        let cancels = [Cancellation::default(), Cancellation::default()];
        let (mut a, mut b) = tls_pair(&cancels, check());
        let (ca, mut fa) =
            Framing::start(&mut a, &contexts[0], cancels[0].clone(), check()).unwrap();
        let (cb, mut fb) =
            Framing::start(&mut b, &contexts[1], cancels[1].clone(), check()).unwrap();
        let la = contexts[0].limits.clone();
        let lb = contexts[1].limits.clone();
        let stop = cancels.clone();
        let driver = tokio::spawn(async move {
            let result = async {
                // Each destination owns its bounded ciphertext scratch. No
                // uncharged Vec grows with the file or the stopped consumer.
                let mut ab = Buffer::new(&lb, MAX_FRAME)?;
                let mut ba = Buffer::new(&la, MAX_FRAME)?;
                let (mut an, mut bn, mut ao, mut bo) = (0, 0, 0, 0);
                loop {
                    for _ in 0..16 {
                        fa.step(&mut a)?;
                        fb.step(&mut b)?;
                        transfer(&mut a, &mut b, &mut ab, &mut an, &mut ao, fragment)?;
                        transfer(&mut b, &mut a, &mut ba, &mut bn, &mut bo, fragment)?;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            .await;
            for c in &stop {
                c.cancel();
            }
            result
        });
        Self {
            a: ca,
            b: cb,
            contexts,
            cancels,
            driver: Some(driver),
        }
    }
    async fn stop(mut self) {
        for c in &self.cancels {
            c.cancel();
        }
        bounded(self.driver.take().unwrap())
            .await
            .unwrap()
            .unwrap_err();
        for ctx in &self.contexts {
            ctx.io.stop_and_wait().await;
        }
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        for c in &self.cancels {
            c.cancel();
        }
        for ctx in &self.contexts {
            ctx.shutdown.cancel();
        }
    }
}
fn transfer(
    from: &mut TlsStream,
    to: &mut TlsStream,
    buffer: &mut Buffer,
    len: &mut usize,
    offset: &mut usize,
    fragment: usize,
) -> Result<()> {
    if *offset == *len {
        *len = from.drain_tls(buffer.spare_mut())?;
        *offset = 0;
    }
    if *len > *offset {
        let end = (*offset + fragment).min(*len);
        match to.receive(&buffer.spare_mut()[*offset..end]) {
            Ok(n) => *offset += n,
            Err(Error::Busy) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[tokio::test]
async fn shared_file_worker_and_protocol_transfer_verified_files_in_both_directions() {
    let link = Link::new(997);
    for (sender, receiver) in [(&link.a, &link.b), (&link.b, &link.a)] {
        let root = Root::new();
        let (resource, source) = root.source(384 * 1024 + 13);
        let incoming = receiver
            .data
            .expect_file(resource.clone(), &root.staging())
            .await
            .unwrap();
        let (sent, received) = bounded(async {
            tokio::join!(
                sender.data.send_file(resource.clone(), &source),
                incoming.finish()
            )
        })
        .await;
        sent.unwrap();
        let verified = received.unwrap();
        assert_eq!(verified.resource, resource);
        assert_eq!(
            raw_sha256(&fs::read(verified.path()).unwrap()),
            resource.sha256
        );
        drop(verified);
        root.clean().await;
    }
    link.stop().await;
}

#[tokio::test]
async fn stopped_file_consumer_keeps_written_credit_zero_and_control_works_then_hash_matches() {
    let mut link = Link::new(MAX_FRAME);
    let root = Root::new();
    let (resource, source) = root.source(1024 * 1024);
    let (resume, pause) = tokio::sync::oneshot::channel();
    *link.b.data.pause_after_ready.lock().await = Some(pause);
    let incoming = link
        .b
        .data
        .expect_file(resource.clone(), &root.staging())
        .await
        .unwrap();
    let receiver = tokio::spawn(incoming.finish());
    let send = link.a.data.clone();
    let r = resource.clone();
    let sender = tokio::spawn(async move { send.send_file(r, &source).await });
    bounded(async {
        while link.b.data.debug_queued().await != DATA_WINDOW {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(!sender.is_finished());
    let files: Vec<_> = fs::read_dir(root.staging())
        .unwrap()
        .map(|p| p.unwrap().metadata().unwrap().len())
        .collect();
    assert_eq!(files, vec![0]);
    let control = link.a.control.clone();
    let request =
        tokio::spawn(async move { control.send(b"control during file backpressure").await });
    let delivery = bounded(link.b.incoming.recv()).await.unwrap();
    assert_eq!(delivery.bytes(), b"control during file backpressure");
    assert!(!request.is_finished());
    delivery.acknowledge().unwrap();
    bounded(request).await.unwrap().unwrap();
    assert!(!sender.is_finished());
    resume.send(()).unwrap();
    bounded(sender).await.unwrap().unwrap();
    let verified = bounded(receiver).await.unwrap().unwrap();
    assert_eq!(
        raw_sha256(&fs::read(verified.path()).unwrap()),
        resource.sha256
    );
    drop(verified);
    root.clean().await;
    link.stop().await;
}

#[tokio::test]
async fn control_consumption_credit_survives_fragmented_headers_without_dropping_messages() {
    let mut link = Link::new(7);
    let mut senders = Vec::new();
    for _ in 0..CONTROL_WINDOW * 2 {
        let channel = link.a.control.clone();
        senders.push(tokio::spawn(async move {
            channel.send(b"fragmented control").await
        }));
    }
    bounded(async {
        while link.b.incoming.len() != CONTROL_WINDOW {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(senders.iter().all(|t| !t.is_finished()));
    for _ in 0..senders.len() {
        let message = bounded(link.b.incoming.recv()).await.unwrap();
        assert_eq!(message.bytes(), b"fragmented control");
        message.acknowledge().unwrap();
    }
    for task in senders {
        bounded(task).await.unwrap().unwrap();
    }
    link.stop().await;
}

#[tokio::test]
async fn cancellation_from_either_end_cleans_partial_and_new_session_starts_whole_file() {
    for cancel_side in 0..2 {
        let link = Link::new(MAX_FRAME);
        let root = Root::new();
        let (resource, source) = root.source(768 * 1024);
        let (written, observed) = tokio::sync::oneshot::channel();
        let (_resume, pause) = tokio::sync::oneshot::channel();
        *link.b.data.pause_after_write.lock().await = Some((written, pause));
        let incoming = link
            .b
            .data
            .expect_file(resource.clone(), &root.staging())
            .await
            .unwrap();
        let receiving = tokio::spawn(incoming.finish());
        let pipe = link.a.data.clone();
        let r = resource.clone();
        let path = source.clone();
        let sending = tokio::spawn(async move { pipe.send_file(r, &path).await });
        assert_eq!(bounded(observed).await.unwrap(), (MAX_FRAME - 25) as u64);
        link.cancels[cancel_side].cancel();
        assert!(bounded(receiving).await.unwrap().is_err());
        assert!(bounded(sending).await.unwrap().is_err());
        root.clean().await;
        link.stop().await;
        let next = Link::new(MAX_FRAME);
        let (written, observed) = tokio::sync::oneshot::channel();
        let (resume, pause) = tokio::sync::oneshot::channel();
        *next.b.data.pause_after_write.lock().await = Some((written, pause));
        let incoming = next
            .b
            .data
            .expect_file(resource.clone(), &root.staging())
            .await
            .unwrap();
        let receiving = tokio::spawn(incoming.finish());
        let pipe = next.a.data.clone();
        let r = resource.clone();
        let sending = tokio::spawn(async move { pipe.send_file(r, &source).await });
        // A fresh Begin started at zero, not the abandoned partial's offset.
        assert_eq!(bounded(observed).await.unwrap(), (MAX_FRAME - 25) as u64);
        resume.send(()).unwrap();
        bounded(sending).await.unwrap().unwrap();
        let verified = bounded(receiving).await.unwrap().unwrap();
        assert_eq!(
            raw_sha256(&fs::read(verified.path()).unwrap()),
            resource.sha256
        );
        drop(verified);
        root.clean().await;
        next.stop().await;
    }
}

#[tokio::test]
async fn malformed_lengths_and_lanes_cancel_before_delivering_or_allocating_unbounded_frames() {
    for header in [
        [0, 0, 0, 0, 1],
        [1, 0, 0, 0, 0],
        [2, 255, 255, 255, 255],
        [1, 0, 0, 64, 1],
    ] {
        let ctx = context();
        let cancels = [Cancellation::default(), Cancellation::default()];
        let (mut c, mut s) = tls_pair(&cancels, check());
        let (channels, mut frame) =
            Framing::start(&mut s, &ctx, cancels[1].clone(), check()).unwrap();
        c.write_plaintext(&header).unwrap();
        let mut bytes = [0; 128];
        let n = c.drain_tls(&mut bytes).unwrap();
        s.receive(&bytes[..n]).unwrap();
        assert_eq!(frame.step(&mut s), Err(Error::Protocol));
        assert!(cancels[1].is_cancelled());
        assert!(channels.incoming.is_empty());
        drop(frame);
        drop(channels);
        ctx.io.stop_and_wait().await;
        assert!(ctx.limits.reserve(APP_QUEUE_BYTES).is_ok());
    }
}

#[tokio::test]
async fn membership_and_account_stop_fence_every_frame_step_and_connection_slots_are_bounded() {
    let ctx = context();
    let mut owners = Vec::new();
    let mut retained = Vec::new();
    for i in 0..5 {
        let cancels = [Cancellation::default(), Cancellation::default()];
        let (_, mut s) = tls_pair(&cancels, check());
        let live = Arc::new(AtomicBool::new(true));
        let checked = live.clone();
        let result = Framing::start(
            &mut s,
            &ctx,
            cancels[1].clone(),
            Arc::new(move || checked.load(Ordering::Acquire)),
        );
        if i == 4 {
            assert!(matches!(result, Err(Error::Busy)));
            break;
        }
        let (channels, frame) = result.unwrap();
        owners.push(channels);
        retained.push((frame, s, live, cancels));
    }
    retained[0].2.store(false, Ordering::Release);
    let (frame, tls, _, cancels) = &mut retained[0];
    assert_eq!(frame.step(tls), Err(Error::Unauthorized));
    assert!(cancels[1].is_cancelled());
    ctx.shutdown.cancel();
    let (frame, tls, _, cancels) = &mut retained[1];
    assert_eq!(frame.step(tls), Err(Error::Closed));
    assert!(cancels[1].is_cancelled());
    drop(retained);
    drop(owners);
    ctx.io.stop_and_wait().await;
}

#[tokio::test]
async fn byte_pressure_keeps_plaintext_in_tls_until_a_real_frame_permit_is_available() {
    let ctx = context();
    let cancels = [Cancellation::default(), Cancellation::default()];
    let (mut c, mut s) = tls_pair(&cancels, check());
    let (mut channels, mut framing) =
        Framing::start(&mut s, &ctx, cancels[1].clone(), check()).unwrap();
    // lane=control, length=12, Message(seq=1, bytes="abc")
    let packet = [1, 0, 0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0, 1, b'a', b'b', b'c'];
    c.write_plaintext(&packet).unwrap();
    let mut bytes = [0; 128];
    let n = c.drain_tls(&mut bytes).unwrap();
    s.receive(&bytes[..n]).unwrap();
    let held = ctx.limits.reserve(APP_QUEUE_BYTES).unwrap();
    assert_eq!(framing.step(&mut s).unwrap(), HEADER);
    assert_eq!(framing.step(&mut s).unwrap(), 0);
    assert!(framing.frame.is_none());
    assert!(channels.incoming.is_empty());
    assert!(!cancels[1].is_cancelled());
    drop(held);
    assert_eq!(framing.step(&mut s).unwrap(), 12);
    let delivery = bounded(channels.incoming.recv()).await.unwrap();
    assert_eq!(delivery.bytes(), b"abc");
    delivery.acknowledge().unwrap();
    let (_, mut other_session) =
        tls_pair(&[Cancellation::default(), Cancellation::default()], check());
    assert_eq!(framing.step(&mut other_session), Err(Error::Unauthorized));
    assert!(cancels[1].is_cancelled());
    drop(framing);
    drop(channels);
    ctx.io.stop_and_wait().await;
}

#[tokio::test]
async fn control_traffic_does_not_reset_the_real_twenty_second_file_write_timeout() {
    let mut link = Link::new(MAX_FRAME);
    let root = Root::new();
    let (resource, source) = root.source(768 * 1024);
    let (_resume, pause) = tokio::sync::oneshot::channel();
    *link.b.data.pause_after_ready.lock().await = Some(pause);
    let incoming = link
        .b
        .data
        .expect_file(resource.clone(), &root.staging())
        .await
        .unwrap();
    let receiving = tokio::spawn(incoming.finish());
    let pipe = link.a.data.clone();
    let sending = tokio::spawn(async move { pipe.send_file(resource, &source).await });
    bounded(async {
        while link.b.data.debug_queued().await != DATA_WINDOW {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    for _ in 0..12 {
        let channel = link.a.control.clone();
        let send = tokio::spawn(async move { channel.send(b"not file progress").await });
        let message = bounded(link.b.incoming.recv()).await.unwrap();
        message.acknowledge().unwrap();
        bounded(send).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // If control traffic reset the file watchdog, the last message would keep
    // it alive past this deadline. This uses real elapsed time, not paused Tokio.
    assert_eq!(
        tokio::time::timeout_at(deadline, sending)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Timeout)
    );
    assert!(bounded(receiving).await.unwrap().is_err());
    root.clean().await;
    link.stop().await;
}
