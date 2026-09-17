use super::*;

async fn count(status: &mut watch::Receiver<usize>, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while *status.borrow_and_update() != expected {
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("actual work ownership changes without polling");
}

#[tokio::test]
async fn work_status_is_memory_only_and_counts_actual_permit_waiters_until_admission_or_cancel() {
    let io = FileIo::start(Limits::default()).unwrap();
    let mut status = io.work_status();
    let held = io.limits.file().unwrap();
    let first = Cancellation::default();
    let second = Cancellation::default();
    for _ in 0..100 {
        assert_eq!(*io.work_status().borrow(), 0);
    }
    assert!(!status.has_changed().unwrap());
    // Creating but not polling a roster future is not a real waiter.
    let idle = io.run_when_available(&first, |_| 0);
    assert_eq!(*status.borrow(), 0);
    drop(idle);
    let waiting = tokio::spawn({
        let (io, cancel) = (io.clone(), first.clone());
        async move {
            io.run_when_available(&cancel, |_| panic!("cancelled waiter ran"))
                .await
        }
    });
    let round = tokio::spawn({
        let (io, cancel) = (io.clone(), second.clone());
        async move { io.run_in_round(&cancel, |_| 7).await }
    });
    count(&mut status, 2).await;
    assert_eq!(io.activity(), 0);
    assert!(matches!(io.run(&first, |_| ()).await, Err(Error::Busy)));
    assert_eq!(
        *status.borrow(),
        2,
        "failed try-admission leaves no fake job"
    );
    first.cancel();
    assert_eq!(waiting.await.unwrap(), Err(Error::Cancelled));
    count(&mut status, 1).await;
    drop(held);
    assert_eq!(round.await.unwrap(), Ok(7));
    count(&mut status, 0).await;
    assert_eq!(
        io.activity(),
        0,
        "waiting/empty metadata did not invent I/O bytes"
    );
    io.stop_and_wait().await;
}

#[tokio::test]
async fn cancelled_awaiter_does_not_hide_file_work_still_running_on_the_original_worker() {
    let io = FileIo::start(Limits::default()).unwrap();
    let mut status = io.work_status();
    let cancel = Cancellation::default();
    let (entered, started) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let pending = tokio::spawn({
        let (io, cancel) = (io.clone(), cancel.clone());
        async move {
            io.run(&cancel, move |local| {
                entered.send(()).unwrap();
                let _ = blocked.recv(); // Simulated blocking file operation.
                local.check()
            })
            .await
        }
    });
    started.await.unwrap();
    count(&mut status, 1).await;
    cancel.cancel();
    assert_eq!(pending.await.unwrap(), Err(Error::Cancelled));
    assert_eq!(*status.borrow(), 1, "the worker still owns the operation");
    assert_eq!(io.activity(), 0);
    assert!(matches!(
        io.run(&Cancellation::default(), |_| ()).await,
        Err(Error::Busy)
    ));
    release.send(()).unwrap();
    count(&mut status, 0).await;
    assert_eq!(
        io.run_in_round(&Cancellation::default(), |_| 9).await,
        Ok(9)
    );
    count(&mut status, 0).await;
    io.stop_and_wait().await;
}

#[tokio::test]
async fn idle_handles_and_verified_files_are_not_work_but_each_real_io_operation_is_observed() {
    let root = Root::new();
    let bytes = vec![31; 128];
    let spec = resource(&bytes);
    let source = root.0.join("source");
    fs::write(&source, &bytes).unwrap();
    let io = FileIo::start(Limits::default()).unwrap();
    let mut status = io.work_status();
    let cancel = Cancellation::default();
    let mut reader = io.read(spec.clone(), &source, &cancel).await.unwrap();
    assert!(status.has_changed().unwrap(), "open was accounted");
    count(&mut status, 0).await;
    let chunk = reader.next().await.unwrap().unwrap();
    assert!(status.has_changed().unwrap(), "read was accounted");
    count(&mut status, 0).await;
    assert_eq!(&chunk.bytes[..], &bytes);
    assert_eq!(
        *status.borrow(),
        0,
        "retained chunks/handles are not I/O jobs"
    );
    drop(chunk);
    assert!(reader.next().await.unwrap().is_none());
    count(&mut status, 0).await;
    drop(reader);
    let mut writer = io.receive(spec.clone(), &root.0, &cancel).await.unwrap();
    count(&mut status, 0).await;
    assert_eq!(
        *status.borrow(),
        0,
        "waiting for a network frame is not disk work"
    );
    writer.write(0, BytesMut::from(&bytes[..])).await.unwrap();
    assert!(status.has_changed().unwrap(), "write was accounted");
    count(&mut status, 0).await;
    let verified = writer.finish(spec.sha256).await.unwrap();
    assert!(status.has_changed().unwrap(), "fsync was accounted");
    count(&mut status, 0).await;
    assert!(io.limits.file().is_err());
    assert_eq!(
        *status.borrow(),
        0,
        "verified file owns a permit, not a running job"
    );
    let partial = verified.path().to_owned();
    drop(verified);
    assert!(status.has_changed().unwrap(), "cleanup was queued");
    io.run_in_round(&cancel, |_| ()).await.unwrap();
    count(&mut status, 0).await;
    assert!(!partial.exists());
    io.stop_and_wait().await;
}

#[tokio::test]
async fn queued_cleanup_remains_work_and_full_queue_rejection_does_not_leak_a_work_count() {
    let root = Root::new();
    let first = root.0.join("first");
    let second = root.0.join("second");
    fs::write(&first, b"temporary").unwrap();
    fs::write(&second, b"temporary").unwrap();
    let io = FileIo::start(Limits::default()).unwrap();
    let mut status = io.work_status();
    let (entered, started) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let local = tokio::spawn({
        let io = io.clone();
        async move {
            io.run(&Cancellation::default(), move |_| {
                entered.send(()).unwrap();
                let _ = blocked.recv();
            })
            .await
        }
    });
    started.await.unwrap();
    io.remove_later(first.clone());
    count(&mut status, 2).await;
    io.remove_later(second.clone()); // Existing bounded queue refuses this job.
    assert_eq!(*status.borrow(), 2);
    assert!(first.exists() && second.exists());
    release.send(()).unwrap();
    local.await.unwrap().unwrap();
    count(&mut status, 0).await;
    assert!(!first.exists());
    assert!(second.exists());
    assert_eq!(io.activity(), 0);
    io.stop_and_wait().await;
}
