//! Waiting for, holding and releasing a lease.

use std::{
    io::Read as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use oer_device_lock::{DeviceAccess, DeviceId};
use oer_stand_claims::{Claim, conflict, covers, normalize};
use oer_stand_journal::{device_label, latest};

use crate::{
    Arbiter,
    balance::{self, HARD_LIMIT, MIN_SLICE},
    estimate::{self, format_duration},
    history::{self, GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance},
    notify,
    process::ProcessIdentity,
    queue,
    state::{Holder, Ticket},
};

/// Token of the lease a command runs inside; such commands join the lease.
pub const LEASE_KEY: &str = "stand.lease";

/// Time a terminated holder has to clean up before it is killed.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(300);
/// How long divisible work holds a lease before it renews it at a
/// boundary: a third of the hard limit, leaving two thirds for its next step.
pub const RENEW_AFTER: Duration = Duration::from_secs(HARD_LIMIT.as_secs() / 3);

fn renewal_due(held: Duration) -> bool {
    held >= RENEW_AFTER
}

/// How often a holder checks the hard limit and waiters that outrank it.
const SUPERVISION: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(500);
/// Waits shorter than this do not notify the user when granted.
const NOTIFY_AFTER_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub owner: String,
    /// What the lease is for; leases of equal work share duration estimates.
    pub work: String,
    /// HIL scenarios the lease executes, recorded for per-scenario estimates.
    pub scenarios: Vec<String>,
    /// The HIL run the lease executes, when a runner requests it; the lease
    /// and its history record name it.
    pub run: Option<String>,
    /// Resources the lease needs; none claims the whole stand.
    pub claims: Vec<Claim>,
}

impl Request {
    /// A request by the owner the environment names.
    pub fn from_environment(work: impl Into<String>) -> crate::Result<Self> {
        Ok(Self {
            owner: oer_stand_owners::from_environment()?.to_string(),
            work: work.into(),
            scenarios: Vec::new(),
            run: None,
            claims: Vec::new(),
        })
    }
}

/// Ownership of the stand. A nested grant belongs to an enclosing lease and
/// releases nothing.
pub struct Grant {
    held: Option<Held>,
    /// The device access of every leased board: this process's owning guard
    /// (or, inside an enclosing lease, the enclosing lease's delegation) for
    /// the whole lease, its hub power restorations included; released after
    /// the lease. The commands a lease starts get the boards as delegates
    /// ([`Grant::context`]); the lease keeps its guards until they exit.
    devices: Vec<DeviceAccess>,
}

struct Held {
    arbiter: Arbiter,
    id: u64,
    token: String,
    owner: String,
    work: String,
    scenarios: Vec<String>,
    held_since: Instant,
    ending: Arc<Ending>,
    watchdog: Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}

/// Why a lease ends early, set by its supervisor or holder.
#[derive(Default)]
struct Ending {
    /// Terminated at the hard limit.
    hard_limit: AtomicBool,
    /// Asked to yield at its next boundary, for divisible work.
    yield_requested: AtomicBool,
    /// Released at a boundary to requeue its remaining work.
    yielded: AtomicBool,
}

impl Ending {
    fn outcome(&self) -> LeaseOutcome {
        if self.hard_limit.load(Ordering::Relaxed) {
            LeaseOutcome::HardLimit
        } else if self.yielded.load(Ordering::Relaxed) {
            LeaseOutcome::YieldedToBalance
        } else {
            LeaseOutcome::Released
        }
    }
}

enum Poll {
    Granted {
        waited: Duration,
        /// The owner's balance at the grant, in milliseconds.
        balance: i64,
    },
    /// A description that changes only with the position or holder, and the
    /// message including the expected start.
    Waiting { key: String, message: String },
}

/// A waiting message is repeated at this interval when nothing changed.
const REPORT_INTERVAL: Duration = Duration::from_secs(300);

