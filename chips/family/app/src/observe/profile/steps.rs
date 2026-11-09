//! The steps inside the network's turns: what its publication attempts
//! (each a choice, the port's answer to the chosen work and the policy's
//! answer to the port) and its notices (each handed to the policy, whose
//! settlement debits the frame's cost) cost, what the policy's choices pulled
//! and weighed, how publishing flushed the port, what came home, and the
//! loans at each frame left unlent for want of credit.
//!
//! A step's cycles run from the event that opens it to the event that
//! closes it, each read on the hart's cycle counter as the sink takes the
//! event: the service reports the same events at the same points whatever
//! its policy, so the steps of two policies have the same bounds. A step
//! includes interrupts taken within it and the sink's own cost for the
//! events inside it (the policy's events within a choice, the notices and
//! settlements within a notice phase), and about one event's cost of the
//! observer at its two ends. Choices and their attempts do not nest; a
//! debit nests within its notice phase. [`ProfileTally`] keeps the turn's
//! steps and adds them to [`TurnSteps`] at its end; frames left unlent count
//! at once.

use super::{ProfileCounter, ProfileTally, Words};

/// The steps a turn times, by their place in [`TurnSteps::steps`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Step {
    /// From an attempt's beginning, through the ready list and the policy's
    /// choice, to the choice's end.
    Choice = 0,
    /// From a work's choice to the port's acceptance or refusal of it: the
    /// fill of the frame included.
    Transmit = 1,
    /// From the port's end of the attempt to the policy's answer.
    Answer = 2,
    /// From the end of publishing to its flush of the port, when it moved
    /// anything.
    Flush = 3,
    /// From the beginning of the turn's notices to their end: the port's
    /// notices taken and handed on, their debits included.
    Notices = 4,
    /// From one notice handed to the policy to its settlement.
    Debit = 5,
}

/// The steps of [`Step`].
pub const STEPS: usize = 6;

/// How a choice ended ([`TurnSteps::chosen`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ChoiceEnd {
    /// It chose a work.
    Work = 0,
    /// It chose nothing from a list it pulled no work from.
    Empty = 1,
    /// It chose nothing although it pulled works.
    Withheld = 2,
    /// The policy cannot go on.
    Fault = 3,
}

/// The ends of [`ChoiceEnd`].
pub const CHOICE_ENDS: usize = 4;

/// Why a policy withheld work it could choose ([`TurnSteps::withheld`]):
/// no account could spend, every reservation was out, or the account granted
/// had no work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Withheld {
    NoGrant = 0,
    Capacity = 1,
    NoWork = 2,
}

/// The reasons of [`Withheld`].
pub const WITHHELD: usize = 3;

/// Why the port published nothing of a chosen work ([`TurnSteps::unsent`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Unsent {
    Stale = 0,
    Port = 1,
    Rejected = 2,
    SlotTooShort = 3,
    Declined = 4,
    FillMisreported = 5,
    PortContract = 6,
}

/// The kinds of [`Unsent`].
pub const UNSENT: usize = 7;

/// The bins of [`TurnSteps::examined_by_choice`]: 0, 1, 2, 3 to 4, 5 to 8,
/// 9 to 16, and 17 or more works.
pub const EXAMINED_BINS: usize = 7;
/// The bins of [`TurnSteps::accepted_by_flush`]: 0 to 7 frames, and 8 or
/// more.
pub const FLUSH_BINS: usize = 9;
/// The bins of [`TurnSteps::choices_by_turn`]: 0 to 8 choices, and 9 or
/// more.
pub const CHOICE_BINS: usize = 10;
/// The bins of [`TurnSteps::loans_by_turn`]: 1, 2, 3 to 4, 5 to 8, 9 to 16,
/// and 17 or more loans.
pub const LOAN_BINS: usize = 6;
/// The bins of [`TurnSteps::returned_at_drop`]: 0, 1, 2, 3 to 4, 5 to 8, and
/// 9 or more loans.
pub const RETURNED_BINS: usize = 6;

