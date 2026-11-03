//! Data A-MPDUs under the station's transmit Block Ack agreements.
//!
//! When a TID's agreement is operational, its owed MPDUs whose sequence
//! numbers follow one another inside the agreement's window go as one
//! aggregate: one ticket and one admission permission, up to the port's
//! subframe limit ([`AggregateWindow`]). Every member stays at its place
//! in the machine's bank of MPDUs, where it waited owed, and the
//! aggregate's context names its places in order; the members' memory
//! travels in the port's aggregate and comes back with its terminal.
//!
//! A member the BlockAck acknowledges retires. A missing one goes back to
//! the head of its TID's queue, in its original order and with its
//! published MPDU, for a later aggregate, until its MSDU's lifetime or the
//! policy's attempts run out ([`AggregateRetry`]); then it is dropped. Each
//! terminal reports what it settled to the agreement as [`BlockAckMarks`]:
//! the originator moves its window over them and asks the recipient to move
//! its own (a BlockAckReq) when it passed a dropped one. A single MPDU of a
//! TID reports its settlement the same way, so a window never waits for an
//! MPDU that went alone.

use super::*;
use core::marker::PhantomData;
use oer_protocols_ieee80211_contracts_port::{AggregateEnd, AggregateOutcome};

/// The most subframes one data aggregate holds: the width of the port's
/// subframe masks.
pub const MAX_AGGREGATE: usize =
    oer_protocols_ieee80211_contracts_port::backend_aggregate::AMPDU_SUBFRAME_MASK_BITS;

/// Where a TID may aggregate now: its agreement's window and what one
/// aggregate takes at its rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateWindow {
    /// The agreement's window head: the oldest sequence number it has not
    /// settled.
    pub start: SequenceNumber,
    /// The agreement's window, at most 64.
    pub size: u16,
    /// The most subframes and octets one aggregate takes.
    pub limit: PpduLimit,
}

/// The most one aggregate takes: subframes, and octets with delimiters and
/// padding. The octet bound holds the length limits and the rate's PPDU
/// time limit at once, computed when the rate or the agreement changes, so
/// a member's admission is one addition and one comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpduLimit {
    subframes: usize,
    octets: u32,
}

impl PpduLimit {
    /// At most `subframes` subframes (and [`MAX_AGGREGATE`]) and
    /// `max_length` octets (the recipient's Maximum A-MPDU Length and the
    /// port's at the aggregate's rate, the lower), within `rate`'s PPDU time
    /// limit ([`PhyRate::max_txtime_micros`](oer_protocols_ieee80211_contracts_port::PhyRate::max_txtime_micros));
    /// `None` checks no time. The time bound counts the whole aggregate at
    /// the rate (IEEE Std 802.11-2020 19.4.3, through
    /// `PhyRate::max_ppdu_duration_micros`), which grows with the octets:
    /// the bound is the longest aggregate within it.
    pub fn new(
        subframes: usize,
        max_length: u32,
        rate: Option<oer_protocols_ieee80211_contracts_port::PhyRate>,
    ) -> Self {
        let timed = rate.and_then(|rate| rate.max_txtime_micros().map(|limit| (rate, limit)));
        let octets = match timed {
            None => max_length,
            Some((rate, limit)) => longest_within(max_length, |octets| {
                rate.max_ppdu_duration_micros(octets)
                    .is_some_and(|duration| duration <= limit)
            }),
        };
        Self {
            subframes: subframes.min(MAX_AGGREGATE),
            octets,
        }
    }

    /// The most subframes one aggregate takes, at most [`MAX_AGGREGATE`].
    pub const fn subframes(&self) -> usize {
        self.subframes
    }

    /// The longest aggregate in octets, delimiters and padding included.
    pub const fn octets(&self) -> u32 {
        self.octets
    }

    /// Whether an aggregate of `octets`, a positive count, stays within the
    /// limit.
    const fn fits(&self, octets: u32) -> bool {
        octets <= self.octets
    }
}

