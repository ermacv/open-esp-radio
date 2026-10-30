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
    HARD_LIMIT, LeaseOutcome, Priority,
    balance::MIN_SLICE,
    claims::{Claim, covers, normalize},
    process::ProcessIdentity,
    readiness::{Confirmation, Recovery},
    scheduler::{Due, Queued, Scheduler},
};

use crate::protocol::{
    Frame, HolderStatus, LeaseRequest, Operation, PROTOCOL, Reply, Request, Status,
};

/// Output an operation may have queued for its reader; more is dropped and
/// reported as lost.
const OUTPUT_BUDGET: usize = 1 << 20;
/// Input chunks an operation may have pending.
const INPUT_DEPTH: usize = 64;
/// How long a driver has to confirm a recovery: the longest reset ladder,
/// a hub power cycle with the board's return to USB included.
const RECOVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);

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
    /// The directory of the stand's records, and its stand file.
    arbiter: oer_lab_arbiter::Arbiter,
    recovery_deadline: std::time::Duration,
}

struct Inner {
    scheduler: Scheduler,
    /// The connection waiting for each ticket, and whether its work is
    /// divisible.
    tickets: BTreeMap<u64, (u64, bool)>,
    /// The owner and reason of each waiting maintenance ticket.
    maintenance: BTreeMap<u64, (String, String)>,
    leases: BTreeMap<u64, Lease>,
    /// Each connection's outbox and its client's process.
    connections: BTreeMap<u64, Arc<Outbox>>,
    clients: BTreeMap<u64, ProcessIdentity>,
    /// Resources whose driver still runs a recovery, also one past its
    /// deadline: nothing else touches them until it returns.
    recovering: std::collections::BTreeSet<String>,
    next_connection: u64,
    next_token: u64,
    /// No more grants: the service is being replaced.
    draining: bool,
}

struct Lease {
    connection: u64,
    token: String,
    owner: String,
    work: String,
    claims: Vec<Claim>,
    divisible: bool,
    /// Whether it was asked to yield.
    yield_asked: bool,
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
    /// Resume the service whose state lives in `arbiter`'s directory, for
    /// the stand's stateful resources `stand`: leases it held end as
    /// abandoned, and every resource left without a confirmed end state is
    /// recovered before it is granted.
    pub fn open(
        arbiter: oer_lab_arbiter::Arbiter,
        stand: Vec<String>,
        drivers: Arc<dyn Drivers>,
        clock: Clock,
    ) -> oer_lab_arbiter::Result<Self> {
        Self::open_with(arbiter, stand, drivers, clock, RECOVERY_DEADLINE)
    }

