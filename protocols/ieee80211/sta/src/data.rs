//! The connected station's data: a sans-IO machine beside the station,
//! in the station's and the arbiter's form: an MSDU is an input, an
//! attempt an action ([`StaData::poll_action`]). Data keeps no timer; every
//! wait ends on a returned credit or an input from the station or network.
//!
//! An MSDU becomes one MPDU in a buffer of the machine's own: QoS Data by
//! its user priority on a QoS BSS, Data in the station's sequence space
//! otherwise, with the Protected bit and the CCMP header's room on a
//! protected link (the port's backend writes the header). It holds its own
//! tickets and buffers, apart from the station's, so data never takes the
//! station's EAPOL, Null or Deauthentication tokens.
//!
//! A received data MPDU of the station's BSS gives its MSDUs only once the
//! replay check of the association's keys passed it
//! ([`CcmpRxReplayEpoch`](oer_ieee80211_upper_mac::ccmp::CcmpRxReplayEpoch)); the reorder, when it comes, runs before it.
//!
//! The station's link ([`LinkView`]) is only there while data may go: it
//! lends the BSS, the pairwise key (an action carries no key: the service
//! names the lent one when it submits, and only the station removes it),
//! the replay check and, on a non-QoS BSS, the station's sequence space.

use crate::Counter;
use oer_ieee80211_port::{
    AckHeader, HeaderSpec, MpduLength, ReturnedMpdu, RxCryptoStatus, RxEvidence, RxMeta, SingleEnd,
    SingleOutcome, SizedFrame, TxBufferMemory, TxOutcome,
};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::{
    qos::{WmmAccessCategory, WmmUserPriority},
    sequence::SequenceNumber,
    station::StaProtectedDataFrame,
};

use crate::epoch::LinkEpoch;
use crate::machine::{LinkMode, LinkProtection, LinkView, StaTokens};
use crate::pool::{Fifo, Pool};
use oer_radio_port::{PoolPath, Root};

/// The tokens of the data machine: the port's tickets and the buffers of
/// the class that holds an MSDU's MPDU.
pub trait StaDataTokens {
    /// Component owning the connected station's epoch source.
    type EpochPath: PoolPath;
    /// The checked class carried by the MPDU description and context.
    type Class: oer_ieee80211_port::TxBufferClass;
    /// A transmit ticket.
    type Ticket;
    /// A transmit buffer: its memory, which the machine encodes into.
    type Buffer: TxBufferMemory;
    /// An immutable published MPDU. Data retains it until retry or retirement.
    type Sent;
    /// What the port observed of a solicited response.
    type AckEvidence: Copy;
    /// The port's evidence that coexistence denied an attempt the air.
    type AirDenied: Copy;
    /// Retire an MPDU and release its body before reusing the allocation.
    fn renew(mpdu: Self::Sent) -> Self::Buffer;
}

/// One MSDU from the network stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Msdu<'a> {
    /// Its destination.
    pub destination: [u8; 6],
    /// Its EtherType.
    pub ether_type: u16,
    /// Its user priority.
    pub priority: WmmUserPriority,
    /// Its payload.
    pub payload: &'a [u8],
    /// When the radio's software admitted it, on the monotonic clock: its
    /// age starts here and stays with each of its attempts.
    pub admitted: oer_time::Instant,
}

/// An MSDU and the station link on which it is submitted.
///
/// ```compile_fail,E0223
/// use oer_ieee80211_sta::{data::DataInput, machine::StaTokens};
/// fn variant<T: StaTokens>(input: DataInput<'_, '_, '_, '_, T>) -> bool {
///     matches!(input, DataInput::Msdu { .. })
/// }
/// ```
///
/// ```
/// use oer_ieee80211_sta::{data::DataInput, machine::StaTokens};
/// fn payload_length<T: StaTokens>(input: DataInput<'_, '_, '_, '_, T>) -> usize {
///     input.msdu.payload.len()
/// }
/// ```
pub struct DataInput<'a, 'l, 'v, 'id, T: StaTokens> {
    /// The MSDU from the network stack.
    pub msdu: Msdu<'a>,
    /// The station link on which it goes.
    pub link: &'l mut LinkView<'v, 'id, T>,
}

