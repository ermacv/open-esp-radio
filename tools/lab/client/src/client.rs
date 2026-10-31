//! A client of the service: the connection a command holds its lease by.
//! Dropping it ends the lease the way a lost client does.
//!
//! One reader thread sorts the service's frames: an operation's output,
//! loss reports, input credits and end go to that operation, every other
//! reply to the request waiting for it.

use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc},
};

use oer_lab_arbiter::claims::Claim;

use crate::protocol::{
    Frame, INPUT_WINDOW, LeaseRequest, Operation, PROTOCOL, Reply, Request, Status,
};

pub type Result<T> = std::result::Result<T, String>;

/// Output an operation may have waiting for its reader in this client; more
/// is dropped and counted, never waited for, so a slow reader never holds
/// the replies behind it.
const ROUTE_BUDGET: usize = 1 << 20;
/// How often a client waiting for its lease looks whether it was cancelled.
const WAIT_POLL: std::time::Duration = std::time::Duration::from_millis(200);

pub struct Client {
    stream: UnixStream,
    writer: Arc<Mutex<UnixStream>>,
    replies: mpsc::Receiver<Reply>,
    routes: Arc<Mutex<BTreeMap<u64, Route>>>,
    notices: Arc<Mutex<Notices>>,
    next_operation: u64,
    /// The client works under an enclosing command's lease.
    joined: bool,
    /// The lease was released and its end heard.
    released: bool,
    /// The token of the lease this client holds or joined, for the
    /// commands it runs.
    token: Option<String>,
}

/// What the service said about the lease outside the replies.
#[derive(Default)]
struct Notices {
    /// Why the service ended the lease.
    revoked: Option<String>,
    /// Why divisible work should yield.
    yield_reason: Option<String>,
}

/// What an operation's client receives.
#[derive(Debug)]
pub enum Event {
    Output(Vec<u8>),
    /// Output dropped for a slow reader, in bytes.
    Lost(u64),
    Finished(Result<serde_json::Value>),
}

/// A lease the client holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Granted {
    pub lease: u64,
    /// Lets a command inside the lease join it.
    pub token: String,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the reader sends an operation's frames.
struct Route {
    events: Arc<RouteQueue>,
    credits: Arc<Credits>,
}

/// What waits for an operation's reader: its output within the budget, the
/// output lost beyond it, and its end, which is never lost.
#[derive(Default)]
struct RouteQueue {
    state: Mutex<RouteState>,
    changed: Condvar,
}

#[derive(Default)]
struct RouteState {
    output: std::collections::VecDeque<Vec<u8>>,
    bytes: usize,
    lost: u64,
    finished: Option<Result<serde_json::Value>>,
    /// The service went away before the operation finished.
    closed: bool,
}

impl RouteQueue {
    fn output(&self, bytes: Vec<u8>) {
        let mut state = lock(&self.state);
        if state.bytes.saturating_add(bytes.len()) > ROUTE_BUDGET {
            state.lost = state.lost.saturating_add(bytes.len() as u64);
        } else {
            state.bytes += bytes.len();
            state.output.push_back(bytes);
        }
        self.changed.notify_all();
    }

    fn lost(&self, bytes: u64) {
        let mut state = lock(&self.state);
        state.lost = state.lost.saturating_add(bytes);
        self.changed.notify_all();
    }

    fn finish(&self, result: Result<serde_json::Value>) {
        lock(&self.state).finished = Some(result);
        self.changed.notify_all();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.changed.notify_all();
    }

