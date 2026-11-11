//! The network core's turns: what the network's task reported of each turn
//! of its service, and how each pass after a turn went on.
//!
//! The network's task is the one writer: it holds a [`TurnTally`] for the
//! turn under way and publishes each ended turn to the [`NetworkCounter`].
//! A reader takes each word on its own. Counts wrap at 2^32. No count reads a
//! clock.

use core::sync::atomic::{AtomicU32, Ordering};

use super::bump;

/// A request a turn took from a socket's lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    /// A reservation asking for `blocks`.
    Reserve {
        blocks: u8,
    },
    Release,
    Send,
    Close,
}

/// How a turn's publishing ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Publish {
    /// The policy chose nothing: no work was ready, or none may go now.
    Chose,
    /// The port refused a frame for room or link.
    Refused,
    /// Publishing took its whole budget of frames.
    Budget,
    /// The port contradicted itself.
    Fault,
}

/// What a turn did with its local producer, or what a wait registered it
/// for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Local {
    /// The producer offered a datagram.
    Offered,
    /// The core refused an offer for good.
    Refused,
    /// The core refused an offer for now: the producer's offers hold.
    Held,
    /// A fill declined.
    Declined,
    /// A declared work was withdrawn, its source having moved on.
    Withdrawn,
    /// A wait registered the producer for new offers.
    WaitsOffers,
    /// A wait registered the producer for changes only.
    WaitsChanges,
}

/// What the network core dropped, by its reason: one word each. The core
/// names each drop where it happens; this counts them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Dropped {
    Malformed,
    Checksum,
    NoSocket,
    ReceiveFull,
    NoBlock,
    GroupLimit,
    TooLong,
    Fragment,
    BadSource,
    ReplyDropped,
    SendOnly,
    /// A lending socket's datagram found the socket at its credit.
    NoCredit,
    /// A lending socket's datagram met a stopping account.
    NotLentStopping,
    /// A lending socket's datagram for a socket the account has no place
    /// for.
    NotLentSocket,
    /// A lending socket's datagram the device had copied by pressure.
    PressureCopy,
    /// A lending socket's lane had no room for a lent datagram.
    LentLaneFull,
    /// A DHCP or an ICMP message the core dropped, by any of its reasons.
    Dhcp,
    Icmp,
}

/// The words of [`NetworkTurns::drops`], one for each [`Dropped`].
pub const DROP_KINDS: usize = Dropped::Icmp as usize + 1;

/// The bins of [`NetworkTurns::loans_at_turn_end`]: turns that ended with
/// 0, 1, 2, 3, 4, and 5 or more receive loans out.
pub const LOAN_BINS: usize = 6;

/// The bins of [`NetworkTurns::accepted_by_turn`]: turns whose port took
/// 0, 1, 2, 3, 4, 5 to 8, and 9 or more frames.
pub const ACCEPTED_BINS: usize = 7;

/// The bins of [`NetworkTurns::reserve_by_size`]: reservations asking for
/// 1, 2, 3 to 4, 5 to 8, and 9 to 16 blocks (a socket asks for at most its
/// look-ahead, 16).
pub const RESERVE_BINS: usize = 5;

