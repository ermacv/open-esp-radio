//! Separate affine P1/P2 endpoints over permanent queue backing.
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

type Cmd<'id, Id, D, const B: usize> = driver::Command<'id, Id, D, B>;
type Msg<'id, Id, D, const B: usize> = owned::OwnedEvent<'id, Id, D, B>;
type FailedCommand<'id, Id, D, const B: usize> = oer_memory::OnceValue<Cmd<'id, Id, D, B>>;
type FailedTerminal<'id, Id, D, const B: usize> =
    oer_memory::OnceValue<owned::OwnedTerminal<'id, Id, D, B>>;

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

pub(super) struct HeldBacking<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) storage: Prepared<&'static PortStorage<D, B>>,
    pub(super) commands: oer_memory::AffineSpscQueue<
        driver::Command<'id, Id, D, B>,
        COMMANDS,
        oer_memory::CompactSpscCursor,
    >,
    pub(super) events: oer_memory::AffineSpscQueue<
        owned::OwnedEvent<'id, Id, D, B>,
        { TERMINALS + TX_TICKETS },
        oer_memory::CompactSpscCursor,
    >,
    pub(super) rx_out: Cell<usize>,
    pub(super) recycled: oer_memory::AffineSpscQueue<
        ReceiveBuffer<'id, Id, D::Receive>,
        TX_TICKETS,
        oer_memory::CompactSpscCursor,
    >,
    pub(super) failed_command: oer_memory::OnceValue<driver::Command<'id, Id, D, B>>,
    pub(super) failed_terminal: oer_memory::OnceValue<owned::OwnedTerminal<'id, Id, D, B>>,
    pub(super) gate: Prepared<GateIssuer<'id, Part<Id, 1>>>,
    pub(super) frontend_identity: Cell<u64>,
    pub(super) clock_replies: oer_memory::AffineSpscQueue<ClockAnswer, 1, CompactSpscCursor>,
    pub(super) clock_use: Cell<ClockUse>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> core::ops::Deref
    for HeldBacking<'_, Id, D, B>
{
    type Target = settings::Settings;
    fn deref(&self) -> &Self::Target {
        &self.storage.shared.settings
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> HeldBacking<'id, Id, D, B> {
    pub(super) const fn vacant() -> Self {
        Self {
            storage: Prepared::vacant(),
            commands: oer_memory::AffineSpscQueue::new(),
            events: oer_memory::AffineSpscQueue::new(),
            recycled: oer_memory::AffineSpscQueue::new(),
            rx_out: Cell::new(0),
            failed_command: oer_memory::OnceValue::new(),
            failed_terminal: oer_memory::OnceValue::new(),
            gate: Prepared::vacant(),
            frontend_identity: Cell::new(1),
            clock_replies: oer_memory::AffineSpscQueue::new(),
            clock_use: Cell::new(ClockUse::Idle),
        }
    }
    pub(super) fn prepare(
        &mut self,
        storage: &'static PortStorage<D, B>,
        issuer: GateIssuer<'id, Part<Id, 1>>,
        channel: Channel,
    ) {
        assert!(
            self.commands.split().0.is_empty()
                && self.events.split().0.is_empty()
                && self.recycled.split().0.is_empty(),
            "joined installation drained its rings"
        );
        assert!(
            !self.failed_command.is_requested() && !self.failed_terminal.is_requested(),
            "joined installation returned failed owners"
        );
        // Pure clock readings carry no affine owner. A cancelled P1 read may
        // leave one reply after its endpoint epoch ended; discard it only
        // under this exclusive, joined preparation borrow.
        let _ = self.clock_replies.split().1.try_receive();
        self.storage.prepare(storage);
        self.clock_use.set(ClockUse::Idle);
        self.clock_requested.set(false);
        self.gate.prepare(issuer);
        self.enabled.set(false);
        self.want_enabled.set(false);
        self.rx_out.set(0);
        self.channel.set(channel);
        self.edca.set([EdcaParameterSet::STANDARD_DEFAULT; 2]);
        self.dirty_edca.set(true);
        self.block_acks.clear();
        self.dirty_ba.set(false);
        self.color.set(None);
        self.loss.set(false);
        #[cfg(oer_debug_control)]
        {
            self.publication_hold.set(0);
            self.completion_hold.set(false);
        }
    }
}

pub(super) struct Held<'s, 'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) storage: &'static PortStorage<D, B>,
    pub(super) commands: Incoming<'s, Cmd<'id, Id, D, B>, COMMANDS>,
    pub(super) events: Outgoing<'s, Msg<'id, Id, D, B>, { TERMINALS + TX_TICKETS }>,
    pub(super) recycled: Incoming<'s, ReceiveBuffer<'id, Id, D::Receive>, TX_TICKETS>,
    pub(super) rx_out: &'s Cell<usize>,
    pub(super) gate: &'s Prepared<GateIssuer<'id, Part<Id, 1>>>,
    pub(super) failed_terminal: &'s FailedTerminal<'id, Id, D, B>,
    pub(super) clock_replies: Outgoing<'s, ClockAnswer, 1>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> core::ops::Deref for Held<'_, '_, Id, D, B> {
    type Target = settings::Settings;
    fn deref(&self) -> &Self::Target {
        &self.storage.shared.settings
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> Held<'_, 'id, Id, D, B> {
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
            assert!(
                self.failed_terminal.set(terminal).is_ok(),
                "one failed terminal per installation"
            );
            self.storage.poison(Fault::Capacity);
        } else {
            #[cfg(all(oer_trace, oer_debug_control))]
            oer_trace::emit(&trace::TerminalPosted);
        }
        self.storage.shared.body.wake();
    }
}