/// What the steps of the network's turns did since boot, as sums.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TurnSteps {
    /// Each step's occurrences ([`Step`]): their count, cycles and cycles
    /// squared.
    pub steps: [[u64; 3]; STEPS],
    /// Each step by turn: the turns it ran in, and the sums of each such
    /// turn's cycles in it and of their squares.
    pub turn_steps: [[u64; 3]; STEPS],
    /// What the choices pulled and the policy weighed: the works pulled
    /// from the ready list over every pass, the passes begun, the candidates
    /// the policy read, the accounts summed over its choices, and the
    /// choices it widened past one pass.
    pub weighed: [u64; 5],
    /// The choices by how they ended ([`ChoiceEnd`]).
    pub chosen: [u64; CHOICE_ENDS],
    /// The choices the policy withheld work in, by why ([`Withheld`]).
    pub withheld: [u64; WITHHELD],
    /// The chosen works the port published nothing of, by why ([`Unsent`]).
    pub unsent: [u64; UNSENT],
    /// The choices by the works they pulled ([`EXAMINED_BINS`]).
    pub examined_by_choice: [u64; EXAMINED_BINS],
    /// The flushes by the frames the port accepted in their turn
    /// ([`FLUSH_BINS`]).
    pub accepted_by_flush: [u64; FLUSH_BINS],
    /// The turns by their choices ([`CHOICE_BINS`]).
    pub choices_by_turn: [u64; CHOICE_BINS],
    /// What came home before the turns' frames: the loans given back to the
    /// account, the turns that gave any back, the blocks the return lane
    /// brought, and the turns that brought any.
    pub home: [u64; 4],
    /// Turns with none of the profile's four items that brought something
    /// home: their count, cycles, cycles squared and loans given back.
    pub home_only: [u64; 4],
    /// The turns that gave loans back, by how many ([`LOAN_BINS`]).
    pub loans_by_turn: [u64; LOAN_BINS],
    /// The takes of a received frame that left frames unlent for want of
    /// credit: their count, the frames, the sums of the loans out and of the
    /// loans back with their port and not yet given back, and the takes with
    /// any such loan.
    pub no_credit: [u64; 5],
    /// Those takes by the loans back and not yet given back
    /// ([`RETURNED_BINS`]).
    pub returned_at_drop: [u64; RETURNED_BINS],
}

/// The first word of each field of a [`TurnSteps`].
mod word {
    use super::*;
    pub const STEPS: usize = 0;
    pub const TURN_STEPS: usize = STEPS + 3 * super::STEPS;
    pub const WEIGHED: usize = TURN_STEPS + 3 * super::STEPS;
    pub const CHOSEN: usize = WEIGHED + 5;
    pub const WITHHELD: usize = CHOSEN + CHOICE_ENDS;
    pub const UNSENT: usize = WITHHELD + super::WITHHELD;
    pub const EXAMINED: usize = UNSENT + super::UNSENT;
    pub const FLUSH: usize = EXAMINED + EXAMINED_BINS;
    pub const CHOICES: usize = FLUSH + FLUSH_BINS;
    pub const HOME: usize = CHOICES + CHOICE_BINS;
    pub const HOME_ONLY: usize = HOME + 4;
    pub const LOANS: usize = HOME_ONLY + 4;
    pub const NO_CREDIT: usize = LOANS + LOAN_BINS;
    pub const RETURNED: usize = NO_CREDIT + 5;
    pub const END: usize = RETURNED + RETURNED_BINS;
}

/// The 64-bit words of a [`TurnSteps`], in its fields' order.
pub const STEP_WORDS: usize = word::END;

impl TurnSteps {
    /// No step yet.
    pub const NONE: Self = Self {
        steps: [[0; 3]; STEPS],
        turn_steps: [[0; 3]; STEPS],
        weighed: [0; 5],
        chosen: [0; CHOICE_ENDS],
        withheld: [0; WITHHELD],
        unsent: [0; UNSENT],
        examined_by_choice: [0; EXAMINED_BINS],
        accepted_by_flush: [0; FLUSH_BINS],
        choices_by_turn: [0; CHOICE_BINS],
        home: [0; 4],
        home_only: [0; 4],
        loans_by_turn: [0; LOAN_BINS],
        no_credit: [0; 5],
        returned_at_drop: [0; RETURNED_BINS],
    };