impl Arbiter {
    /// Wait in the queue until the stand is granted to `request`.
    ///
    /// A process inside a lease (its token in [`LEASE_KEY`]) or already
    /// holding one joins it without waiting. Cancellation leaves the queue.
    pub fn acquire(&self, request: &Request) -> crate::Result<Grant> {
        let enclosing = oer_process::Context::current()?
            .get(LEASE_KEY)
            .map(str::to_owned);
        self.acquire_within(request, enclosing)
    }

    /// Acquire a lease for stand maintenance, such as a fixture software
    /// installation: it is served before every ordinary request, and every
    /// holder whose claims conflict with it is preempted for `reason`, with
    /// the ordinary cancellation, cleanup and notification.
    pub fn acquire_maintenance(&self, request: &Request, reason: &str) -> crate::Result<Grant> {
        let enclosing = oer_process::Context::current()?
            .get(LEASE_KEY)
            .map(str::to_owned);
        self.acquire_as(request, enclosing, Some(reason))
    }

    pub(crate) fn acquire_within(
        &self,
        request: &Request,
        enclosing: Option<String>,
    ) -> crate::Result<Grant> {
        self.acquire_as(request, enclosing, None)
    }

    /// Queue `request` and wait for its grant; with a maintenance `reason`
    /// it goes first and preempts the holders it conflicts with.
    fn acquire_as(
        &self,
        request: &Request,
        enclosing: Option<String>,
        maintenance: Option<&str>,
    ) -> crate::Result<Grant> {
        let priority = match maintenance {
            Some(_) => crate::state::Priority::Maintenance,
            None => crate::state::Priority::Ordinary,
        };
        // Every lease is charged to one agent under its one name.
        oer_stand_owners::Owner::new(&request.owner)?;
        let me = ProcessIdentity::current()?;
        let claims = normalize(&request.claims);
        // A lease that could not lock its boards is refused before it queues.
        self.leased_boards(&claims)?;
        if let Some(nested) = self.join_enclosing(me, enclosing, &claims)? {
            return Ok(nested);
        }
        // A run waits for what it needs to return to service; a tool that
        // acts on a board (a reset, a check) is told at once.
        while let Some(refusal) =
            crate::maintenance::refusal(&self.maintenance()?, &request.owner, &claims)
        {
            if request.scenarios.is_empty() {
                return Err(refusal.into());
            }
            eprintln!("hil-arbiter: waiting for service: {refusal}");
            let wanted = claims
                .iter()
                .filter_map(|claim| claim.resource.strip_prefix("board:").map(str::to_owned))
                .collect::<Vec<_>>();
            self.wait_for_service(&wanted, |_| {})?;
            eprintln!("hil-arbiter: back in service");
        }
        let (estimate, _) = estimate::estimate(&request.work, &request.scenarios, &self.history()?);
        let id = self.transaction(|state| {
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push(Ticket {
                id,
                owner: request.owner.clone(),
                work: request.work.clone(),
                estimate_secs: estimate.as_secs(),
                process: me,
                enqueued_unix: oer_durable::unix_seconds(),
                claims: claims.clone(),
                priority,
                job: crate::jobs::current()?,
                run: request.run.clone(),
                unknown: Default::default(),
            });
            Ok(id)
        })?;
        let mut waiting = Waiting {
            arbiter: self,
            id,
            granted: false,
        };
        let token = token()?;
        let started = Instant::now();
        let mut reported: Option<(String, Instant)> = None;
        let mut preempting = std::collections::BTreeSet::new();
        let (waited, balance) = loop {
            if let Some(reason) = maintenance {
                self.preempt_conflicting(id, &request.owner, reason, &mut preempting)?;
            }
            // A board another process holds outside the arbiter (`cargo fw`,
            // a tool of another stand) is busy until it lets go.
            let foreign = self.foreign_devices(&claims)?;
            if !foreign.is_empty() {
                let key = format!("device:{}", foreign.join(","));
                if reported.as_ref().is_none_or(|(previous, at)| {
                    *previous != key || at.elapsed() >= REPORT_INTERVAL
                }) {
                    eprintln!("hil-arbiter: waiting as #{id}: {}", foreign.join("; "));
                    reported = Some((key, Instant::now()));
                }
                oer_process::sleep(POLL)?;
                continue;
            }
            match self.poll(id, &token, started)? {
                Poll::Granted { waited, balance } => break (waited, balance),
                Poll::Waiting { key, message } => {
                    if reported.as_ref().is_none_or(|(previous, at)| {
                        *previous != key || at.elapsed() >= REPORT_INTERVAL
                    }) {
                        eprintln!("hil-arbiter: {message}");
                        reported = Some((key, Instant::now()));
                    }
                }
            }
            oer_process::sleep(POLL)?;
        };
        waiting.granted = true;
        eprintln!(
            "hil-arbiter: lease #{id} granted to {} for `{}` on {}; balance {}; the time it \
             holds is charged, divisible work yields after {} to a waiter with a higher \
             balance, and every lease ends at {}",
            request.owner,
            request.work,
            describe_claims(&claims),
            signed_duration(balance),
            format_duration(MIN_SLICE),
            format_duration(HARD_LIMIT)
        );
        if waited >= NOTIFY_AFTER_WAIT {
            notify::send(
                "HIL stand granted",
                &format!(
                    "{} got the stand after {} for {}",
                    request.owner,
                    format_duration(waited),
                    request.work
                ),
            );
        }
        self.report_board(&request.owner);
        let mut grant = Grant {
            held: Some(Held {
                arbiter: self.clone(),
                id,
                token,
                owner: request.owner.clone(),
                work: request.work.clone(),
                scenarios: request.scenarios.clone(),
                held_since: Instant::now(),
                ending: Arc::new(Ending::default()),
                watchdog: None,
            }),
            devices: Vec::new(),
        };
        // The lease's process owns its boards for the whole lease: a board
        // another process took since the check above is waited for. The
        // boards are named again, from the stand file as it is now; one that
        // cannot be named ends the grant, whose drop releases the hold.
        let boards = self.leased_boards(&claims)?;
        let command = format!("lease #{id} of {}: {}", request.owner, request.work);
        for mac in &boards {
            grant.devices.push(DeviceAccess::wait(mac, &command)?);
        }
        // A holder that ended without its release left its boards as they were.
        crate::restore::restore(self, &grant.devices, &format!("grant of lease #{id}"));
        Ok(grant)
    }

