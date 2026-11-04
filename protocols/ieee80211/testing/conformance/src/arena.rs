//! The TX arena suite: intake, publication, settlement and withdrawal of
//! any [`TxArena`] backend.
//!
//! A fixture lends its backend, fresh memories and publications at rates its
//! phase checked, writes each frame with a tag it can read back from the
//! memory and from what its hardware published, and arranges the air's
//! answer to the next publication. The runner drives the port through the
//! caller's operations and compares returned memories, published frames and
//! completions with the contract. A BlockAck that misses chosen MPDUs, a
//! protection failure and a fault need an injected answer.

use super::{Environment, Execution, ModelOnlyReason, Report};
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, Arena, ArenaId, ArenaPublication, AttemptEnd, CompletedOf, FrameMask,
    IntakeRefused, PublishError, PublishRefused, RetainError, TxArena,
};

/// The memory type of a fixture's backend.
pub type Memory<'p, 'id, F> = <<F as Fixture<'p, 'id>>::Port as TxArena<'p, 'id>>::Memory;
/// The publication type of a fixture's backend.
pub type Publication<'p, 'id, F> =
    ArenaPublication<'p, <<F as Fixture<'p, 'id>>::Port as TxArena<'p, 'id>>::Caps>;

/// One arena case, each on a fresh fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Fill an arena until it refuses a frame with its memory, then withdraw.
    IntakeUntilFull,
    /// A partial BlockAck: retain keeps the missing frames in order and the
    /// next publication carries only them, with their Retry bit.
    PartialBlockAck,
    /// Release returns every memory in position order, after an A-MPDU and
    /// after a single MPDU.
    ReleaseOrder,
    /// Withdrawal returns an unpublished arena's memories in order.
    Withdraw,
    /// Empty and concurrent publications are refused with the publication
    /// returned.
    PublishRefusals,
    /// A single MPDU sends only position 0 and completes with every frame:
    /// keeping the others drains the arena one MPDU at a time, a kept frame
    /// that did not go without its Retry bit.
    SingleDrain,
    /// A protection failure republishes without the Retry bit, a missing
    /// response with it.
    ProtectionRepublish,
    /// With a whole BlockAck, the other arena goes on the air before the
    /// completed one settles, and each returns its memories in order.
    PublishAheadOfSettlement,
    /// A fault fails every later operation and returns refused memory.
    Fault,
}

impl Case {
    /// Every arena case, in suite order.
    pub const ALL: [Self; 9] = [
        Self::IntakeUntilFull,
        Self::PartialBlockAck,
        Self::ReleaseOrder,
        Self::Withdraw,
        Self::PublishRefusals,
        Self::SingleDrain,
        Self::ProtectionRepublish,
        Self::PublishAheadOfSettlement,
        Self::Fault,
    ];

    /// Whether real air can supply the case's answers.
    pub const fn execution(self) -> Execution {
        match self {
            Self::PartialBlockAck => Execution::ModelOnly(ModelOnlyReason::PartialBlockAck),
            Self::ProtectionRepublish => Execution::ModelOnly(ModelOnlyReason::ProtectionFailure),
            Self::Fault => Execution::ModelOnly(ModelOnlyReason::PoisonedBackend),
            _ => Execution::Hardware,
        }
    }
}

/// The air's answer a case asks for, for the publication on the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Answer {
    /// The recipient received the frames at these positions and answers
    /// the A-MPDU with a BlockAck.
    BlockAck(FrameMask),
    /// An ACK answers the single MPDU.
    Ack,
    /// Nothing answers the PPDU.
    NoResponse,
    /// The protection exchange draws no CTS.
    ProtectionFailure,
    /// The backend faults when it serves the publication.
    Fault,
}

/// What a fixture's hardware observer read of one publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Published {
    /// Each frame's tag, positions `0..count`.
    pub tags: [u8; ARENA_FRAMES as usize],
    /// The number of frames.
    pub count: u8,
    /// The positions whose frames carried the Retry bit.
    pub retried: FrameMask,
    /// Whether the PPDU was an A-MPDU.
    pub aggregate: bool,
}

impl Published {
    /// The tags of the published frames.
    pub fn tags(&self) -> &[u8] {
        self.tags.get(..usize::from(self.count)).unwrap_or(&[])
    }
}