/// Ownership returned from one operation; consuming it cannot refuse resources.
pub enum DataReturn<'id, D: StaDataTokens, K> {
    /// Preparation refused before admission: this MSDU ends, its resources
    /// return and its admission count is released without a radio terminal.
    Refused {
        /// Original operation, moved back from submit.
        context: DataTxContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its memory.
        buffer: ReturnedMpdu<D::Buffer, D::Sent>,
        /// The explicit preparation failure.
        reason: crate::machine::TxRefusal,
    },
    /// A data MPDU's terminal: its ticket and buffer come back. An attempt
    /// of the current link that ended before any transmission (a closing
    /// gate aborts the ones it holds) goes again: back at the head of its
    /// TID's queue, before newer MSDUs, in its buffer, which comes back
    /// fresh, with its sequence number. One of a link that ended only
    /// returns its tokens ([`StaData::leave`]).
    TxDone {
        /// The original operation, returned unchanged by the port.
        context: DataTxContext<'id, K, D::Class, AcPermit, D::EpochPath>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its buffer.
        buffer: ReturnedMpdu<D::Buffer, D::Sent>,
        /// How it ended.
        completion: SingleOutcome<D::AckEvidence, D::AirDenied>,
    },
}

/// One data MPDU to submit: its tokens, its kind and its access category.
///
/// The data machine returns this operation directly:
///
/// ```compile_fail,E0432
/// use oer_ieee80211_sta::data::DataAction;
/// ```
/// It names no key: on a protected link the service submits it under the
/// link's pairwise key.
///
/// The operation cannot be changed into an open frame by changing a flag:
///
/// ```compile_fail,E0609
/// use oer_ieee80211_sta::data::{DataTransmit, StaDataTokens};
/// fn open<D: StaDataTokens, K>(transmit: &mut DataTransmit<'_, D, K>) {
///     transmit.protected = false;
/// }
/// ```
///
/// Its frame and access category belong only to the closed context:
///
/// ```compile_fail,E0609
/// use oer_ieee80211_sta::data::{DataTransmit, StaDataTokens};
/// fn replace<D: StaDataTokens, K>(
///     first: &mut DataTransmit<'_, D, K>, second: DataTransmit<'_, D, K>,
/// ) { first.frame = second.frame; }
/// ```
///
/// ```compile_fail,E0609
/// use oer_ieee80211_wire::qos::WmmAccessCategory;
/// use oer_ieee80211_sta::data::{DataTransmit, StaDataTokens};
/// fn change_category<D: StaDataTokens, K>(transmit: &mut DataTransmit<'_, D, K>) {
///     transmit.access_category = WmmAccessCategory::Voice;
/// }
/// ```
///
/// The service consumes the operation and returns every resource on refusal:
///
/// ```
/// use oer_ieee80211_sta::data::{DataReturn, DataTransmit, DataTransmitParts, StaDataTokens};
/// use oer_ieee80211_sta::machine::TxRefusal;
/// fn refuse<D: StaDataTokens, K>(transmit: DataTransmit<'_, D, K>) -> DataReturn<'_, D, K> {
///     let DataTransmitParts { context, ticket, buffer } = transmit.into_parts();
///     DataReturn::Refused { context, ticket, buffer, reason: TxRefusal::NoRate }
/// }
/// ```
#[must_use = "the operation owns a ticket, buffer and admission permission"]
pub struct DataTransmit<'id, D: StaDataTokens, K, A = AcPermit> {
    /// The operation identity and parameters, moved through the port.
    context: DataTxContext<'id, K, D::Class, A, D::EpochPath>,
    /// Its ticket.
    ticket: D::Ticket,
    /// The buffer holding the MPDU, from its header on.
    buffer: ReturnedMpdu<D::Buffer, D::Sent>,
}

impl<'id, D: StaDataTokens, K, A> DataTransmit<'id, D, K, A> {
    /// Consume the operation, preserving its context and both resource owners.
    pub fn into_parts(self) -> DataTransmitParts<'id, D, K, A> {
        DataTransmitParts {
            context: self.context,
            ticket: self.ticket,
            buffer: self.buffer,
        }
    }
}

/// All owners consumed from a data action for submission or a refusal return.
/// The frame, protection and access category remain in the closed context.
#[must_use = "all owners must reach the port or return to Data"]
pub struct DataTransmitParts<'id, D: StaDataTokens, K, A = AcPermit> {
    /// The original operation and its admission permission.
    pub context: DataTxContext<'id, K, D::Class, A, D::EpochPath>,
    /// The operation's ticket.
    pub ticket: D::Ticket,
    /// Its fresh or previously published MPDU.
    pub buffer: ReturnedMpdu<D::Buffer, D::Sent>,
}