/// The largest count up to `max` that `fits`, which holds up to a count and
/// fails above it; zero when it holds for none above zero.
fn longest_within(max: u32, fits: impl Fn(u32) -> bool) -> u32 {
    if fits(max) {
        return max;
    }
    // `low` fits or is zero; `high` does not fit.
    let (mut low, mut high) = (0_u32, max);
    while high.saturating_sub(low) > 1 {
        let middle = low.saturating_add(high.saturating_sub(low) / 2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// The octets one member takes in an aggregate: its delimiter, its MPDU
/// with the CCMP MIC the port appends to a protected one and the FCS,
/// padded to a four-octet boundary.
///
/// SOURCE: IEEE Std 802.11-2020, 9.7.1 (A-MPDU format: four-octet delimiter
/// and padding to a multiple of four octets).
pub(super) fn subframe_octets(mpdu: usize, protection: DataProtection<()>) -> Option<u32> {
    const DELIMITER: usize = 4;
    const FCS: usize = 4;
    let mic = match protection {
        DataProtection::Open => 0,
        DataProtection::Ccmp(()) => oer_protocols_ieee80211_wire::ccmp::CCMP_MIC_LEN,
    };
    let unpadded = mpdu.checked_add(mic)?.checked_add(FCS)?;
    let padded = unpadded.checked_add(3)? & !3;
    u32::try_from(padded.checked_add(DELIMITER)?).ok()
}

impl AggregateWindow {
    /// Whether `sequence` lies inside the window.
    fn holds(&self, sequence: SequenceNumber) -> bool {
        oer_protocols_ieee80211_core_upper_mac::sequence::forward_distance(self.start, sequence)
            < self.size
    }

    /// Whether `sequence` lies beyond the window's end: after its head in
    /// modulo-4096 order, but not inside it. The originator sends no MPDU
    /// there, alone or aggregated, until the window moves over the MPDUs
    /// before it; settling one sent there would move no window.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 10.25.6 (HT-immediate block ack: the
    /// originator transmits within WinStartO to WinStartO + WinSizeO - 1)
    /// and 10.24.7 (the half-space horizon of sequence comparisons).
    fn ahead(&self, sequence: SequenceNumber) -> bool {
        use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceSpace;
        let distance = self.start.forward_distance(sequence);
        distance >= self.size && distance < SequenceNumber::HALF_SPACE
    }
}

impl<T: StaTokens> LinkView<'_, '_, T> {
    /// The link's access point as the recipient of an aggregate.
    pub fn recipient(&self) -> Option<IndividualAddress> {
        IndividualAddress::new(self.bssid.get())
    }
}

/// How long a missing member is retried: the composition's policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateRetry {
    /// A member whose MSDU is this old is dropped instead of retried.
    pub lifetime: oer_portable_time::Duration,
    /// Aggregates on the air a member may miss before it is dropped.
    pub attempts: u8,
}

/// What one terminal settled of a TID's sequence numbers, from `start`:
/// bit `i` of `delivered` acknowledged `start + i`, of `expired` dropped it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockAckMarks {
    /// The TID whose agreement the marks belong to.
    pub tid: WmmUserPriority,
    /// The sequence number of bit zero.
    pub start: SequenceNumber,
    /// Acknowledged sequence numbers.
    pub delivered: u64,
    /// Dropped sequence numbers.
    pub expired: u64,
}

impl BlockAckMarks {
    fn new(tid: WmmUserPriority, start: SequenceNumber) -> Self {
        Self {
            tid,
            start,
            delivered: 0,
            expired: 0,
        }
    }

    fn mark(&mut self, sequence: SequenceNumber, delivered: bool) {
        let offset = oer_protocols_ieee80211_core_upper_mac::sequence::forward_distance(
            self.start, sequence,
        );
        let Some(bit) = 1_u64.checked_shl(u32::from(offset)) else {
            return;
        };
        if delivered {
            self.delivered |= bit;
        } else {
            self.expired |= bit;
        }
    }

    /// Whether the terminal settled nothing.
    pub const fn is_empty(&self) -> bool {
        self.delivered == 0 && self.expired == 0
    }
}

/// The original aggregate operation. It travels by value with the attempt;
/// its places keep the members' order, the same as the port's subframes.
#[derive(Debug)]
pub struct DataAggregateContext<
    'id,
    K,
    C,
    A = AcPermit,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
> {
    admission: A,
    link: LinkEpoch<'id, Path>,
    tid: WmmUserPriority,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    /// Each member's place in the data machine's bank of MPDUs, in order.
    places: heapless::Vec<u8, MAX_AGGREGATE>,
    /// The members' octets in the aggregate ([`subframe_octets`]).
    octets: u32,
    members: PhantomData<fn() -> (K, C)>,
}

/// Aggregate operation data retained with an owned data epoch.
pub struct DataAggregateInfo<K, C, A> {
    admission: A,
    tid: WmmUserPriority,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    places: heapless::Vec<u8, MAX_AGGREGATE>,
    octets: u32,
    members: PhantomData<fn() -> (K, C)>,
}

impl<'p, K: 'p, C: 'p, A: 'p, Path: PoolPath + 'p>
    oer_protocols_radio_port::LoanedContext<'p, Path::Identity>
    for DataAggregateContext<'p, K, C, A, Path>
where
    Path::Identity: 'p,
{
    type Owned = crate::epoch::EpochContext<Path, DataAggregateInfo<K, C, A>>;

    fn enclose(self) -> oer_protocols_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        self.link.context(DataAggregateInfo {
            admission: self.admission,
            tid: self.tid,
            access_category: self.access_category,
            protection: self.protection,
            places: self.places,
            octets: self.octets,
            members: PhantomData,
        })
    }

    fn restore(
        context: oer_protocols_radio_port::PortContext<'p, Path::Identity, Self::Owned>,
    ) -> Self {
        let (link, data) = context.into_parts();
        Self {
            admission: data.admission,
            link,
            tid: data.tid,
            access_category: data.access_category,
            protection: data.protection,
            places: data.places,
            octets: data.octets,
            members: PhantomData,
        }
    }
}