/// What the network's turns did since boot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkTurns {
    pub turns: u32,
    /// Requests the turns took, by kind.
    pub reserves: u32,
    pub releases: u32,
    pub sends: u32,
    pub closes: u32,
    /// Sockets whose lane still held requests when the turn had taken its
    /// budget of them, summed over the turns.
    pub budget_spent: u32,
    /// Turns that left at least one such socket.
    pub budget_turns: u32,
    /// Turns that took in their whole budget of received frames.
    pub intake_full: u32,
    /// Frames the port accepted, and the turns by how many they had it take.
    pub accepted: u32,
    pub accepted_by_turn: [u32; ACCEPTED_BINS],
    /// Turns by how their publishing ended.
    pub publish_chose: u32,
    pub publish_refused: u32,
    pub publish_budget: u32,
    pub publish_fault: u32,
    /// Turns that took their whole budget of notices.
    pub notices_full: u32,
    /// Turns that ended with no free block in the pool.
    pub blocks_out: u32,
    /// Turns that ended with an open socket out of credits.
    pub credits_out: u32,
    /// Passes after a turn that moved, given up to the executor in a yield:
    /// all of them, and those after a turn with signs of possible work left
    /// (a budget spent with requests left, a full intake, publishing at its
    /// budget or deferred). A spent frame budget does not prove that a next
    /// frame exists.
    pub yields: u32,
    pub yields_with_work: u32,
    /// Waits after a turn that moved nothing: those that found nothing when
    /// they registered, which end their poll unless a deadline was due, and
    /// those that found something, whose next turn runs in the same poll.
    pub waits_pending: u32,
    pub waits_ready: u32,
    /// The blocks the reservations asked for, and the reservations by how
    /// many ([`RESERVE_BINS`]).
    pub reserve_blocks: u32,
    pub reserve_by_size: [u32; RESERVE_BINS],
    /// A local producer's offers, final refusals, holds, declining fills
    /// and withdrawals, and the waits that registered it for offers or for
    /// changes only.
    pub local_offered: u32,
    pub local_refused: u32,
    pub local_held: u32,
    pub local_declined: u32,
    pub local_withdrawn: u32,
    pub local_waits_offers: u32,
    pub local_waits_changes: u32,
    /// The core's drops, by [`Dropped`] reason.
    pub drops: [u32; DROP_KINDS],
    /// Turns by the receive loans out when they ended ([`LOAN_BINS`]),
    /// old installations' included.
    pub loans_at_turn_end: [u32; LOAN_BINS],
}

/// The words of [`NetworkCounter`], by what they count.
mod word {
    pub const TURNS: usize = 0;
    pub const RESERVES: usize = 1;
    pub const RELEASES: usize = 2;
    pub const SENDS: usize = 3;
    pub const CLOSES: usize = 4;
    pub const BUDGET_SPENT: usize = 5;
    pub const BUDGET_TURNS: usize = 6;
    pub const INTAKE_FULL: usize = 7;
    pub const ACCEPTED: usize = 8;
    /// [`super::ACCEPTED_BINS`] words.
    pub const ACCEPTED_BY_TURN: usize = 9;
    pub const PUBLISH_CHOSE: usize = 16;
    pub const PUBLISH_REFUSED: usize = 17;
    pub const PUBLISH_BUDGET: usize = 18;
    pub const PUBLISH_FAULT: usize = 19;
    pub const NOTICES_FULL: usize = 20;
    pub const BLOCKS_OUT: usize = 21;
    pub const CREDITS_OUT: usize = 22;
    pub const YIELDS: usize = 23;
    pub const YIELDS_WITH_WORK: usize = 24;
    pub const WAITS_PENDING: usize = 25;
    pub const WAITS_READY: usize = 26;
    pub const RESERVE_BLOCKS: usize = 27;
    /// [`super::RESERVE_BINS`] words.
    pub const RESERVE_BY_SIZE: usize = 28;
    pub const LOCAL_OFFERED: usize = 33;
    pub const LOCAL_REFUSED: usize = 34;
    pub const LOCAL_HELD: usize = 35;
    pub const LOCAL_DECLINED: usize = 36;
    pub const LOCAL_WITHDRAWN: usize = 37;
    pub const LOCAL_WAITS_OFFERS: usize = 38;
    pub const LOCAL_WAITS_CHANGES: usize = 39;
    /// [`super::DROP_KINDS`] words.
    pub const DROPS: usize = 40;
    /// [`super::LOAN_BINS`] words.
    pub const LOANS_AT_TURN_END: usize = 58;
    pub const COUNT: usize = 64;
}

const _: () = {
    if word::ACCEPTED_BY_TURN + ACCEPTED_BINS != word::PUBLISH_CHOSE {
        panic!("the histogram's words lie between the accepted count and the publish ends");
    }
    if word::RESERVE_BY_SIZE + RESERVE_BINS != word::LOCAL_OFFERED {
        panic!("the producer's words follow the reservations' histogram");
    }
    if word::LOCAL_WAITS_CHANGES + 1 != word::DROPS {
        panic!("the drops follow the producer's waits");
    }
    if word::DROPS + DROP_KINDS != word::LOANS_AT_TURN_END {
        panic!("the loans' histogram follows the drops");
    }
    if word::LOANS_AT_TURN_END + LOAN_BINS != word::COUNT {
        panic!("the loans' histogram is the last of the words");
    }
};