    /// The steps from their words, in their fields' order.
    pub fn from_words(words: &[u64; STEP_WORDS]) -> Self {
        let at = |index: usize| words.get(index).copied().unwrap_or(0);
        let from = |first: usize| move |index: usize| at(first.saturating_add(index));
        let rows = |first: usize| {
            move |row: usize| -> [u64; 3] {
                core::array::from_fn(|column| {
                    at(first
                        .saturating_add(row.saturating_mul(3))
                        .saturating_add(column))
                })
            }
        };
        Self {
            steps: core::array::from_fn(rows(word::STEPS)),
            turn_steps: core::array::from_fn(rows(word::TURN_STEPS)),
            weighed: core::array::from_fn(from(word::WEIGHED)),
            chosen: core::array::from_fn(from(word::CHOSEN)),
            withheld: core::array::from_fn(from(word::WITHHELD)),
            unsent: core::array::from_fn(from(word::UNSENT)),
            examined_by_choice: core::array::from_fn(from(word::EXAMINED)),
            accepted_by_flush: core::array::from_fn(from(word::FLUSH)),
            choices_by_turn: core::array::from_fn(from(word::CHOICES)),
            home: core::array::from_fn(from(word::HOME)),
            home_only: core::array::from_fn(from(word::HOME_ONLY)),
            loans_by_turn: core::array::from_fn(from(word::LOANS)),
            no_credit: core::array::from_fn(from(word::NO_CREDIT)),
            returned_at_drop: core::array::from_fn(from(word::RETURNED)),
        }
    }

    /// The steps' words, in their fields' order.
    pub fn words(&self) -> [u64; STEP_WORDS] {
        let mut words = [0; STEP_WORDS];
        let fields = self
            .steps
            .iter()
            .flatten()
            .chain(self.turn_steps.iter().flatten())
            .chain(&self.weighed)
            .chain(&self.chosen)
            .chain(&self.withheld)
            .chain(&self.unsent)
            .chain(&self.examined_by_choice)
            .chain(&self.accepted_by_flush)
            .chain(&self.choices_by_turn)
            .chain(&self.home)
            .chain(&self.home_only)
            .chain(&self.loans_by_turn)
            .chain(&self.no_credit)
            .chain(&self.returned_at_drop);
        for (place, value) in words.iter_mut().zip(fields) {
            *place = *value;
        }
        words
    }

    /// The sums between readings `start` and `end`, wrapping at 2^64.
    pub fn window(start: &Self, end: &Self) -> Self {
        let (start, end) = (start.words(), end.words());
        let mut words = [0; STEP_WORDS];
        for ((place, end), start) in words.iter_mut().zip(end).zip(start) {
            *place = end.wrapping_sub(start);
        }
        Self::from_words(&words)
    }
}

/// The bin of `count` among 0, 1, 2, 3 to 4, 5 to 8, 9 to 16 and more, at
/// most `last`.
fn doubling_bin(count: u32, last: usize) -> usize {
    let bin = match count {
        0..=2 => count as usize,
        _ => count
            .saturating_sub(1)
            .checked_ilog2()
            .map_or(0, |log| (log as usize).saturating_add(2)),
    };
    bin.min(last)
}

/// The bin of `count` by one, the last `last` and more.
fn linear_bin(count: u32, last: usize) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX).min(last)
}

/// The turn's steps under way.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StepTally {
    /// The open step of the attempt under way, and when it opened.
    attempt: Option<(Step, u64)>,
    /// When the open notice phase, debit and flush opened.
    notices: Option<u64>,
    debit: Option<u64>,
    flush: Option<u64>,
    /// Each step's count and cycles within the turn.
    counts: [u32; STEPS],
    cycles: [u64; STEPS],
    choices: u32,
    loans: u32,
    blocks: u32,
}

impl StepTally {
    pub(super) const fn new() -> Self {
        Self {
            attempt: None,
            notices: None,
            debit: None,
            flush: None,
            counts: [0; STEPS],
            cycles: [0; STEPS],
            choices: 0,
            loans: 0,
            blocks: 0,
        }
    }

