//! Data A-MPDUs under the station's transmit Block Ack agreements.
//!
//! When a TID's agreement is operational, its owed MPDUs whose sequence
//! numbers follow one another inside the agreement's window go as one
//! aggregate: one ticket and one admission permission, up to the port's
//! subframe limit ([`AggregateWindow`]). Every member's closed description
//! waits in the machine's [`MemberBank`] and the aggregate's context names
//! its places in order; the members' memory travels in the port's
//! aggregate and comes back with its terminal.
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
use oer_ieee80211_contracts_port::{AggregateEnd, AggregateOutcome};

/// The most subframes one data aggregate holds: the width of the port's
/// subframe masks.
pub const MAX_AGGREGATE: usize =
    oer_ieee80211_contracts_port::backend_aggregate::AMPDU_SUBFRAME_MASK_BITS;

/// Where a TID may aggregate now: its agreement's window and the port's
/// subframe limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateWindow {
    /// The agreement's window head: the oldest sequence number it has not
    /// settled.
    pub start: SequenceNumber,
    /// The agreement's window, at most 64.
    pub size: u16,
    /// The most subframes one aggregate takes, at most [`MAX_AGGREGATE`].
    pub subframes: usize,
    /// The longest aggregate in octets, delimiters and padding included:
    /// the recipient's Maximum A-MPDU Length and the port's, the lower.
    pub max_length: u32,
}

/// The octets one member takes in an aggregate: its delimiter, its MPDU
/// with the CCMP MIC the port appends to a protected one and the FCS,
/// padded to a four-octet boundary.
///
/// SOURCE: IEEE Std 802.11-2020, 9.7.1 (A-MPDU format: four-octet delimiter
/// and padding to a multiple of four octets).
fn subframe_octets(mpdu: usize, protection: DataProtection<()>) -> Option<u32> {
    const DELIMITER: usize = 4;
    const FCS: usize = 4;
    let mic = match protection {
        DataProtection::Open => 0,
        DataProtection::Ccmp(()) => oer_ieee80211_wire::ccmp::CCMP_MIC_LEN,
    };
    let unpadded = mpdu.checked_add(mic)?.checked_add(FCS)?;
    let padded = unpadded.checked_add(3)? & !3;
    u32::try_from(padded.checked_add(DELIMITER)?).ok()
}