/// The protection selected from the station's link for one data operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataProtection<K> {
    /// An ordinary open MPDU, without a CCMP header or key.
    Open,
    /// A CCMP MPDU using the link's pairwise key.
    Ccmp(K),
}

impl<'a, T: StaTokens> From<&LinkProtection<'a, T>> for DataProtection<&'a T::PairwiseKey> {
    fn from(protection: &LinkProtection<'a, T>) -> Self {
        match protection {
            LinkProtection::Open => Self::Open,
            LinkProtection::Ccmp { key, .. } => Self::Ccmp(key),
        }
    }
}

impl<K> DataProtection<K> {
    fn without_key(&self) -> DataProtection<()> {
        match self {
            Self::Open => DataProtection::Open,
            Self::Ccmp(_) => DataProtection::Ccmp(()),
        }
    }
}

/// Why an MSDU did not become an MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataRefusal {
    /// The machine holds no ticket or buffer now; the MSDU waits.
    NoTokens,
    /// The MSDU does not fit a buffer.
    TooLong,
}

/// The original data operation. It travels by value with the attempt;
/// completion routing never inspects the returned MPDU's bytes.
/// Its closed, checked frame description survives every unpublished return;
/// neither the MPDU length nor its header is reconstructed at completion.
#[derive(Debug)]
pub struct DataTxContext<
    'id,
    K,
    C,
    A = AcPermit,
    Path: PoolPath = Root<oer_radio_port::LocalId<'id>>,
> {
    admission: A,
    link: LinkEpoch<'id, Path>,
    tid: WmmUserPriority,
    frame: oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K>,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    order: u32,
    admitted: oer_time::Instant,
}

/// Operation data retained with an owned data epoch. Its private fields cannot
/// reconstruct an epoch or an access-category admission permission.
pub struct DataTxInfo<K, C, A> {
    admission: A,
    tid: WmmUserPriority,
    frame: oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K>,
    access_category: WmmAccessCategory,
    protection: DataProtection<()>,
    order: u32,
    admitted: oer_time::Instant,
}

impl<'p, K: 'p, C: 'p, A: 'p, Path: PoolPath + 'p> oer_radio_port::LoanedContext<'p, Path::Identity>
    for DataTxContext<'p, K, C, A, Path>
where
    Path::Identity: 'p,
{
    type Owned = crate::epoch::EpochContext<Path, DataTxInfo<K, C, A>>;

    fn enclose(self) -> oer_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        self.link.context(DataTxInfo {
            admission: self.admission,
            tid: self.tid,
            frame: self.frame,
            access_category: self.access_category,
            protection: self.protection,
            order: self.order,
            admitted: self.admitted,
        })
    }

    fn restore(context: oer_radio_port::PortContext<'p, Path::Identity, Self::Owned>) -> Self {
        let (link, data) = context.into_parts();
        Self {
            admission: data.admission,
            link,
            tid: data.tid,
            frame: data.frame,
            access_category: data.access_category,
            protection: data.protection,
            order: data.order,
            admitted: data.admitted,
        }
    }
}

/// One non-cloneable permission for a data attempt in an access category.
/// Only the data machine's depth pool creates it; the context returns it.
#[derive(Debug)]
pub struct AcPermit {
    bit: core::num::NonZeroU8,
}

/// At most eight tickets can exist in this machine. One bit represents
/// one actual permission; taking clears it, returning the owned token sets
/// it. No increment/decrement or metadata lookup reconstructs admission.
struct Admissions {
    by_category: [u8; 4],
}

impl Admissions {
    fn new(depth: core::num::NonZeroU8, tickets: usize) -> Self {
        let count = usize::from(depth.get()).min(tickets);
        let mask = (0..count).fold(0u8, |mask, _| (mask << 1) | 1);
        Self {
            by_category: [mask; 4],
        }
    }

    fn available(&self, category: WmmAccessCategory) -> u8 {
        *category.select(&self.by_category)
    }

    fn take(&mut self, category: WmmAccessCategory) -> Option<AcPermit> {
        let pool = category.select_mut(&mut self.by_category);
        let bit = core::num::NonZeroU8::new(*pool & pool.wrapping_neg())?;
        *pool &= !bit.get();
        Some(AcPermit { bit })
    }

    fn put(&mut self, category: WmmAccessCategory, permit: AcPermit) {
        *category.select_mut(&mut self.by_category) |= permit.bit.get();
    }
}