    /// Step `step`, opened at `from`, closed at `now`.
    fn close(&mut self, words: &Words<STEP_WORDS>, step: Step, from: u64, now: u64) {
        let cycles = u64::from(u32::try_from(now.wrapping_sub(from)).unwrap_or(u32::MAX));
        let first = word::STEPS.saturating_add(3 * step as usize);
        words.add(first, 1);
        words.add(first.saturating_add(1), cycles);
        words.add(first.saturating_add(2), cycles.wrapping_mul(cycles));
        if let Some(count) = self.counts.get_mut(step as usize) {
            *count = count.saturating_add(1);
        }
        if let Some(sum) = self.cycles.get_mut(step as usize) {
            *sum = sum.wrapping_add(cycles);
        }
    }

    /// The turn ended, its cycles `cycles`, with none of the profile's four
    /// items when `none`.
    pub(super) fn ended(&mut self, words: &Words<STEP_WORDS>, none: bool, cycles: u64) {
        for step in 0..STEPS {
            let (Some(count), Some(sum)) = (self.counts.get(step), self.cycles.get(step)) else {
                continue;
            };
            if *count == 0 {
                continue;
            }
            let first = word::TURN_STEPS.saturating_add(3 * step);
            words.add(first, 1);
            words.add(first.saturating_add(1), *sum);
            words.add(first.saturating_add(2), sum.wrapping_mul(*sum));
        }
        words.add(
            word::CHOICES.saturating_add(linear_bin(self.choices, CHOICE_BINS - 1)),
            1,
        );
        if self.loans != 0 {
            words.add(word::HOME, u64::from(self.loans));
            words.add(word::HOME.saturating_add(1), 1);
            words.add(
                word::LOANS.saturating_add(doubling_bin(self.loans, LOAN_BINS).saturating_sub(1)),
                1,
            );
        }
        if self.blocks != 0 {
            words.add(word::HOME.saturating_add(2), u64::from(self.blocks));
            words.add(word::HOME.saturating_add(3), 1);
        }
        if none && (self.loans != 0 || self.blocks != 0) {
            words.add(word::HOME_ONLY, 1);
            words.add(word::HOME_ONLY.saturating_add(1), cycles);
            words.add(
                word::HOME_ONLY.saturating_add(2),
                cycles.wrapping_mul(cycles),
            );
            words.add(word::HOME_ONLY.saturating_add(3), u64::from(self.loans));
        }
        *self = Self::new();
    }
}

impl ProfileTally {
    /// A publication attempt began at cycle `now`.
    pub fn choice_began(&mut self, now: u64) {
        self.steps.attempt = Some((Step::Choice, now));
    }

    /// The policy weighed `candidates` works in `accounts` accounts, past
    /// one pass when `widened`.
    pub fn weighed(
        &mut self,
        counter: &ProfileCounter,
        candidates: u16,
        accounts: u8,
        widened: bool,
    ) {
        let words = &counter.steps;
        words.add(word::WEIGHED.saturating_add(2), u64::from(candidates));
        words.add(word::WEIGHED.saturating_add(3), u64::from(accounts));
        words.add(word::WEIGHED.saturating_add(4), u64::from(widened));
    }

    /// The policy withheld work for `why`.
    pub fn withheld(&mut self, counter: &ProfileCounter, why: Withheld) {
        counter
            .steps
            .add(word::WITHHELD.saturating_add(why as usize), 1);
    }

    /// The choice ended at cycle `now`, having pulled `examined` works over
    /// `walks` passes; a chosen work goes to the port from here.
    pub fn chose(
        &mut self,
        counter: &ProfileCounter,
        now: u64,
        examined: u16,
        walks: u8,
        chosen: Option<bool>,
    ) {
        let words = &counter.steps;
        if let Some((Step::Choice, from)) = self.steps.attempt.take() {
            self.steps.close(words, Step::Choice, from, now);
        }
        self.steps.choices = self.steps.choices.saturating_add(1);
        words.add(word::WEIGHED, u64::from(examined));
        words.add(word::WEIGHED.saturating_add(1), u64::from(walks));
        words.add(
            word::EXAMINED.saturating_add(doubling_bin(u32::from(examined), EXAMINED_BINS - 1)),
            1,
        );
        let end = match chosen {
            Some(true) => {
                self.steps.attempt = Some((Step::Transmit, now));
                ChoiceEnd::Work
            }
            Some(false) if examined == 0 => ChoiceEnd::Empty,
            Some(false) => ChoiceEnd::Withheld,
            None => ChoiceEnd::Fault,
        };
        words.add(word::CHOSEN.saturating_add(end as usize), 1);
    }