pub(super) struct FrontHeld<'p, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) storage: &'static PortSharedStorage<B>,
    pub(super) commands: LocalSpscSender<'p, Cmd<'static, Id, D, B>, COMMANDS, CompactSpscCursor>,
    pub(super) events: LocalSpscReceiver<
        'p,
        Msg<'static, Id, D, B>,
        { TERMINALS + TX_TICKETS },
        CompactSpscCursor,
    >,
    pub(super) recycled:
        LocalSpscSender<'p, ReceiveBuffer<'static, Id, D::Receive>, TX_TICKETS, CompactSpscCursor>,
    pub(super) failed_command: &'p FailedCommand<'static, Id, D, B>,
    pub(super) failed_terminal: &'p FailedTerminal<'static, Id, D, B>,
    pub(super) gate: &'p Prepared<GateIssuer<'static, Part<Id, 1>>>,
    pub(super) identity: &'p Cell<u64>,
    pub(super) timer: D::Timer,
    pub(super) clock_replies: LocalSpscReceiver<'p, ClockAnswer, 1, CompactSpscCursor>,
    pub(super) clock_use: &'p Cell<ClockUse>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> core::ops::Deref for FrontHeld<'_, Id, D, B> {
    type Target = settings::Settings;
    fn deref(&self) -> &Self::Target {
        &self.storage.settings
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> FrontHeld<'_, Id, D, B> {
    pub(super) fn serial(&self) -> u64 {
        let serial = self.identity.get();
        match serial.checked_add(1) {
            Some(next) => self.identity.set(next),
            None => self.storage.poison(Fault::Capacity),
        }
        serial
    }
    pub(super) fn command(&self, command: Cmd<'static, Id, D, B>) {
        if let Err(error) = self.commands.try_send(command) {
            assert!(
                self.failed_command.set(error.0).is_ok(),
                "one failed command per installation"
            );
            self.storage.poison(Fault::Capacity);
        }
        self.storage.driver.wake();
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
impl<Id: SlotIdentity, D: Device<B>, const B: usize> FrontHeld<'_, Id, D, B> {
    pub(super) async fn now(&self) -> radio::PortResult<Ieee80211Instant, ClockError, Fault> {
        self.storage.check()?;
        if self.clock_use.get() == ClockUse::Cancelled && self.clock_replies.try_receive().is_ok() {
            self.clock_use.set(ClockUse::Idle);
        }
        // A competing/cancelled fresh latch never waits for another caller.
        if self.clock_use.get() != ClockUse::Idle {
            return Ok(Err(ClockError::Unavailable));
        }
        self.clock_use.set(ClockUse::Waiting);
        let _guard = ClockGuard(self.clock_use);
        self.clock_requested.set(true);
        self.storage.driver.wake();
        let mut bound = core::pin::pin!(
            self.timer
                .wait_until(self.timer.now().saturating_add(CLOCK_BOUND))
        );
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