impl AggregateWindow {
    /// Whether `sequence` lies inside the window.
    fn holds(&self, sequence: SequenceNumber) -> bool {
        oer_ieee80211_core_upper_mac::sequence::forward_distance(self.start, sequence) < self.size
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
    pub lifetime: oer_time::Duration,
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
        let offset = oer_ieee80211_core_upper_mac::sequence::forward_distance(self.start, sequence);
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

/// One member's closed description, which the data machine keeps in its
/// [`MemberBank`] while its aggregate is out.
#[derive(Debug)]
pub(crate) struct Member<K, C> {
    frame: oer_ieee80211_contracts_port::ClassSizedFrame<C, AckHeader, K>,
    order: u32,
    admitted: oer_time::Instant,
    sequence: SequenceNumber,
    attempts: u8,
}

/// The members of every aggregate out at the port, each at the place its
/// aggregate's context names. Only the places travel with the attempt, so an
/// aggregate's context is no larger than a single MPDU's. A place is taken
/// when its member is stored and vacated when the context comes back; no
/// two contexts name one place. `N` is the machine's buffer capacity: every
/// member holds a buffer, so the bank has a place for each.
pub(crate) struct MemberBank<K, C, const N: usize> {
    members: [Option<Member<K, C>>; N],
}

impl<K, C, const N: usize> MemberBank<K, C, N> {
    /// Every place a `u8` names.
    const PLACES: () = {
        if N > u8::MAX as usize + 1 {
            panic!("a member place is a u8: the bank holds at most 256 members");
        }
    };

    pub(crate) const fn new() -> Self {
        let () = Self::PLACES;
        Self {
            members: [const { None }; N],
        }
    }

    /// Initialize an empty bank in place.
    #[cfg(feature = "placement")]
    pub(crate) fn init_in_place(place: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        let () = Self::PLACES;
        oer_memory::init_in_place!(place, Self {
            members => |members| oer_memory::init_array(members, |_| None),
        })
    }

    /// The places no member holds.
    fn vacancies(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.is_none())
            .count()
    }

    /// Store `member` at a vacant place, or give it back without one.
    fn store(&mut self, member: Member<K, C>) -> Result<u8, Member<K, C>> {
        let Some((place, vacant)) = self
            .members
            .iter_mut()
            .enumerate()
            .find(|(_, member)| member.is_none())
        else {
            return Err(member);
        };
        let Ok(place) = u8::try_from(place) else {
            return Err(member);
        };
        *vacant = Some(member);
        Ok(place)
    }

    /// The member at `place`, vacating it.
    fn take(&mut self, place: u8) -> Option<Member<K, C>> {
        self.members.get_mut(usize::from(place))?.take()
    }

    /// The member at `place`.
    fn get(&self, place: u8) -> Option<&Member<K, C>> {
        self.members.get(usize::from(place))?.as_ref()
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
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    admission: A,
    link: LinkEpoch<'id, Path>,
    tid: WmmUserPriority,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    /// Each member's place in the data machine's [`MemberBank`], in order.
    places: heapless::Vec<u8, MAX_AGGREGATE>,
    members: PhantomData<fn() -> Member<K, C>>,
}

/// Aggregate operation data retained with an owned data epoch.
pub struct DataAggregateInfo<K, C, A> {
    admission: A,
    tid: WmmUserPriority,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    places: heapless::Vec<u8, MAX_AGGREGATE>,
    members: PhantomData<fn() -> Member<K, C>>,
}

impl<'p, K: 'p, C: 'p, A: 'p, Path: PoolPath + 'p> oer_radio_port::LoanedContext<'p, Path::Identity>
    for DataAggregateContext<'p, K, C, A, Path>
where
    Path::Identity: 'p,
{
    type Owned = crate::epoch::EpochContext<Path, DataAggregateInfo<K, C, A>>;

    fn enclose(self) -> oer_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        self.link.context(DataAggregateInfo {
            admission: self.admission,
            tid: self.tid,
            access_category: self.access_category,
            protection: self.protection,
            places: self.places,
            members: PhantomData,
        })
    }

    fn restore(context: oer_radio_port::PortContext<'p, Path::Identity, Self::Owned>) -> Self {
        let (link, data) = context.into_parts();
        Self {
            admission: data.admission,
            link,
            tid: data.tid,
            access_category: data.access_category,
            protection: data.protection,
            places: data.places,
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

    /// The member at `index` as the single operation it was.
    fn single(&self, member: Member<K, C>) -> DataTxContext<'id, K, C, (), Path> {
        DataTxContext {
            admission: (),
            link: self.link,
            tid: self.tid,
            frame: member.frame,
            access_category: self.access_category,
            protection: self.protection,
            order: member.order,
            admitted: member.admitted,
            sequence: member.sequence,
            attempts: member.attempts,
        }
    }
}

/// One aggregate to submit: its context, ticket and members' memory.
#[must_use = "the aggregate owns a ticket, buffers and an admission permission"]
pub struct DataAggregate<'id, D: StaDataTokens, K> {
    /// The operation and its admission permission.
    pub context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
    /// Its ticket.
    pub ticket: D::Ticket,
    /// Each member's fresh or published MPDU, in the context's order.
    pub mpdus: heapless::Vec<ReturnedMpdu<D::Buffer, D::Sent>, MAX_AGGREGATE>,
}

/// The next data attempt: one MPDU or one aggregate.
pub enum DataAttempt<'id, D: StaDataTokens, K> {
    /// One MPDU, alone.
    Single(DataTransmit<'id, D, K>),
    /// An aggregate of a TID under its agreement.
    Aggregate(DataAggregate<'id, D, K>),
}

/// Ownership an aggregate returns; consuming it cannot refuse resources.
pub enum DataAggregateReturn<'id, D: StaDataTokens, K, B, X> {
    /// Its first subframe, or the aggregate, was refused before the port:
    /// every member ends with no radio terminal, as a refused single MPDU
    /// does; the agreement hears them dropped.
    Refused {
        /// The original operation.
        context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Every member's memory, in order.
        mpdus: heapless::Vec<ReturnedMpdu<D::Buffer, D::Sent>, MAX_AGGREGATE>,
    },
    /// The aggregate's terminal.
    TxDone {
        /// The original operation, returned unchanged by the port.
        context: DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Every member's memory, in order.
        mpdus: heapless::Vec<ReturnedMpdu<D::Buffer, D::Sent>, MAX_AGGREGATE>,
        /// How it ended.
        completion: AggregateOutcome<B, X>,
    },
}

impl<
    'id,
    D: StaDataTokens,
    K: oer_ieee80211_contracts_port::Ieee80211Capabilities,
    const CAP: usize,
> StaData<'id, D, K, CAP>
{
    /// The next attempt whose category owns a free admission permission and
    /// which finds a free ticket: an aggregate of the chosen TID when
    /// `window` gives it one and its first owed MPDU lies inside it,
    /// otherwise that MPDU alone. The earliest MSDU of an admitted category
    /// goes first, whatever its TID.
    pub fn poll_attempt(
        &mut self,
        window: impl Fn(WmmUserPriority) -> Option<AggregateWindow>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<DataAttempt<'id, D, K>> {
        let tid = self.next_tid()?;
        let front = self.owed.first(|owed| owed.context.tid == tid)?;
        let Some(window) = window(tid).filter(|window| window.holds(front.context.sequence)) else {
            return self.poll_action().map(DataAttempt::Single);
        };
        let category = front.context.access_category;
        let ticket = self.tickets.take()?;
        let Some(permit) = self.admission.take(category) else {
            self.tickets.put(ticket, &mut ());
            return None;
        };
        // The members: the queue's first MPDUs whose sequence numbers follow
        // one another inside the window, at most the limit.
        // Every owed MPDU holds a buffer and the bank has a place for each,
        // so its vacancies never limit an aggregate the window allows.
        let limit = window
            .subframes
            .min(MAX_AGGREGATE)
            .min(self.members.vacancies());
        let mut previous: Option<SequenceNumber> = None;
        let mut length = 0_u32;
        let count = self
            .owed
            .iter()
            .filter(|owed| owed.context.tid == tid)
            .take(limit)
            .take_while(|owed| {
                let sequence = owed.context.sequence;
                let follows = previous.is_none_or(|previous| {
                    oer_ieee80211_core_upper_mac::sequence::forward_distance(previous, sequence)
                        == 1
                });
                previous = Some(sequence);
                let fits = subframe_octets(
                    owed.context.frame.frame().len().get(),
                    owed.context.protection,
                )
                .and_then(|octets| length.checked_add(octets))
                .filter(|total| *total <= window.max_length)
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
        let mut mpdus = heapless::Vec::new();
        let mut shared = None;
        for _ in 0..count {
            let Some(OwedMpdu { context, buffer }) =
                self.owed.remove_first(|owed| owed.context.tid == tid)
            else {
                break;
            };
            shared.get_or_insert((context.link, context.protection));
            let member = Member {
                frame: context.frame,
                order: context.order,
                admitted: context.admitted,
                sequence: context.sequence,
                attempts: context.attempts,
            };
            // `count` is at most the bank's vacancies and both vectors'
            // capacity: none refuses.
            let lost = match self.members.store(member) {
                Err(_member) => Some(buffer),
                Ok(place) => match (places.push(place), mpdus.push(buffer)) {
                    (Ok(()), Ok(())) => None,
                    (Ok(()), Err(buffer)) => {
                        let _place = places.pop();
                        let _member = self.members.take(place);
                        Some(buffer)
                    }
                    (Err(_), Ok(())) => {
                        let _member = self.members.take(place);
                        mpdus.pop()
                    }
                    (Err(_), Err(buffer)) => {
                        let _member = self.members.take(place);
                        Some(buffer)
                    }
                },
            };
            if let Some(buffer) = lost {
                report(events, || StaEvent::DataLost(DataLoss::Overflow));
                self.buffers
                    .put(crate::events::fresh(buffer, D::renew, events), events);
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
                members: PhantomData,
            },
            ticket,
            mpdus,
        }))
    }

    /// Give back the members the port's builder did not take: the tail of
    /// `context`, whose memory `tail` holds in the same order. They return
    /// to their queue unchanged.
    pub fn unbuilt(
        &mut self,
        context: &mut DataAggregateContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        mut tail: heapless::Vec<ReturnedMpdu<D::Buffer, D::Sent>, MAX_AGGREGATE>,
        events: &mut impl EventSink<StaEvent>,
    ) {
        while let Some(buffer) = tail.pop() {
            let member = context.places.pop().map(|place| self.members.take(place));
            match member {
                Some(Some(member)) => {
                    let single = context.single(member);
                    self.requeue(single, buffer, events);
                }
                Some(None) => {
                    // The bank lost the member its place named.
                    report(events, || StaEvent::TokenLost);
                    self.buffers
                        .put(crate::events::fresh(buffer, D::renew, events), events);
                }
                None => self
                    .buffers
                    .put(crate::events::fresh(buffer, D::renew, events), events),
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
        now: oer_time::Instant,
        retry: Option<AggregateRetry>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<BlockAckMarks> {
        // Without a policy a missing member is not retried.
        let retry = retry.unwrap_or(AggregateRetry {
            lifetime: oer_time::Duration::ZERO,
            attempts: 0,
        });
        let (context, ticket, mut mpdus, completion) = match input {
            DataAggregateReturn::Refused {
                context,
                ticket,
                mpdus,
            } => (context, ticket, mpdus, None),
            DataAggregateReturn::TxDone {
                context,
                ticket,
                mpdus,
                completion,
            } => (context, ticket, mpdus, Some(completion)),
        };
        self.tickets.put(ticket, events);
        let DataAggregateContext {
            admission,
            link,
            tid,
            access_category,
            protection,
            mut places,
            members: _,
        } = context;
        self.admission.put(access_category, admission);
        let shell = DataAggregateContext {
            admission: (),
            link,
            tid,
            access_category,
            protection,
            places: heapless::Vec::new(),
            members: PhantomData,
        };
        let current = Some(link) == self.link;
        let mut marks = places
            .first()
            .and_then(|place| self.members.get(*place))
            .map(|member| BlockAckMarks::new(tid, member.sequence));
        // Members and memory pair from the back; the port returns them in order.
        let mut index = places.len();
        while let Some(buffer) = mpdus.pop() {
            let Some(place) = places.pop() else {
                self.buffers
                    .put(crate::events::fresh(buffer, D::renew, events), events);
                continue;
            };
            index = index.saturating_sub(1);
            let Some(mut member) = self.members.take(place) else {
                // The bank lost the member its place named.
                report(events, || StaEvent::TokenLost);
                self.buffers
                    .put(crate::events::fresh(buffer, D::renew, events), events);
                continue;
            };
            if !current {
                self.buffers
                    .put(crate::events::fresh(buffer, D::renew, events), events);
                continue;
            }
            let settled = match &completion {
                // A refused aggregate ends its MSDUs, as a refused single
                // does: the same refusal would only come again.
                None => Some(false),
                Some(TxOutcome::Withdrawn(
                    oer_ieee80211_contracts_port::WithdrawCause::GateClosed
                    | oer_ieee80211_contracts_port::WithdrawCause::Disabled,
                )) => None,
                Some(TxOutcome::Withdrawn(
                    oer_ieee80211_contracts_port::WithdrawCause::KeyRemoved,
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
                            oer_ieee80211_core_upper_mac::block_ack::acknowledges(
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
                        marks.mark(member.sequence, delivered);
                    }
                    self.buffers
                        .put(crate::events::fresh(buffer, D::renew, events), events);
                }
                None => {
                    let single = shell.single(member);
                    self.requeue(single, buffer, events);
                }
            }
        }
        // Members whose memory did not come back cannot be retried.
        while let Some(place) = places.pop() {
            let _member = self.members.take(place);
            report(events, || StaEvent::TokenLost);
        }
        marks.filter(|marks| current && !marks.is_empty())
    }

    /// The checked description of the member at `index` of `context`.
    pub fn aggregate_frame<A>(
        &self,
        context: &DataAggregateContext<'id, K, D::Class, A, D::EpochPath>,
        index: usize,
    ) -> Option<oer_ieee80211_contracts_port::ClassSizedFrame<D::Class, AckHeader, K>>
    where
        oer_ieee80211_contracts_port::ClassSizedFrame<D::Class, AckHeader, K>: Copy,
    {
        let place = *context.places.get(index)?;
        self.members.get(place).map(|member| member.frame)
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
        let next_order = self.next_order;
        let again = OwedMpdu { context, buffer };
        if let Err(again) = self.owed.insert_by(again, |a, b| {
            a.context.order.wrapping_sub(next_order) < b.context.order.wrapping_sub(next_order)
        }) {
            self.overflowed(again, events);
        }
    }
}