    /// Preempt, each on its own thread, the holders that conflict with
    /// waiting ticket `id` and are not in `started` yet.
    fn preempt_conflicting(
        &self,
        id: u64,
        by: &str,
        reason: &str,
        started: &mut std::collections::BTreeSet<u64>,
    ) -> crate::Result<()> {
        let holders = self.transaction(|state| {
            let Some(ticket) = state.queue.iter().find(|ticket| ticket.id == id) else {
                return Ok(Vec::new());
            };
            Ok(state
                .holders
                .iter()
                .filter(|holder| {
                    holder.preempted.is_none() && conflict(&holder.ticket.claims, &ticket.claims)
                })
                .map(|holder| holder.ticket.id)
                .collect::<Vec<_>>())
        })?;
        for holder in holders {
            if started.insert(holder) {
                let (arbiter, by, reason) = (self.clone(), by.to_owned(), reason.to_owned());
                std::thread::spawn(move || {
                    let _ = arbiter.preempt(holder, &by, &reason, SHUTDOWN_GRACE);
                });
            }
        }
        Ok(())
    }

    fn join_enclosing(
        &self,
        me: ProcessIdentity,
        token: Option<String>,
        claims: &[Claim],
    ) -> crate::Result<Option<Grant>> {
        let enclosing = self.transaction(|state| {
            Ok(state
                .holders
                .iter()
                .find(|holder| {
                    token.as_deref() == Some(holder.token.as_str()) || holder.ticket.process == me
                })
                .map(|holder| holder.ticket.claims.clone()))
        })?;
        match (enclosing, token) {
            (Some(held), _) if covers(&held, claims) => {
                // The enclosing lease's process holds the boards; this one
                // is its delegate.
                let mut devices = Vec::new();
                for mac in self.leased_boards(claims)? {
                    devices.push(
                        DeviceAccess::acquire(&mac, "inside an enclosing lease").map_err(
                            |busy| format!("{busy}: the enclosing lease does not hold it"),
                        )?,
                    );
                }
                Ok(Some(Grant {
                    held: None,
                    devices,
                }))
            }
            (Some(held), _) => Err(format!(
                "the enclosing lease holds {} but this command needs {}; claim it in the \
                 enclosing lease",
                describe_claims(&held),
                describe_claims(claims)
            )
            .into()),
            (None, Some(_)) => Err(format!(
                "{LEASE_KEY} names a lease that is no longer held; the enclosing lease ended"
            )
            .into()),
            (None, None) => Ok(None),
        }
    }

