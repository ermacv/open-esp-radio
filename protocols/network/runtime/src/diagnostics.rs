//! The service's switchable diagnostics (docs/architecture.md, "Switchable
//! network-core diagnostics"): what the hot path's work was, counted only
//! while an image's [`Diagnostics`] says it is on, and switched at runtime.
//!
//! A service without diagnostics has `()`, whose [`Diagnostics::ENABLED`] is
//! `false`: no point asks it anything, and the points leave no code. With
//! one, each turn, and each pass of the wait that takes returns, asks
//! [`Diagnostics::on`] once; off, that is all it costs, beside the checks of
//! that answer at each point. On, the turn counts into its own [`Diagnosis`]
//! on the task's stack, reads the diagnostics' clock at each stage's end, and
//! hands the whole count over once at its end. Nothing the service or the
//! core decides reads any of it.

use core::cell::Cell;
use core::marker::PhantomData;

use oer_protocols_network_core::Count;
use oer_protocols_network_port::PlanStop;

/// An image's diagnostics of its network service.
pub trait Diagnostics {
    /// Whether these can ever be on; a service asks nothing of ones that
    /// cannot.
    const ENABLED: bool = true;

    /// Whether to count the turn, or the pass of the wait, about to begin:
    /// the one check an off diagnostics costs a turn.
    fn on(&mut self) -> bool;

    /// The diagnostics' own clock, in its own units, which only differences
    /// of are used. Read only while on: at a turn's start and at each of its
    /// stages' ends, and around a pass's return servicing.
    fn clock(&mut self) -> u64;

    /// What one turn, or one pass of the wait, counted while on.
    fn add(&mut self, counted: &Diagnosis);
}

/// The diagnostics that count nothing.
impl Diagnostics for () {
    const ENABLED: bool = false;

    #[inline(always)]
    fn on(&mut self) -> bool {
        false
    }

    #[inline(always)]
    fn clock(&mut self) -> u64 {
        0
    }

    #[inline(always)]
    fn add(&mut self, _counted: &Diagnosis) {}
}

/// The main stages of a turn, in order, and the wait's return servicing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Stage {
    /// From the turn's start: the port's link, the core's tick and the
    /// sockets' commands.
    Intake,
    /// The returns taken as one batch, and blocks given to waiting sockets.
    Returns,
    /// Received frames taken in.
    Receive,
    /// The port's notices handed to the policy.
    Notices,
    /// The producer's offers, and publishing: collections, plans and frames.
    Publish,
    /// Eviction, the delivery of results and datagrams, and the turn's end.
    Settle,
    /// A pass of the wait taking returns, with no turn.
    Waiting,
}

/// The number of [`Stage`]s.
pub const STAGES: usize = Stage::Waiting as usize + 1;

/// What the hot path did while the diagnostics were on. Each count wraps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Diagnosis {
    /// Turns counted: the stages' time is theirs.
    pub turns: u32,
    /// Full candidate collections: each a walk of all ready work into a
    /// transmit plan.
    pub collections: u32,
    /// Running places the collections' readiness walks visited, ready or
    /// not, on every pass a policy made over them.
    pub walked: u32,
    /// Ends the sockets' and the producer's ended chains told.
    pub told: u32,
    /// Frames a plan sent after its first: each a frame with no collection.
    pub continued: u32,
    /// Plans that stopped, by [`PlanStop`].
    pub stopped_spent: u32,
    pub stopped_changed: u32,
    pub stopped_grant: u32,
    pub stopped_account: u32,
    /// Return servicing that took something, in turns and passes of the
    /// wait: its batches, the blocks they brought home, and the receive
    /// loans they gave back to the pool's account.
    pub return_batches: u32,
    pub blocks_returned: u32,
    pub credits_restored: u32,
    /// Lending refusals for want of credit (`NotLent::NoCredit`), and summed
    /// over them the receive loans out of the pool's account at the
    /// refusal, and the lent buffers back with their port whose loans the
    /// service had not yet taken back, read right after the refusal's frame.
    pub no_credit: u32,
    pub no_credit_loans: u32,
    pub no_credit_unreturned: u32,
    /// Frames the core built whole in the port's slot.
    pub materialised: u32,
    /// The diagnostics' clock spent in each [`Stage`].
    pub stage_ticks: [u64; STAGES],
}