    fn open_with(
        arbiter: oer_lab_arbiter::Arbiter,
        stand: Vec<String>,
        drivers: Arc<dyn Drivers>,
        clock: Clock,
        recovery_deadline: std::time::Duration,
    ) -> oer_lab_arbiter::Result<Self> {
        let state_path = arbiter.state_path();
        let history_path = arbiter.history_path();
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
                    maintenance: BTreeMap::new(),
                    leases: BTreeMap::new(),
                    connections: BTreeMap::new(),
                    clients: BTreeMap::new(),
                    recovering: Default::default(),
                    next_connection: 1,
                    draining: false,
                    next_token: 1,
                }),
                changed: Condvar::new(),
                drivers,
                clock,
                state_path,
                history_path,
                arbiter,
                recovery_deadline,
            }),
        };
        {
            let mut inner = lock(&service.shared.inner);
            for recovery in resumed.recoveries {
                service.spawn_recovery(&mut inner, recovery);
            }
        }
        Ok(service)
    }

    /// Serve every client `listener` accepts, until it fails.
    pub fn serve(&self, listener: std::os::unix::net::UnixListener) -> io::Result<()> {
        let ticker = self.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                ticker.tick();
            }
        });
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
        let client = rustix::net::sockopt::socket_peercred(&stream)
            .ok()
            .and_then(|credentials| {
                ProcessIdentity::of(u32::try_from(credentials.pid.as_raw_nonzero().get()).ok()?)
            });
        let id = {
            let mut inner = lock(&self.shared.inner);
            let id = inner.next_connection;
            inner.next_connection += 1;
            inner.connections.insert(id, outbox.clone());
            if let Some(client) = client {
                inner.clients.insert(id, client);
            }
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
            Request::Drain => {
                lock(&self.shared.inner).draining = true;
                let service = self.clone();
                let outbox = outbox_of(&self.shared, connection);
                std::thread::spawn(move || {
                    service.wait_until(|status| status.holders.is_empty());
                    if let Some(outbox) = outbox {
                        outbox.push(&Reply::Done { changed: true });
                    }
                });
            }
            Request::Report => {
                let inner = lock(&self.shared.inner);
                match self.shared.arbiter.status(&inner.scheduler) {
                    Ok(status) => outbox.push(&Reply::Report {
                        status: Box::new(status),
                    }),
                    Err(error) => outbox.push(&Reply::Refused {
                        reason: error.to_string(),
                    }),
                }
            }
            Request::Maintain { mac, owner, reason } => {
                let mut inner = lock(&self.shared.inner);
                inner
                    .scheduler
                    .set_maintenance(oer_lab_arbiter::Maintenance {
                        mac,
                        owner,
                        reason,
                        since_unix: (self.shared.clock)() / 1000,
                        unknown: Default::default(),
                    });
                let changed = self.persist(&inner);
                outbox.push(&Reply::Done { changed });
            }
            Request::EndMaintenance { mac } => {
                let mut inner = lock(&self.shared.inner);
                let changed = inner.scheduler.clear_maintenance(&mac);
                self.persist(&inner);
                self.pump(&mut inner);
                outbox.push(&Reply::Done { changed });
            }
            Request::MergeOwner { old, new } => {
                let mut inner = lock(&self.shared.inner);
                inner.scheduler.merge_owner(&old, &new);
                self.persist(&inner);
                match oer_lab_arbiter::history::rename_owner(&self.shared.history_path, &old, &new)
                {
                    Ok(()) => outbox.push(&Reply::Done { changed: true }),
                    Err(error) => outbox.push(&Reply::Refused {
                        reason: error.to_string(),
                    }),
                }
            }
            Request::ForgetOwner { name } => {
                let mut inner = lock(&self.shared.inner);
                let changed = inner.scheduler.forget_owner(&name);
                self.persist(&inner);
                outbox.push(&Reply::Done { changed });
            }
            Request::RecordFlash { owner, mac, image } => {
                let recorded = oer_lab_model::device_id::DeviceId::parse(&mac)
                    .map_err(|error| error.to_string())
                    .and_then(|mac| {
                        self.shared
                            .arbiter
                            .journal()
                            .record_flash(owner, &mac, &image)
                            .map_err(|error| error.to_string())
                    });
                match recorded {
                    Ok(()) => outbox.push(&Reply::Done { changed: true }),
                    Err(reason) => outbox.push(&Reply::Refused { reason }),
                }
            }
            Request::Record {
                owner,
                device,
                event,
            } => match self
                .shared
                .arbiter
                .journal()
                .record_by(owner, device, event)
            {
                Ok(()) => outbox.push(&Reply::Done { changed: true }),
                Err(error) => outbox.push(&Reply::Refused {
                    reason: error.to_string(),
                }),
            },
            Request::Preempt { lease, by, reason } => {
                let mut inner = lock(&self.shared.inner);
                let now = (self.shared.clock)();
                match inner.scheduler.preempt(lease, &by, &reason, now) {
                    Ok(()) => {
                        self.end(
                            &mut inner,
                            lease,
                            LeaseOutcome::PreemptedOnRequest,
                            Some(format!("preempted by {by}: {reason}")),
                        );
                        outbox.push(&Reply::Waiting {
                            ticket: 0,
                            message: format!("lease #{lease} is being stopped"),
                        });
                    }
                    Err(error) => outbox.push(&Reply::Refused {
                        reason: error.to_string(),
                    }),
                }
            }
            Request::Return { resource } => {
                let recovery = {
                    let mut inner = lock(&self.shared.inner);
                    if inner.recovering.contains(&resource) {
                        outbox.push(&Reply::Refused {
                            reason: format!(
                                "{resource}'s last recovery still runs; return it once it ended"
                            ),
                        });
                        return;
                    }
                    inner.scheduler.release(&resource)
                };
                match recovery {
                    Some(recovery) => {
                        let mut inner = lock(&self.shared.inner);
                        self.persist(&inner);
                        self.spawn_recovery(&mut inner, recovery);
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
        let busy = inner
            .tickets
            .values()
            .any(|(holder, _)| *holder == connection)
            || own_lease(&inner, connection).is_some();
        if busy {
            outbox.push(&Reply::Refused {
                reason: "this connection already waits for or holds a lease".to_owned(),
            });
            return;
        }
        // A tool that acts on a board now is told at once; a run waits.
        if request.scenarios.is_empty()
            && request.maintenance.is_none()
            && let Some(refusal) = oer_lab_arbiter::maintenance::refusal(
                inner.scheduler.maintenance(),
                &request.owner,
                &oer_lab_arbiter::claims::normalize(&request.claims),
            )
        {
            outbox.push(&Reply::Refused { reason: refusal });
            return;
        }
        let maintenance = request
            .maintenance
            .clone()
            .map(|reason| (request.owner.clone(), reason));
        let divisible = request.divisible;
        let history = oer_lab_arbiter::history::read(&self.shared.history_path).unwrap_or_default();
        let estimate_secs = oer_lab_arbiter::estimate(&request.work, &request.scenarios, &history)
            .0
            .as_secs();
        let client = inner
            .clients
            .get(&connection)
            .copied()
            .unwrap_or(ProcessIdentity {
                pid: 0,
                start_ticks: 0,
            });
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
                estimate_secs,
                client,
            },
            (self.shared.clock)() / 1000,
        );
        inner.tickets.insert(ticket, (connection, divisible));
        if let Some(maintenance) = maintenance {
            inner.maintenance.insert(ticket, maintenance);
        }
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
        // A command inside the lease joins it, for the stand tools it runs.
        let mut operation = operation;
        if let Operation::Command { environment, .. } = &mut operation {
            environment.retain(|(key, _)| key != crate::protocol::LEASE_ENV);
            environment.push((crate::protocol::LEASE_ENV.to_owned(), lease.token.clone()));
        }
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
        inner.clients.remove(&connection);
        let tickets = inner
            .tickets
            .iter()
            .filter(|(_, (holder, _))| *holder == connection)
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        for ticket in tickets {
            inner.tickets.remove(&ticket);
            inner.maintenance.remove(&ticket);
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
            self.spawn_recovery(inner, recovery);
        }
        self.pump(inner);
    }

    /// Run `recovery` by its driver. A driver past the deadline leaves the
    /// resource unavailable, and its resource stays untouched until the
    /// driver returns.
    fn spawn_recovery(&self, inner: &mut Inner, recovery: Recovery) {
        if !inner.recovering.insert(recovery.resource.clone()) {
            // The running recovery's result is stale; a person returns the
            // resource once it ends.
            return;
        }
        let (done, confirmed) = mpsc::channel();
        let driver = self.clone();
        let running = recovery.clone();
        std::thread::spawn(move || {
            let confirmation = driver.shared.drivers.recover(&running);
            let _ = done.send(confirmation);
            lock(&driver.shared.inner)
                .recovering
                .remove(&running.resource);
            driver.shared.changed.notify_all();
        });
        let service = self.clone();
        std::thread::spawn(move || {
            let confirmation = confirmed
                .recv_timeout(service.shared.recovery_deadline)
                .unwrap_or_else(|_| Confirmation::Unavailable {
                    reason: format!(
                        "its recovery did not finish within {}",
                        oer_lab_arbiter::format_duration(service.shared.recovery_deadline)
                    ),
                    quiet: false,
                });
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
        if inner.draining {
            self.shared.changed.notify_all();
            return;
        }
        let mut next_token = inner.next_token;
        let granted = inner.scheduler.grant(now, &mut || {
            next_token += 1;
            format!("{now:x}-{next_token:x}")
        });
        inner.next_token = next_token;
        if !granted.is_empty() && !self.persist(inner) {
            // A grant the service could not record would be forgotten by a
            // restart: it is refused, and its resources recovered.
            for grant in granted {
                if let Some((connection, _)) = inner.tickets.remove(&grant.lease)
                    && let Some(outbox) = inner.connections.get(&connection)
                {
                    outbox.push(&Reply::Refused {
                        reason: "the LAB service cannot record its state".to_owned(),
                    });
                }
                if let Some(ended) = inner
                    .scheduler
                    .end(grant.lease, LeaseOutcome::Abandoned, now)
                {
                    for recovery in ended.recoveries {
                        self.spawn_recovery(inner, recovery);
                    }
                }
            }
            self.shared.changed.notify_all();
            return;
        }
        for grant in granted {
            inner.maintenance.remove(&grant.lease);
            let Some((connection, divisible)) = inner.tickets.remove(&grant.lease) else {
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
                    divisible,
                    yield_asked: false,
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
        // A waiting maintenance request preempts every holder it conflicts with.
        for (lease, ticket) in inner.scheduler.maintenance_conflicts() {
            let Some((by, reason)) = inner.maintenance.get(&ticket).cloned() else {
                continue;
            };
            if inner.scheduler.preempt(lease, &by, &reason, now).is_ok() {
                self.end(
                    inner,
                    lease,
                    LeaseOutcome::PreemptedOnRequest,
                    Some(format!("stand maintenance by {by}: {reason}")),
                );
            }
        }
        self.shared.changed.notify_all();
    }

    /// End the leases at the hard limit and ask divisible work held for its
    /// slice to yield to a waiter with a higher balance.
    pub fn tick(&self) {
        let mut inner = lock(&self.shared.inner);
        let now = (self.shared.clock)();
        for (lease, due) in inner.scheduler.due(now, MIN_SLICE, HARD_LIMIT) {
            match due {
                Due::HardLimit => self.end(
                    &mut inner,
                    lease,
                    LeaseOutcome::HardLimit,
                    Some(format!(
                        "the lease reached the hard limit of {}",
                        oer_lab_arbiter::format_duration(HARD_LIMIT)
                    )),
                ),
                Due::Yield { waiter, balance_ms } => {
                    let Some(held) = inner.leases.get_mut(&lease) else {
                        continue;
                    };
                    if !held.divisible || held.yield_asked || held.ending.is_some() {
                        continue;
                    }
                    held.yield_asked = true;
                    let connection = held.connection;
                    if let Some(outbox) = inner.connections.get(&connection) {
                        outbox.push(&Reply::Yield {
                            reason: format!(
                                "{waiter} waits with a higher balance ({balance_ms} ms)"
                            ),
                        });
                    }
                }
            }
        }
        self.persist(&inner);
        self.shared.changed.notify_all();
    }

    /// Persist the scheduler state; false, reported, when it failed.
    fn persist(&self, inner: &Inner) -> bool {
        match inner.scheduler.save(&self.shared.state_path) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("lab-service: cannot persist its state: {error}");
                false
            }
        }
    }
}

fn outbox_of(shared: &Shared, connection: u64) -> Option<Arc<Outbox>> {
    lock(&shared.inner).connections.get(&connection).cloned()
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
        recovering: inner.recovering.iter().cloned().collect(),
        resources: inner.scheduler.resources(),
    }
}

#[cfg(test)]
mod tests;