/// The network's turn counts, published word by word.
pub struct NetworkCounter {
    counts: [AtomicU32; word::COUNT],
}

impl NetworkCounter {
    pub const fn new() -> Self {
        Self {
            counts: [const { AtomicU32::new(0) }; word::COUNT],
        }
    }

    fn bump(&self, index: usize, by: u32) {
        if let Some(count) = self.counts.get(index) {
            bump(count, by);
        }
    }

    fn load(&self, index: usize) -> u32 {
        self.counts
            .get(index)
            .map_or(0, |count| count.load(Ordering::Relaxed))
    }

    /// The counts now.
    pub fn read(&self) -> NetworkTurns {
        let at = |index| self.load(index);
        NetworkTurns {
            turns: at(word::TURNS),
            reserves: at(word::RESERVES),
            releases: at(word::RELEASES),
            sends: at(word::SENDS),
            closes: at(word::CLOSES),
            budget_spent: at(word::BUDGET_SPENT),
            budget_turns: at(word::BUDGET_TURNS),
            intake_full: at(word::INTAKE_FULL),
            accepted: at(word::ACCEPTED),
            accepted_by_turn: core::array::from_fn(|bin| {
                at(word::ACCEPTED_BY_TURN.saturating_add(bin))
            }),
            publish_chose: at(word::PUBLISH_CHOSE),
            publish_refused: at(word::PUBLISH_REFUSED),
            publish_budget: at(word::PUBLISH_BUDGET),
            publish_fault: at(word::PUBLISH_FAULT),
            notices_full: at(word::NOTICES_FULL),
            blocks_out: at(word::BLOCKS_OUT),
            credits_out: at(word::CREDITS_OUT),
            yields: at(word::YIELDS),
            yields_with_work: at(word::YIELDS_WITH_WORK),
            waits_pending: at(word::WAITS_PENDING),
            waits_ready: at(word::WAITS_READY),
            reserve_blocks: at(word::RESERVE_BLOCKS),
            reserve_by_size: core::array::from_fn(|bin| {
                at(word::RESERVE_BY_SIZE.saturating_add(bin))
            }),
            local_offered: at(word::LOCAL_OFFERED),
            local_refused: at(word::LOCAL_REFUSED),
            local_held: at(word::LOCAL_HELD),
            local_declined: at(word::LOCAL_DECLINED),
            local_withdrawn: at(word::LOCAL_WITHDRAWN),
            local_waits_offers: at(word::LOCAL_WAITS_OFFERS),
            local_waits_changes: at(word::LOCAL_WAITS_CHANGES),
            drops: core::array::from_fn(|kind| at(word::DROPS.saturating_add(kind))),
            loans_at_turn_end: core::array::from_fn(|bin| {
                at(word::LOANS_AT_TURN_END.saturating_add(bin))
            }),
        }
    }
}

impl Default for NetworkCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The bin of a turn whose port took `accepted` frames.
const fn accepted_bin(accepted: u32) -> usize {
    match accepted {
        0..=4 => accepted as usize,
        5..=8 => 5,
        _ => 6,
    }
}

/// The bin of a reservation asking for `blocks`.
const fn reserve_bin(blocks: u8) -> usize {
    match blocks {
        0..=1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        _ => 4,
    }
}

/// The turn under way, as its writer holds it, and whether the last turn
/// left work.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnTally {
    spent: u32,
    accepted: u32,
    intake_full: bool,
    notices_full: bool,
    publish: Option<Publish>,
    /// The last ended turn left work it knew of.
    work_left: bool,
}

impl TurnTally {
    pub const fn new() -> Self {
        Self {
            spent: 0,
            accepted: 0,
            intake_full: false,
            notices_full: false,
            publish: None,
            work_left: false,
        }
    }

    /// The turn took `request`.
    pub fn took(&mut self, counter: &NetworkCounter, request: Request) {
        if let Request::Reserve { blocks } = request {
            counter.bump(word::RESERVE_BLOCKS, u32::from(blocks));
            counter.bump(word::RESERVE_BY_SIZE.saturating_add(reserve_bin(blocks)), 1);
        }
        counter.bump(
            match request {
                Request::Reserve { .. } => word::RESERVES,
                Request::Release => word::RELEASES,
                Request::Send => word::SENDS,
                Request::Close => word::CLOSES,
            },
            1,
        );
    }