impl<K: oer_ieee80211_port::Ieee80211Capabilities, C, A, Path: PoolPath>
    DataTxContext<'_, K, C, A, Path>
{
    /// A data MPDU goes without an RTS/CTS or CTS-to-self exchange.
    pub const PROTECTION: oer_ieee80211_port::Protection = oer_ieee80211_port::Protection::None;
    /// A data MPDU goes at the port's calibrated power.
    pub const POWER: oer_ieee80211_port::SupportedPower<K> =
        crate::machine::calibrated_power::<K>();
    /// A data MPDU takes the ordinary coexistence class of its access category.
    pub const COEX_OPERATION: oer_ieee80211_port::TxOperation =
        oer_ieee80211_port::TxOperation::Data;
}

impl<'id, K, C, A, Path: PoolPath> DataTxContext<'id, K, C, A, Path> {
    /// The checked MPDU description selected when the bytes were encoded.
    pub const fn frame(&self) -> &oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K> {
        &self.frame
    }

    /// The sole access category of this operation and its admission permission.
    pub const fn access_category(&self) -> WmmAccessCategory {
        self.access_category
    }

    /// When the radio's software admitted the MSDU: its age starts here,
    /// through every retransmission.
    pub const fn admitted(&self) -> oer_time::Instant {
        self.admitted
    }

    /// Borrow the matching link's protection for this operation. A stale link
    /// or a changed protection mode refuses before the port sees the frame.
    /// In particular, a CCMP encoding never falls through to open submission.
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

    fn admission<B>(self, admission: B) -> (DataTxContext<'id, K, C, B, Path>, A) {
        (
            DataTxContext {
                admission,
                link: self.link,
                tid: self.tid,
                frame: self.frame,
                access_category: self.access_category,
                protection: self.protection,
                order: self.order,
                admitted: self.admitted,
            },
            self.admission,
        )
    }
}

/// What the data machine dropped, counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCounters {
    /// Current-link MSDUs ended by a preparation refusal, before admission.
    pub preparation_refused: u32,
    /// Current-link MSDUs retired after an unknown hardware outcome.
    pub indeterminate: u32,
    /// Current-link MSDUs retired after their lifetime expired.
    pub expired: u32,
    /// Current-link MSDUs retired after contention budget exhaustion.
    pub contention_limited: u32,
    /// MPDUs a full queue could not hold: their tokens came back, their
    /// MSDUs are gone.
    pub overflows: u32,
    /// Tokens given back beyond the machine's own: dropped.
    pub lost_tokens: u32,
    /// MPDUs of a link that ended before they reached the air: the ones
    /// owed when it ended, and its submitted ones that ended without a
    /// transmission.
    pub left_behind: u32,
}

/// The connected station's data machine. `CAP` sizes each TID's queues of
/// owed MPDUs: each holds a ticket, so the machine's tickets
/// fit (checked at build).
///
/// Data has no timer: its queued work advances on inputs and returned credits.
///
/// ```compile_fail,E0061
/// use oer_ieee80211_port::Ieee80211Capabilities;
/// use oer_ieee80211_sta::data::{StaData, StaDataTokens};
/// use oer_time::Instant;
/// fn poll<D: StaDataTokens, K: Ieee80211Capabilities>(data: &mut StaData<'_, D, K>) {
///     drop(data.poll_action(Instant::from_micros(1)));
/// }
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_port::Ieee80211Capabilities;
/// use oer_ieee80211_sta::data::{StaData, StaDataTokens};
/// fn deadline<D: StaDataTokens, K: Ieee80211Capabilities>(data: &StaData<'_, D, K>) {
///     let _deadline = data.poll_deadline();
/// }
/// ```
///
/// The service drains ready actions after each input:
///
/// ```
/// use oer_ieee80211_port::Ieee80211Capabilities;
/// use oer_ieee80211_sta::data::{DataTransmit, StaData, StaDataTokens};
/// fn action<'id, D: StaDataTokens, K: Ieee80211Capabilities>(
///     data: &mut StaData<'id, D, K>,
/// ) -> Option<DataTransmit<'id, D, K>> {
///     data.poll_action()
/// }
/// ```
pub struct StaData<'id, D: StaDataTokens, K, const CAP: usize = 8> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    /// The attempts owed, by TID (a non-QoS link's in the first), each in
    /// its order.
    owed: [Fifo<DataTransmit<'id, D, K, ()>, CAP>; 8],
    admission: Admissions,
    /// The order of the next MSDU.
    next_order: u32,
    /// The current link: [`Self::leave`] ends it.
    link: Option<LinkEpoch<'id, D::EpochPath>>,
    overflows: Counter,
    left_behind: Counter,
    preparation_refused: Counter,
    indeterminate: Counter,
    expired: Counter,
    contention_limited: Counter,
}

