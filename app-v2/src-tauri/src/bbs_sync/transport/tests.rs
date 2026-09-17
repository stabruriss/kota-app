//! Actual product PeerConnection/DTLS/SCTP/file-worker paths. Loopback is a test
//! configuration, not evidence for STUN, NAT or the deployed Worker.
use super::session::NetworkMode;
use super::*;
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    raw_sha256, DeviceIdentity,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

fn identities() -> (DeviceIdentity, DeviceIdentity, PeerIdentity, PeerIdentity) {
    let a = DeviceIdentity::from_seed([71; 32]).unwrap();
    let b = DeviceIdentity::from_seed([72; 32]).unwrap();
    let members: Vec<_> = [&a, &b]
        .into_iter()
        .enumerate()
        .map(|(i, k)| Member {
            device_id: k.device_id().unwrap(),
            public_key: k.public_key.clone(),
            name: "test".into(),
            role: if i == 0 { Role::Owner } else { Role::Member },
            membership_id: format!("membership-{i}"),
            last_seen_at: 0,
            online: true,
        })
        .collect();
    let mut group = Membership {
        group_id: "group-test".into(),
        worker_url: "https://example.invalid".into(),
        role: Role::Owner,
        membership_id: "membership-0".into(),
    };
    let ab = PeerIdentity::current(&group, &members, &a, &b.device_id().unwrap()).unwrap();
    group.role = Role::Member;
    group.membership_id = "membership-1".into();
    let ba = PeerIdentity::current(&group, &members, &b, &a.device_id().unwrap()).unwrap();
    (a, b, ab, ba)
}
struct Pair {
    a: Arc<Connection>,
    b: Arc<Connection>,
    ha: NetworkHost,
    hb: NetworkHost,
    authorized: Arc<AtomicBool>,
}
impl Drop for Pair {
    fn drop(&mut self) {
        self.a.cancel();
        self.b.cancel();
        self.ha.stop();
        self.hb.stop();
    }
}
async fn pair() -> Pair {
    let ha = NetworkHost::start().await.unwrap();
    let hb = NetworkHost::start().await.unwrap();
    let (ia, ib, ab, ba) = identities();
    let nonce = "0123456789abcdef".repeat(2);
    let authorized = Arc::new(AtomicBool::new(true));
    let auth = authorized.clone();
    let nonce_b = nonce.clone();
    let (mut a, offer) = ha
        .execute(move |ctx| async move {
            let mut a = PendingConnection::new(
                ctx,
                ab,
                SessionRole::Offer,
                nonce,
                Arc::new(move || auth.load(Ordering::Acquire)),
                NetworkMode::Loopback,
            )
            .await?;
            let offer = a.local_description(&ia).await?;
            Ok((a, offer))
        })
        .await
        .unwrap();
    let auth = authorized.clone();
    let (b, answer) = hb
        .execute(move |ctx| async move {
            let mut b = PendingConnection::new(
                ctx,
                ba,
                SessionRole::Answer,
                nonce_b,
                Arc::new(move || auth.load(Ordering::Acquire)),
                NetworkMode::Loopback,
            )
            .await?;
            b.remote_description(&offer).await?;
            let answer = b.local_description(&ib).await?;
            Ok((b, answer))
        })
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        ha.execute(move |_| async move {
            a.remote_description(&answer).await?;
            a.connect().await
        }),
        hb.execute(move |_| async move { b.connect().await })
    );
    let a = Arc::new(a.unwrap());
    let b = Arc::new(b.unwrap());
    a.rtc_policy()
        .forbid_plaintext(b"KOTA-BBS-PLAINTEXT-TEST-MARKER");
    b.rtc_policy()
        .forbid_plaintext(b"KOTA-BBS-PLAINTEXT-TEST-MARKER");
    Pair {
        a,
        b,
        ha,
        hb,
        authorized,
    }
}
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("bbs-product-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::create_dir(p.join("staging")).unwrap();
        Self(p)
    }
    fn source(&self, size: usize) -> (Resource, PathBuf) {
        let marker = b"KOTA-BBS-PLAINTEXT-TEST-MARKER";
        let bytes: Vec<u8> = (0..size).map(|i| marker[i % marker.len()]).collect();
        let hash = raw_sha256(&bytes);
        let path = self.0.join("source");
        fs::write(&path, bytes).unwrap();
        (
            Resource {
                identity: ResourceKind::Post {
                    thread_id: "thread-test".into(),
                    post_id: "post-test".into(),
                    version_id: hash.clone(),
                },
                sha256: hash,
                size_bytes: size as u64,
            },
            path,
        )
    }
    fn staging(&self) -> PathBuf {
        self.0.join("staging")
    }
    async fn assert_clean(&self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if fs::read_dir(self.staging()).unwrap().next().is_none() {
                return;
            }
            assert!(Instant::now() < deadline, "partial not cleaned");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn product_control_stops_at_credit_window_and_loses_no_message_when_consumer_resumes() {
    let p = pair().await;
    let count = CONTROL_WINDOW * 3;
    let completed = Arc::new(AtomicUsize::new(0));
    let a = p.a.clone();
    let done = completed.clone();
    let host = p.ha.clone();
    let sender = tokio::spawn(async move {
        host.execute(move |_| async move {
            let mut tasks = Vec::new();
            for i in 0..count {
                let a = a.clone();
                let done = done.clone();
                tasks.push(tokio::spawn(async move {
                    let message = format!("KOTA-BBS-PLAINTEXT-TEST-MARKER:{i}");
                    a.send_control(message.as_bytes()).await?;
                    done.fetch_add(1, Ordering::Release);
                    Ok::<_, Error>(())
                }));
            }
            for task in tasks {
                task.await.map_err(|_| Error::Runtime)??;
            }
            Ok(())
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let b = p.b.clone();
    let queued =
        p.hb.execute(move |_| async move { Ok(b.debug_queued().await.0) })
            .await
            .unwrap();
    assert_eq!(queued, CONTROL_WINDOW);
    assert_eq!(completed.load(Ordering::Acquire), 0);
    let b = p.b.clone();
    let messages =
        p.hb.execute(move |_| async move {
            let mut messages = Vec::new();
            for _ in 0..count {
                let m = b.next_control().await?;
                messages.push(String::from_utf8(m.bytes().to_vec()).unwrap());
                m.acknowledge()?;
            }
            Ok(messages)
        })
        .await
        .unwrap();
    sender.await.unwrap().unwrap();
    assert_eq!(completed.load(Ordering::Acquire), count);
    let mut ids: Vec<_> = messages
        .iter()
        .map(|m| m.rsplit(':').next().unwrap().parse::<usize>().unwrap())
        .collect();
    ids.sort();
    assert_eq!(ids, (0..count).collect::<Vec<_>>());
    for (host, c) in [(&p.ha, p.a.clone()), (&p.hb, p.b.clone())] {
        let d = host
            .execute(move |_| async move { c.diagnostics().await })
            .await
            .unwrap()
            .expect("RTC diagnostics remain available");
        assert_eq!(d.denied_datagrams, 0);
        assert!(d.sent_bytes > 0);
        assert_eq!(d.local_candidate_type.as_deref(), Some("host"));
        assert_eq!(d.remote_candidate_type.as_deref(), Some("host"));
    }
}

#[tokio::test]
async fn product_file_window_stalls_with_zero_writes_control_keeps_working_then_sha_matches() {
    let p = pair().await;
    let root = Root::new();
    let (resource, source) = root.source(2 * 1024 * 1024);
    let (release, gate) = oneshot::channel();
    let b = p.b.clone();
    let r = resource.clone();
    let stage = root.staging();
    let incoming =
        p.hb.execute(move |_| async move {
            b.pause_file(gate).await;
            b.expect_file(r, &stage).await
        })
        .await
        .unwrap();
    let host = p.hb.clone();
    let receiver = tokio::spawn(async move {
        host.execute(move |_| async move { incoming.finish().await })
            .await
    });
    let a = p.a.clone();
    let r = resource.clone();
    let host = p.ha.clone();
    let sender = tokio::spawn(async move {
        host.execute(move |_| async move { a.send_file(r, &source).await })
            .await
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let b = p.b.clone();
        let queued = p
            .hb
            .execute(move |_| async move { Ok(b.debug_queued().await.1) })
            .await
            .unwrap();
        if queued == DATA_WINDOW {
            break;
        }
        assert!(queued < DATA_WINDOW, "file window exceeded its credit");
        assert!(Instant::now() < deadline, "file window did not fill: {queued}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!sender.is_finished());
    let partial = fs::read_dir(root.staging())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::metadata(partial).unwrap().len(), 0);
    let a = p.a.clone();
    let b = p.b.clone();
    let (sent, received) = tokio::join!(
        p.ha.execute(move |_| async move { a.send_control(b"control during stopped file").await }),
        p.hb.execute(move |_| async move {
            let m = b.next_control().await?;
            assert_eq!(m.bytes(), b"control during stopped file");
            m.acknowledge()
        })
    );
    sent.unwrap();
    received.unwrap();
    release.send(()).unwrap();
    sender.await.unwrap().unwrap();
    let verified = receiver.await.unwrap().unwrap();
    assert_eq!(
        raw_sha256(&fs::read(verified.path()).unwrap()),
        resource.sha256
    );
    drop(verified);
    root.assert_clean().await;
    assert_eq!(p.a.rtc_policy().denied.load(Ordering::Relaxed), 0);
    assert_eq!(p.b.rtc_policy().denied.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn product_cancel_from_either_endpoint_closes_peer_and_cleans_partial() {
    for sender_cancel in [true, false] {
        let p = pair().await;
        let root = Root::new();
        let (resource, source) = root.source(1024 * 1024);
        let (_release, gate) = oneshot::channel();
        let b = p.b.clone();
        let r = resource.clone();
        let stage = root.staging();
        let incoming =
            p.hb.execute(move |_| async move {
                b.pause_file(gate).await;
                b.expect_file(r, &stage).await
            })
            .await
            .unwrap();
        let h = p.hb.clone();
        let receiver = tokio::spawn(async move {
            h.execute(move |_| async move { incoming.finish().await })
                .await
        });
        let h = p.ha.clone();
        let a = p.a.clone();
        let sender = tokio::spawn(async move {
            h.execute(move |_| async move { a.send_file(resource, &source).await })
                .await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let begin = Instant::now();
        if sender_cancel {
            p.a.cancel();
        } else {
            p.b.cancel();
        }
        let (a, b) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(sender, receiver)
        })
        .await
        .expect("peer cancellation hung");
        assert!(a.unwrap().is_err());
        assert!(b.unwrap().is_err());
        root.assert_clean().await;
        assert!(begin.elapsed() < Duration::from_secs(2));
    }
}

#[tokio::test]
async fn product_twenty_second_no_progress_timeout_cleans_registered_partial() {
    let p = pair().await;
    let root = Root::new();
    let (resource, _) = root.source(10);
    let b = p.b.clone();
    let stage = root.staging();
    let begin = Instant::now();
    let result =
        p.hb.execute(move |_| async move { b.expect_file(resource, &stage).await?.finish().await })
            .await;
    assert!(matches!(result, Err(Error::Timeout)));
    assert!(begin.elapsed() >= PROGRESS_TIMEOUT);
    assert!(begin.elapsed() < Duration::from_secs(22));
    root.assert_clean().await;
}

#[tokio::test]
async fn product_membership_revocation_blocks_payload_and_default_construction_starts_nothing() {
    assert!(matches!(
        runtime_host::require_network_thread(),
        Err(Error::Runtime)
    ));
    let limits = Limits::default();
    assert_eq!(limits.bytes.available_permits(), APP_QUEUE_BYTES);
    assert_eq!(limits.files.available_permits(), 1);
    assert_eq!(limits.connections.available_permits(), MAX_CONNECTIONS);
    let p = pair().await;
    p.authorized.store(false, Ordering::Release);
    let a = p.a.clone();
    let result =
        p.ha.execute(move |_| async move { a.send_control(b"must not be sent").await })
            .await;
    assert_eq!(result, Err(Error::Unauthorized));
    assert!(p.b.recheck_membership().is_err());
}

#[tokio::test]
async fn product_rejects_bad_identity_signature_and_nonce_before_native_remote_description() {
    let ha = NetworkHost::start().await.unwrap();
    let hb = NetworkHost::start().await.unwrap();
    let (ia, _, ab, ba) = identities();
    let nonce = "a".repeat(32);
    let nonce_b = nonce.clone();
    let (a, offer) = ha
        .execute(move |ctx| async move {
            let mut p = PendingConnection::new(
                ctx,
                ab,
                SessionRole::Offer,
                nonce,
                Arc::new(|| true),
                NetworkMode::Loopback,
            )
            .await?;
            let offer = p.local_description(&ia).await?;
            Ok((p, offer))
        })
        .await
        .unwrap();
    hb.execute(move |ctx| async move {
        let mut b = PendingConnection::new(
            ctx,
            ba,
            SessionRole::Answer,
            nonce_b,
            Arc::new(|| true),
            NetworkMode::Loopback,
        )
        .await?;
        for field in ["identity", "signature", "nonce"] {
            let mut value: serde_json::Value = serde_json::from_str(&offer.encode()?).unwrap();
            match field {
                "identity" => value["body"]["from"] = "0".repeat(64).into(),
                "signature" => value["signature"] = "A".repeat(88).into(),
                _ => value["body"]["nonce"] = "b".repeat(32).into(),
            }
            let bad = SignedDescription::decode(&serde_json::to_string(&value).unwrap())?;
            assert!(matches!(
                b.remote_description(&bad).await,
                Err(Error::Unauthorized)
            ));
        }
        // No candidate was installed and no connectivity/payload packet was sent.
        assert_eq!(b.debug_sent_bytes(), 0);
        Ok(())
    })
    .await
    .unwrap();
    drop(a);
    ha.stop();
    hb.stop();
}

#[tokio::test]
async fn product_valid_signature_with_wrong_ephemeral_dtls_fingerprint_cannot_open_payload_channels(
) {
    let ha = NetworkHost::start().await.unwrap();
    let hb = NetworkHost::start().await.unwrap();
    let (ia, ib, ab, ba) = identities();
    let nonce = "c".repeat(32);
    let nonce_b = nonce.clone();
    let (mut a, offer) = ha
        .execute(move |ctx| async move {
            let mut a = PendingConnection::new(
                ctx,
                ab.clone(),
                SessionRole::Offer,
                nonce.clone(),
                Arc::new(|| true),
                NetworkMode::Loopback,
            )
            .await?;
            let offer = a.local_description(&ia).await?;
            let value: serde_json::Value = serde_json::from_str(&offer.encode()?).unwrap();
            let fingerprint = std::iter::repeat_n("01", 32).collect::<Vec<_>>().join(":");
            let sdp = value["body"]["sdp"]
                .as_str()
                .unwrap()
                .lines()
                .map(|line| {
                    if line.starts_with("a=fingerprint:") {
                        format!("a=fingerprint:sha-256 {fingerprint}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n")
                + "\r\n";
            let description =
                serde_json::from_value(serde_json::json!({"type":"offer","sdp":sdp})).unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let signed =
                SignedDescription::create(&ia, &ab, SessionRole::Offer, &nonce, &description, now)?;
            Ok((a, signed))
        })
        .await
        .unwrap();
    let (b, answer) = hb
        .execute(move |ctx| async move {
            let mut b = PendingConnection::new(
                ctx,
                ba,
                SessionRole::Answer,
                nonce_b,
                Arc::new(|| true),
                NetworkMode::Loopback,
            )
            .await?;
            b.remote_description(&offer).await?;
            let answer = b.local_description(&ib).await?;
            Ok((b, answer))
        })
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        ha.execute(move |_| async move {
            a.remote_description(&answer).await?;
            a.connect().await
        }),
        hb.execute(move |_| async move { b.connect().await })
    );
    assert!(a.is_err());
    assert!(b.is_err()); // No Connection capability => no payload API.
    ha.stop();
    hb.stop();
}

#[tokio::test]
async fn stopped_account_runtime_joins_its_file_worker_before_replacement() {
    let host = NetworkHost::start().await.unwrap();
    let io = host.execute(|ctx| async move { Ok(ctx.io) }).await.unwrap();
    io.run(&Cancellation::default(), |io| io.check())
        .await
        .unwrap()
        .unwrap();
    host.stop_and_wait().await;
    assert_eq!(
        io.run(&Cancellation::default(), |_| ()).await,
        Err(Error::Closed)
    );
    assert_eq!(
        host.execute(|_| async move { Ok(()) }).await,
        Err(Error::Closed)
    );
    let replacement = NetworkHost::start().await.unwrap();
    replacement
        .execute(|ctx| async move {
            ctx.io
                .run(&Cancellation::default(), |io| io.check())
                .await?
        })
        .await
        .unwrap();
    replacement.stop_and_wait().await;
}