impl<'id, K, C, A, Path: PoolPath> DataAggregateContext<'id, K, C, A, Path> {
    /// The TID of every member.
    pub const fn tid(&self) -> WmmUserPriority {
        self.tid
    }

    /// The sole access category of this operation and its permission.
    pub const fn access_category(&self) -> WmmAccessCategory {
        self.access_category
    }

    /// The members, in the order of the port's subframes.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// Whether no member is left.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// Borrow the matching link's protection, as a single operation does.
    pub fn protection<'a, T: StaTokens<EpochPath = Path>>(
        &self,
        link: &LinkView<'a, 'id, T>,
    ) -> Result<DataProtection<&'a T::PairwiseKey>, crate::machine::TxRefusal> {
        let protection = DataProtection::from(&link.protection);
        if self.link != link.epoch || self.protection != protection.without_key() {
            return Err(crate::machine::TxRefusal::LinkChanged);
        }
        Ok(protection)
    }
}

/// One aggregate to submit: its context and ticket. Each member's memory
/// waits in the data machine's bank: [`StaData::aggregate_mpdu`] takes it
/// for the port's builder.
#[must_use = "the aggregate owns a ticket, buffers and an admission permission"]
pub struct DataAggregate<'id, D: StaDataTokens, K> {
    /// The operation and its admission permission.
    pub context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
    /// Its ticket.
    pub ticket: D::Ticket,
}

/// The next data attempt: one MPDU or one aggregate.
pub enum DataAttempt<'id, D: StaDataTokens, K> {
    /// One MPDU, alone.
    Single(DataTransmit<'id, D, K>),
    /// An aggregate of a TID under its agreement.
    Aggregate(DataAggregate<'id, D, K>),
}

/// Ownership an aggregate returns; consuming it cannot refuse resources.
/// Each member's memory has been given back to the bank first
/// ([`StaData::aggregate_mpdu_returned`]); a member whose memory did not
/// come back ends lost.
pub enum DataAggregateReturn<'id, D: StaDataTokens, K, B, X> {
    /// Its first subframe, or the aggregate, was refused before the port:
    /// every member ends with no radio terminal, as a refused single MPDU
    /// does; the agreement hears them dropped.
    Refused {
        /// The original operation.
        context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
    },
    /// The aggregate's terminal.
    TxDone {
        /// The original operation, returned unchanged by the port.
        context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
        /// How it ended.
        completion: AggregateOutcome<B, X>,
    },
}

impl<
    'id,
    D: StaDataTokens,
    K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities,
    const CAP: usize,