impl Diagnosis {
    /// Add `other`'s counts to these, each wrapping.
    pub fn accumulate(&mut self, other: &Self) {
        let Self {
            turns,
            collections,
            walked,
            told,
            continued,
            stopped_spent,
            stopped_changed,
            stopped_grant,
            stopped_account,
            return_batches,
            blocks_returned,
            credits_restored,
            no_credit,
            no_credit_loans,
            no_credit_unreturned,
            materialised,
            stage_ticks,
        } = self;
        for (count, more) in [
            (turns, other.turns),
            (collections, other.collections),
            (walked, other.walked),
            (told, other.told),
            (continued, other.continued),
            (stopped_spent, other.stopped_spent),
            (stopped_changed, other.stopped_changed),
            (stopped_grant, other.stopped_grant),
            (stopped_account, other.stopped_account),
            (return_batches, other.return_batches),
            (blocks_returned, other.blocks_returned),
            (credits_restored, other.credits_restored),
            (no_credit, other.no_credit),
            (no_credit_loans, other.no_credit_loans),
            (no_credit_unreturned, other.no_credit_unreturned),
            (materialised, other.materialised),
        ] {
            *count = count.wrapping_add(more);
        }
        for (ticks, more) in stage_ticks.iter_mut().zip(other.stage_ticks) {
            *ticks = ticks.wrapping_add(more);
        }
    }

    /// How many plans stopped for `stop`.
    #[must_use]
    pub const fn stopped(&self, stop: PlanStop) -> u32 {
        match stop {
            PlanStop::Spent => self.stopped_spent,
            PlanStop::Changed => self.stopped_changed,
            PlanStop::Grant => self.stopped_grant,
            PlanStop::Account => self.stopped_account,
        }
    }

    /// The clock spent in `stage`.
    #[must_use]
    pub fn ticks(&self, stage: Stage) -> u64 {
        self.stage_ticks
            .get(usize::from(stage as u8))
            .copied()
            .unwrap_or(0)
    }
}

/// What the core counts of a turn or a pass, while it is on.
pub(crate) struct Steps<D> {
    on: bool,
    walked: Cell<u32>,
    told: Cell<u32>,
    materialised: Cell<u32>,
    no_credit: Cell<u32>,
    loans: Cell<u32>,
    /// Refusals for want of credit whose unreturned loans are not yet read.
    unread: Cell<u32>,
    diagnostics: PhantomData<fn() -> D>,
}

fn bump(cell: &Cell<u32>, by: u32) {
    cell.set(cell.get().wrapping_add(by));
}

impl<D: Diagnostics> Count for Steps<D> {
    const ENABLED: bool = D::ENABLED;

    #[inline(always)]
    fn on(&self) -> bool {
        self.on
    }

    fn walked(&self) {
        bump(&self.walked, 1);
    }

    fn told(&self) {
        bump(&self.told, 1);
    }

    fn materialised(&self) {
        bump(&self.materialised, 1);
    }

    fn no_credit(&self, loans: usize) {
        bump(&self.no_credit, 1);
        bump(&self.unread, 1);
        bump(&self.loans, u32::try_from(loans).unwrap_or(u32::MAX));
    }
}

/// One turn's or one pass's count, on the network task's stack.
pub(crate) struct Tally<D> {
    steps: Steps<D>,
    counts: Diagnosis,
    /// The clock at the last stage's end.
    last: u64,
}

