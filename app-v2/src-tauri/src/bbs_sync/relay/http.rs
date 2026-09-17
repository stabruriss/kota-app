//! Fixed blocking HTTPS workers. One lane is reserved for ACK/probe/receipts; large
//! uploads can never occupy both workers. A cancelled caller returns at once,
//! while an uninterruptible system DNS call keeps its original worker/bytes.
use super::{
    destination::Destination,
    envelope::{self, Metadata, Packet},
    proof::SignedRequest,
    upload::{Body, Reader as BodyReader},
    window::{Buffer, Payload},
    Error, Result, MAX_ENVELOPE,
};
use crate::bbs_sync::transport::{
    background_thread, BytePermit, Cancellation, Limits, MembershipCheck, MAX_FRAME,
    PROGRESS_TIMEOUT,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::Read,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::Instant,
};
use tokio::sync::oneshot;

const QUEUED_PER_LANE: usize = 4;
// Two maximum small requests (body + response + both header charges).
const SMALL_RESERVE: usize = 8 * MAX_ENVELOPE;
// Covers the pinned ureq/std finite copy buffer while streaming send(Read).
// It is additional to the immutable ciphertext and released after HTTP exits.
const SEND_SCRATCH: usize = MAX_FRAME;
pub(super) const SEND_ADMISSION_BYTES: usize =
    super::proof::MAX_JSON + 2 * MAX_ENVELOPE + SEND_SCRATCH;
pub(super) const RECEIVE_ADMISSION_BYTES: usize = super::MAX_BATCH + 3 * MAX_ENVELOPE;
#[derive(Clone, Copy)]
enum Lane {
    Small,
    Data,
}

pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) headers: BTreeMap<String, Vec<String>>,
    pub(super) body: Option<Payload>,
    received: Option<Metadata>,
    _headers: BytePermit,
}
impl Reply {
    pub(super) fn error(&self) -> Error {
        super::response::rejected(
            self.status,
            &self.headers,
            self.body.as_ref().map_or(&[], Payload::bytes),
        )
    }
    pub(super) fn json(&self) -> Result<&[u8]> {
        super::response::json(
            self.status,
            &self.headers,
            self.body.as_ref().map_or(&[], Payload::bytes),
        )
    }
    pub(super) fn into_receive(self) -> Result<Packet> {
        let Self {
            headers,
            body,
            received,
            _headers,
            ..
        } = self;
        drop(headers);
        received
            .ok_or(Error::Protocol)?
            .into_packet(body.ok_or(Error::Protocol)?, _headers)
    }
}
struct Parts {
    status: u16,
    headers: BTreeMap<String, Vec<String>>,
}
struct Guard {
    cancel: Cancellation,
    binding: Cancellation,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    abandoned: Cancellation,
    shutdown: Cancellation,
    authorized: MembershipCheck,
    deadline: Instant,
}
impl Guard {
    fn check(&self) -> Result<()> {
        if self.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if self.cancel.is_cancelled()
            || self.binding.is_cancelled()
            || self.abandoned.is_cancelled()
            || self.generation.load(Ordering::Acquire) != self.expected_generation
        {
            return Err(Error::Cancelled);
        }
        if !(self.authorized)() {
            return Err(Error::Unauthorized);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }
}
struct Job {
    id: u64,
    endpoint: Arc<Endpoint>,
    request: SignedRequest,
    body: Option<Body>,
    response: Buffer,
    guard: Guard,
    reply: oneshot::Sender<Result<Reply>>,
    headers: BytePermit,
    _stream: Option<BytePermit>,
}
#[derive(Default)]
struct State {
    closed: bool,
    discard: bool,
    jobs: VecDeque<Job>,
}
enum Work {
    Discard,
    Request(Job),
}
#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    wake: Condvar,
}
impl Queue {
    fn push(&self, job: Job) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Error::Closed)?;
        if state.closed {
            return Err(Error::Closed);
        }
        if state.jobs.len() == QUEUED_PER_LANE {
            return Err(Error::Busy);
        }
        state.jobs.push_back(job);
        self.wake.notify_one();
        Ok(())
    }
    fn pop(&self) -> Option<Work> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }
            if std::mem::take(&mut state.discard) {
                return Some(Work::Discard);
            }
            if let Some(job) = state.jobs.pop_front() {
                return Some(Work::Request(job));
            }
            state = self.wake.wait(state).ok()?;
        }
    }
    fn remove(&self, id: u64) {
        let removed = self.state.lock().ok().and_then(|mut s| {
            s.jobs
                .iter()
                .position(|j| j.id == id)
                .and_then(|p| s.jobs.remove(p))
        });
        drop(removed);
    }
    fn retire(&self, endpoint: &Arc<Endpoint>) {
        if let Ok(mut state) = self.state.lock() {
            // At most four jobs, and Drop only releases permits/oneshots.
            state
                .jobs
                .retain(|job| !Arc::ptr_eq(&job.endpoint, endpoint));
            // A coalesced wake, not another request/queue slot or a timer. An
            // idle worker must drop the old keep-alive Agent even with no new job.
            state.discard = true;
            self.wake.notify_one();
        }
    }
    fn close(&self) {
        let jobs = if let Ok(mut s) = self.state.lock() {
            s.closed = true;
            std::mem::take(&mut s.jobs)
        } else {
            VecDeque::new()
        };
        self.wake.notify_all();
        drop(jobs);
    }
}
struct Ticket {
    id: u64,
    queue: Arc<Queue>,
    abandoned: Cancellation,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.abandoned.cancel();
        self.queue.remove(self.id);
    }
}
struct Inner {
    small: Arc<Queue>,
    data: Arc<Queue>,
    shutdown: Cancellation,
    next: AtomicU64,
    limits: Limits,
    small_limits: Limits,
    active: Mutex<Option<Arc<Endpoint>>>,
    generation: Arc<AtomicU64>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.small.close();
        self.data.close();
    }
}