> StaData<'id, D, K, CAP>
{
    /// The next attempt whose category owns a free admission permission and
    /// which finds a free ticket: an aggregate of the chosen TID when
    /// `window` gives it one and its first owed MPDU lies inside it,
    /// otherwise that MPDU alone. A first MPDU beyond the window waits,
    /// with its TID, for the terminals of the MPDUs before it to move the
    /// window. The earliest MSDU of an admitted category goes first,
    /// whatever its TID. `window` is asked once, for the chosen TID, and
    /// only when an MPDU of it is owed.
    pub fn poll_attempt(
        &mut self,
        window: impl FnOnce(WmmUserPriority) -> Option<AggregateWindow>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<DataAttempt<'id, D, K>> {
        self.waits = false;
        if self.tickets.is_empty() {
            self.waits = true;
            return None;
        }
        let tid = self.next_tid()?;
        let front = self.first_owed(tid)?;
        let window = window(tid);
        if window.is_some_and(|window| window.ahead(front.sequence)) {
            return None;
        }
        let Some(window) = window.filter(|window| window.holds(front.sequence)) else {
            return self.poll_action().map(DataAttempt::Single);
        };
        let category = front.access_category;
        let ticket = self.tickets.take()?;
        let Some(permit) = self.admission.take(category) else {
            self.tickets.put(ticket, &mut ());
            return None;
        };
        // The members: the queue's first MPDUs whose sequence numbers follow
        // one another inside the window, at most the limit. They keep their
        // places in the bank.
        let limit = window.limit.subframes();
        let mut previous: Option<SequenceNumber> = None;
        let mut length = 0_u32;
        let count = self
            .owed()
            .filter(|owed| owed.tid == tid)
            .take(limit)
            .take_while(|owed| {
                let sequence = owed.sequence;
                let follows = previous.is_none_or(|previous| {
                    oer_protocols_ieee80211_core_upper_mac::sequence::forward_distance(
                        previous, sequence,
                    ) == 1
                });
                previous = Some(sequence);
                let fits = subframe_octets(owed.frame.frame().len().get(), owed.protection)
                    .and_then(|octets| length.checked_add(octets))
                    .filter(|total| window.limit.fits(*total))
                    .inspect(|total| length = *total)
                    .is_some();
                follows && window.holds(sequence) && fits
            })
            .count();
        if count == 0 {
            // The first MPDU alone does not fit an aggregate: it goes alone.
            self.admission.put(category, permit);
            self.tickets.put(ticket, &mut ());
            return self.poll_action().map(DataAttempt::Single);
        }
        let mut places = heapless::Vec::new();
        let mut shared = None;
        let mut octets = 0_u32;
        for _ in 0..count {
            let Some(place) = self.take_owed(tid) else {
                break;
            };
            let Some(member) = self.mpdus.get(place).map(|held| &held.context) else {
                report(events, || StaEvent::TokenLost);
                continue;
            };
            shared.get_or_insert((member.link, member.protection));
            // Every member fit the window's length above.
            octets = subframe_octets(member.frame.frame().len().get(), member.protection)
                .map_or(octets, |member| octets.saturating_add(member));
            // `count` is at most the places' capacity: none refuses.
            if let Err(place) = places.push(place) {
                report(events, || StaEvent::DataLost(DataLoss::Overflow));
                self.end(place, events);
            }
        }
        let Some((link, protection)) = shared else {
            self.admission.put(category, permit);
            self.tickets.put(ticket, &mut ());
            return None;
        };
        Some(DataAttempt::Aggregate(DataAggregate {
            context: DataAggregateContext {
                admission: permit,
                link,
                tid,
                access_category: category,
                protection,
                places,
                octets,
                members: PhantomData,
            },
            ticket,
        }))
    }

    /// The next owed MPDU of `context`'s TID joins it, with no ticket, when
    /// its sequence number follows the last member's inside the window and
    /// the aggregate takes it (the window's subframes and length): its
    /// index, whose memory waits in the bank for the port
    /// ([`Self::aggregate_mpdu`]). A refusal there gives it back to its
    /// queue through [`Self::unbuilt`]. `window` is asked only when such an
    /// MPDU is owed.
    pub fn extend_aggregate(
        &mut self,
        context: &mut DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        window: impl FnOnce() -> Option<AggregateWindow>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<usize> {
        let last = self.mpdus.get(*context.places.last()?)?.context.sequence;
        let next = self.first_owed(context.tid)?;
        let sequence = next.sequence;
        if next.link != context.link
            || next.protection != context.protection
            || oer_protocols_ieee80211_core_upper_mac::sequence::forward_distance(last, sequence)
                != 1
        {
            return None;
        }
        let length = next.frame.frame().len().get();
        let protection = next.protection;
        let window = window()?;
        let index = context.places.len();
        let octets = subframe_octets(length, protection)
            .and_then(|member| context.octets.checked_add(member))
            .filter(|total| window.limit.fits(*total))?;
        if index >= window.limit.subframes() || !window.holds(sequence) {
            return None;
        }
        let place = self.take_owed(context.tid)?;
        match context.places.push(place) {
            Ok(()) => {
                context.octets = octets;
                Some(index)
            }
            // The places' capacity is the limit above: it never refuses.
            Err(place) => {
                report(events, || StaEvent::DataLost(DataLoss::Overflow));
                self.end(place, events);
                None
            }
        }
    }

    /// Take the memory of the member at `index` of `context` from the bank,
    /// for the port's builder; `None` when it does not wait there.
    pub fn aggregate_mpdu<A>(
        &mut self,
        context: &DataAggregateContext<'id, K, D::Class, A, D::EpochPath>,
        index: usize,
    ) -> Option<ReturnedMpdu<D::Buffer, D::Sent>> {
        let place = *context.places.get(index)?;
        self.mpdus.get_mut(place)?.memory.take()
    }

    /// Give back the memory of the member at `index` of `context`: a
    /// subframe the builder refused, or one a terminal returned. Memory no
    /// member's place takes is freed as an unused buffer and reported lost.
    pub fn aggregate_mpdu_returned<A>(
        &mut self,
        context: &DataAggregateContext<'id, K, D::Class, A, D::EpochPath>,
        index: usize,
        mpdu: ReturnedMpdu<D::Buffer, D::Sent>,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let kept = match context
            .places
            .get(index)
            .and_then(|place| self.mpdus.get_mut(*place))
        {
            Some(held) if held.memory.is_none() => {
                held.memory = Some(mpdu);
                Ok(())
            }
            _ => Err(mpdu),
        };
        if let Err(mpdu) = kept {
            report(events, || StaEvent::TokenLost);
            self.retire(mpdu, events);
        }
    }

    /// Give back the members the port's builder did not take: those of
    /// `context` from `from` on, whose memory waits in the bank. They return
    /// to their queue unchanged.
    pub fn unbuilt(
        &mut self,
        context: &mut DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        from: usize,
        events: &mut impl EventSink<StaEvent>,
    ) {
        while context.places.len() > from {
            let Some(place) = context.places.pop() else {
                break;
            };
            if self
                .mpdus
                .get(place)
                .is_some_and(|held| held.memory.is_some())
            {
                self.owe_again(place, events);
            } else {
                // Its memory did not come back to the bank.
                report(events, || StaEvent::TokenLost);
                drop(self.mpdus.take(place));
            }
        }
    }

    /// Return an aggregate's terminal or its refusal and all its owners
    /// (a refusal ends every member):
    /// acknowledged members retire, missing ones go back to their queue
    /// while `retry` allows and are dropped after (at once without a
    /// policy). The marks say what the
    /// terminal settled of the TID's sequence numbers; none for another
    /// link's aggregate.
    pub fn aggregate_returned<B, X>(
        &mut self,
        input: DataAggregateReturn<'id, D, K, B, X>,
        now: oer_portable_time::Instant,
        retry: Option<AggregateRetry>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<BlockAckMarks> {
        self.waits = false;
        // Without a policy a missing member is not retried.
        let retry = retry.unwrap_or(AggregateRetry {
            lifetime: oer_portable_time::Duration::ZERO,
            attempts: 0,
        });
        let (context, ticket, completion) = match input {
            DataAggregateReturn::Refused { context, ticket } => (context, ticket, None),
            DataAggregateReturn::TxDone {
                context,
                ticket,
                completion,
            } => (context, ticket, Some(completion)),
        };
        self.tickets.put(ticket, events);
        let DataAggregateContext {
            admission,
            link,
            tid,
            access_category,
            protection: _,
            mut places,
            octets: _,
            members: _,
        } = context;
        self.admission.put(access_category, admission);
        let current = Some(link) == self.link;
        let mut marks = places
            .first()
            .and_then(|place| self.mpdus.get(*place))
            .map(|member| BlockAckMarks::new(tid, member.context.sequence));
        let aired = match &completion {
            Some(TxOutcome::Ended(
                AggregateEnd::Delivered { air, .. } | AggregateEnd::AckTimeout(air),
            )) => Some(air),
            Some(TxOutcome::Ended(
                AggregateEnd::CtsTimeout(air)
                | AggregateEnd::ContentionLimit(air)
                | AggregateEnd::AirDenied { air, .. },
            )) => air.as_ref(),
            _ => None,
        };
        let mut settlement = crate::events::AggregateSettled {
            members: u8::try_from(places.len()).unwrap_or(u8::MAX),
            refused: completion.is_none(),
            rate: aired.map(|air| air.rate),
            ..crate::events::AggregateSettled::default()
        };
        // From the back: each member settles with its memory, waiting at its
        // place, and the requeued keep their order.
        let mut index = places.len();
        while let Some(place) = places.pop() {
            index = index.saturating_sub(1);
            let Some(member) = self
                .mpdus
                .get_mut(place)
                .filter(|held| held.memory.is_some())
                .map(|held| &mut held.context)
            else {
                // Members whose memory did not come back cannot be retried.
                report(events, || StaEvent::TokenLost);
                settlement.dropped = settlement.dropped.saturating_add(1);
                drop(self.mpdus.take(place));
                continue;
            };
            if !current {
                self.end(place, events);
                continue;
            }
            let sequence = member.sequence;
            let settled = match &completion {
                // A refused aggregate ends its MSDUs, as a refused single
                // does: the same refusal would only come again.
                None => Some(false),
                Some(TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::GateClosed
                    | oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled,
                )) => None,
                Some(TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::KeyRemoved,
                )) => Some(false),
                Some(TxOutcome::Indeterminate(_)) => {
                    report(events, || StaEvent::DataLost(DataLoss::Indeterminate));
                    Some(false)
                }
                Some(TxOutcome::Expired { msdus }) => {
                    if 1_u64
                        .checked_shl(u32::try_from(index).unwrap_or(u32::MAX))
                        .is_some_and(|bit| msdus & bit != 0)
                    {
                        report(events, || StaEvent::DataLost(DataLoss::Expired));
                        Some(false)
                    } else {
                        None
                    }
                }
                Some(TxOutcome::Ended(end)) => {
                    let (acknowledged, aired) = match end {
                        AggregateEnd::Delivered { report, .. } => (
                            oer_protocols_ieee80211_core_upper_mac::block_ack::acknowledges(
                                *report,
                                member.sequence,
                            ),
                            true,
                        ),
                        AggregateEnd::AckTimeout(_) | AggregateEnd::CtsTimeout(Some(_)) => {
                            (false, true)
                        }
                        AggregateEnd::CtsTimeout(None) => (false, false),
                        AggregateEnd::ContentionLimit(air)
                        | AggregateEnd::AirDenied { air, .. } => (false, air.is_some()),
                    };
                    if acknowledged {
                        Some(true)
                    } else {
                        if aired {
                            member.attempts = member.attempts.saturating_add(1);
                        }
                        let aged = now
                            .checked_duration_since(member.admitted)
                            .is_some_and(|age| age >= retry.lifetime);
                        if aged {
                            report(events, || StaEvent::DataLost(DataLoss::Expired));
                            Some(false)
                        } else if member.attempts >= retry.attempts {
                            report(events, || StaEvent::DataLost(DataLoss::RetryLimit));
                            Some(false)
                        } else {
                            None
                        }
                    }
                }
            };
            match settled {
                Some(delivered) => {
                    if let Some(marks) = &mut marks {
                        marks.mark(sequence, delivered);
                    }
                    if delivered {
                        settlement.acknowledged = settlement.acknowledged.saturating_add(1);
                    } else {
                        settlement.dropped = settlement.dropped.saturating_add(1);
                    }
                    self.end(place, events);
                }
                None => {
                    settlement.retried = settlement.retried.saturating_add(1);
                    self.owe_again(place, events);
                }
            }
        }
        if current {
            report(events, || StaEvent::AggregateSettled(settlement));
        }
        marks.filter(|marks| current && !marks.is_empty())
    }

    /// The checked description of the member at `index` of `context`.
    pub fn aggregate_frame<A>(
        &self,
        context: &DataAggregateContext<'id, K, D::Class, A, D::EpochPath>,
        index: usize,
    ) -> Option<oer_protocols_ieee80211_contracts_port::ClassSizedFrame<D::Class, AckHeader, K>>
    where
        oer_protocols_ieee80211_contracts_port::ClassSizedFrame<D::Class, AckHeader, K>: Copy,
    {
        let place = *context.places.get(index)?;
        self.mpdus.get(place).map(|held| held.context.frame)
    }

    /// Whether this aggregate belongs to the current link; the service uses
    /// the same guard before teaching the rate selection.
    pub fn aggregate_is_current<A>(
        &self,
        context: &DataAggregateContext<'id, K, D::Class, A, D::EpochPath>,
    ) -> bool {
        Some(context.link) == self.link
    }

    /// Put an owed MPDU back by its original order, before newer MSDUs.
    pub(super) fn requeue(
        &mut self,
        context: DataTxContext<'id, K, D::Class, (), D::EpochPath>,
        buffer: ReturnedMpdu<D::Buffer, D::Sent>,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let held = HeldMpdu {
            context,
            memory: Some(buffer),
        };
        match self.mpdus.store(held) {
            Ok(place) => self.owe_again(place, events),
            Err(held) => self.overflowed(held.memory, events),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_protocols_ieee80211_contracts_port::PhyRate;
    use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth};

    /// The admission the octet bound replaces: the length, then the rate's
    /// time limit over the whole aggregate at the rate.
    fn fits_in_time(max_length: u32, rate: Option<PhyRate>, octets: u32) -> bool {
        octets <= max_length
            && rate.is_none_or(|rate| {
                rate.max_txtime_micros().is_none_or(|limit| {
                    rate.max_ppdu_duration_micros(octets)
                        .is_some_and(|duration| duration <= limit)
                })
            })
    }

    fn ht_rates() -> impl Iterator<Item = PhyRate> {
        (0..=31).flat_map(|mcs| {
            [PpduBandwidth::Mhz20, PpduBandwidth::Mhz40]
                .into_iter()
                .flat_map(move |bandwidth| {
                    [false, true].map(move |short_gi| {
                        PhyRate::Ht(
                            HtRate::new(HtMcs::new(mcs).unwrap(), bandwidth, short_gi).unwrap(),
                        )
                    })
                })
        })
    }

    #[test]
    fn the_octet_bound_admits_every_ht_aggregate_the_time_limit_admits_and_no_other() {
        for rate in ht_rates() {
            let limit = PpduLimit::new(MAX_AGGREGATE, 65_535, Some(rate));
            for octets in 1..=70_000 {
                assert_eq!(
                    limit.fits(octets),
                    fits_in_time(65_535, Some(rate), octets),
                    "{rate:?} at {octets} octets"
                );
            }
        }
    }

    #[test]
    fn the_octet_bound_keeps_every_length_limit_at_its_boundaries() {
        let rates = ht_rates()
            .map(Some)
            .chain([
                Some(PhyRate::Legacy(LegacyRate::Dsss1M)),
                Some(PhyRate::Legacy(LegacyRate::Ofdm6M)),
                Some(PhyRate::Legacy(LegacyRate::Ofdm54M)),
                None,
            ])
            .collect::<std::vec::Vec<_>>();
        for max_length in [
            1,
            4_095,
            8_191,
            16_383,
            32_767,
            65_535,
            262_143,
            1_048_575,
            u32::MAX,
        ] {
            for &rate in &rates {
                let limit = PpduLimit::new(MAX_AGGREGATE, max_length, rate);
                let bound = limit.octets();
                for octets in [
                    1,
                    bound.saturating_sub(1),
                    bound,
                    bound.saturating_add(1),
                    max_length,
                    max_length.saturating_add(1),
                    u32::MAX,
                ] {
                    assert_eq!(
                        limit.fits(octets),
                        fits_in_time(max_length, rate, octets),
                        "{rate:?} within {max_length} at {octets} octets"
                    );
                }
            }
        }
    }

    #[test]
    fn the_subframe_bound_is_at_most_the_ports_mask_width() {
        assert_eq!(PpduLimit::new(3, 65_535, None).subframes(), 3);
        assert_eq!(
            PpduLimit::new(MAX_AGGREGATE + 1, 65_535, None).subframes(),
            MAX_AGGREGATE
        );
    }
}