/// A backend under test with its memories, rates and air.
pub trait Fixture<'p, 'id: 'p> {
    /// The backend.
    type Port: TxArena<'p, 'id>;
    /// The backend, freshly constructed for this fixture.
    fn port(&mut self) -> &mut Self::Port;
    /// Fresh memory for the frame tagged `tag`, which [`Self::tag`] reads
    /// back even before the frame is written; `None` when the fixture has
    /// no more.
    fn memory(&mut self, tag: u8) -> Option<Memory<'p, 'id, Self>>;
    /// Write the frame tagged `tag` from the region's first octet and
    /// return its length, as an intake writer does.
    fn write(tag: u8, region: &mut [u8]) -> Option<u16>;
    /// The tag of the frame in a returned memory.
    fn tag(memory: &Memory<'p, 'id, Self>) -> Option<u8>;
    /// A single-MPDU publication of `arena`.
    fn single(&self, arena: Arena) -> Publication<'p, 'id, Self>;
    /// An A-MPDU publication of `arena`.
    fn aggregate(&self, arena: Arena) -> Publication<'p, 'id, Self>;
    /// Arrange `answer` for the publication on the air.
    fn answer(&mut self, answer: Answer);
    /// The backend's events for its next service.
    fn events(&mut self) -> <Self::Port as TxArena<'p, 'id>>::Events;
    /// The fixture's clock.
    fn now(&mut self) -> Instant;
    /// What the hardware read of the last publication.
    fn published(&self) -> Option<Published>;
}

/// A mismatch between the arena contract and the backend's behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The fixture could not supply arenas, memory or capacity for the case.
    Fixture,
    /// Intake named a wrong position, refused wrongly or lost the memory.
    Intake,
    /// Memories came back wrong, incomplete or out of position order.
    Owners,
    /// The published frames, their order or their format differ.
    Publication,
    /// A Retry bit is set or missing.
    Retry,
    /// A publication or retention was refused wrongly or not refused.
    Refusal,
    /// The completion's arena, frame count or end differs.
    Completion,
    /// A fault was not reported, or an operation after it succeeded.
    Fault,
}

/// Services before a published arena must have completed.
pub const SERVICE_BOUND: usize = 64;

/// Frame tags in order: one arena's frames and two more.
struct Tags {
    tags: [u8; ARENA_FRAMES as usize + 2],
    count: usize,
}

impl Tags {
    const fn new() -> Self {
        Self {
            tags: [0; ARENA_FRAMES as usize + 2],
            count: 0,
        }
    }
    /// Tags `base..base + count`.
    fn range(base: u8, count: u8) -> Self {
        let mut tags = Self::new();
        for offset in 0..count {
            tags.push(base.wrapping_add(offset));
        }
        tags
    }
    /// The tags of `self` at the positions of `mask`, in position order.
    fn select(&self, mask: FrameMask) -> Self {
        let mut selected = Self::new();
        for index in mask {
            if let Some(&tag) = self.get().get(usize::from(index.get())) {
                selected.push(tag);
            }
        }
        selected
    }
    /// Append `tag`; `false` when no room is left.
    fn push(&mut self, tag: u8) -> bool {
        let Some(slot) = self.tags.get_mut(self.count) else {
            return false;
        };
        *slot = tag;
        self.count += 1;
        true
    }
    fn get(&self) -> &[u8] {
        self.tags.get(..self.count).unwrap_or(&[])
    }
}

/// Tags of returned memories, in return order.
struct Returned {
    tags: Tags,
    unknown: bool,
}

impl Returned {
    const fn new() -> Self {
        Self {
            tags: Tags::new(),
            unknown: false,
        }
    }
    fn push(&mut self, tag: Option<u8>) {
        if !tag.is_some_and(|tag| self.tags.push(tag)) {
            self.unknown = true;
        }
    }
    fn expect(&self, tags: &[u8]) -> Result<(), Violation> {
        if self.unknown || self.tags.get() != tags {
            Err(Violation::Owners)
        } else {
            Ok(())
        }
    }
}

type Done<'p, 'id, F> = CompletedOf<'p, 'id, <F as Fixture<'p, 'id>>::Port>;

fn arenas<'p, 'id: 'p, F: Fixture<'p, 'id>>(fixture: &mut F) -> Result<[Arena; 2], Violation> {
    let arenas = fixture.port().arenas().ok_or(Violation::Fixture)?;
    if fixture.port().arenas().is_some() {
        return Err(Violation::Refusal);
    }
    Ok(arenas)
}

fn fill<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    arena: &Arena,
    tags: &[u8],
) -> Result<(), Violation> {
    for &tag in tags {
        let memory = fixture.memory(tag).ok_or(Violation::Fixture)?;
        let expected = fixture.port().len(arena.id());
        let index = fixture
            .port()
            .intake(arena, memory, |region| F::write(tag, region))
            .map_err(|_| Violation::Intake)?;
        if index.get() != expected || fixture.port().len(arena.id()) != expected + 1 {
            return Err(Violation::Intake);
        }
    }
    Ok(())
}

