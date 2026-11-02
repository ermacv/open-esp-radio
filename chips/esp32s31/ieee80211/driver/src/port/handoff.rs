//! Separate affine body/driver endpoints over permanent queue backing.
use super::*;
use oer_memory::{
    AffineSpscReceiver, AffineSpscSender, CompactSpscCursor, LocalSpscReceiver, LocalSpscSender,
};

pub(super) type ClockAnswer = Result<Ieee80211Instant, ClockError>;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ClockUse {
    Idle,
    Waiting,
    Cancelled,
}

type Cmd<'id, Id, D, const B: usize, O: Observations> = driver::Command<'id, Id, D, B, O>;
type Msg<'id, Id, D, const B: usize> = owned::OwnedEvent<'id, Id, D, B>;
type FailedCommand<'id, Id, D, const B: usize, O: Observations> =
    oer_memory::OnceValue<Cmd<'id, Id, D, B, O>>;
type FailedTerminal<'id, Id, D, const B: usize, O: Observations> =
    oer_memory::OnceValue<owned::Retained<'id, Id, D, B, O>>;

/// The transmissions' terminals, each at its ticket's place from the driver's
/// completion until the body takes it, so only the place travels through the
/// event queue. A ticket names one operation at a time: a place holds at
/// most one terminal.
pub(super) type Completions<'id, Id, D, const B: usize, O> =
    oer_memory::ReturnBank<Option<owned::OwnedTxCompleted<'id, Id, D, B, O>>, TX_TICKETS>;

/// The place of one parked transmission terminal. Only the driver makes one, when
/// it parks the terminal; the body consumes it once.
pub(super) struct ParkedCompletion {
    place: u8,
}

/// Take the terminal parked at `parked`'s place, leaving the place empty;
/// `None` when it holds none.
pub(super) fn take_completion<
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    completions: &Completions<'id, Id, D, B, O>,
    parked: ParkedCompletion,
) -> Option<owned::OwnedTxCompleted<'id, Id, D, B, O>> {
    let (held, token) = completions.take(usize::from(parked.place))?;
    token.return_value(None);
    held
}

/// Whether no place holds a terminal. A place never seeded holds none.
fn completions_vacant<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    completions: &Completions<'_, Id, D, B, O>,
) -> bool {
    (0..TX_TICKETS).all(|place| match completions.take(place) {
        Some((held, token)) => {
            let vacant = held.is_none();
            token.return_value(held);
            vacant
        }
        None => true,
    })
}

// Keep the ownership-preserving failure path shared by body and driver callers.
// It never extracts the retained owner through a shared reference.
#[inline(never)]
pub(super) fn retain_terminal<
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    cell: &FailedTerminal<'id, Id, D, B, O>,
    terminal: owned::Retained<'id, Id, D, B, O>,
) {
    // The first failed terminal already poisoned the installation; a later
    // one stays quarantined with it.
    if let Err(terminal) = cell.set(terminal) {
        let _quarantined = core::mem::ManuallyDrop::new(terminal);
    }
}

pub(super) struct Incoming<'s, T, const N: usize>(
    pub(super) AffineSpscReceiver<'s, T, N, CompactSpscCursor>,
);
impl<T, const N: usize> Incoming<'_, T, N> {
    pub(super) fn pop(&self) -> Option<T> {
        self.0.try_receive().ok()
    }
    pub(super) fn empty(&self) -> bool {
        self.0.is_empty()
    }
}
pub(super) struct Outgoing<'s, T, const N: usize>(
    pub(super) AffineSpscSender<'s, T, N, CompactSpscCursor>,
);
impl<T, const N: usize> Outgoing<'_, T, N> {
    pub(super) fn push(&self, value: T) -> Result<(), T> {
        self.0.try_send(value).map_err(|error| error.0)
    }
    pub(super) fn empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What one installation attaches to the placed backing: its storage and
/// its gate issuer, present together from preparation on.
pub(super) struct Attached<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) storage: &'static PortStorage<D, B>,
    /// The device's timer, cloned once at preparation.
    pub(super) timer: D::Timer,
    pub(super) gate: GateIssuer<'id, Part<Id, 1>>,
    /// The body's view of the channel it last published; the body alone writes it.
    pub(super) tuned: Cell<Channel>,
}

