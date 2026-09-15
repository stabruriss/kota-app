//! Explicit account runtime. Construction starts nothing; start is used only
//! after joining, and dropping/stopping the host cancels its connections.
use super::{background_thread, Cancellation, Error, FileIo, Limits, Result};
use std::{cell::Cell, future::Future, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Semaphore};
thread_local! {static NETWORK_THREAD: Cell<bool> = const {Cell::new(false)};}
pub(super) fn require_network_thread() -> Result<()> {
    if NETWORK_THREAD.with(Cell::get) {
        Ok(())
    } else {
        Err(Error::Runtime)
    }
}
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) io: FileIo,
    pub(crate) limits: Limits,
    pub(crate) shutdown: Cancellation,
}
struct HostInner {
    handle: tokio::runtime::Handle,
    context: Context,
    admission: Arc<Semaphore>,
    finished: tokio::sync::watch::Receiver<bool>,
    owns_io: bool,
}
impl Drop for HostInner {
    fn drop(&mut self) {
        self.context.shutdown.cancel();
    }
}
#[derive(Clone)]
pub(crate) struct NetworkHost(Arc<HostInner>);
impl NetworkHost {
    pub(crate) async fn start() -> Result<Self> {
        Self::start_inner(None).await
    }
    /// Production shares the independent local roster/file service. Retiring a
    /// P2P host must not disable identity-free local names or avatar reads.
    pub(crate) async fn start_with_files(io: FileIo, limits: Limits) -> Result<Self> {
        Self::start_inner(Some((io, limits))).await
    }
    async fn start_inner(files: Option<(FileIo, Limits)>) -> Result<Self> {
        let owns_io = files.is_none();
        let (tx, rx) = oneshot::channel();
        let shutdown = Cancellation::default();
        let worker_stop = shutdown.clone();
        let (finished_tx, finished) = tokio::sync::watch::channel(false);
        // Starting a thread is bounded and does no file/network work on IPC.
        std::thread::Builder::new()
            .name("kota-bbs-network".into())
            .spawn(move || {
                struct Finished(tokio::sync::watch::Sender<bool>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        self.0.send_replace(true);
                    }
                }
                let _finished = Finished(finished_tx);
                let setup = (|| {
                    background_thread()?;
                    NETWORK_THREAD.with(|flag| flag.set(true));
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .on_thread_start(|| {
                            let _ = background_thread();
                        })
                        .build()
                        .map_err(|_| Error::Runtime)?;
                    let (io, limits) = match files {
                        Some(pair) => pair,
                        None => {
                            let limits = Limits::default();
                            (FileIo::start(limits.clone())?, limits)
                        }
                    };
                    Ok((
                        runtime,
                        Context {
                            io,
                            limits,
                            shutdown: worker_stop.clone(),
                        },
                    ))
                })();
                match setup {
                    Ok((runtime, context)) => {
                        if tx.send(Ok((runtime.handle().clone(), context))).is_err() {
                            return;
                        }
                        runtime.block_on(async {
                            worker_stop.cancelled().await;
                            // Give stream-reset and close tasks their bounded cleanup
                            // window. This wait runs on this background thread only.
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                    }
                }
            })
            .map_err(|_| Error::Runtime)?;
        // If start() is abandoned while the thread starts, tell it to terminate.
        struct Starting(Option<Cancellation>);
        impl Drop for Starting {
            fn drop(&mut self) {
                if let Some(cancel) = &self.0 {
                    cancel.cancel();
                }
            }
        }
        let mut guard = Starting(Some(shutdown));
        let (handle, context) = rx.await.map_err(|_| Error::Runtime)??;
        let host = Self(Arc::new(HostInner {
            handle,
            context,
            admission: Arc::new(Semaphore::new(8)),
            finished,
            owns_io,
        }));
        guard.0.take(); // The shared stop token is now owned by HostInner.
        Ok(host)
    }
    pub(crate) fn stop(&self) {
        self.0.context.shutdown.cancel();
    }
    /// Used by the background owner before replacing this account runtime.
    /// Cancellation itself remains immediate; waiting never runs on IPC.
    pub(crate) async fn stop_and_wait(&self) {
        self.stop();
        let mut finished = self.0.finished.clone();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
        if self.0.owns_io {
            self.0.context.io.stop_and_wait().await;
        }
    }
    /// At most eight accepted operations. Their crypto, ICE and parsing are
    /// polled only on the owned background runtime; dropping an await aborts it.
    pub(crate) async fn execute<F, Fut, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce(Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        if self.0.context.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        let permit = self
            .0
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let context = self.0.context.clone();
        let shutdown = context.shutdown.clone();
        let join = self.0.handle.spawn(async move {
            let _permit = permit;
            tokio::select! {result = operation(context) => result, _ = shutdown.cancelled() => Err(Error::Cancelled)}
        });
        struct Abort<T>(tokio::task::JoinHandle<T>);
        impl<T> Drop for Abort<T> {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let mut task = Abort(join);
        (&mut task.0).await.map_err(|_| Error::Runtime)?
    }
}
