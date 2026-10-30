//! The service: one owner of the stand's queue, its leases and the
//! operations that run on the leased hardware.
//!
//! A lease belongs to the connection that acquired it. When the lease ends
//! (released, revoked, or its connection lost) the service stops its
//! operations and waits until each one has returned; only then do its
//! resources go to their drivers' recovery, and only a confirmed recovery
//! lets the scheduler grant them again. A lost client therefore releases
//! nothing while its hardware work still runs.

use std::{
    collections::{BTreeMap, VecDeque},
    io,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use oer_lab_arbiter::{
    LeaseOutcome, Priority,
    claims::{Claim, covers, normalize},
    process::ProcessIdentity,
    readiness::{Confirmation, Recovery},
    scheduler::{Queued, Scheduler},
};

use crate::protocol::{
    Frame, HolderStatus, LeaseRequest, Operation, PROTOCOL, Reply, Request, Status,
};

/// Output an operation may have queued for its reader; more is dropped and
/// reported as lost.
const OUTPUT_BUDGET: usize = 1 << 20;
/// Input chunks an operation may have pending.
const INPUT_DEPTH: usize = 64;

/// The hardware the service drives: each kind of resource's recovery and
/// every operation.
pub trait Drivers: Send + Sync + 'static {
    /// Bring `recovery`'s resource to an end state its kind declares and
    /// confirm it; within the kind's deadline.
    fn recover(&self, recovery: &Recovery) -> Confirmation;
    /// Run `operation` for a lease holding `claims`. It returns soon after
    /// `io.stop` is set and only once the hardware it touched is no longer
    /// in use.
    fn run(
        &self,
        operation: &Operation,
        claims: &[Claim],
        io: OperationIo,
    ) -> Result<serde_json::Value, String>;
}

/// What an operation exchanges with its client.
pub struct OperationIo {
    pub output: Output,
    /// Input chunks; the channel ends when the client closes the input.
    pub input: mpsc::Receiver<Vec<u8>>,
    pub stop: Arc<AtomicBool>,
}

/// An operation's output stream to its client.
pub struct Output {
    outbox: Arc<Outbox>,
    operation: u64,
    lost: Arc<Mutex<u64>>,
}

impl Output {
    /// Queue `bytes` for the client; when the client reads too slowly they
    /// are dropped and counted, never waited for.
    pub fn write(&self, bytes: &[u8]) {
        for chunk in bytes.chunks(crate::protocol::MAX_FRAME / 2) {
            let frame = Frame::Data {
                operation: self.operation,
                bytes: chunk.to_vec(),
            };
            if !self.outbox.push_data(frame, chunk.len()) {
                let mut lost = lock(&self.lost);
                *lost = lost.saturating_add(chunk.len() as u64);
            }
        }
    }
}

/// The frames waiting for one connection's writer.
struct Outbox {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Frame>,
    data_bytes: usize,
    closed: bool,
}