pub(super) struct HeldBacking<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
{
    /// Absent until an installation is prepared; `split` reaches the
    /// installation's views only through it.
    pub(super) attached: Option<Attached<'id, Id, D, B>>,
    pub(super) commands: oer_memory::AffineSpscQueue<
        driver::Command<'id, Id, D, B, O>,
        COMMANDS,
        oer_memory::CompactSpscCursor,
    >,
    pub(super) events: oer_memory::AffineSpscQueue<
        owned::OwnedEvent<'id, Id, D, B>,
        { TERMINAL_CAPACITY + RX_HELD },
        oer_memory::CompactSpscCursor,
    >,
    /// The received units the body holds: admitted to it and not yet
    /// returned to the ring or lent to the network.
    pub(super) rx_out: Cell<usize>,
    pub(super) failed_command: oer_memory::OnceValue<driver::Command<'id, Id, D, B, O>>,
    pub(super) failed_terminal: FailedTerminal<'id, Id, D, B, O>,
    pub(super) completions: Completions<'id, Id, D, B, O>,
    pub(super) frontend_identity: Cell<u64>,
    pub(super) clock_replies: oer_memory::AffineSpscQueue<ClockAnswer, 1, CompactSpscCursor>,
    pub(super) clock_use: Cell<ClockUse>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    HeldBacking<'id, Id, D, B, O>
{
    pub(super) const fn vacant() -> Self {
        Self {
            attached: None,
            commands: oer_memory::AffineSpscQueue::new(),
            events: oer_memory::AffineSpscQueue::new(),
            rx_out: Cell::new(0),
            failed_command: oer_memory::OnceValue::new(),
            failed_terminal: oer_memory::OnceValue::new(),
            completions: oer_memory::ReturnBank::new(),
            frontend_identity: Cell::new(1),
            clock_replies: oer_memory::AffineSpscQueue::new(),
            clock_use: Cell::new(ClockUse::Idle),
        }
    }
    /// A joined installation drained its rings and returned no failed owner.
    pub(super) fn drained(&mut self) -> bool {
        self.commands.split().0.is_empty()
            && self.events.split().0.is_empty()
            && !self.failed_command.is_requested()
            && !self.failed_terminal.is_requested()
            && completions_vacant(&self.completions)
    }
    /// The one caller checked `drained` before taking any resource.
    pub(super) fn prepare(
        &mut self,
        storage: &'static PortStorage<D, B>,
        issuer: GateIssuer<'id, Part<Id, 1>>,
        channel: Channel,
        timer: D::Timer,
    ) {
        // Pure clock readings carry no affine owner. A cancelled body read may
        // leave one reply after its endpoint epoch ended; discard it only
        // under this exclusive, joined preparation borrow.
        let _ = self.clock_replies.split().1.try_receive();
        self.attached = Some(Attached {
            storage,
            timer,
            gate: issuer,
            tuned: Cell::new(channel),
        });
        self.clock_use.set(ClockUse::Idle);
        self.rx_out.set(0);
        // `drained` saw every place empty. A place this backing never used
        // takes its first, empty value; one it used holds none already.
        for place in 0..TX_TICKETS {
            let used = self.completions.take(place).map(|(held, token)| {
                let vacant = held.is_none();
                token.return_value(held);
                vacant
            });
            let ready = match used {
                Some(vacant) => vacant,
                None => self.completions.seed(place, None).is_ok(),
            };
            if !ready {
                storage.poison(Fault::Capacity);
            }
        }
        storage.shared.network.reset();
        let settings = &storage.shared.settings;
        settings.clock_requested.set(false);
        settings.enabled.set(false);
        settings.want_enabled.set(false);
        // The backend's channel passed the S31 caps at construction and the
        // standard table fits; a refusal is a typed fault of this install.
        let mut published = settings.channel.set(channel);
        for vif in 0..2 {
            published = published.and(settings.edca.set(vif, EdcaParameterSet::STANDARD_DEFAULT));
        }
        if let Err(fault) = published {
            storage.poison(Fault::Setting(fault));
        }
        settings.dirty_edca.set(true);
        settings.block_acks.clear();
        settings.dirty_ba.set(false);
        settings.color.set(None);
        settings.loss.set(false);
        settings.receive_due.set(false);
        #[cfg(oer_debug_control)]
        {
            settings.publication_hold.set(0);
            settings.completion_hold.set(false);
            settings.reject_group_replacement.set(false);
        }
    }
}

pub(super) struct Held<'s, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) storage: &'static PortStorage<D, B>,
    pub(super) commands: Incoming<'s, Cmd<'id, Id, D, B, O>, COMMANDS>,
    pub(super) events: Outgoing<'s, Msg<'id, Id, D, B>, { TERMINAL_CAPACITY + RX_HELD }>,
    pub(super) rx_out: &'s Cell<usize>,
    pub(super) gate: &'s GateIssuer<'id, Part<Id, 1>>,
    pub(super) failed_terminal: &'s FailedTerminal<'id, Id, D, B, O>,
    pub(super) completions: &'s Completions<'id, Id, D, B, O>,
    pub(super) clock_replies: Outgoing<'s, ClockAnswer, 1>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> core::ops::Deref
    for Held<'_, '_, Id, D, B, O>
{
    type Target = settings::Settings;
    fn deref(&self) -> &Self::Target {
        &self.storage.shared.settings
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Held<'_, 'id, Id, D, B, O>
{
    pub(super) fn serial(&self) -> u64 {
        let serial = self.storage.identity.get();
        match serial.checked_add(1) {
            Some(next) => self.storage.identity.set(next),
            None => self.storage.poison(Fault::Capacity),
        }
        serial
    }
    pub(super) fn terminal(&self, terminal: owned::OwnedTerminal<'id, Id, D, B>) {
        if let Err(owned::OwnedEvent::Terminal(terminal)) =
            self.events.push(owned::OwnedEvent::Terminal(terminal))
        {
            retain_terminal(self.failed_terminal, terminal.into());
            self.storage.poison(Fault::Capacity);
        } else {
            #[cfg(all(oer_trace, oer_debug_control))]
            oer_espressif_staged_runtime::trace::emit(&trace::TerminalPosted);
        }
        self.storage.shared.wake_body();
    }
    /// Post a transmission's terminal: it waits at its ticket's `place` and
    /// only the place enters the event queue. A place that holds a terminal
    /// already, or a full queue, poisons the installation; the terminal
    /// stays quarantined.
    pub(super) fn tx_completed(
        &self,
        place: u8,
        completed: owned::OwnedTxCompleted<'id, Id, D, B, O>,
    ) {
        let refused = match self.completions.take(usize::from(place)) {
            Some((None, token)) => {
                token.return_value(Some(completed));
                None
            }
            Some((held, token)) => {
                token.return_value(held);
                Some(completed)
            }
            None => Some(completed),
        };
        let posted = match refused {
            Some(completed) => {
                retain_terminal(self.failed_terminal, completed.into());
                false
            }
            // A full queue leaves the terminal at its place, which
            // retirement checks.
            None => self
                .events
                .push(owned::OwnedEvent::TxCompleted(ParkedCompletion { place }))
                .is_ok(),
        };
        if posted {
            #[cfg(all(oer_trace, oer_debug_control))]
            oer_espressif_staged_runtime::trace::emit(&trace::TerminalPosted);
        } else {
            self.storage.poison(Fault::Capacity);
        }
        self.storage.shared.wake_body();
    }
    /// Whether no transmission terminal waits at its place.
    pub(super) fn completions_vacant(&self) -> bool {
        completions_vacant(self.completions)
    }
}

pub(super) struct FrontHeld<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) storage: &'static PortSharedStorage<B, D::Lent, D::Body>,
    pub(super) commands:
        LocalSpscSender<'p, Cmd<'static, Id, D, B, O>, COMMANDS, CompactSpscCursor>,
    pub(super) events: LocalSpscReceiver<
        'p,
        Msg<'static, Id, D, B>,
        { TERMINAL_CAPACITY + RX_HELD },
        CompactSpscCursor,
    >,
    /// The device and the driver's state, for the body's receive pull and
    /// returns: both tasks run on the station's executor, and each borrows
    /// them only within one synchronous step.
    pub(super) port: &'static PortStorage<D, B>,
    pub(super) driver: &'p RefCell<driver::Driver<'static, Id, D, B, O>>,
    pub(super) rx_out: &'p Cell<usize>,
    pub(super) failed_command: &'p FailedCommand<'static, Id, D, B, O>,
    pub(super) failed_terminal: &'p FailedTerminal<'static, Id, D, B, O>,
    pub(super) completions: &'p Completions<'static, Id, D, B, O>,
    pub(super) gate: &'p GateIssuer<'static, Part<Id, 1>>,
    pub(super) tuned: &'p Cell<Channel>,
    pub(super) identity: &'p Cell<u64>,
    pub(super) timer: D::Timer,
    pub(super) clock_replies: LocalSpscReceiver<'p, ClockAnswer, 1, CompactSpscCursor>,
    pub(super) clock_use: &'p Cell<ClockUse>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> core::ops::Deref
    for FrontHeld<'_, Id, D, B, O>
{
    type Target = settings::Settings;
    fn deref(&self) -> &Self::Target {
        &self.storage.settings
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> FrontHeld<'_, Id, D, B, O> {
    pub(super) fn serial(&self) -> u64 {
        let serial = self.identity.get();
        match serial.checked_add(1) {
            Some(next) => self.identity.set(next),
            None => self.storage.poison(Fault::Capacity),
        }
        serial
    }
    pub(super) fn command(&self, command: Cmd<'static, Id, D, B, O>) {
        if let Err(error) = self.commands.try_send(command) {
            // The first failed command already poisoned the installation; a
            // later one stays quarantined with it.
            if let Err(command) = self.failed_command.set(error.0) {
                let _quarantined = core::mem::ManuallyDrop::new(command);
            }
            self.storage.poison(Fault::Capacity);
        }
        self.storage.driver.wake();
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> FrontHeld<'_, Id, D, B, O> {
    /// Return a unit the body held to the ring, in the body's own turn.
    pub(super) fn recycle(&self, buffer: ReceiveBuffer<'static, Id, D::Receive>) {
        let (unit, _) = buffer.into_unit();
        let returned = self.port.device().and_then(|mut device| {
            device.recycle(unit);
            self.rx_out
                .get()
                .checked_sub(1)
                .map(|held| self.rx_out.set(held))
                .ok_or(Fault::Capacity)
        });
        match returned {
            // The returned unit waits for the next pull to rearm the ring.
            Ok(()) => self.receive_due.set(true),
            Err(fault) => self.storage.poison(fault),
        }
    }
    /// The body's receive pull: the next received unit the ring holds and
    /// a receive rule admits, serviced in the body's own turn. `None` once
    /// the ring has none now: the driver wakes the body when its interrupt
    /// leaves ring work, and `cx` when the ring has more at once.
    pub(super) fn pull(
        &self,
        cx: &mut Context<'_>,
    ) -> Result<Option<owned::OwnedEvent<'static, Id, D, B>>, Fault> {
        if !self.enabled.get() {
            return Ok(None);
        }
        // A unit the network returned waits for this pull to rearm the
        // ring: an exhausted walker raises no receive interrupt, so the
        // return itself wakes the body.
        if self.storage.network.poll_returns(cx) {
            self.receive_due.set(true);
        }
        let mut device = self.port.device()?;
        // The receive facts wake the body itself; taking them registers it.
        if device.poll_received_facts(cx)? {
            self.receive_due.set(true);
        }
        if !self.receive_due.get() {
            return Ok(None);
        }
        let mut driver = self
            .driver
            .try_borrow_mut()
            .map_err(|_| Fault::HardwareUnavailable)?;
        driver::settle_lent(&self.storage.network, self.rx_out, &mut **device)?;
        // A unit no rule admits goes back to the ring and the pull takes
        // the next, up to one driver turn's worth.
        for _ in 0..EVENTS_PER_POLL {
            let room = RX_HELD.saturating_sub(self.rx_out.get());
            let Some(unit) = device.pull_received(room, cx)? else {
                // A continuation timer's ring work waits for the driver,
                // whose deadline covers it.
                self.receive_due.set(device.receive_due());
                return Ok(None);
            };
            if let Some(event) = driver.admit(&mut **device, self.rx_out, self.storage, unit)? {
                return Ok(Some(event));
            }
        }
        cx.waker().wake_by_ref();
        Ok(None)
    }
}

struct ClockGuard<'a>(&'a Cell<ClockUse>);
impl Drop for ClockGuard<'_> {
    fn drop(&mut self) {
        if self.0.get() == ClockUse::Waiting {
            self.0.set(ClockUse::Cancelled);
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> FrontHeld<'_, Id, D, B, O> {
    pub(super) async fn now(&self) -> radio::PortResult<Ieee80211Instant, ClockError, Fault> {
        self.storage.check()?;
        if self.clock_use.get() == ClockUse::Cancelled && self.clock_replies.try_receive().is_ok() {
            self.clock_use.set(ClockUse::Idle);
        }
        // A competing/cancelled fresh latch never waits for another caller.
        if self.clock_use.get() != ClockUse::Idle {
            return Ok(Err(ClockError::Unavailable));
        }
        let deadline = self.timer.now().saturating_add(CLOCK_BOUND);
        self.clock_use.set(ClockUse::Waiting);
        let _guard = ClockGuard(self.clock_use);
        self.clock_requested.set(true);
        self.storage.driver.wake();
        let mut bound = core::pin::pin!(self.timer.wait_until(deadline));
        poll_fn(|cx| {
            self.storage.body.register(cx.waker());
            self.storage.check()?;
            let expired = bound.as_mut().poll(cx).is_ready();
            let answer = self.clock_replies.try_receive().ok();
            if expired || bound.as_mut().poll(cx).is_ready() {
                self.clock_use.set(if answer.is_some() {
                    ClockUse::Idle
                } else {
                    ClockUse::Cancelled
                });
                return Poll::Ready(Ok(Err(ClockError::Unavailable)));
            }
            if let Some(answer) = answer {
                self.clock_use.set(ClockUse::Idle);
                Poll::Ready(Ok(answer))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}