    /// The next event: output first, then a loss, then the end.
    fn next(&self) -> Event {
        let mut state = lock(&self.state);
        loop {
            if let Some(bytes) = state.output.pop_front() {
                state.bytes = state.bytes.saturating_sub(bytes.len());
                return Event::Output(bytes);
            }
            if state.lost > 0 {
                return Event::Lost(std::mem::take(&mut state.lost));
            }
            if let Some(result) = state.finished.take() {
                return Event::Finished(result);
            }
            if state.closed {
                return Event::Finished(Err("the LAB service closed the connection".to_owned()));
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// The input frames an operation may still be sent, until it finished.
#[derive(Default)]
struct Credits {
    state: Mutex<(usize, bool)>,
    changed: Condvar,
}

impl Credits {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((INPUT_WINDOW, false)),
            changed: Condvar::new(),
        })
    }

    fn grant(&self) {
        let mut state = lock(&self.state);
        state.0 = state.0.saturating_add(1);
        self.changed.notify_all();
    }

    fn finish(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }

    /// Take one credit, waiting for the operation to return one; an error
    /// once it finished.
    fn take(&self) -> Result<()> {
        let mut state = lock(&self.state);
        loop {
            if state.1 {
                return Err("the operation finished".to_owned());
            }
            if state.0 > 0 {
                state.0 -= 1;
                return Ok(());
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Client {
    /// Connect to the installed service as `client`.
    pub fn open(client: &str) -> Result<Self> {
        let socket = oer_lab_model::paths::socket().map_err(|error| error.to_string())?;
        let stream = UnixStream::connect(&socket).map_err(|error| {
            format!(
                "the LAB service does not answer at {}: {error}; start it with `systemctl --user \
                 start oer-lab` or install it with `cargo stand install`",
                socket.display()
            )
        })?;
        Self::connect(stream, client)
    }

    /// Greet the service on `stream` as `client`; another protocol is
    /// refused with the service's reason.
    pub fn connect(stream: UnixStream, client: &str) -> Result<Self> {
        let failed = |error: io::Error| error.to_string();
        let writer = stream.try_clone().map_err(failed)?;
        let mut reader = stream.try_clone().map_err(failed)?;
        let (replies_in, replies) = mpsc::channel();
        let routes = Arc::new(Mutex::new(BTreeMap::<u64, Route>::new()));
        let notices = Arc::new(Mutex::new(Notices::default()));
        let (thread_routes, thread_notices) = (routes.clone(), notices.clone());
        std::thread::spawn(move || {
            while let Ok(Some(frame)) = Frame::read(&mut reader) {
                let reply = match frame {
                    Frame::Data { operation, bytes } => {
                        // Never waited for: the replies behind it go on.
                        let route = lock(&thread_routes)
                            .get(&operation)
                            .map(|route| route.events.clone());
                        if let Some(route) = route {
                            route.output(bytes);
                        }
                        continue;
                    }
                    Frame::Message(json) => match parse(&json) {
                        Ok(reply) => reply,
                        Err(_) => break,
                    },
                };
                match reply {
                    Reply::Lost { id, bytes } => {
                        let route = lock(&thread_routes)
                            .get(&id)
                            .map(|route| route.events.clone());
                        if let Some(route) = route {
                            route.lost(bytes);
                        }
                    }
                    Reply::Credit { id } => {
                        if let Some(route) = lock(&thread_routes).get(&id) {
                            route.credits.grant();
                        }
                    }
                    Reply::Finished { id, result } => {
                        let route = lock(&thread_routes).remove(&id);
                        if let Some(route) = route {
                            route.credits.finish();
                            route.events.finish(result);
                        }
                    }
                    Reply::Revoked { reason } => lock(&thread_notices).revoked = Some(reason),
                    Reply::Yield { reason } => lock(&thread_notices).yield_reason = Some(reason),
                    reply => {
                        if replies_in.send(reply).is_err() {
                            break;
                        }
                    }
                }
            }
            // The service is gone: every operation and request hears it.
            for (_, route) in std::mem::take(&mut *lock(&thread_routes)) {
                route.credits.finish();
                route.events.close();
            }
        });
        let mut this = Self {
            stream,
            writer: Arc::new(Mutex::new(writer)),
            replies,
            routes,
            notices,
            next_operation: 1,
            joined: false,
            released: false,
            token: None,
        };
        this.send(&Request::Hello {
            protocol: PROTOCOL,
            client: client.to_owned(),
        })?;
        match this.reply()? {
            Reply::Welcome { .. } => Ok(this),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to hello: {other:?}")),
        }
    }

    /// The token of the lease this client holds or joined: a command it
    /// runs gets it in [`crate::protocol::LEASE_ENV`] to join the lease for
    /// no more than this client holds.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Why the service ended the lease, if it did.
    pub fn revoked(&self) -> Option<String> {
        lock(&self.notices).revoked.clone()
    }

    /// Why divisible work should release its lease at the next boundary,
    /// if the service asked it to.
    pub fn yield_requested(&self) -> Option<String> {
        lock(&self.notices).yield_reason.clone()
    }

    /// End held lease `lease` for `reason`, on behalf of `by`.
    pub fn preempt(&mut self, lease: u64, by: &str, reason: &str) -> Result<()> {
        self.send(&Request::Preempt {
            lease,
            by: by.to_owned(),
            reason: reason.to_owned(),
        })?;
        match self.reply()? {
            Reply::Waiting { .. } => Ok(()),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to preempt: {other:?}")),
        }
    }

    /// Queue for a lease and wait for it; `waiting` sees each report. A
    /// command inside a lease (its token in [`crate::protocol::LEASE_ENV`])
    /// joins that lease instead, for the claims it covers.
    pub fn acquire(
        &mut self,
        lease: LeaseRequest,
        waiting: &mut dyn FnMut(&str),
    ) -> Result<Granted> {
        if let Some(token) = std::env::var(crate::protocol::LEASE_ENV)
            .ok()
            .filter(|token| !token.is_empty())
        {
            let granted = self.join(&token, lease.claims)?;
            self.joined = true;
            return Ok(granted);
        }
        self.send(&Request::Acquire { lease })?;
        loop {
            // A command cancelled while it waits stops waiting: dropping the
            // client takes its request out of the queue.
            let reply = match self.replies.recv_timeout(WAIT_POLL) {
                Ok(reply) => reply,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if oer_os_process::cancellation_requested() {
                        return Err("cancelled while waiting for the lease".to_owned());
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the LAB service closed the connection".to_owned());
                }
            };
            match reply {
                Reply::Waiting { message, .. } => waiting(&message),
                Reply::Granted { lease, token } => {
                    self.token = Some(token.clone());
                    return Ok(Granted { lease, token });
                }
                Reply::Refused { reason } => return Err(reason),
                other => return Err(format!("unexpected reply to acquire: {other:?}")),
            }
        }
    }

    /// Work under the lease `token` names, for `claims` it holds.
    pub fn join(&mut self, token: &str, claims: Vec<Claim>) -> Result<Granted> {
        self.send(&Request::Join {
            token: token.to_owned(),
            claims,
        })?;
        match self.reply()? {
            Reply::Granted { lease, token } => {
                self.token = Some(token.clone());
                Ok(Granted { lease, token })
            }
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to join: {other:?}")),
        }
    }

    /// Finish the lease: the service admits no more operations, waits until
    /// its operations stopped, recovers its resources and reports each.
    /// An error names every resource left unavailable and why: the run that
    /// held it did not end clean. A joined lease stays with the command
    /// that holds it.
    pub fn release(&mut self) -> Result<()> {
        if self.joined || self.released {
            return Ok(());
        }
        self.send(&Request::Release)?;
        self.released = true;
        self.ended()
    }

    /// Wait until the lease ended and its resources' recoveries ended; an
    /// error names each resource left unavailable.
    pub fn ended(&mut self) -> Result<()> {
        loop {
            match self.reply()? {
                Reply::Ended { unavailable } if unavailable.is_empty() => return Ok(()),
                Reply::Ended { unavailable } => {
                    return Err(format!(
                        "the lease ended with {} left unavailable",
                        unavailable
                            .iter()
                            .map(|(resource, reason)| format!("{resource} ({reason})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                Reply::Refused { reason } => return Err(reason),
                _ => {}
            }
        }
    }

    pub fn status(&mut self) -> Result<Status> {
        self.send(&Request::Status)?;
        loop {
            if let Reply::Status { status } = self.reply()? {
                return Ok(status);
            }
        }
    }

    /// Send an administration `request` and wait until it was served;
    /// whether it changed anything.
    pub fn administer(&mut self, request: Request) -> Result<bool> {
        self.send(&request)?;
        match self.reply()? {
            Reply::Done { changed } => Ok(changed),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply: {other:?}")),
        }
    }

    /// The boards attached to the host, as the service finds them.
    pub fn inventory(&mut self) -> Result<Vec<crate::protocol::AttachedBoard>> {
        self.send(&Request::Inventory)?;
        match self.reply()? {
            Reply::Inventory { boards } => Ok(boards),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to inventory: {other:?}")),
        }
    }

    /// `uhubctl`'s report of the stand's hubs.
    pub fn hubs(&mut self) -> Result<String> {
        self.send(&Request::Hubs)?;
        match self.reply()? {
            Reply::Hubs { report } => Ok(report),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to hubs: {other:?}")),
        }
    }

    /// The stand's full report.
    pub fn report(&mut self) -> Result<oer_lab_arbiter::Status> {
        self.send(&Request::Report)?;
        match self.reply()? {
            Reply::Report { status } => Ok(*status),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to report: {other:?}")),
        }
    }

    /// Ask the service to recover unavailable `resource` again.
    pub fn return_resource(&mut self, resource: &str) -> Result<()> {
        self.send(&Request::Return {
            resource: resource.to_owned(),
        })?;
        match self.reply()? {
            Reply::Waiting { .. } => Ok(()),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to return: {other:?}")),
        }
    }

    /// Start `operation` under the lease.
    pub fn start(&mut self, operation: Operation) -> Result<Running> {
        let id = self.next_operation;
        self.next_operation += 1;
        let events = Arc::new(RouteQueue::default());
        let credits = Credits::new();
        lock(&self.routes).insert(
            id,
            Route {
                events: events.clone(),
                credits: credits.clone(),
            },
        );
        if let Err(error) = self.send(&Request::Start { id, operation }) {
            lock(&self.routes).remove(&id);
            return Err(error);
        }
        Ok(Running {
            id,
            events,
            writer: self.writer.clone(),
            credits,
            pending: Vec::new(),
            finished: None,
        })
    }

    /// Run `operation` under the lease: its output goes to `output`, `input`
    /// feeds it until its end. Lost output is reported on `output`, never
    /// hidden.
    pub fn run(
        &mut self,
        operation: Operation,
        output: &mut dyn Write,
        input: Option<Box<dyn Read + Send>>,
    ) -> Result<serde_json::Value> {
        let running = self.start(operation)?;
        if let Some(mut input) = input {
            let feed = running.input();
            std::thread::spawn(move || {
                let mut buffer = vec![0u8; 16 * 1024];
                while let Ok(read) = input.read(&mut buffer) {
                    let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
                        break;
                    };
                    if feed.send(chunk).is_err() {
                        return;
                    }
                }
                let _ = feed.close();
            });
        }
        running.finish(output)
    }

    /// Send `request` as it is; its reply comes to the next wait.
    #[cfg(feature = "testing")]
    pub fn send_raw(&self, request: &Request) -> Result<()> {
        send(&self.writer, request)
    }

    fn send(&self, request: &Request) -> Result<()> {
        send(&self.writer, request)
    }

    /// The next reply not routed to an operation.
    fn reply(&mut self) -> Result<Reply> {
        self.replies
            .recv()
            .map_err(|_| "the LAB service closed the connection".to_owned())
    }
}

fn send(writer: &Mutex<UnixStream>, request: &Request) -> Result<()> {
    let frame = Frame::message(request).map_err(|error| error.to_string())?;
    frame
        .write(&mut *lock(writer))
        .map_err(|error| format!("the LAB service is unreachable: {error}"))
}

pub(crate) fn parse(json: &[u8]) -> Result<Reply> {
    serde_json::from_slice(json).map_err(|error| format!("unreadable reply: {error}"))
}

impl Client {
    /// Close the connection the way a dying client does: without
    /// releasing its lease.
    #[cfg(feature = "testing")]
    pub fn sever(mut self) {
        self.joined = true;
    }
}

impl Drop for Client {
    /// A client that ends releases its lease; one that dies without this
    /// leaves it abandoned. Either way the service stops its operations
    /// and recovers its resources.
    fn drop(&mut self) {
        if !self.joined && !self.released {
            let _ = self.send(&Request::Release);
        }
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// An operation the service runs for this client: a duplex stream of its
/// output and input until it finishes.
pub struct Running {
    id: u64,
    events: Arc<RouteQueue>,
    writer: Arc<Mutex<UnixStream>>,
    credits: Arc<Credits>,
    /// Output received but not read yet.
    pending: Vec<u8>,
    finished: Option<Result<serde_json::Value>>,
}

/// The input side of a running operation, for a feeding thread.
pub struct Input {
    id: u64,
    writer: Arc<Mutex<UnixStream>>,
    credits: Arc<Credits>,
}

impl Input {
    /// Send `bytes`, one frame per credit: it waits while the operation
    /// has not taken what was sent before, and fails once it finished.
    pub fn send(&self, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(crate::protocol::MAX_FRAME / 2) {
            self.credits.take()?;
            Frame::Data {
                operation: self.id,
                bytes: chunk.to_vec(),
            }
            .write(&mut *lock(&self.writer))
            .map_err(|error| format!("the LAB service is unreachable: {error}"))?;
        }
        Ok(())
    }

    /// The input ended.
    pub fn close(&self) -> Result<()> {
        send(&self.writer, &Request::CloseInput { id: self.id })
    }

    /// Ask the service to stop the operation.
    pub fn cancel(&self) -> Result<()> {
        send(&self.writer, &Request::Cancel { id: self.id })
    }
}

impl Running {
    pub fn input(&self) -> Input {
        Input {
            id: self.id,
            writer: self.writer.clone(),
            credits: self.credits.clone(),
        }
    }

    /// Ask the service to stop the operation; its end still follows.
    pub fn cancel(&self) -> Result<()> {
        send(&self.writer, &Request::Cancel { id: self.id })
    }

    /// The next event; an operation whose service went away finished with
    /// that error.
    pub fn event(&mut self) -> Event {
        self.events.next()
    }

    /// Copy the output to `output` until the operation finishes.
    pub fn finish(mut self, output: &mut dyn Write) -> Result<serde_json::Value> {
        output
            .write_all(&self.pending)
            .map_err(|error| error.to_string())?;
        self.pending.clear();
        if let Some(result) = self.finished.take() {
            return result;
        }
        let mut lost = 0u64;
        loop {
            match self.event() {
                Event::Output(bytes) => {
                    output
                        .write_all(&bytes)
                        .map_err(|error| error.to_string())?;
                }
                Event::Lost(bytes) => lost = lost.saturating_add(bytes),
                Event::Finished(result) => {
                    if lost > 0 {
                        let _ = writeln!(
                            output,
                            "lab-service: {lost} bytes of output were lost to a slow reader"
                        );
                    }
                    return result;
                }
            }
        }
    }
}

impl Read for Running {
    /// The operation's output; the end of the stream once it finished. Lost
    /// output is an error, never a silent gap.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            if self.finished.is_some() {
                return Ok(0);
            }
            match self.event() {
                Event::Output(bytes) => self.pending = bytes,
                Event::Lost(bytes) => {
                    return Err(io::Error::other(format!(
                        "{bytes} bytes of output were lost to a slow reader"
                    )));
                }
                Event::Finished(result) => self.finished = Some(result),
            }
        }
        let count = buffer.len().min(self.pending.len());
        let rest = self.pending.split_off(count);
        buffer
            .get_mut(..count)
            .ok_or_else(|| io::Error::other("short buffer"))?
            .copy_from_slice(&self.pending);
        self.pending = rest;
        Ok(count)
    }
}

impl Write for Running {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.input().send(bytes).map_err(io::Error::other)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