impl Outbox {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
        })
    }

    /// Queue a reply; replies are few per lease and operation, so they are
    /// never dropped.
    fn push(&self, reply: &Reply) {
        let Ok(frame) = Frame::message(reply) else {
            return;
        };
        let mut queue = lock(&self.queue);
        if !queue.closed {
            queue.frames.push_back(frame);
            self.ready.notify_all();
        }
    }

    /// Queue output within the budget; false when it was dropped.
    fn push_data(&self, frame: Frame, length: usize) -> bool {
        let mut queue = lock(&self.queue);
        if queue.closed {
            // Nobody reads any more: nothing is lost to a reader.
            return true;
        }
        if queue.data_bytes.saturating_add(length) > OUTPUT_BUDGET {
            return false;
        }
        queue.data_bytes += length;
        queue.frames.push_back(frame);
        self.ready.notify_all();
        true
    }

    fn close(&self) {
        lock(&self.queue).closed = true;
        self.ready.notify_all();
    }

    /// Write queued frames to `stream` until the outbox closes and drains.
    fn drain(&self, mut stream: UnixStream) {
        loop {
            let frame = {
                let mut queue = lock(&self.queue);
                loop {
                    if let Some(frame) = queue.frames.pop_front() {
                        if let Frame::Data { bytes, .. } = &frame {
                            queue.data_bytes = queue.data_bytes.saturating_sub(bytes.len());
                        }
                        break Some(frame);
                    }
                    if queue.closed {
                        break None;
                    }
                    queue = self
                        .ready
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            let Some(frame) = frame else {
                let _ = stream.shutdown(std::net::Shutdown::Write);
                return;
            };
            if frame.write(&mut stream).is_err() {
                // The client is gone: its reader sees the end too.
                let _ = stream.shutdown(std::net::Shutdown::Both);
                self.close();
                return;
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The service's time, in Unix milliseconds.
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

/// A running service.
#[derive(Clone)]
pub struct Service {
    shared: Arc<Shared>,
}

struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
    drivers: Arc<dyn Drivers>,
    clock: Clock,
    /// The persisted scheduler state.
    state_path: PathBuf,
    /// The lease history.
    history_path: PathBuf,
}

struct Inner {
    scheduler: Scheduler,
    /// The connection waiting for each ticket.
    tickets: BTreeMap<u64, u64>,
    leases: BTreeMap<u64, Lease>,
    connections: BTreeMap<u64, Arc<Outbox>>,
    next_connection: u64,
    next_token: u64,
}

struct Lease {
    connection: u64,
    token: String,
    owner: String,
    work: String,
    claims: Vec<Claim>,
    /// Why it ends, once it ends.
    ending: Option<LeaseOutcome>,
    /// The connections of commands that joined it.
    joined: std::collections::BTreeSet<u64>,
    /// Running operations by connection and identity.
    operations: BTreeMap<(u64, u64), Running>,
}

struct Running {
    stop: Arc<AtomicBool>,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
}

impl Service {
    /// Resume the service whose state lives in `directory`, for the stand's
    /// stateful resources `stand`: leases it held end as abandoned, and every
    /// resource left without a confirmed end state is recovered before it is
    /// granted.
    pub fn open(
        directory: &std::path::Path,
        stand: Vec<String>,
        drivers: Arc<dyn Drivers>,
        clock: Clock,
    ) -> oer_lab_arbiter::Result<Self> {
        let state_path = directory.join("service.json");
        let history_path = directory.join("history.jsonl");
        let (scheduler, resumed) = Scheduler::open(&state_path, stand, clock())?;
        scheduler.save(&state_path)?;
        for record in &resumed.abandoned {
            oer_lab_arbiter::history::append(&history_path, record)?;
        }
        let service = Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    scheduler,
                    tickets: BTreeMap::new(),
                    leases: BTreeMap::new(),
                    connections: BTreeMap::new(),
                    next_connection: 1,
                    next_token: 1,
                }),
                changed: Condvar::new(),
                drivers,
                clock,
                state_path,
                history_path,
            }),
        };
        for recovery in resumed.recoveries {
            service.spawn_recovery(recovery);
        }
        Ok(service)
    }

    /// Serve every client `listener` accepts, until it fails.
    pub fn serve(&self, listener: std::os::unix::net::UnixListener) -> io::Result<()> {
        for stream in listener.incoming() {
            self.attach(stream?)?;
        }
        Ok(())
    }

    /// A client connection to this service in the same process.
    pub fn connect(&self) -> io::Result<UnixStream> {
        let (client, served) = UnixStream::pair()?;
        self.attach(served)?;
        Ok(client)
    }

    fn attach(&self, stream: UnixStream) -> io::Result<()> {
        let writer = stream.try_clone()?;
        let outbox = Outbox::new();
        let id = {
            let mut inner = lock(&self.shared.inner);
            let id = inner.next_connection;
            inner.next_connection += 1;
            inner.connections.insert(id, outbox.clone());
            id
        };
        let drain = outbox.clone();
        std::thread::spawn(move || drain.drain(writer));
        let service = self.clone();
        std::thread::spawn(move || service.read(id, stream, &outbox));
        Ok(())
    }

    /// The service's state.
    pub fn status(&self) -> Status {
        status(&lock(&self.shared.inner))
    }

    /// Block until `done` holds for the service's state.
    pub fn wait_until(&self, done: impl Fn(&Status) -> bool) {
        let mut inner = lock(&self.shared.inner);
        while !done(&status(&inner)) {
            inner = self
                .shared
                .changed
                .wait(inner)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Read connection `id`'s requests until it ends, then let go of what
    /// it held.
    fn read(&self, id: u64, mut stream: UnixStream, outbox: &Outbox) {
        let mut greeted = false;
        while let Ok(Some(frame)) = Frame::read(&mut stream) {
            match frame {
                Frame::Data { operation, bytes } => {
                    // Input beyond the operation's depth is dropped: the
                    // client sent it faster than the operation reads.
                    let inner = lock(&self.shared.inner);
                    if let Some(input) =
                        running(&inner, id, operation).and_then(|running| running.input.as_ref())
                    {
                        let _ = input.try_send(bytes);
                    }
                }
                Frame::Message(json) => {
                    let request = match serde_json::from_slice::<Request>(&json) {
                        Ok(request) => request,
                        Err(error) => {
                            outbox.push(&Reply::Refused {
                                reason: format!("unreadable request: {error}"),
                            });
                            break;
                        }
                    };
                    if !greeted {
                        match request {
                            Request::Hello { protocol, .. } if protocol == PROTOCOL => {
                                greeted = true;
                                outbox.push(&Reply::Welcome { protocol: PROTOCOL });
                                continue;
                            }
                            Request::Hello { protocol, .. } => {
                                outbox.push(&Reply::Refused {
                                    reason: format!(
                                        "this client speaks protocol {protocol}, the installed \
                                         service {PROTOCOL}: update the client or reinstall the \
                                         service (`cargo stand install`)"
                                    ),
                                });
                            }
                            _ => outbox.push(&Reply::Refused {
                                reason: "the first request must be hello".to_owned(),
                            }),
                        }
                        break;
                    }
                    self.handle(id, request, outbox);
                }
            }
        }
        self.disconnected(id);
        outbox.close();
    }

    /// Serve one request of connection `connection`.
    fn handle(&self, connection: u64, request: Request, outbox: &Outbox) {
        match request {
            Request::Hello { .. } => outbox.push(&Reply::Refused {
                reason: "hello was already sent".to_owned(),
            }),
            Request::Acquire { lease } => self.acquire(connection, lease, outbox),
            Request::Join { token, claims } => self.join(connection, &token, &claims, outbox),
            Request::Release => {
                let mut inner = lock(&self.shared.inner);
                match own_lease(&inner, connection) {
                    Some(lease) => self.end(&mut inner, lease, LeaseOutcome::Released, None),
                    None => outbox.push(&Reply::Refused {
                        reason: "this connection holds no lease".to_owned(),
                    }),
                }
            }
            Request::Start { id, operation } => self.start(connection, id, operation, outbox),
            Request::Cancel { id } => {
                if let Some(running) = running(&lock(&self.shared.inner), connection, id) {
                    running.stop.store(true, Ordering::SeqCst);
                }
            }
            Request::CloseInput { id } => {
                let mut inner = lock(&self.shared.inner);
                if let Some(running) = running_mut(&mut inner, connection, id) {
                    running.input = None;
                }
            }
            Request::Status => outbox.push(&Reply::Status {
                status: self.status(),
            }),
            Request::Return { resource } => {
                let recovery = lock(&self.shared.inner).scheduler.release(&resource);
                match recovery {
                    Some(recovery) => {
                        self.persist(&lock(&self.shared.inner));
                        self.spawn_recovery(recovery);
                        outbox.push(&Reply::Waiting {
                            ticket: 0,
                            message: format!("{resource} is being recovered"),
                        });
                    }
                    None => outbox.push(&Reply::Refused {
                        reason: format!("{resource} is not unavailable"),
                    }),
                }
            }
        }
    }

    fn acquire(&self, connection: u64, request: LeaseRequest, outbox: &Outbox) {
        let mut inner = lock(&self.shared.inner);
        let busy = inner.tickets.values().any(|holder| *holder == connection)
            || own_lease(&inner, connection).is_some();
        if busy {
            outbox.push(&Reply::Refused {
                reason: "this connection already waits for or holds a lease".to_owned(),
            });
            return;
        }
        let ticket = inner.scheduler.enqueue(
            Queued {
                owner: request.owner,
                work: request.work,
                scenarios: request.scenarios,
                run: request.run,
                claims: request.claims,
                priority: if request.maintenance.is_some() {
                    Priority::Maintenance
                } else {
                    Priority::Ordinary
                },
                estimate_secs: 0,
                client: ProcessIdentity {
                    pid: 0,
                    start_ticks: 0,
                },
            },
            (self.shared.clock)() / 1000,
        );
        inner.tickets.insert(ticket, connection);
        outbox.push(&Reply::Waiting {
            ticket,
            message: format!("queued as #{ticket}"),
        });
        self.pump(&mut inner);
    }

    fn join(&self, connection: u64, token: &str, claims: &[Claim], outbox: &Outbox) {
        let mut inner = lock(&self.shared.inner);
        let Some((id, lease)) = inner
            .leases
            .iter_mut()
            .find(|(_, lease)| lease.token == token && lease.ending.is_none())
        else {
            outbox.push(&Reply::Refused {
                reason: "the token names no lease that is still held".to_owned(),
            });
            return;
        };
        if !covers(&lease.claims, &normalize(claims)) {
            outbox.push(&Reply::Refused {
                reason: "the enclosing lease does not hold what this command claims".to_owned(),
            });
            return;
        }
        lease.joined.insert(connection);
        outbox.push(&Reply::Granted {
            lease: *id,
            token: lease.token.clone(),
        });
    }

    fn start(&self, connection: u64, id: u64, operation: Operation, outbox: &Outbox) {
        let mut inner = lock(&self.shared.inner);
        let Some(lease_id) = serving_lease(&inner, connection) else {
            outbox.push(&Reply::Finished {
                id,
                result: Err("this connection holds no lease".to_owned()),
            });
            return;
        };
        let Some(outbox) = inner.connections.get(&connection).cloned() else {
            return;
        };
        let Some(lease) = inner.leases.get_mut(&lease_id) else {
            return;
        };
        if lease.ending.is_some() || lease.operations.contains_key(&(connection, id)) {
            outbox.push(&Reply::Finished {
                id,
                result: Err("the lease is ending or the operation exists".to_owned()),
            });
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (input, receiver) = mpsc::sync_channel(INPUT_DEPTH);
        lease.operations.insert(
            (connection, id),
            Running {
                stop: stop.clone(),
                input: Some(input),
            },
        );
        let claims = lease.claims.clone();
        drop(inner);
        self.shared.changed.notify_all();
        let service = self.clone();
        let lost = Arc::new(Mutex::new(0));
        let io = OperationIo {
            output: Output {
                outbox: outbox.clone(),
                operation: id,
                lost: lost.clone(),
            },
            input: receiver,
            stop,
        };
        std::thread::spawn(move || {
            let result = service.shared.drivers.run(&operation, &claims, io);
            let lost = *lock(&lost);
            if lost > 0 {
                outbox.push(&Reply::Lost { id, bytes: lost });
            }
            outbox.push(&Reply::Finished { id, result });
            service.finished(lease_id, connection, id);
        });
    }

    /// Operation `id` of `connection` under `lease` returned.
    fn finished(&self, lease: u64, connection: u64, id: u64) {
        let mut inner = lock(&self.shared.inner);
        let Some(held) = inner.leases.get_mut(&lease) else {
            return;
        };
        held.operations.remove(&(connection, id));
        if let Some(outcome) = held.ending
            && held.operations.is_empty()
        {
            self.release(&mut inner, lease, outcome);
        }
        self.shared.changed.notify_all();
    }

    /// Connection `connection` ended: its ticket leaves the queue, its
    /// lease ends, and the operations it started under another lease stop.
    fn disconnected(&self, connection: u64) {
        let mut inner = lock(&self.shared.inner);
        inner.connections.remove(&connection);
        let tickets = inner
            .tickets
            .iter()
            .filter(|(_, holder)| **holder == connection)
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        for ticket in tickets {
            inner.tickets.remove(&ticket);
            inner.scheduler.leave(ticket);
        }
        if let Some(lease) = own_lease(&inner, connection) {
            self.end(&mut inner, lease, LeaseOutcome::Abandoned, None);
        }
        for lease in inner.leases.values_mut() {
            lease.joined.remove(&connection);
            for ((owner, _), running) in &mut lease.operations {
                if *owner == connection {
                    running.stop.store(true, Ordering::SeqCst);
                    running.input = None;
                }
            }
        }
        self.persist(&inner);
        self.pump(&mut inner);
    }

    /// End lease `lease` with `outcome`; `revoked` tells its client why the
    /// service ended it. Its operations stop first.
    fn end(&self, inner: &mut Inner, lease: u64, outcome: LeaseOutcome, revoked: Option<String>) {
        let Some(held) = inner.leases.get_mut(&lease) else {
            return;
        };
        if held.ending.is_some() {
            return;
        }
        held.ending = Some(outcome);
        for running in held.operations.values_mut() {
            running.stop.store(true, Ordering::SeqCst);
            running.input = None;
        }
        let connection = held.connection;
        let idle = held.operations.is_empty();
        if let (Some(reason), Some(outbox)) = (revoked, inner.connections.get(&connection)) {
            outbox.push(&Reply::Revoked { reason });
        }
        if idle {
            self.release(inner, lease, outcome);
        }
        self.shared.changed.notify_all();
    }

    /// Lease `lease`'s operations all returned: end it in the scheduler and
    /// recover its resources.
    fn release(&self, inner: &mut Inner, lease: u64, outcome: LeaseOutcome) {
        let Some(held) = inner.leases.remove(&lease) else {
            return;
        };
        let Some(ended) = inner.scheduler.end(lease, outcome, (self.shared.clock)()) else {
            return;
        };
        self.persist(inner);
        if let Err(error) =
            oer_lab_arbiter::history::append(&self.shared.history_path, &ended.record)
        {
            eprintln!("lab-service: cannot record lease #{lease}: {error}");
        }
        if let Some(outbox) = inner.connections.get(&held.connection) {
            outbox.push(&Reply::Ended);
        }
        for recovery in ended.recoveries {
            self.spawn_recovery(recovery);
        }
        self.pump(inner);
    }

    fn spawn_recovery(&self, recovery: Recovery) {
        let service = self.clone();
        std::thread::spawn(move || {
            let confirmation = service.shared.drivers.recover(&recovery);
            let mut inner = lock(&service.shared.inner);
            if inner.scheduler.confirm(&recovery, confirmation) {
                service.persist(&inner);
            }
            service.pump(&mut inner);
        });
    }

    /// Grant what can be granted and tell the waiting clients.
    fn pump(&self, inner: &mut Inner) {
        let now = (self.shared.clock)();
        let mut next_token = inner.next_token;
        let granted = inner.scheduler.grant(now, &mut || {
            next_token += 1;
            format!("{now:x}-{next_token:x}")
        });
        inner.next_token = next_token;
        if !granted.is_empty() {
            self.persist(inner);
        }
        for grant in granted {
            let Some(connection) = inner.tickets.remove(&grant.lease) else {
                continue;
            };
            let Some(holder) = inner
                .scheduler
                .holder(grant.lease)
                .map(|holder| (holder.0.to_owned(), holder.1.to_owned(), holder.2.to_vec()))
            else {
                continue;
            };
            inner.leases.insert(
                grant.lease,
                Lease {
                    connection,
                    token: grant.token.clone(),
                    owner: holder.0,
                    work: holder.1,
                    claims: holder.2,
                    ending: None,
                    joined: Default::default(),
                    operations: BTreeMap::new(),
                },
            );
            match inner.connections.get(&connection) {
                Some(outbox) => outbox.push(&Reply::Granted {
                    lease: grant.lease,
                    token: grant.token,
                }),
                // The client left between its grant and this notice.
                None => self.end(inner, grant.lease, LeaseOutcome::Abandoned, None),
            }
        }
        self.shared.changed.notify_all();
    }

    fn persist(&self, inner: &Inner) {
        if let Err(error) = inner.scheduler.save(&self.shared.state_path) {
            eprintln!("lab-service: cannot persist its state: {error}");
        }
    }
}

/// Running operation `id` of connection `connection`.
fn running(inner: &Inner, connection: u64, id: u64) -> Option<&Running> {
    inner
        .leases
        .values()
        .find_map(|lease| lease.operations.get(&(connection, id)))
}

fn running_mut(inner: &mut Inner, connection: u64, id: u64) -> Option<&mut Running> {
    inner
        .leases
        .values_mut()
        .find_map(|lease| lease.operations.get_mut(&(connection, id)))
}

/// The lease connection `connection` acquired.
fn own_lease(inner: &Inner, connection: u64) -> Option<u64> {
    inner
        .leases
        .iter()
        .find(|(_, lease)| lease.connection == connection && lease.ending.is_none())
        .map(|(id, _)| *id)
}

/// The lease connection `connection` works under: its own, or the one it
/// joined.
fn serving_lease(inner: &Inner, connection: u64) -> Option<u64> {
    own_lease(inner, connection).or_else(|| {
        inner
            .leases
            .iter()
            .find(|(_, lease)| lease.ending.is_none() && lease.joined.contains(&connection))
            .map(|(id, _)| *id)
    })
}

fn status(inner: &Inner) -> Status {
    Status {
        holders: inner
            .leases
            .iter()
            .map(|(lease, held)| HolderStatus {
                lease: *lease,
                owner: held.owner.clone(),
                work: held.work.clone(),
                ending: held.ending.is_some(),
                operations: held.operations.len(),
            })
            .collect(),
        waiting: inner.tickets.keys().copied().collect(),
        resources: inner.scheduler.resources(),
    }
}

#[cfg(test)]
mod tests;