impl<D: Diagnostics> Tally<D> {
    const fn new(on: bool, last: u64) -> Self {
        Self {
            steps: Steps {
                on,
                walked: Cell::new(0),
                told: Cell::new(0),
                materialised: Cell::new(0),
                no_credit: Cell::new(0),
                loans: Cell::new(0),
                unread: Cell::new(0),
                diagnostics: PhantomData,
            },
            counts: Diagnosis {
                turns: 0,
                collections: 0,
                walked: 0,
                told: 0,
                continued: 0,
                stopped_spent: 0,
                stopped_changed: 0,
                stopped_grant: 0,
                stopped_account: 0,
                return_batches: 0,
                blocks_returned: 0,
                credits_restored: 0,
                no_credit: 0,
                no_credit_loans: 0,
                no_credit_unreturned: 0,
                materialised: 0,
                stage_ticks: [0; STAGES],
            },
            last,
        }
    }

    /// A tally that counts nothing, where no turn or pass counts.
    pub(crate) const fn off() -> Self {
        Self::new(false, 0)
    }

    /// The tally of a pass that may count, as `diagnostics` say now: a
    /// turn's or a wait's pass's.
    #[inline(always)]
    pub(crate) fn begin(diagnostics: &mut D) -> Self {
        if D::ENABLED && diagnostics.on() {
            Self::new(true, diagnostics.clock())
        } else {
            Self::off()
        }
    }

    /// The tally of a turn, which counts the turn itself.
    #[inline(always)]
    pub(crate) fn turn(diagnostics: &mut D) -> Self {
        let mut tally = Self::begin(diagnostics);
        tally.note(|counts| counts.turns = 1);
        tally
    }

    /// Whether it counts.
    #[inline(always)]
    pub(crate) const fn on(&self) -> bool {
        D::ENABLED && self.steps.on
    }

    /// What the core counts into.
    #[inline(always)]
    pub(crate) const fn steps(&self) -> &Steps<D> {
        &self.steps
    }

    /// Count with `note`, while on.
    #[inline(always)]
    pub(crate) fn note(&mut self, note: impl FnOnce(&mut Diagnosis)) {
        if self.on() {
            note(&mut self.counts);
        }
    }

    /// A plan stopped for `stop`.
    #[inline(always)]
    pub(crate) fn stopped(&mut self, stop: PlanStop) {
        self.note(|counts| {
            let count = match stop {
                PlanStop::Spent => &mut counts.stopped_spent,
                PlanStop::Changed => &mut counts.stopped_changed,
                PlanStop::Grant => &mut counts.stopped_grant,
                PlanStop::Account => &mut counts.stopped_account,
            };
            *count = count.wrapping_add(1);
        });
    }

    /// The refusals for want of credit since the last read, with the lent
    /// buffers back but not yet taken back that `unreturned` reads, once for
    /// all of them.
    #[inline(always)]
    pub(crate) fn read_unreturned(&mut self, unreturned: impl FnOnce() -> u32) {
        if self.on() {
            let unread = self.steps.unread.replace(0);
            if unread != 0 {
                let more = unreturned().saturating_mul(unread);
                self.counts.no_credit_unreturned =
                    self.counts.no_credit_unreturned.wrapping_add(more);
            }
        }
    }

    /// `stage` ended now, by `diagnostics`' clock.
    #[inline(always)]
    pub(crate) fn stage(&mut self, stage: Stage, diagnostics: &mut D) {
        if self.on() {
            let now = diagnostics.clock();
            if let Some(ticks) = self.counts.stage_ticks.get_mut(usize::from(stage as u8)) {
                *ticks = ticks.wrapping_add(now.wrapping_sub(self.last));
            }
            self.last = now;
        }
    }

    /// Hand what it counted to `diagnostics`, while on.
    #[inline(always)]
    pub(crate) fn end(self, diagnostics: &mut D) {
        if self.on() {
            let Self {
                steps, mut counts, ..
            } = self;
            counts.walked = steps.walked.get();
            counts.told = steps.told.get();
            counts.materialised = steps.materialised.get();
            counts.no_credit = steps.no_credit.get();
            counts.no_credit_loans = steps.loans.get();
            diagnostics.add(&counts);
        }
    }
}