#[derive(Clone)]
pub(crate) struct HttpPool(Arc<Inner>);
struct Endpoint {
    generation: u64,
    destination: Destination,
    retired: Cancellation,
}
/// A capability for one owner-selected origin/work epoch. Cloning it does not
/// acquire a new epoch; an A -> B -> A rebind never reauthorizes the old A handle.
#[derive(Clone)]
pub(crate) struct HttpClient {
    pool: HttpPool,
    endpoint: Arc<Endpoint>,
}
trait Backend: Send {
    fn discard(&mut self);
    fn run(
        &mut self,
        endpoint: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        response: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts>;
}
impl HttpPool {
    /// The account owner retains this pool across work epochs; it must not make
    /// another pool to replace a still-blocked DNS/HTTPS worker after Cancel.
    pub(crate) async fn start(limits: Limits) -> Result<Self> {
        Self::start_with(limits, |_| Box::new(Https::default())).await
    }
    async fn start_with(
        limits: Limits,
        make: impl Fn(Lane) -> Box<dyn Backend> + Send + Sync + 'static,
    ) -> Result<Self> {
        let pool = Self(Arc::new(Inner {
            small: Arc::new(Queue::default()),
            data: Arc::new(Queue::default()),
            shutdown: Cancellation::default(),
            next: AtomicU64::new(1),
            small_limits: limits.partition(SMALL_RESERVE)?,
            limits,
            active: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
        }));
        let make = Arc::new(make);
        let mut readiness = Vec::with_capacity(2);
        for (lane, queue, name) in [
            (Lane::Small, pool.0.small.clone(), "kota-bbs-https-small"),
            (Lane::Data, pool.0.data.clone(), "kota-bbs-https-data"),
        ] {
            let make = make.clone();
            let (ready, receive) = oneshot::channel();
            readiness.push(receive);
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    if let Err(e) = background_thread() {
                        let _ = ready.send(Err(e));
                        return;
                    }
                    let mut backend = make(lane);
                    if ready.send(Ok(())).is_err() {
                        return;
                    }
                    while let Some(work) = queue.pop() {
                        let mut job = match work {
                            Work::Discard => {
                                backend.discard();
                                continue;
                            }
                            Work::Request(job) => job,
                        };
                        let result = (|| {
                            job.guard.check()?;
                            let parts = backend.run(
                                &job.endpoint,
                                &job.request,
                                job.body.as_ref(),
                                &mut job.response,
                                &job.guard,
                            )?;
                            job.guard.check()?;
                            let body = if job.response.len() == 0 {
                                None
                            } else {
                                Some(job.response.freeze()?)
                            };
                            let received = receive_metadata(&job.request, &parts, body.as_ref());
                            let received = received?;
                            job.guard.check()?;
                            Ok(Reply {
                                status: parts.status,
                                headers: parts.headers,
                                body,
                                received,
                                _headers: job.headers,
                            })
                        })();
                        if result.is_err() {
                            // Includes a successful HTTP response from a retired
                            // binding: neither its result nor its pool is reused.
                            backend.discard();
                        }
                        let _ = job.reply.send(result);
                    }
                })
                .map_err(|_| Error::Runtime)?;
        }
        for ready in readiness {
            ready.await.map_err(|_| Error::Runtime)??;
        }
        Ok(pool)
    }
    /// Called by the account owner from its configured origin, not peer input.
    /// Always creates a fresh epoch, even for the same origin. No DNS or HTTP.
    pub(crate) fn bind(&self, origin: &str) -> Result<HttpClient> {
        let destination = Destination::new(origin)?;
        let mut active = self.0.active.lock().map_err(|_| Error::Closed)?;
        if self.0.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        let generation = self
            .0
            .generation
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or(Error::Runtime)?;
        let endpoint = Arc::new(Endpoint {
            generation,
            destination,
            retired: Cancellation::default(),
        });
        self.0.generation.store(generation, Ordering::Release);
        if let Some(old) = active.replace(endpoint.clone()) {
            old.retired.cancel();
            self.0.small.retire(&old);
            self.0.data.retire(&old);
        }
        Ok(HttpClient {
            pool: self.clone(),
            endpoint,
        })
    }
    /// Retire current work without shutting down/replacing the account workers.
    pub(crate) fn unbind(&self) {
        if let Ok(mut active) = self.0.active.lock() {
            if let Some(old) = active.take() {
                old.retired.cancel();
                self.0.small.retire(&old);
                self.0.data.retire(&old);
            }
        }
    }
    pub(crate) fn stop(&self) {
        self.0.shutdown.cancel();
        self.unbind();
        self.0.small.close();
        self.0.data.close();
    }
}
/// Owns response/header headroom before TLS ciphertext is drained into blocks.
/// Dropping it before enqueue releases that headroom without any network call.
/// The original deadline includes filling, queueing and the blocking request.
pub(super) struct SendAdmission {
    client: HttpClient,
    guard: Guard,
    response: Buffer,
    headers: BytePermit,
    stream: Option<BytePermit>,
    small: bool,
    response_limit: usize,
    send_only: bool,
}
/// Owns a job only after bounded queue admission. This boundary lets the
/// ciphertext window record submitted() before a concurrent receive can return
/// the recipient's ACK, without guessing whether a spawned future was polled.
pub(super) struct PendingHttp {
    client: HttpClient,
    cancel: Cancellation,
    authorized: MembershipCheck,
    deadline: Instant,
    receive: oneshot::Receiver<Result<Reply>>,
    _ticket: Ticket,
}
impl HttpClient {
    /// A route-specific decoder found a malformed successful response. Retire
    /// pooled sockets without retiring this binding or its queued work. An old
    /// decoder cannot discard a newer origin's agents.
    pub(super) fn checked<T>(&self, value: Result<T>) -> Result<T> {
        if matches!(value, Err(Error::Protocol | Error::Integrity)) {
            if let Ok(_active) = self.pool.0.active.lock() {
                if self.check_binding().is_ok() {
                    for queue in [&self.pool.0.small, &self.pool.0.data] {
                        if let Ok(mut state) = queue.state.lock() {
                            state.discard = true;
                            queue.wake.notify_one();
                        }
                    }
                }
            }
        }
        value
    }
    pub(super) async fn retired(&self) {
        tokio::select! {
            _ = self.endpoint.retired.cancelled() => {},
            _ = self.pool.0.shutdown.cancelled() => {},
        }
    }
    pub(super) async fn wait_for_small_bytes(&self, minimum: usize) -> Result<()> {
        self.pool.0.small_limits.wait_for_bytes(minimum).await
    }
    pub(super) fn prepare_ack(
        &self,
        request: &SignedRequest,
        cancel: Cancellation,
        authorized: MembershipCheck,
        deadline: Instant,
    ) -> Result<SendAdmission> {
        if !request.small_request() || request.receive_request().is_some() {
            return Err(Error::Protocol);
        }
        self.admission(
            true,
            request.response_limit(),
            false,
            cancel,
            authorized,
            deadline,
        )
    }
    pub(super) fn check_binding(&self) -> Result<()> {
        if self.pool.0.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if self.endpoint.retired.is_cancelled()
            || self.pool.0.generation.load(Ordering::Acquire) != self.endpoint.generation
        {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    pub(super) async fn execute(
        &self,
        request: SignedRequest,
        body: Option<Body>,
        cancel: Cancellation,
        authorized: MembershipCheck,
        deadline: Instant,
    ) -> Result<Reply> {
        self.admission(
            request.small_request(),
            request.response_limit(),
            request.is_send(),
            cancel,
            authorized,
            deadline,
        )?
        .execute(request, body)
        .await
    }
    /// Reserve BEFORE constructing a send batch, so retained ciphertext cannot
    /// consume the bytes required to submit itself. No HTTP/timer is started.
    pub(super) fn prepare_send(
        &self,
        cancel: Cancellation,
        authorized: MembershipCheck,
        deadline: Instant,
    ) -> Result<SendAdmission> {
        self.admission(
            false,
            super::proof::MAX_JSON,
            true,
            cancel,
            authorized,
            deadline,
        )
    }
    /// A finite signed receive reserves its response before bounded enqueue.
    /// The read scheduler can distinguish byte admission from queue admission.
    pub(super) fn prepare_receive(
        &self,
        request: &SignedRequest,
        cancel: Cancellation,
        authorized: MembershipCheck,
        deadline: Instant,
    ) -> Result<SendAdmission> {
        request.receive_request().ok_or(Error::Protocol)?;
        self.admission(
            request.small_request(),
            request.response_limit(),
            false,
            cancel,
            authorized,
            deadline,
        )
    }
    fn admission(
        &self,
        small: bool,
        response_limit: usize,
        send_only: bool,
        cancel: Cancellation,
        authorized: MembershipCheck,
        deadline: Instant,
    ) -> Result<SendAdmission> {
        let guard = Guard {
            cancel,
            binding: self.endpoint.retired.clone(),
            generation: self.pool.0.generation.clone(),
            expected_generation: self.endpoint.generation,
            abandoned: Cancellation::default(),
            shutdown: self.pool.0.shutdown.clone(),
            authorized,
            deadline: deadline.min(Instant::now() + PROGRESS_TIMEOUT),
        };
        guard.check()?;
        let limits = if small {
            &self.pool.0.small_limits
        } else {
            &self.pool.0.limits
        };
        let response = Buffer::new(limits, response_limit)?;
        // Both queued proof headers and the returned parsed headers retain a
        // conservative charge; cancelled in-flight requests keep it until exit.
        let headers = limits.reserve(2 * MAX_ENVELOPE)?;
        let stream = send_only
            .then(|| limits.reserve(SEND_SCRATCH))
            .transpose()?;
        guard.check()?;
        Ok(SendAdmission {
            client: self.clone(),
            guard,
            response,
            headers,
            stream,
            small,
            response_limit,
            send_only,
        })
    }
    /// ACK construction uses the reserved small lane too: it must not need a
    /// new general-payload permit while every file/ciphertext byte is occupied.
    pub(super) fn small_body(&self, bytes: &[u8]) -> Result<Payload> {
        self.check_binding()?;
        if bytes.is_empty() || bytes.len() > MAX_ENVELOPE {
            return Err(Error::Protocol);
        }
        let mut buffer = Buffer::new(&self.pool.0.small_limits, bytes.len())?;
        buffer.spare_mut().copy_from_slice(bytes);
        buffer.advance(bytes.len())?;
        buffer.freeze()
    }
    /// Metadata queued by the coordinator belongs to the same account budget.
    /// It cannot consume the ACK/probe reservation, even for a tiny JSON body.
    pub(super) fn metadata_body(&self, bytes: &[u8]) -> Result<Payload> {
        self.check_binding()?;
        if bytes.is_empty() || bytes.len() > super::proof::MAX_JSON {
            return Err(Error::Protocol);
        }
        let mut buffer = Buffer::new(&self.pool.0.limits, bytes.len())?;
        buffer.spare_mut().copy_from_slice(bytes);
        buffer.advance(bytes.len())?;
        buffer.freeze()
    }
}

impl SendAdmission {
    pub(super) fn deadline(&self) -> Instant {
        self.guard.deadline
    }
    pub(super) fn check(&self) -> Result<()> {
        self.guard.check()
    }
    pub(super) async fn execute(self, request: SignedRequest, body: Option<Body>) -> Result<Reply> {
        self.submit(request, body)?.wait().await
    }
    pub(super) fn submit(self, request: SignedRequest, body: Option<Body>) -> Result<PendingHttp> {
        let Self {
            client,
            guard,
            response,
            headers,
            stream,
            small,
            response_limit,
            send_only,
        } = self;
        guard.check()?;
        client.endpoint.destination.validate_url(&request.url())?;
        if (send_only && !request.is_send())
            || request.small_request() != small
            || request.response_limit() != response_limit
            || !request.matches_upload(body.as_ref())
            || (request.method() == "GET" && body.as_ref().is_some_and(|p| p.len() != 0))
            || (small && body.as_ref().is_some_and(|p| p.len() > MAX_ENVELOPE))
        {
            return Err(Error::Protocol);
        }
        let cancel = guard.cancel.clone();
        let authorized = guard.authorized.clone();
        let deadline = guard.deadline;
        let queue = if small {
            &client.pool.0.small
        } else {
            &client.pool.0.data
        };
        let id = client.pool.0.next.fetch_add(1, Ordering::Relaxed);
        if id == u64::MAX {
            return Err(Error::Runtime);
        }
        let (reply, receive) = oneshot::channel();
        let ticket = Ticket {
            id,
            queue: queue.clone(),
            abandoned: guard.abandoned.clone(),
        };
        let job = Job {
            id,
            endpoint: client.endpoint.clone(),
            request,
            body,
            response,
            guard,
            reply,
            headers,
            _stream: stream,
        };
        {
            // Serialize admission with bind/unbind so no old job can queue
            // behind the retirement sweep. Never hold this lock across I/O.
            let active = client.pool.0.active.lock().map_err(|_| Error::Closed)?;
            job.guard.check()?;
            if !active
                .as_ref()
                .is_some_and(|e| e.generation == client.endpoint.generation)
            {
                return Err(Error::Cancelled);
            }
            queue.push(job)?;
        }
        Ok(PendingHttp {
            client,
            cancel,
            authorized,
            deadline,
            receive,
            _ticket: ticket,
        })
    }
}
impl PendingHttp {
    pub(super) async fn wait(self) -> Result<Reply> {
        let Self {
            client,
            cancel,
            authorized,
            deadline,
            receive,
            _ticket,
        } = self;
        let result = tokio::select! {
            _=cancel.cancelled()=>Err(Error::Cancelled),
            _=client.endpoint.retired.cancelled()=>Err(Error::Cancelled),
            _=client.pool.0.shutdown.cancelled()=>Err(Error::Closed),
            response=tokio::time::timeout_at(deadline.into(),receive)=>response
                .map_err(|_|Error::Timeout)
                .and_then(|r|r.map_err(|_|Error::Closed))
                .and_then(|r|r),
        };
        // A response racing revocation/Cancel has no authority in the new epoch.
        if client.pool.0.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if cancel.is_cancelled()
            || client.endpoint.retired.is_cancelled()
            || client.pool.0.generation.load(Ordering::Acquire) != client.endpoint.generation
        {
            return Err(Error::Cancelled);
        }
        if !(authorized)() {
            return Err(Error::Unauthorized);
        }
        result
    }
}

#[derive(Default)]
struct Https {
    selected: Option<(u64, Destination, ureq::Agent)>,
    // Only isolated HTTPS tests may substitute their own root/resolver. The
    // production constructor and binary expose no such configuration surface.
    #[cfg(test)]
    test_agent: Option<Arc<dyn Fn(Destination) -> ureq::Agent + Send + Sync>>,
}
fn receive_metadata(
    request: &SignedRequest,
    parts: &Parts,
    body: Option<&Payload>,
) -> Result<Option<Metadata>> {
    let Some(expected) = request.receive_request() else {
        return Ok(None);
    };
    // The common error decoder, not the binary codec, handles non-200 bodies.
    if parts.status != 200 {
        return Ok(None);
    }
    let singleton = |name: &str, value: &str| {
        parts
            .headers
            .get(name)
            .is_some_and(|v| v.len() == 1 && v[0] == value)
    };
    if !singleton("content-type", envelope::CONTENT_TYPE)
        || (parts.headers.contains_key("content-encoding")
            && !singleton("content-encoding", "identity"))
    {
        return Err(Error::Protocol);
    }
    Metadata::parse(expected, body.ok_or(Error::Protocol)?).map(Some)
}
impl Https {
    fn agent(destination: Destination) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .https_only(true)
            .try_proxy_from_env(false)
            .redirects(0)
            .timeout(PROGRESS_TIMEOUT)
            .max_idle_connections(1)
            .max_idle_connections_per_host(1)
            .resolver(destination)
            .build()
    }
    fn make_agent(&self, destination: Destination) -> ureq::Agent {
        #[cfg(test)]
        if let Some(make) = &self.test_agent {
            return make(destination);
        }
        Self::agent(destination)
    }
    fn request(
        &mut self,
        endpoint: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        buffer: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts> {
        guard.check()?;
        let url = request.url();
        let destination = &endpoint.destination;
        destination.validate_url(&url)?;
        if !self
            .selected
            .as_ref()
            .is_some_and(|(generation, current, _)| {
                *generation == endpoint.generation && current == destination
            })
        {
            self.selected = Some((
                endpoint.generation,
                destination.clone(),
                self.make_agent(destination.clone()),
            ));
        }
        guard.check()?;
        let mut http = self
            .selected
            .as_ref()
            .ok_or(Error::Closed)?
            .2
            .request(request.method(), &url)
            .set("content-type", request.content_type())
            .set("accept-encoding", "identity");
        // Only the reviewed proof encoder creates the relay-prefixed headers.
        for (key, value) in request.headers()? {
            http = http.set(&key, &value);
        }
        let remaining = guard
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or(Error::Timeout)?;
        let response = if request.method() == "GET" {
            http.timeout(remaining).call()
        } else {
            let http = http
                .timeout(remaining)
                .set("content-length", &body.map_or(0, Body::len).to_string());
            match body {
                Some(body) => http.send(GuardedBody {
                    reader: body.reader(),
                    guard,
                }),
                None => http.send_bytes(&[]),
            }
        };
        let response = match response {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(error) => {
                guard.check()?;
                return Err(request_error(&error));
            }
        };
        read_response(response, request.response_limit(), buffer, guard)
    }
}

struct GuardedBody<'a> {
    reader: BodyReader<'a>,
    guard: &'a Guard,
}
impl Read for GuardedBody<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.guard
            .check()
            .map_err(|_| std::io::Error::other("relay request retired"))?;
        self.reader.read(bytes)
    }
}