    /// The port ended the attempt at cycle `now`: it accepted the frame, or
    /// published nothing for `unsent`. The policy's answer follows.
    pub fn transmitted(&mut self, counter: &ProfileCounter, now: u64, unsent: Option<Unsent>) {
        let words = &counter.steps;
        if let Some((Step::Transmit, from)) = self.steps.attempt.take() {
            self.steps.close(words, Step::Transmit, from, now);
        }
        match unsent {
            None => self.accepted(),
            Some(why) => words.add(word::UNSENT.saturating_add(why as usize), 1),
        }
        self.steps.attempt = Some((Step::Answer, now));
    }

    /// The policy answered the attempt at cycle `now`.
    pub fn answered(&mut self, counter: &ProfileCounter, now: u64) {
        if let Some((Step::Answer, from)) = self.steps.attempt.take() {
            self.steps.close(&counter.steps, Step::Answer, from, now);
        }
    }

    /// Publishing ended at cycle `now`; a flush may follow.
    pub fn published(&mut self, now: u64) {
        self.steps.attempt = None;
        self.steps.flush = Some(now);
    }

    /// Publishing flushed the port at cycle `now`.
    pub fn flushed(&mut self, counter: &ProfileCounter, now: u64) {
        let words = &counter.steps;
        if let Some(from) = self.steps.flush.take() {
            self.steps.close(words, Step::Flush, from, now);
        }
        words.add(
            word::FLUSH.saturating_add(linear_bin(self.accepted, FLUSH_BINS - 1)),
            1,
        );
    }

    /// The turn's notices began at cycle `now`.
    pub fn notices_began(&mut self, now: u64) {
        self.steps.notices = Some(now);
    }

    /// The turn handed the policy one notice at cycle `now`.
    pub fn notice_at(&mut self, now: u64) {
        self.noticed();
        self.steps.debit = Some(now);
    }

    /// The policy settled the notice at cycle `now`.
    pub fn settled(&mut self, counter: &ProfileCounter, now: u64) {
        if let Some(from) = self.steps.debit.take() {
            self.steps.close(&counter.steps, Step::Debit, from, now);
        }
    }

    /// The turn's notices ended at cycle `now`.
    pub fn notices_ended(&mut self, counter: &ProfileCounter, now: u64) {
        if let Some(from) = self.steps.notices.take() {
            self.steps.close(&counter.steps, Step::Notices, from, now);
        }
    }

    /// The turn brought `blocks` blocks and `loans` loans home.
    pub fn home(&mut self, blocks: u16, loans: u16) {
        self.steps.blocks = self.steps.blocks.saturating_add(u32::from(blocks));
        self.steps.loans = self.steps.loans.saturating_add(u32::from(loans));
    }

    /// A take of a received frame left `drops` frames unlent for want of
    /// credit, with `outstanding` loans out and `returned` back with their
    /// port and not yet given back.
    pub fn no_credit(
        &mut self,
        counter: &ProfileCounter,
        drops: u8,
        outstanding: u16,
        returned: u16,
    ) {
        let words = &counter.steps;
        words.add(word::NO_CREDIT, 1);
        words.add(word::NO_CREDIT.saturating_add(1), u64::from(drops));
        words.add(word::NO_CREDIT.saturating_add(2), u64::from(outstanding));
        words.add(word::NO_CREDIT.saturating_add(3), u64::from(returned));
        words.add(word::NO_CREDIT.saturating_add(4), u64::from(returned != 0));
        words.add(
            word::RETURNED.saturating_add(doubling_bin(u32::from(returned), RETURNED_BINS - 1)),
            1,
        );
    }
}

/// The bins of [`Intakes::msdus_by_intake`]: 0, 1, 2, 3 to 4, 5 to 8, 9 to
/// 16, and 17 or more MSDUs.
pub const INTAKE_BINS: usize = 7;

/// The datapath actor's intakes of the network's MSDUs into its standby
/// arena since boot, each while the link carried data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Intakes {
    /// Intakes, those that took any MSDU, and the MSDUs they took.
    pub intakes: u32,
    pub taking: u32,
    pub msdus: u32,
    pub msdus_by_intake: [u32; INTAKE_BINS],
}