    /// The turn, or the wait after it, did `local` with its producer.
    pub fn local(&mut self, counter: &NetworkCounter, local: Local) {
        counter.bump(
            match local {
                Local::Offered => word::LOCAL_OFFERED,
                Local::Refused => word::LOCAL_REFUSED,
                Local::Held => word::LOCAL_HELD,
                Local::Declined => word::LOCAL_DECLINED,
                Local::Withdrawn => word::LOCAL_WITHDRAWN,
                Local::WaitsOffers => word::LOCAL_WAITS_OFFERS,
                Local::WaitsChanges => word::LOCAL_WAITS_CHANGES,
            },
            1,
        );
    }

    /// The core dropped something, for `reason`.
    pub fn dropped(&mut self, counter: &NetworkCounter, reason: Dropped) {
        counter.bump(word::DROPS.saturating_add(reason as usize), 1);
    }

    /// A socket's lane held requests once the turn had taken its budget.
    pub fn budget_spent(&mut self) {
        self.spent = self.spent.wrapping_add(1);
    }

    /// The turn took in its whole budget of received frames.
    pub fn intake_full(&mut self) {
        self.intake_full = true;
    }

    /// The turn took its whole budget of notices.
    pub fn notices_full(&mut self) {
        self.notices_full = true;
    }

    /// The port accepted a frame.
    pub fn accepted(&mut self) {
        self.accepted = self.accepted.wrapping_add(1);
    }

    /// The turn's publishing ended so.
    pub fn published(&mut self, end: Publish) {
        self.publish = Some(end);
    }

    /// The turn ended, with a free block or none, every open socket with a
    /// free credit or not, and `loans` receive loans out.
    pub fn ended(
        &mut self,
        counter: &NetworkCounter,
        blocks_free: bool,
        credits_free: bool,
        loans: usize,
    ) {
        let Self {
            spent,
            accepted,
            intake_full,
            notices_full,
            publish,
            ..
        } = core::mem::take(self);
        counter.bump(word::TURNS, 1);
        counter.bump(word::BUDGET_SPENT, spent);
        counter.bump(word::BUDGET_TURNS, u32::from(spent != 0));
        counter.bump(word::INTAKE_FULL, u32::from(intake_full));
        counter.bump(word::NOTICES_FULL, u32::from(notices_full));
        counter.bump(word::ACCEPTED, accepted);
        counter.bump(
            word::ACCEPTED_BY_TURN.saturating_add(accepted_bin(accepted)),
            1,
        );
        if let Some(publish) = publish {
            counter.bump(
                match publish {
                    Publish::Chose => word::PUBLISH_CHOSE,
                    Publish::Refused => word::PUBLISH_REFUSED,
                    Publish::Budget => word::PUBLISH_BUDGET,
                    Publish::Fault => word::PUBLISH_FAULT,
                },
                1,
            );
        }
        counter.bump(word::BLOCKS_OUT, u32::from(!blocks_free));
        counter.bump(word::CREDITS_OUT, u32::from(!credits_free));
        counter.bump(
            word::LOANS_AT_TURN_END.saturating_add(loans.min(LOAN_BINS.saturating_sub(1))),
            1,
        );
        self.work_left =
            spent != 0 || intake_full || notices_full || matches!(publish, Some(Publish::Budget));
    }

    /// The pass after the last turn yielded to the executor.
    pub fn yielded(&mut self, counter: &NetworkCounter) {
        counter.bump(word::YIELDS, 1);
        counter.bump(word::YIELDS_WITH_WORK, u32::from(self.work_left));
    }

    /// The pass after the last turn waited; `ready` when something was there
    /// as it registered.
    pub fn waited(&mut self, counter: &NetworkCounter, ready: bool) {
        counter.bump(
            if ready {
                word::WAITS_READY
            } else {
                word::WAITS_PENDING
            },
            1,
        );
    }
}

#[cfg(test)]
mod tests;