fn request_error(error: &ureq::Error) -> Error {
    if matches!(
        error.kind(),
        ureq::ErrorKind::BadHeader | ureq::ErrorKind::BadStatus | ureq::ErrorKind::TooManyRedirects
    ) {
        return Error::Protocol;
    }
    // This is the pinned local ureq parser's exact exception, not a remote
    // response-body keyword or a Cloudflare quota classifier. Oversized lines
    // can fail inside ureq before a Response exists for our 8 KiB check.
    if let Some(source) =
        std::error::Error::source(error).and_then(|s| s.downcast_ref::<std::io::Error>())
    {
        if source.kind() == std::io::ErrorKind::Other
            && source.to_string() == "header field longer than 102400 bytes"
        {
            return Error::Protocol;
        }
    }
    Error::Closed
}

fn read_response(
    response: ureq::Response,
    limit: usize,
    buffer: &mut Buffer,
    guard: &Guard,
) -> Result<Parts> {
    guard.check()?;
    let status = response.status();
    if (300..400).contains(&status) {
        return Err(Error::Protocol);
    }
    let mut size = response.http_version().len() + response.status_text().len() + 9;
    let mut headers = BTreeMap::new();
    for name in response
        .headers_names()
        .into_iter()
        .collect::<BTreeSet<_>>()
    {
        let values = response.all(&name);
        if values.is_empty() {
            return Err(Error::Protocol);
        }
        for value in &values {
            size += name.len() + value.len() + 4;
        }
        if size > MAX_ENVELOPE {
            return Err(Error::Protocol);
        }
        headers.insert(name, values.into_iter().map(str::to_owned).collect());
    }
    if size > MAX_ENVELOPE {
        return Err(Error::Protocol);
    }
    // Error pages never get the larger binary envelope allowance. Even a
    // receive request caps non-200 bodies at the JSON limit before parsing.
    let limit = if status == 200 {
        limit
    } else {
        limit.min(super::proof::MAX_JSON)
    };
    read_body(
        response.into_reader(),
        limit,
        buffer,
        guard,
        (200..300).contains(&status),
    )?;
    Ok(Parts { status, headers })
}
fn read_body(
    reader: impl Read,
    limit: usize,
    buffer: &mut Buffer,
    guard: &Guard,
    success: bool,
) -> Result<()> {
    if buffer.len() != 0 || buffer.spare_mut().len() < limit {
        return Err(Error::Protocol);
    }
    let mut reader = reader.take(limit as u64 + u64::from(success));
    while buffer.len() < limit {
        guard.check()?;
        let remaining = limit - buffer.len();
        let count = reader
            .read(&mut buffer.spare_mut()[..remaining])
            .map_err(|_| {
                guard.check().err().unwrap_or(if success {
                    Error::Closed
                } else {
                    Error::CloudflareResourceLimit
                })
            })?;
        if count == 0 {
            return Ok(());
        }
        buffer.advance(count)?;
    }
    guard.check()?;
    if !success {
        // Do not read/drain a platform page beyond the route's allowance or
        // parse a possibly truncated JSON prefix. Returning an error discards
        // this worker's Agent as well as the response reader/connection.
        return Err(Error::CloudflareResourceLimit);
    }
    if reader
        .read(&mut [0u8; 1])
        .map_err(|_| guard.check().err().unwrap_or(Error::Closed))?
        != 0
    {
        return Err(Error::Protocol);
    }
    Ok(())
}
impl Backend for Https {
    fn discard(&mut self) {
        self.selected = None;
    }
    fn run(
        &mut self,
        endpoint: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        response: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts> {
        let result = self.request(endpoint, request, body, response, guard);
        if result.is_err() || result.as_ref().is_ok_and(|parts| parts.status != 200) {
            // ureq may already have pooled a zero-length response before our
            // header check. Replacing this worker's sole agent drops that pool
            // too; error/oversized responses can never reuse its connection.
            self.discard();
        }
        result
    }
}

#[cfg(test)]
mod tests;