/// The words of [`IntakeCounter`].
const INTAKE_WORDS: usize = 3 + INTAKE_BINS;

impl Intakes {
    /// No intake yet.
    pub const NONE: Self = Self {
        intakes: 0,
        taking: 0,
        msdus: 0,
        msdus_by_intake: [0; INTAKE_BINS],
    };

    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        let (start, end) = (start.words(), end.words());
        let mut words = [0; INTAKE_WORDS];
        for ((place, end), start) in words.iter_mut().zip(end).zip(start) {
            *place = end.wrapping_sub(start);
        }
        Self::from_words(&words)
    }

    fn words(&self) -> [u32; INTAKE_WORDS] {
        let mut words = [0; INTAKE_WORDS];
        let fields = [self.intakes, self.taking, self.msdus]
            .into_iter()
            .chain(self.msdus_by_intake);
        for (place, value) in words.iter_mut().zip(fields) {
            *place = value;
        }
        words
    }

    fn from_words(words: &[u32; INTAKE_WORDS]) -> Self {
        let at = |index: usize| words.get(index).copied().unwrap_or(0);
        Self {
            intakes: at(0),
            taking: at(1),
            msdus: at(2),
            msdus_by_intake: core::array::from_fn(|bin| at(3usize.saturating_add(bin))),
        }
    }
}

/// The datapath actor's intakes, counted word by word; the actor is their
/// one writer, and a reader takes each word on its own.
pub struct IntakeCounter {
    counts: [core::sync::atomic::AtomicU32; INTAKE_WORDS],
}

impl IntakeCounter {
    pub const fn new() -> Self {
        Self {
            counts: [const { core::sync::atomic::AtomicU32::new(0) }; INTAKE_WORDS],
        }
    }

    fn bump(&self, index: usize, by: u32) {
        if let Some(count) = self.counts.get(index) {
            super::super::bump(count, by);
        }
    }

    /// One intake took `msdus`.
    pub fn intake(&self, msdus: u8) {
        self.bump(0, 1);
        self.bump(1, u32::from(msdus != 0));
        self.bump(2, u32::from(msdus));
        self.bump(
            3usize.saturating_add(doubling_bin(u32::from(msdus), INTAKE_BINS - 1)),
            1,
        );
    }

    /// The counts now.
    pub fn read(&self) -> Intakes {
        let words: [u32; INTAKE_WORDS] = core::array::from_fn(|index| {
            self.counts
                .get(index)
                .map_or(0, |count| count.load(core::sync::atomic::Ordering::Relaxed))
        });
        Intakes::from_words(&words)
    }
}

impl Default for IntakeCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The station's frame costs computed since boot, each a miss of its
/// one-entry cache: least grants and ceilings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AirCosts {
    pub least: u32,
    pub ceiling: u32,
}

impl AirCosts {
    /// No cost computed yet.
    pub const NONE: Self = Self {
        least: 0,
        ceiling: 0,
    };

    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        Self {
            least: end.least.wrapping_sub(start.least),
            ceiling: end.ceiling.wrapping_sub(start.ceiling),
        }
    }
}

/// The station's computed frame costs; the network's task, where its
/// egress asks them, is their one writer.
pub struct AirCostCounter {
    least: core::sync::atomic::AtomicU32,
    ceiling: core::sync::atomic::AtomicU32,
}

impl AirCostCounter {
    pub const fn new() -> Self {
        Self {
            least: core::sync::atomic::AtomicU32::new(0),
            ceiling: core::sync::atomic::AtomicU32::new(0),
        }
    }

    /// A least grant was computed.
    pub fn least(&self) {
        super::super::bump(&self.least, 1);
    }

    /// A ceiling was computed.
    pub fn ceiling(&self) {
        super::super::bump(&self.ceiling, 1);
    }

    /// The counts now.
    pub fn read(&self) -> AirCosts {
        use core::sync::atomic::Ordering;
        AirCosts {
            least: self.least.load(Ordering::Relaxed),
            ceiling: self.ceiling.load(Ordering::Relaxed),
        }
    }
}

impl Default for AirCostCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