fn publish<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    publication: Publication<'p, 'id, F>,
) -> Result<(), Violation> {
    let id = publication.arena.id();
    let now = fixture.now();
    fixture
        .port()
        .publish(publication, now)
        .map_err(|_| Violation::Refusal)?;
    if fixture.port().on_air() != Some(id) || fixture.port().deadline().is_none() {
        return Err(Violation::Publication);
    }
    Ok(())
}

fn serve<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    arena: ArenaId,
    frames: u8,
) -> Result<Done<'p, 'id, F>, Violation> {
    for _ in 0..SERVICE_BOUND {
        let events = fixture.events();
        let now = fixture.now();
        match fixture.port().service(events, now) {
            Ok(Some(done)) => {
                if done.id() != arena
                    || done.frames() != frames
                    || fixture.port().on_air().is_some()
                    || fixture.port().len(arena) != frames
                {
                    return Err(Violation::Completion);
                }
                return Ok(done);
            }
            Ok(None) => {}
            Err(_) => return Err(Violation::Fault),
        }
    }
    Err(Violation::Completion)
}

fn release<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    done: Done<'p, 'id, F>,
    tags: &[u8],
) -> Result<Arena, Violation> {
    let id = done.id();
    let mut returned = Returned::new();
    let arena = fixture
        .port()
        .release(done, |memory| returned.push(F::tag(&memory)))
        .map_err(|_| Violation::Fault)?;
    returned.expect(tags)?;
    if arena.id() != id || fixture.port().len(id) != 0 {
        return Err(Violation::Owners);
    }
    Ok(arena)
}

fn withdraw<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    arena: &Arena,
    tags: &[u8],
) -> Result<(), Violation> {
    let mut returned = Returned::new();
    fixture
        .port()
        .withdraw(arena, |memory| returned.push(F::tag(&memory)))
        .map_err(|_| Violation::Fault)?;
    returned.expect(tags)?;
    if fixture.port().len(arena.id()) != 0 {
        return Err(Violation::Owners);
    }
    Ok(())
}

fn expect_published<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &F,
    tags: &[u8],
    aggregate: bool,
    retried: bool,
) -> Result<(), Violation> {
    let published = fixture.published().ok_or(Violation::Publication)?;
    if published.tags() != tags || published.aggregate != aggregate {
        return Err(Violation::Publication);
    }
    let all = FrameMask::first(published.count).ok_or(Violation::Publication)?;
    let expected = if retried { all } else { FrameMask::EMPTY };
    if published.retried != expected {
        return Err(Violation::Retry);
    }
    Ok(())
}