    fn poll(&self, id: u64, token: &str, started: Instant) -> crate::Result<Poll> {
        self.transaction(|state| {
            if queue::grantable(state, id) {
                let index = state
                    .queue
                    .iter()
                    .position(|ticket| ticket.id == id)
                    .ok_or("this request left the HIL queue")?;
                let ticket = state.queue.remove(index);
                let balance = balance::of(&state.balances, &ticket.owner);
                let over = state
                    .queue
                    .iter()
                    .filter(|other| {
                        other.owner != ticket.owner && conflict(&other.claims, &ticket.claims)
                    })
                    .map(|other| OwnerBalance {
                        owner: other.owner.clone(),
                        balance_ms: balance::of(&state.balances, &other.owner),
                    })
                    .collect();
                state.holders.push(Holder {
                    ticket,
                    token: token.to_owned(),
                    granted_unix: oer_durable::unix_seconds(),
                    reason: Some(GrantReason {
                        balance_ms: balance,
                        over,
                    }),
                    preempted: None,
                    unknown: Default::default(),
                });
                return Ok(Poll::Granted {
                    waited: started.elapsed(),
                    balance,
                });
            }
            let (_, ahead, wait) =
                queue::expected_starts(state, oer_durable::unix_seconds(), SHUTDOWN_GRACE)
                    .into_iter()
                    .find(|(ticket, ..)| *ticket == id)
                    .ok_or("this request left the HIL queue")?;
            let claims = state
                .queue
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.claims.clone())
                .unwrap_or_default();
            let blocking = state
                .holders
                .iter()
                .filter(|holder| conflict(&holder.ticket.claims, &claims))
                .map(|holder| format!("{} `{}`", holder.ticket.owner, holder.ticket.work))
                .collect::<Vec<_>>();
            let mut holders = if blocking.is_empty() {
                String::from("no conflicting holder")
            } else {
                format!("held by {}", blocking.join(", "))
            };
            let own = state.queue.iter().find(|ticket| ticket.id == id);
            let maintenance = state
                .queue
                .iter()
                .filter(|ticket| {
                    ticket.id != id
                        && ticket.priority == crate::state::Priority::Maintenance
                        && own.is_some_and(|own| {
                            own.priority == crate::state::Priority::Ordinary
                                && conflict(&ticket.claims, &own.claims)
                        })
                })
                .map(|ticket| format!("#{} `{}` by {}", ticket.id, ticket.work, ticket.owner))
                .collect::<Vec<_>>();
            if !maintenance.is_empty() {
                holders.push_str(&format!(
                    "; stand maintenance {} goes ahead of every request",
                    maintenance.join(", ")
                ));
            }
            let owner = state
                .queue
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.owner.clone())
                .unwrap_or_default();
            let (key, message) = waiting_report(
                id,
                &describe_claims(&claims),
                ahead,
                &holders,
                balance::of(&state.balances, &owner),
                wait,
            );
            Ok(Poll::Waiting { message, key })
        })
    }

    /// The MACs of the boards `claims` reach, through the stand file; a
    /// board that cannot be named is an error, never a board left unlocked.
    pub(crate) fn leased_boards(&self, claims: &[Claim]) -> crate::Result<Vec<DeviceId>> {
        crate::restore::claimed_boards(claims, || {
            self.stand().map_err(|error| {
                format!(
                    "the stand file {} does not load, so a whole-stand lease cannot lock \
                     its boards: {error}",
                    self.stand_file().display()
                )
                .into()
            })
        })
    }

    /// The boards of `claims` whose device lock a process holds that this
    /// one may not use, each with its holder.
    pub fn foreign_devices(&self, claims: &[Claim]) -> crate::Result<Vec<String>> {
        Ok(self
            .leased_boards(claims)?
            .into_iter()
            .filter_map(|mac| match oer_device_lock::foreign_holder(&mac) {
                Ok(Some(Some(holder))) => Some(format!("board {mac} is held by {holder}")),
                Ok(Some(None)) => Some(format!("board {mac} is held by another process")),
                Ok(None) => None,
                Err(error) => Some(format!("board {mac}: device lock unreadable: {error}")),
            })
            .collect())
    }

    /// Show every board's newest firmware and the flashes other owners made
    /// since this owner's previous lease.
    fn report_board(&self, owner: &str) {
        let (Ok(history), Ok(events)) = (self.history(), self.journal().events()) else {
            eprintln!("hil-arbiter: board journal unavailable");
            return;
        };
        let stand = self.stand().ok();
        let label = |event: &oer_stand_journal::BoardEvent| {
            device_label(event.device.as_deref(), stand.as_ref())
        };
        let previous = history
            .iter()
            .rev()
            .find(|record| record.owner == owner)
            .map(|record| record.released_unix);
        if let Some(previous) = previous {
            let changes = events
                .iter()
                .filter(|event| {
                    event.owner != owner
                        && event.unix >= previous
                        && matches!(
                            event.kind,
                            oer_stand_journal::BoardEventKind::Flashed { .. }
                        )
                })
                .collect::<Vec<_>>();
            if !changes.is_empty() {
                eprintln!("hil-arbiter: flashes by others since your previous lease:");
                for event in changes.iter().rev().take(10).rev() {
                    eprintln!("hil-arbiter:   {}: {event}", label(event));
                }
            }
        }
        let (flashes, _) = latest(&events);
        if flashes.is_empty() {
            eprintln!("hil-arbiter: board firmware: unknown");
        }
        for flash in flashes {
            eprintln!("hil-arbiter: firmware on {}: {flash}", label(flash));
        }
    }
}