/// Terminal data bank. Queued work has returned its frame credits; it has no
/// admission, input or transmission methods and cannot begin another link.
pub struct RetiredData<'id, D: StaDataTokens, K, const CAP: usize = 8> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    owed: [Fifo<DataTransmit<'id, D, K, ()>, CAP>; 8],
    admission: Admissions,
    next_order: u32,
    overflows: Counter,
    left_behind: Counter,
    preparation_refused: Counter,
    indeterminate: Counter,
    expired: Counter,
    contention_limited: Counter,
}

impl<'id, D: StaDataTokens, K: oer_ieee80211_port::Ieee80211Capabilities, const CAP: usize>
    RetiredData<'id, D, K, CAP>
{
    pub fn counters(&self) -> DataCounters {
        DataCounters {
            preparation_refused: self.preparation_refused.get(),
            indeterminate: self.indeterminate.get(),
            expired: self.expired.get(),
            contention_limited: self.contention_limited.get(),
            overflows: self.overflows.get(),
            lost_tokens: self.tickets.lost().combined(self.buffers.lost()).get(),
            left_behind: self.left_behind.get(),
        }
    }

    pub fn available(&self, category: WmmAccessCategory) -> u32 {
        self.admission.available(category).count_ones()
    }

    /// Move the same credits, ordering frontier and admission permissions into
    /// another run on this installation. No pool or permission is minted.
    pub fn restart(self) -> StaData<'id, D, K, CAP> {
        let Self {
            tickets,
            buffers,
            owed,
            admission,
            next_order,
            overflows,
            left_behind,
            preparation_refused,
            indeterminate,
            expired,
            contention_limited,
        } = self;
        StaData {
            tickets,
            buffers,
            owed,
            admission,
            next_order,
            overflows,
            left_behind,
            preparation_refused,
            indeterminate,
            expired,
            contention_limited,
            link: None,
        }
    }
}