/// Run one case on a fresh fixture and compare its observations.
pub fn exercise<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    case: Case,
) -> Result<(), Violation> {
    let [first, second] = arenas(fixture)?;
    let capacity = fixture.port().capacity();
    if capacity == 0 || capacity > ARENA_FRAMES {
        return Err(Violation::Fixture);
    }
    match case {
        Case::IntakeUntilFull => {
            let full = Tags::range(0, capacity);
            fill(fixture, &first, full.get())?;
            let memory = fixture.memory(capacity).ok_or(Violation::Fixture)?;
            match fixture
                .port()
                .intake(&first, memory, |region| F::write(capacity, region))
            {
                Err((memory, IntakeRefused::Full)) if F::tag(&memory) == Some(capacity) => {}
                _ => return Err(Violation::Intake),
            }
            if fixture.port().len(first.id()) != capacity {
                return Err(Violation::Intake);
            }
            let other = capacity.wrapping_add(1);
            fill(fixture, &second, &[other])?;
            withdraw(fixture, &first, full.get())?;
            withdraw(fixture, &second, &[other])
        }
        Case::PartialBlockAck => {
            let frames = capacity.min(5);
            if frames < 3 {
                return Err(Violation::Fixture);
            }
            let sent = Tags::range(10, frames);
            fill(fixture, &first, sent.get())?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            expect_published(fixture, sent.get(), true, false)?;
            let all = FrameMask::first(frames).ok_or(Violation::Fixture)?;
            let even = FrameMask::from_bits(0x5555_5555_5555_5555 & all.bits())
                .ok_or(Violation::Fixture)?;
            fixture.answer(Answer::BlockAck(even));
            let done = serve(fixture, ArenaId::First, frames)?;
            if !matches!(done.end(), AttemptEnd::Answered { .. }) || done.air().is_none() {
                return Err(Violation::Completion);
            }
            let outside = FrameMask::first(frames + 1).ok_or(Violation::Fixture)?;
            let done = match fixture.port().retain(done, outside, drop) {
                Err(RetainError::Outside(done)) => done,
                _ => return Err(Violation::Refusal),
            };
            let retry = all.difference(even);
            let mut returned = Returned::new();
            let first = fixture
                .port()
                .retain(done, retry, |memory| returned.push(F::tag(&memory)))
                .map_err(|_| Violation::Fault)?;
            let missing = sent.select(retry);
            returned.expect(sent.select(even).get())?;
            if fixture.port().len(first.id()) != retry.count() {
                return Err(Violation::Owners);
            }
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            expect_published(fixture, missing.get(), true, true)?;
            fixture.answer(Answer::BlockAck(
                FrameMask::first(retry.count()).ok_or(Violation::Fixture)?,
            ));
            let done = serve(fixture, ArenaId::First, retry.count())?;
            let first = release(fixture, done, missing.get())?;
            withdraw(fixture, &first, &[])?;
            withdraw(fixture, &second, &[])
        }
        Case::ReleaseOrder => {
            let frames = capacity.min(4);
            let sent = Tags::range(20, frames);
            fill(fixture, &first, sent.get())?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            fixture.answer(Answer::BlockAck(
                FrameMask::first(frames).ok_or(Violation::Fixture)?,
            ));
            let done = serve(fixture, ArenaId::First, frames)?;
            let first = release(fixture, done, sent.get())?;
            fill(fixture, &second, &[30])?;
            let single = fixture.single(second);
            publish(fixture, single)?;
            expect_published(fixture, &[30], false, false)?;
            fixture.answer(Answer::Ack);
            let done = serve(fixture, ArenaId::Second, 1)?;
            if !matches!(done.end(), AttemptEnd::Acked { .. }) {
                return Err(Violation::Completion);
            }
            let second = release(fixture, done, &[30])?;
            withdraw(fixture, &first, &[])?;
            withdraw(fixture, &second, &[])
        }
        Case::Withdraw => {
            let frames = capacity.min(3);
            let sent = Tags::range(40, frames);
            fill(fixture, &first, sent.get())?;
            withdraw(fixture, &first, sent.get())?;
            fill(fixture, &first, &[50])?;
            withdraw(fixture, &first, &[50])?;
            withdraw(fixture, &second, &[])
        }
        Case::PublishRefusals => {
            let empty = fixture.aggregate(first);
            let first = refuse(fixture, empty, PublishRefused::Empty)?;
            if capacity >= 2 {
                fill(fixture, &first, &[60, 61])?;
                fill(fixture, &second, &[62])?;
                let aggregate = fixture.aggregate(first);
                publish(fixture, aggregate)?;
                let single = fixture.single(second);
                let second = refuse(fixture, single, PublishRefused::Busy)?;
                fixture.answer(Answer::BlockAck(
                    FrameMask::first(2).ok_or(Violation::Fixture)?,
                ));
                let done = serve(fixture, ArenaId::First, 2)?;
                let first = release(fixture, done, &[60, 61])?;
                let single = fixture.single(second);
                publish(fixture, single)?;
                fixture.answer(Answer::Ack);
                let done = serve(fixture, ArenaId::Second, 1)?;
                let second = release(fixture, done, &[62])?;
                withdraw(fixture, &first, &[])?;
                withdraw(fixture, &second, &[])
            } else {
                withdraw(fixture, &first, &[])?;
                withdraw(fixture, &second, &[])
            }
        }
        Case::SingleDrain => {
            let frames = capacity.min(3);
            let sent = Tags::range(80, frames);
            fill(fixture, &first, sent.get())?;
            let mut arena = first;
            for (at, &tag) in (0_u8..).zip(sent.get()) {
                let single = fixture.single(arena);
                publish(fixture, single)?;
                expect_published(fixture, &[tag], false, false)?;
                fixture.answer(Answer::Ack);
                let remaining = frames.saturating_sub(at);
                let done = serve(fixture, ArenaId::First, remaining)?;
                if !matches!(done.end(), AttemptEnd::Acked { .. }) {
                    return Err(Violation::Completion);
                }
                let rest = FrameMask::first(remaining)
                    .zip(FrameMask::first(1))
                    .map(|(all, head)| all.difference(head))
                    .ok_or(Violation::Fixture)?;
                let mut returned = Returned::new();
                arena = fixture
                    .port()
                    .retain(done, rest, |memory| returned.push(F::tag(&memory)))
                    .map_err(|_| Violation::Fault)?;
                returned.expect(&[tag])?;
                if fixture.port().len(arena.id()) != rest.count() {
                    return Err(Violation::Owners);
                }
            }
            withdraw(fixture, &arena, &[])?;
            withdraw(fixture, &second, &[])
        }
        Case::ProtectionRepublish => {
            let frames = capacity.min(3);
            let sent = Tags::range(70, frames);
            let sent = sent.get();
            let all = FrameMask::first(frames).ok_or(Violation::Fixture)?;
            fill(fixture, &first, sent)?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            fixture.answer(Answer::ProtectionFailure);
            let done = serve(fixture, ArenaId::First, frames)?;
            if !matches!(done.end(), AttemptEnd::ProtectionFailure(None)) {
                return Err(Violation::Completion);
            }
            let first = retain_all(fixture, done, all)?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            expect_published(fixture, sent, true, false)?;
            fixture.answer(Answer::NoResponse);
            let done = serve(fixture, ArenaId::First, frames)?;
            if !matches!(done.end(), AttemptEnd::NoResponse(_)) {
                return Err(Violation::Completion);
            }
            let first = retain_all(fixture, done, all)?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            expect_published(fixture, sent, true, true)?;
            fixture.answer(Answer::BlockAck(all));
            let done = serve(fixture, ArenaId::First, frames)?;
            let first = release(fixture, done, sent)?;
            withdraw(fixture, &first, &[])?;
            withdraw(fixture, &second, &[])
        }
        Case::PublishAheadOfSettlement => {
            fill(fixture, &first, &[90, 91])?;
            fill(fixture, &second, &[92, 93])?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            fixture.answer(Answer::BlockAck(
                FrameMask::first(2).ok_or(Violation::Fixture)?,
            ));
            let done = serve(fixture, ArenaId::First, 2)?;
            // The second goes on the air with the first unsettled.
            let aggregate = fixture.aggregate(second);
            publish(fixture, aggregate)?;
            expect_published(fixture, &[92, 93], true, false)?;
            let first = release(fixture, done, &[90, 91])?;
            if fixture.port().on_air() != Some(ArenaId::Second) {
                return Err(Violation::Publication);
            }
            fixture.answer(Answer::BlockAck(
                FrameMask::first(2).ok_or(Violation::Fixture)?,
            ));
            let done = serve(fixture, ArenaId::Second, 2)?;
            let second = release(fixture, done, &[92, 93])?;
            drop((first, second));
            Ok(())
        }
        Case::Fault => {
            fill(fixture, &first, &[80, 81])?;
            let aggregate = fixture.aggregate(first);
            publish(fixture, aggregate)?;
            fixture.answer(Answer::Fault);
            let events = fixture.events();
            let now = fixture.now();
            if fixture.port().service(events, now).is_ok() {
                return Err(Violation::Fault);
            }
            let memory = fixture.memory(82).ok_or(Violation::Fixture)?;
            match fixture
                .port()
                .intake(&second, memory, |region| F::write(82, region))
            {
                Err((memory, IntakeRefused::Quarantined)) if F::tag(&memory) == Some(82) => {}
                _ => return Err(Violation::Fault),
            }
            if fixture.port().withdraw(&second, drop).is_ok() {
                return Err(Violation::Fault);
            }
            Ok(())
        }
    }
}