/// What a waiting request reports, and the key that decides when it reports
/// again: a change of its position or of who holds what it needs. The
/// balance and the expected start change every second while it waits, so
/// they are shown but are not part of the key.
fn waiting_report(
    id: u64,
    claims: &str,
    ahead: usize,
    holders: &str,
    balance_ms: i64,
    wait_secs: u64,
) -> (String, String) {
    let key = format!(
        "waiting as #{id} for {claims}, behind {ahead} conflicting request(s) of owners with a \
         higher balance; {holders}"
    );
    let message = format!(
        "{key}; balance {}, expected start in ~{}",
        signed_duration(balance_ms),
        format_duration(Duration::from_secs(wait_secs))
    );
    (key, message)
}

/// Removes the ticket of a request that stops waiting.
struct Waiting<'a> {
    arbiter: &'a Arbiter,
    id: u64,
    granted: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if !self.granted {
            let id = self.id;
            let _ = self.arbiter.transaction(|state| {
                state.queue.retain(|ticket| ticket.id != id);
                Ok(())
            });
        }
    }
}

impl Grant {
    /// Whether this grant joined an enclosing lease.
    pub fn is_nested(&self) -> bool {
        self.held.is_none()
    }

    /// Explicit context for a command admitted to this lease: only its
    /// selected boards, owner, lease and job. Ordinary children receive none.
    pub fn context(&self) -> crate::Result<oer_process::Context> {
        let mut context = oer_process::Context::default();
        if let Some(held) = &self.held {
            context.set(LEASE_KEY, &held.token);
            context.set(oer_stand_owners::OWNER_KEY, &held.owner);
        } else {
            for key in [LEASE_KEY, oer_stand_owners::OWNER_KEY] {
                if let Some(value) = oer_process::Context::current()?.get(key) {
                    context.set(key, value);
                }
            }
        }
        if let Some(job) = crate::jobs::current()? {
            context.set(crate::jobs::JOB_KEY, job);
        }
        for access in &self.devices {
            access.delegate(&mut context)?;
        }
        Ok(context)
    }