impl<'id, D: StaDataTokens, K: oer_ieee80211_port::Ieee80211Capabilities, const CAP: usize>
    StaData<'id, D, K, CAP>
{
    /// Retire the machine into caller-owned storage without copying its
    /// queued-work banks through the CPU stack.
    #[cfg(feature = "placement")]
    pub fn finish_placed<'a, 'b>(
        mut machine: oer_memory::Placed<'a, Self>,
        destination: &'b mut core::mem::MaybeUninit<RetiredData<'id, D, K, CAP>>,
    ) -> oer_memory::Placed<'b, RetiredData<'id, D, K, CAP>> {
        machine.leave();
        let (
            tickets,
            buffers,
            owed,
            admission,
            next_order,
            link,
            overflows,
            left_behind,
            preparation_refused,
            indeterminate,
            expired,
            contention_limited,
        ) = oer_memory::unpack_placed!(
            machine,
            Self {
                tickets,
                buffers,
                owed,
                admission,
                next_order,
                link,
                overflows,
                left_behind,
                preparation_refused,
                indeterminate,
                expired,
                contention_limited
            }
        );
        drop(link);
        oer_memory::Placed::init(destination, |place| {
            oer_memory::init_in_place!(place, RetiredData {
                tickets: tickets.read(),
                buffers: buffers.read(),
                owed => |place| owed.move_to(place).into_mut(),
                admission: admission.read(),
                next_order: next_order.read(),
                overflows: overflows.read(),
                left_behind: left_behind.read(),
                preparation_refused: preparation_refused.read(),
                indeterminate: indeterminate.read(),
                expired: expired.read(),
                contention_limited: contention_limited.read(),
            })
        })
    }

    /// End admission and reclaim queued frames after the backend returned its
    /// submitted terminals. Returned owners remain in the terminal resource bank.
    pub fn finish(mut self) -> RetiredData<'id, D, K, CAP> {
        self.leave();
        let Self {
            tickets,
            buffers,
            owed,
            admission,
            next_order,
            overflows,
            left_behind,
            preparation_refused,
            indeterminate,
            expired,
            contention_limited,
            link: _,
        } = self;
        RetiredData {
            tickets,
            buffers,
            owed,
            admission,
            next_order,
            overflows,
            left_behind,
            preparation_refused,
            indeterminate,
            expired,
            contention_limited,
        }
    }

    /// The machine with the tickets and buffers the composition granted it,
    /// at most eight of a kind and no more tickets than its queues hold,
    /// checked at compile time.
    pub fn new<const TICKETS: usize, const BUFFERS: usize>(
        tickets: [D::Ticket; TICKETS],
        buffers: [D::Buffer; BUFFERS],
        depth: core::num::NonZeroU8,
    ) -> Self {
        const {
            if TICKETS > CAP {
                panic!("each MPDU a data queue holds takes a ticket: the queues hold every ticket");
            }
        };
        Self {
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            owed: [const { Fifo::new() }; 8],
            admission: Admissions::new(depth, TICKETS),
            next_order: 0,
            link: None,
            preparation_refused: Counter::ZERO,
            indeterminate: Counter::ZERO,
            expired: Counter::ZERO,
            contention_limited: Counter::ZERO,
            overflows: Counter::ZERO,
            left_behind: Counter::ZERO,
        }
    }

    /// Initialize a data bank in place; futures retain its borrowed owner.
    #[cfg(feature = "placement")]
    pub fn init_in_place<const TICKETS: usize, const BUFFERS: usize>(
        storage: &mut core::mem::MaybeUninit<Self>,
        tickets: [D::Ticket; TICKETS],
        buffers: [D::Buffer; BUFFERS],
        depth: core::num::NonZeroU8,
    ) -> &mut Self {
        const {
            if TICKETS > CAP {
                panic!("each MPDU a data queue holds takes a ticket: the queues hold every ticket");
            }
        };
        oer_memory::init_in_place!(storage, Self {
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            owed => |place| oer_memory::init_array(place, |_| Fifo::new()),
            admission: Admissions::new(depth, TICKETS),
            next_order: 0,
            link: None,
            preparation_refused: Counter::ZERO,
            indeterminate: Counter::ZERO,
            expired: Counter::ZERO,
            contention_limited: Counter::ZERO,
            overflows: Counter::ZERO,
            left_behind: Counter::ZERO,
        })
    }

    /// What the machine dropped.
    pub const fn counters(&self) -> DataCounters {
        DataCounters {
            preparation_refused: self.preparation_refused.get(),
            indeterminate: self.indeterminate.get(),
            expired: self.expired.get(),
            contention_limited: self.contention_limited.get(),
            overflows: self.overflows.get(),
            lost_tokens: self.tickets.lost().combined(self.buffers.lost()).get(),
            left_behind: self.left_behind.get(),
        }
    }

    /// The station's link ended: the queue belongs to it. Every MPDU owed
    /// for it drains into its tokens, counted; its submitted ones return
    /// their tokens with their terminals and never go again.
    pub fn leave(&mut self) {
        for queue in &mut self.owed {
            while let Some(owed) = queue.pop() {
                self.left_behind.increment();
                self.tickets.put(owed.ticket);
                self.buffers.put(owed.buffer.into_fresh(D::renew));
            }
        }
        self.link = None;
    }

    /// An MPDU a full queue did not take: its tokens come back, counted.
    fn overflowed(&mut self, transmit: DataTransmit<'id, D, K, ()>) {
        self.overflows.increment();
        self.tickets.put(transmit.ticket);
        self.buffers.put(transmit.buffer.into_fresh(D::renew));
    }

    /// Whether the machine takes one more MSDU now.
    pub fn accepts(&self) -> bool {
        !self.tickets.is_empty() && !self.buffers.is_empty()
    }

    /// Return an accepted terminal or a local refusal and all its owners.
    /// This operation always consumes the resources; it has no refusal result.
    pub fn returned(&mut self, input: DataReturn<'id, D, K>) {
        match input {
            DataReturn::Refused {
                context,
                ticket,
                buffer,
                reason: _,
            } => {
                let (context, permit) = context.admission(());
                self.admission.put(context.access_category, permit);
                if self.is_current(&context) {
                    self.preparation_refused.increment();
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(D::renew));
            }
            DataReturn::TxDone {
                context,
                ticket,
                buffer,
                completion,
            } => {
                let (context, permit) = context.admission(());
                self.admission.put(context.access_category, permit);
                if !self.is_current(&context) {
                    if matches!(completion, TxOutcome::Withdrawn(_)) {
                        self.left_behind.increment();
                    }
                    self.tickets.put(ticket);
                    self.buffers.put(buffer.into_fresh(D::renew));
                    return;
                }
                match completion {
                    TxOutcome::Expired { .. } => {
                        self.expired.increment();
                    }
                    TxOutcome::Indeterminate(_) => {
                        self.indeterminate.increment();
                    }
                    TxOutcome::Ended(SingleEnd::ContentionLimit(_)) => {
                        self.contention_limited.increment();
                    }
                    _ => {}
                }
                // A gate closure preserves the original MPDU, TID and
                // order. Disabled keeps its return behavior across a retune;
                // KeyRemoved releases this MSDU rather than retrying it.
                if matches!(
                    completion,
                    TxOutcome::Withdrawn(
                        oer_ieee80211_port::WithdrawCause::GateClosed
                            | oer_ieee80211_port::WithdrawCause::Disabled
                    )
                ) {
                    let tid = context.tid;
                    let again = DataTransmit {
                        ticket,
                        buffer,
                        context,
                    };
                    let next_order = self.next_order;
                    if let Err(again) = tid.select_mut(&mut self.owed).insert_by(again, |a, b| {
                        a.context.order.wrapping_sub(next_order)
                            < b.context.order.wrapping_sub(next_order)
                    }) {
                        self.overflowed(again);
                    }
                    return;
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(D::renew));
            }
        }
    }

    /// Take one input. An MSDU goes on its link and becomes an attempt
    /// owed.
    ///
    /// # Errors
    ///
    /// Why an MSDU did not become an MPDU: it waits, or is too long.
    pub fn input<T: StaTokens<EpochPath = D::EpochPath>>(
        &mut self,
        input: DataInput<'_, '_, '_, 'id, T>,
    ) -> Result<(), DataRefusal> {
        let DataInput { msdu, link } = input;
        if self.link != Some(link.epoch) {
            self.leave();
            self.link = Some(link.epoch);
        }
        let ticket = self.tickets.take().ok_or(DataRefusal::NoTokens)?;
        let Some(mut buffer) = self.buffers.take() else {
            self.tickets.put(ticket);
            return Err(DataRefusal::NoTokens);
        };
        let tid = msdu.priority;
        let admitted = msdu.admitted;
        let qos = matches!(link.mode, LinkMode::Qos(_));
        // A non-QoS link has one queue, best effort.
        let (slot, access_category) = if qos {
            (tid, msdu.priority.access_category())
        } else {
            (WmmUserPriority::UP0, WmmAccessCategory::BestEffort)
        };
        let protection = DataProtection::from(&link.protection).without_key();
        // A frame that does not fit its buffer takes no sequence
        // number.
        if mpdu_len(qos, protection, msdu.payload.len())
            .is_none_or(|length| length > buffer.bytes_mut().len())
        {
            self.tickets.put(ticket);
            self.buffers.put(buffer);
            return Err(DataRefusal::TooLong);
        }
        let sequence_number = match &mut link.mode {
            LinkMode::Qos(counters) => tid.select_mut(counters).take(),
            LinkMode::NonQos(counter) => counter.take(),
        };
        let frame = encode_frame::<_, K, D::Class>(
            link,
            qos,
            msdu,
            sequence_number,
            protection,
            buffer.bytes_mut(),
        );
        match frame {
            Some(frame) => {
                let order = self.next_order;
                self.next_order = self.next_order.wrapping_add(1);
                let context = DataTxContext {
                    admission: (),
                    link: link.epoch,
                    tid: slot,
                    frame,
                    access_category,
                    protection,
                    order,
                    admitted,
                };
                let owed = DataTransmit {
                    context,
                    ticket,
                    buffer: ReturnedMpdu::Fresh(buffer),
                };
                if let Err(owed) = slot.select_mut(&mut self.owed).push(owed) {
                    self.overflowed(owed);
                }
                Ok(())
            }
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(DataRefusal::TooLong)
            }
        }
    }

    /// Free data permissions in this category, bounded by the backend's
    /// declared queue depth and the number of tickets granted to Data.
    pub fn available(&self, category: WmmAccessCategory) -> u32 {
        self.admission.available(category).count_ones()
    }

    /// Whether this operation belongs to the current link. The service
    /// uses the same guard before updating the link's rate selection.
    pub fn is_current<A>(
        &self,
        context: &DataTxContext<'id, K, D::Class, A, D::EpochPath>,
    ) -> bool {
        Some(context.link) == self.link
    }

    /// The next action whose category owns a free admission permission.
    /// The context carries that permission until its return; queued work
    /// consumes none. The service takes every action before waiting.
    pub fn poll_action(&mut self) -> Option<DataTransmit<'id, D, K>> {
        // The earliest MSDU of an admitted category first, whatever its TID.
        let tid = WmmUserPriority::ALL
            .into_iter()
            .filter_map(|tid| {
                tid.select(&self.owed)
                    .front()
                    .filter(|owed| self.admission.available(owed.context.access_category) != 0)
                    .map(|owed| (owed.context.order, tid))
            })
            .min_by_key(|&(order, _)| order.wrapping_sub(self.next_order))?
            .1;
        // A queue can be empty or an AC can have no free permit: both are
        // normal polling states. Reserve and pop under the same exclusive
        // borrow; no actor can change either between these operations.
        let category = tid.select(&self.owed).front()?.context.access_category;
        let permit = self.admission.take(category)?;
        tid.select_mut(&mut self.owed).pop().map(|transmit| {
            let (context, ()) = transmit.context.admission(permit);
            DataTransmit {
                context,
                ticket: transmit.ticket,
                buffer: transmit.buffer,
            }
        })
    }
}