fn refuse<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    publication: Publication<'p, 'id, F>,
    expected: PublishRefused,
) -> Result<Arena, Violation> {
    let id = publication.arena.id();
    let now = fixture.now();
    match fixture.port().publish(publication, now) {
        Err(PublishError::Refused {
            publication,
            reason,
        }) if reason == expected && publication.arena.id() == id => Ok(publication.arena),
        _ => Err(Violation::Refusal),
    }
}

fn retain_all<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    fixture: &mut F,
    done: Done<'p, 'id, F>,
    all: FrameMask,
) -> Result<Arena, Violation> {
    let mut returned = Returned::new();
    let arena = fixture
        .port()
        .retain(done, all, |memory| returned.push(F::tag(&memory)))
        .map_err(|_| Violation::Fault)?;
    returned.expect(&[])?;
    if fixture.port().len(arena.id()) != all.count() {
        return Err(Violation::Owners);
    }
    Ok(arena)
}

/// Run the cases this environment executes, each on a fresh fixture.
pub fn check<'p, 'id: 'p, F: Fixture<'p, 'id>>(
    mut fixture: impl FnMut() -> F,
    environment: Environment,
) -> Result<Report, (Case, Violation)> {
    let mut report = Report::default();
    for case in Case::ALL {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        exercise(&mut fixture(), case).map_err(|why| (case, why))?;
        report.passed();
    }
    Ok(report)
}