    /// The device access of the leased board `id`.
    pub fn device(&self, id: &DeviceId) -> Option<&DeviceAccess> {
        self.devices.iter().find(|device| device.id() == id)
    }

    /// Admit I/O for every leased board before starting an external command.
    /// Pin each operation's lifetime into the command and retain these guards
    /// until it exits, so exclusion also survives the lease owner's death.
    pub fn operations(&self) -> crate::Result<Vec<oer_device_lock::DeviceOperation>> {
        self.devices.iter().map(DeviceAccess::operation).collect()
    }

    /// Whether a waiting request needs a resource this lease holds.
    pub fn blocks_waiters(&self) -> bool {
        self.held.as_ref().is_some_and(|held| {
            held.arbiter
                .transaction(|state| Ok(queue::blocks_waiters(state, held.id)))
                .unwrap_or(false)
        })
    }

    /// Whether the supervisor asked divisible work to yield at its next
    /// boundary: a waiter it blocks has a higher balance and the lease has
    /// held its minimum slice.
    pub fn yield_requested(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| held.ending.yield_requested.load(Ordering::Relaxed))
    }

    /// Whether divisible work should renew this lease at its next
    /// boundary: it has held a third of the hard limit, so the next step
    /// gets the rest of a fresh lease instead of meeting the limit.
    pub fn renewal_due(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| renewal_due(held.held_since.elapsed()))
    }

    /// Record that the holder is being terminated at the hard limit.
    pub fn mark_hard_limit(&self) {
        if let Some(held) = &self.held {
            held.ending.hard_limit.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: {}: `{}` reached the hard limit {}; terminating",
                held.owner,
                held.work,
                format_duration(HARD_LIMIT)
            );
        }
    }

    /// Record that the holder releases at a boundary to renew its lease
    /// before the hard limit.
    pub fn mark_renewed(&self) {
        if let Some(held) = &self.held {
            held.ending.yielded.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: lease #{} of {} renews before the hard limit {} and queues again",
                held.id,
                held.owner,
                format_duration(HARD_LIMIT)
            );
        }
    }

    /// Record that the holder releases at a boundary to requeue its work.
    pub fn mark_yielded(&self) {
        if let Some(held) = &self.held {
            held.ending.yielded.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: lease #{} of {} yields to a waiter with a higher balance and \
                 queues again",
                held.id, held.owner
            );
        }
    }

    /// Supervise this process. Divisible work is asked to yield at its next
    /// boundary ([`Self::yield_requested`]) once it has held
    /// [`MIN_SLICE`] and a waiter it blocks has a higher balance; indivisible
    /// work runs to completion. At [`HARD_LIMIT`] the process receives
    /// `SIGTERM`, its ordinary cancellation and cleanup, and `SIGKILL` after
    /// the shutdown grace.
    pub fn supervise_self(&mut self, divisible: bool) {
        self.supervise_with(divisible, MIN_SLICE, HARD_LIMIT);
    }

    /// [`Self::supervise_self`] with explicit limits, for tests.
    pub(crate) fn supervise_with(&mut self, divisible: bool, slice: Duration, limit: Duration) {
        let Some(held) = &mut self.held else {
            return;
        };
        let (stop, stopped) = mpsc::channel::<()>();
        let (arbiter, id, owner, work, since) = (
            held.arbiter.clone(),
            held.id,
            held.owner.clone(),
            held.work.clone(),
            held.held_since,
        );
        let ending = held.ending.clone();
        let thread = std::thread::spawn(move || {
            let wait = |duration| {
                matches!(
                    stopped.recv_timeout(duration),
                    Err(mpsc::RecvTimeoutError::Timeout)
                )
            };
            loop {
                if since.elapsed() >= limit {
                    ending.hard_limit.store(true, Ordering::Relaxed);
                    eprintln!(
                        "hil-arbiter: {owner}: `{work}` reached the hard limit {}; terminating \
                         with cleanup",
                        format_duration(limit)
                    );
                    notify::send(
                        "HIL stand lease terminated",
                        &format!("{owner}: {work} reached {}", format_duration(limit)),
                    );
                    signal_self(rustix::process::Signal::TERM);
                    if wait(SHUTDOWN_GRACE) {
                        signal_self(rustix::process::Signal::KILL);
                    }
                    return;
                }
                if divisible
                    && !ending.yield_requested.load(Ordering::Relaxed)
                    && let Ok(Some((waiter, balance))) = arbiter.transaction(|state| {
                        Ok(queue::outranked_by(
                            state,
                            id,
                            oer_durable::unix_seconds(),
                            slice,
                        ))
                    })
                {
                    ending.yield_requested.store(true, Ordering::Relaxed);
                    eprintln!(
                        "hil-arbiter: lease #{id} of {owner} yields after the current step to \
                         {waiter}, whose balance {} is higher",
                        signed_duration(balance)
                    );
                }
                if !wait(SUPERVISION) {
                    return;
                }
            }
        });
        held.watchdog = Some((stop, thread));
    }
}