pub(crate) fn integrity(meta: &RxMeta) -> oer_ieee80211_upper_mac::receive::Integrity {
    use oer_ieee80211_upper_mac::receive::Integrity;
    match meta.crypto {
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
        | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified) => {
            Integrity::DecryptedAndVerified
        }
        _ => Integrity::Unverified,
    }
}

/// The length of the MPDU of a payload of `payload` octets: its header,
/// the CCMP header's room on a protected link, the LLC/SNAP header and the
/// payload.
const fn mpdu_len(qos: bool, protection: DataProtection<()>, payload: usize) -> Option<usize> {
    use oer_ieee80211_wire::{
        ccmp::CCMP_HEADER_LEN,
        data::{
            IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN, LLC_SNAP_HEADER_LEN,
        },
    };
    let prefix = match (qos, protection) {
        (false, DataProtection::Open) => IEEE80211_LEGACY_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN,
        (true, DataProtection::Open) => IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN,
        (false, DataProtection::Ccmp(())) => {
            IEEE80211_LEGACY_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN
        }
        (true, DataProtection::Ccmp(())) => {
            IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN
        }
    };
    payload.checked_add(prefix)
}

/// The MPDU of `msdu` on `link`, written into `output`.
fn encode_frame<
    T: StaTokens,
    K: oer_ieee80211_port::Ieee80211Capabilities,
    C: oer_ieee80211_port::TxBufferClass,