/// `+12m`, `-3m30s`, `0s`.
pub(crate) fn signed_duration(milliseconds: i64) -> String {
    let magnitude = format_duration(Duration::from_millis(milliseconds.unsigned_abs()));
    match milliseconds.signum() {
        1 => format!("+{magnitude}"),
        -1 => format!("-{magnitude}"),
        _ => magnitude,
    }
}

/// `board:AA, air (shared)` or `the whole stand`.
pub(crate) fn describe_claims(claims: &[Claim]) -> String {
    if claims
        .iter()
        .any(|claim| claim.resource == oer_stand_claims::STAND)
    {
        return String::from("the whole stand");
    }
    claims
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn signal_self(signal: rustix::process::Signal) {
    let _ = rustix::process::kill_process(rustix::process::getpid(), signal);
}

impl Drop for Grant {
    fn drop(&mut self) {
        let Some(mut held) = self.held.take() else {
            return;
        };
        if let Some((stop, thread)) = held.watchdog.take() {
            drop(stop);
            let _ = thread.join();
        }
        // Still held: the next holder gets the boards in their working state.
        crate::restore::restore(
            &held.arbiter,
            &self.devices,
            &format!("release of lease #{}", held.id),
        );
        let history_path = held.arbiter.history_path();
        let outcome = held.ending.outcome();
        let released = held.arbiter.transaction(|state| {
            let Some(index) = state
                .holders
                .iter()
                .position(|holder| holder.ticket.id == held.id)
            else {
                return Ok(None);
            };
            let holder = state.holders.remove(index);
            if let Some(preemption) = &holder.preempted {
                eprintln!(
                    "hil-arbiter: lease #{} was preempted by {}: {}",
                    held.id, preemption.by, preemption.reason
                );
            }
            let charged_ms =
                crate::preempt::charged_ms(&holder, held.held_since.elapsed().as_millis() as u64);
            history::append(
                &history_path,
                &LeaseRecord {
                    id: held.id,
                    balance_after_ms: balance::of(&state.balances, &holder.ticket.owner),
                    owner: holder.ticket.owner,
                    work: holder.ticket.work,
                    granted_unix: holder.granted_unix,
                    released_unix: oer_durable::unix_seconds(),
                    outcome: if holder.preempted.is_some() {
                        LeaseOutcome::PreemptedOnRequest
                    } else {
                        outcome
                    },
                    charged_ms,
                    reason: holder.reason,
                    preempted: holder.preempted,
                    scenarios: held.scenarios.clone(),
                    run: holder.ticket.run,
                    unknown: Default::default(),
                },
            )?;
            Ok(Some(state.queue.is_empty() && state.holders.is_empty()))
        });
        match released {
            Ok(Some(idle)) => {
                eprintln!("hil-arbiter: lease #{} released", held.id);
                if idle {
                    notify::send(
                        "HIL stand free",
                        &format!("{} finished {}", held.owner, held.work),
                    );
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("hil-arbiter: cannot release lease #{}: {error}", held.id),
        }
    }
}

fn token() -> crate::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests;