>(
    link: &LinkView<'_, '_, T>,
    qos: bool,
    msdu: Msdu<'_>,
    sequence_number: SequenceNumber,
    protection: DataProtection<()>,
    output: &mut [u8],
) -> Option<oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K>> {
    if protection == DataProtection::Open {
        // The open encoding writes the payload once; it never needs a
        // temporary CCMP gap or a second move of the payload.
        let len = oer_ieee80211_wire::station::StaOpenDataFrame {
            source: link.address,
            bssid: link.bssid,
            destination: msdu.destination,
            sequence_number,
            priority: msdu.priority,
            peer_qos: qos,
            ether_type: msdu.ether_type,
            payload: msdu.payload,
        }
        .encode(output)
        .ok()?;
        return sized(output.split_at_checked(len)?.0);
    }
    let len = StaProtectedDataFrame {
        source: link.address,
        bssid: link.bssid,
        destination: msdu.destination,
        sequence_number,
        user_priority: msdu.priority.value(),
        peer_qos: qos,
        ccmp_header: [0; 8],
        ether_type: msdu.ether_type,
        payload: msdu.payload,
    }
    .encode(output)
    .ok()?;
    sized(output.split_at_checked(len)?.0)
}

fn sized<K: oer_ieee80211_port::Ieee80211Capabilities, C: oer_ieee80211_port::TxBufferClass>(
    frame: &[u8],
) -> Option<oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K>> {
    let header = HeaderSpec::from_mac_header(
        oer_ieee80211_wire::frame::MacHeader::decode(frame)?,
        IndividualAddress::new,
    )?;
    oer_ieee80211_port::ClassSizedFrame::new(SizedFrame::new(
        AckHeader::new(header)?,
        MpduLength::new(frame.len())?,
    )?)
    .ok()
}

mod arp;
mod ethernet;
pub use arp::ArpFrameObservation;
pub use ethernet::{EthernetFrame, FrameTooLong, MAX_ETHERNET_PAYLOAD_LEN, TxMsdu};

#[cfg(test)]
mod tests;
