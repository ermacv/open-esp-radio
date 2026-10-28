//! The station: one sans-IO machine for the whole lifecycle of an
//! infrastructure station, from its scan to its disconnect.
//!
//! The machine takes the port's events, the channel arbiter's answers and
//! notices, its commands and time as [`StaInput`] values, and leaves what
//! the port and the arbiter must do as [`StaAction`] values, in their own
//! terms (`oer-ieee80211-port`, `oer-ieee80211-arbiter`): a thin
//! service loop feeds it and carries its actions out, and holds no logic
//! of its own:
//!
//! ```text
//! loop {
//!     select(phase.next_event(), timer(deadline), commands)
//!     station.input(now, input)
//!     while let Some(action) = station.poll_action(now) { carry it out }
//! }
//! ```
//!
//! # Tokens
//!
//! The machine is generic over its tokens ([`StaTokens`]): the transmit
//! tickets and buffers the composition hands it at its start, minted by
//! the port, its key slots, and the arbiter's [`LeasePlace`]. A token
//! leaves in an action by value and comes back in an input, so what the
//! machine holds is what it may do, and a host test plays the port and the
//! arbiter with tokens of its own. The station encodes each frame straight
//! into the memory of its buffer token ([`TxBufferMemory`]).
//! Management requests use the configured station address as transmitter;
//! the selected BSSID identifies the receiver and the BSS.
//!
//! Station owns keys and their free slots across protocol transitions. Install
//! actions carry private affine pending tokens (key ID and link epoch for a
//! group key); responses return those tokens with the keys. A stale install
//! is removed normally. Group slots are fungible: removal returns a raw slot
//! to the free pool, whose fullness proves that every group key came back.
//! Construction receives the complete vacant association bank, retaining even
//! spare PTK and GTK slots outside the station's selected active capacity.
//! The negotiated SPP decision moves into the PTK installation and its returned
//! owner. GTK installation waits for that owner and borrows the same decision.
//! Leave removes every GTK before the PTK, then synchronously ends the
//! association with the complete returned bank. No extra timer or terminal
//! is introduced; pending key installation stays under the original D4 deadline.
//!
//! # Channels
//!
//! The station never changes the port's channel: it asks the arbiter for a
//! lease of each channel it needs ([`StaAction::RequestChannel`]), listens
//! and transmits there only between the lease's start and its end, and
//! gives it back. Beside an access point, its scan goes in absences. A
//! scan the application asked for ends with a partial result after
//! [`StaConfig::skipped_absences`] consecutive cancelled leases. Among the
//! network's access points, one on the channel of the access point beside
//! the station wins within [`StaConfig::access_point_margin_db`] of the
//! strongest, so a join rarely moves that access point ([follow
//! decision]).
//!
//! A connected station's background scan visits each other channel in an
//! absence announced with a Null (PM=1), and its network's own channel
//! without one: it widens its receive rules to the other networks'
//! management frames for the dwell, then narrows them again and waits for
//! the port to confirm.
//!
//! # States
//!
//! Its state is an enum whose variants hold their own types. Transitions
//! borrow the resident phase and return a replacement description. Established
//! state and its RSN exchange are constructed in place and remain there across
//! D4 confirmation, inputs and actions. Each state names the action it owes,
//! so [`Station::poll_action`] hands it out and moves to the state that
//! awaits its end: there is no queue of actions to overflow.

use core::marker::PhantomData;
use oer_ieee80211_cipher::aes::software_aes128_key_unwrap;
use oer_ieee80211_port::association::KEY_BANK_CAPACITY;
use oer_ieee80211_wire::ccmp::GroupKeyId;

use oer_ieee80211_arbiter::{
    AbsenceAnswer, AbsencePlace, Answer, ChannelRequest, Lease, LeaseId, LeasePlace, LeaveReason,
    Purpose, Refusal, StationNotice, StationPlaceReturns, StationPlaces, UpstreamFact,
};
use oer_ieee80211_upper_mac::protection::HePacketPadding;
use oer_ieee80211_port::{
    AckHeader, ConnectionFrame, GroupAddress, HeaderSpec, Ieee80211Capabilities, MpduLength,
    ReceiveFilter, ReturnedMpdu, RxMetadata, SingleEnd, SingleOutcome, SizedFrame,
    Station as StationRole, StationReceive, SupportedChannel, SupportedEdca, SupportedFilter,
    TxBufferMemory, TxOutcome,
};
use oer_ieee80211_rsn::{
    DEFAULT_EAPOL_FRAME_CAPACITY, OwnedEapolFrame, Pmk, RsnInterface,
    frames::RsnTxFrame,
    keys::{RsnKeyInstall, RsnKeyKind},
    sae::{SaeCommit, SaePassword, SaePasswordElement, SaePasswordToken},
    supplicant::{
        RsnConnectedAction, RsnConnectedProcessError, RsnConnectedSupplicant,
        RsnConnectedSupplicantError, RsnGroupKeyInstallRequest, RsnStaDeadlineEvent,
        RsnStaKeyInstallRequest, RsnStaReady, RsnStaResponseDeadline, RsnStaResponseWait,
        RsnStaRetryInstall, RsnStaSupplicant, RsnStaSupplicantAction, RsnStaSupplicantError,
    },
};
use oer_ieee80211_wire::address::{Bssid, IndividualAddress, OwnAddress};
use {
    crate::{scan::ScanTable, sequence::StaSequenceCounter},
    oer_ieee80211_upper_mac::phy_mode::PhyMode,
    oer_ieee80211_wire::{
        management::{MAX_SSID_LEN, ProbeRequest},
        scan::ScanRecord,
        security::{AssociationAkm, AssociationSecurity, RsnAssociation, StaSecurityPolicy},
        sequence::SequenceNumber,
        station::{
            AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest, SelectedRsn,
            StaManagementFrame, StaManagementSubtype, select_association_rsn,
        },
        station_beacon::parse_sta_beacon,
        station_power_save::{StaNullDataFrame, StaPowerManagement},
    },
};
use {oer_ieee80211_upper_mac::ccmp::CcmpRxReplayEpoch, oer_ieee80211_wire::ccmp::Ccmp128Key};

use crate::association::StaAssociatedPeer;
use crate::receive::{Management, Received, observe_scan};
// The machine's tests build receive metadata through `use super::*`.
#[cfg(test)]
use oer_ieee80211_port::RxEvidence;
use oer_time::{Duration, Instant};

use crate::epoch::{LinkEpoch, LinkEpochSource};
use crate::join::sae::{StaSaeAuthentication, StaSaeEvent, StaSaeFailure, StaSaeTransmission};
use crate::pmksa::{StaPmksa, StaPmksaCache};
use crate::pool::Pool;
use crate::request::StationListenInterval;
use oer_ieee80211_port::ConfigurePermission;
use oer_radio_port::{PoolPath, Root};
mod block_ack;
mod commands;
mod deferred_eapol;
mod group_keys;
mod management;
use crate::power_save::{NullOutcome, PowerSaveDecision, PowerSaveView};
pub use block_ack::{
    BlockAckReplyRefusal, BlockAckReplyResult, PendingRxBlockAck, RxBlockAckReply,
};
use commands::Commands;
pub use commands::{
    BackgroundScanEnd, CommandRefusal, ConnectCommand, DisconnectCommand, ScanCommand,
    StaCommandCredit, StaCommandCredits, StaCommandOutcome, StaCommandPermission,
    StaCommandRefused, StaCommandTerminal, StaRequest,
};
pub use deferred_eapol::{DeferredEapol, PendingGroupM1, PendingM3};
use group_keys::{GroupKeys, GroupPlace, Installed as InstalledGroup, Replay as GroupReplay};
use management::{ManagementEvent, ManagementProtection};

use crate::join::{
    association::{
        StaAssociationEvent, StaAssociationFailure, StaAssociationPoll, StaAssociationRuntime,
    },
    authentication::{StaAuthenticationEvent, StaAuthenticationExchange, StaAuthenticationRuntime},
};

/// A refusal before admission, delivered by the service to the owning machine.
/// It is not a radio terminal and never updates transmission statistics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxRefusal {
    /// The key's core-owned 48-bit packet-number space is exhausted.
    PacketNumberExhausted,
    /// A data operation's connection epoch or protection no longer matches
    /// the station's link. Its original resources return without submission.
    LinkChanged,
    /// Service rate selection found no rate for the current phase.
    NoRate,
    /// The pool refused preparation and returned the unchanged source.
    Preparation(oer_ieee80211_port::PreparationError),
    /// Portable placement failed before entering the backend.
    Layout(oer_ieee80211_port::LayoutError),
}

/// The tokens a station holds: the port's, or a host test's.
pub trait StaTokens {
    /// Total slots in the complete bank; unused PTKs remain with the station.
    const PAIRWISE_SLOTS: usize;
    /// Physical GTK slots, including those outside the selected active bank.
    const TOTAL_GROUP_SLOTS: usize;
    /// Group slots granted by this backend resource type.
    const GROUP_SLOTS: usize;
    /// Component owning the station's channel arbiter and lease admission.
    type GatePath: PoolPath;
    /// Component owning this station's connection epoch source.
    type EpochPath: PoolPath;
    /// Installation component owning this interface's command permissions.
    type ConfigurationPath: PoolPath;
    /// The checked size class of management frames.
    type Small: oer_ieee80211_port::TxBufferClass;
    /// A transmit ticket: a place in flight and a place for its terminal.
    type Ticket;
    /// A transmit buffer of the small class, for management frames: its
    /// memory, which the station encodes into.
    type SmallBuffer: TxBufferMemory;
    /// An immutable published MPDU, including its body and original description.
    type Sent;
    /// What the port observed of a solicited response.
    type AckEvidence: Copy;
    /// The port's evidence that coexistence denied an attempt the air.
    type AirDenied: Copy;
    /// Release a completed MPDU for a new frame, ending its retained body owner.
    fn renew(mpdu: Self::Sent) -> Self::SmallBuffer;
    /// The affine start permission, reclaimed only through the complete key bank.
    type AssociationStart;
    /// Immutable association policy shared by every installed key.
    type Association;
    /// Every vacant PTK and GTK slot of this interface.
    type VacantKeyBank;
    /// A start permission together with every vacant slot.
    type VacantAssociation;
    /// Split the complete initial bank; one PTK is active, the rest are retained.
    fn split_vacant(
        vacant: Self::VacantAssociation,
    ) -> oer_ieee80211_port::association::DistributedKeyBank<
        Self::AssociationStart,
        Self::PairwiseSlot,
        Self::GroupSlot,
        KEY_BANK_CAPACITY,
    >;
    /// Select SPP once before installing PTK.
    fn begin_association(
        start: Self::AssociationStart,
        spp: oer_ieee80211_wire::security::Spp,
    ) -> Self::Association;
    /// Collect returns; a partial bank remains owned on failure.
    fn collect_key_bank(
        parts: oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    ) -> Result<
        Self::VacantKeyBank,
        oer_ieee80211_port::association::KeyBankParts<
            Self::PairwiseSlot,
            Self::GroupSlot,
            KEY_BANK_CAPACITY,
        >,
    >;
    /// End synchronously, then distribute the complete bank for another association.
    fn end_association(
        association: Self::Association,
        bank: Self::VacantKeyBank,
    ) -> oer_ieee80211_port::association::DistributedKeyBank<
        Self::AssociationStart,
        Self::PairwiseSlot,
        Self::GroupSlot,
        KEY_BANK_CAPACITY,
    >;
    /// The installed PTK retains the association owner until its removal terminal.
    fn association(key: &Self::PairwiseKey) -> &Self::Association;
    /// A pairwise key slot.
    type PairwiseSlot;
    /// A unique, fungible group slot retained by the backend until removal.
    type GroupSlot;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey;
    /// An installed group key, which holds its slot.
    type GroupKey;
    /// Unique admission to this station interface's GTK mutations.
    type GroupPermission;
    /// The metadata the port gives a received frame.
    type RxMeta: RxMetadata;
}

/// A resource-return violation prevents any further key or association mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaKeyContractViolation {
    /// Removal returned the association before all of its slots were recovered.
    IncompleteBank,
    /// A group installation has no installed PTK association owner.
    MissingAssociation,
}

/// How the station protects its link.
pub enum StaSecurity {
    /// An open network.
    Open,
    /// Personal credentials: the password supports SAE and the prederived
    /// PMK supports PSK. Selection prefers SAE whenever the BSS offers it.
    Personal { password: SaePassword, pmk: Pmk },
}

/// What the application asks of the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCommand {
    /// Connect to the configured network.
    Connect,
    /// Leave the network and stay idle.
    Disconnect,
    /// Scan the channel list while connected.
    BackgroundScan,
}

/// One input of the station.
pub enum StaInput<'id, T: StaTokens, K> {
    /// A deadline the station named passed.
    Tick,
    /// The terminal of a transmission: its ticket and buffer come back.
    TxDone {
        /// The original operation, returned by the port.
        context: StaTxContext<'id, T::EpochPath>,
        /// The transmission's ticket.
        ticket: T::Ticket,
        /// The transmission's buffer.
        buffer: ReturnedMpdu<T::SmallBuffer, T::Sent>,
        /// How it ended.
        completion: SingleOutcome<T::AckEvidence, T::AirDenied>,
    },
    /// A frame the service did not send (no rate of its plan is on the
    /// port's channel, or it does not fit its buffer class): its ticket and
    /// buffer come back with a typed refusal. This is not an accepted TX
    /// completion and cannot complete the handshake.
    Unsent {
        /// Refused before admission; no transmission terminal exists.
        reason: TxRefusal,
        /// The operation that was not submitted.
        context: StaTxContext<'id, T::EpochPath>,
        /// The frame's ticket.
        ticket: T::Ticket,
        /// The frame's buffer.
        buffer: ReturnedMpdu<T::SmallBuffer, T::Sent>,
    },
    /// The same-step answer consumes the request's unique LeasePlace<'id, T::GatePath>, either
    /// returning it on refusal or transferring its ownership into a Lease.
    Answer(Answer<'id, K, T::GatePath>),
    /// The same-step answer returns the unique AbsencePlace or its Lease.
    /// Neither resource is cloneable or constructible by the service.
    AbsenceAnswer(AbsenceAnswer<'id, K, T::GatePath>),
    /// The primary lease of the channel the port moved to, which the
    /// arbiter exchanged for the old one. Exchange consumes the old affine
    /// Lease and returns its replacement in the same service step. A refusal
    /// returns the original lease for cleanup and ends the connected link.
    Exchanged(Result<Lease<'id, K, T::GatePath>, Lease<'id, K, T::GatePath>>),
    /// A notice of the arbiter.
    Arbiter(StationNotice<'id, K, T::GatePath>),
    /// The receive configuration the station set took effect.
    Configured(
        ConfigurePermission<
            'id,
            StationRole,
            T::ConfigurationPath,
            StaConfigurationContext<'id, T::EpochPath>,
        >,
    ),
    /// The pairwise key the station asked to install, installed.
    PairwiseKeyInstalled {
        /// The original installation action.
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        /// The installed key.
        key: T::PairwiseKey,
    },
    /// The group key the station asked to install, installed under the
    /// index its action named.
    GroupKeyInstalled {
        /// Permission returned by the successful FIFO terminal.
        permission: T::GroupPermission,
        /// The installed key.
        key: T::GroupKey,
        /// The original installation action, including epoch and key index.
        pending: PendingGroupKey<'id, T::EpochPath>,
    },
    /// The FIFO replacement terminal for the pending operation's slot.
    GroupKeyReplaced {
        /// Permission returned by the successful FIFO terminal.
        permission: T::GroupPermission,
        /// The private successor epoch and installation record.
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        /// The successor key, retaining the same physical slot.
        key: T::GroupKey,
    },
    /// Selection of an existing installation reached its RX boundary.
    GroupKeySelected {
        pending: PendingGroupSelection<'id, T::EpochPath>,
        key: T::GroupKey,
        permission: T::GroupPermission,
    },
    /// A removed pairwise key's slot.
    PairwiseKeyRemoved {
        /// Returned key slot.
        slot: T::PairwiseSlot,
        /// The same association retained through removal.
        association: T::Association,
    },
    /// A removed group key's slot and its unique mutation permission.
    GroupKeyRemoved {
        pending: PendingGroupRemoval<'id, T::EpochPath>,
        slot: T::GroupSlot,
        permission: T::GroupPermission,
    },
}

/// One action of the station, which the service carries out at once.
/// Discarding a produced action is diagnosed by `unused_must_use`:
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use oer_ieee80211_sta::machine::{StaAction, StaTokens};
/// fn ignore<'a, 'id, T: StaTokens, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>)
/// where T::PairwiseKey: 'a {
///     next();
/// }
/// ```
/// The consumer may transfer it to the component that performs it:
/// ```
/// #![deny(unused_must_use)]
/// use oer_ieee80211_sta::machine::{StaAction, StaTokens};
/// fn forward<'a, 'id, T: StaTokens, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>)
///     -> StaAction<'a, 'id, T, K>
/// where T::PairwiseKey: 'a {
///     next()
/// }
/// ```
#[must_use = "carry out the action or return its owners through a terminal path"]
pub enum StaAction<'a, 'id, T: StaTokens, K> {
    /// A validated Block Ack action of the current BSS, for its RX bank owner.
    BlockAck(PendingRxBlockAck<'id, T::EpochPath>),
    /// Stop the port; the machine retains the incomplete resource bank.
    ContractViolation(StaKeyContractViolation),
    /// Ask the arbiter for a channel, with the station's place; the service
    /// hands the answer back ([`StaInput::Answer`]) in the same step.
    RequestChannel(ChannelRequest<K>, LeasePlace<'id, T::GatePath>),
    /// Ask the arbiter for an absence from the primary lease `home`, with
    /// the absence's place; the service hands the answer back
    /// ([`StaInput::AbsenceAnswer`]) in the same step.
    RequestAbsence {
        /// The absence's channel and purpose.
        request: ChannelRequest<K>,
        /// The running primary lease.
        home: &'a Lease<'id, K, T::GatePath>,
        /// The absence's place.
        place: AbsencePlace<'id, T::GatePath>,
    },
    /// Tell the arbiter the station is ready for its absence: its access
    /// point knows it leaves.
    Ready(LeaseId<'id, T::GatePath>),
    /// Exchange the primary lease after the port moved; the service hands
    /// the new one back ([`StaInput::Exchanged`]).
    ExchangeLease(Lease<'id, K, T::GatePath>),
    /// Give a lease back to the arbiter.
    ReleaseChannel(Lease<'id, K, T::GatePath>),
    /// Tell the arbiter a fact about the station's upstream.
    Upstream(UpstreamFact<K>),
    /// Set what the station's interface receives ([`StaInput::Configured`]
    /// follows).
    Configure(
        StationReceive<K>,
        ConfigurePermission<
            'id,
            StationRole,
            T::ConfigurationPath,
            StaConfigurationContext<'id, T::EpochPath>,
        >,
    ),
    /// Set the EDCA parameters the station's interface contends with: the
    /// access point's, from its association on and whenever its beacons
    /// announce another set. No input follows.
    SetEdca(SupportedEdca<K>),
    /// Install a pairwise key ([`StaInput::PairwiseKeyInstalled`]).
    InstallPairwiseKey {
        /// Unique policy owner, returned inside the installed pairwise owner.
        association: T::Association,
        /// The response must return this token.
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        /// The slot it takes.
        slot: T::PairwiseSlot,
        /// The access point.
        peer: IndividualAddress,
        /// The temporal key.
        key: Ccmp128Key,
    },
    /// Install a group key ([`StaInput::GroupKeyInstalled`]).
    InstallGroupKey {
        /// The decision retained by the PTK of this association.
        association: &'a T::Association,
        /// Exclusive GTK mutation permission, held through the terminal.
        permission: T::GroupPermission,
        /// The response must return this token.
        pending: PendingGroupKey<'id, T::EpochPath>,
        /// The slot it takes.
        slot: T::GroupSlot,
        /// Its key index.
        key_id: GroupKeyId,
        /// The temporal key.
        key: Ccmp128Key,
    },
    /// Replace the consumed key in one FIFO operation. Its slot reserves the
    /// terminal; only that terminal switches the station's replay epoch.
    ReplaceGroupKey {
        /// Exclusive GTK mutation permission, held through the terminal.
        permission: T::GroupPermission,
        /// Return with the replacement terminal of this installed key's slot.
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        /// The previous installation and its slot.
        installed: T::GroupKey,
        /// The successor's wire ID.
        key_id: GroupKeyId,
        /// The successor's material.
        key: Ccmp128Key,
    },
    /// Choose a retained installation without writing material or resetting replay.
    SelectGroupKey {
        pending: PendingGroupSelection<'id, T::EpochPath>,
        installed: T::GroupKey,
        permission: T::GroupPermission,
    },
    /// Remove the pairwise key ([`StaInput::PairwiseKeyRemoved`]).
    RemovePairwiseKey(T::PairwiseKey),
    /// Remove a group key ([`StaInput::GroupKeyRemoved`]).
    RemoveGroupKey {
        pending: PendingGroupRemoval<'id, T::EpochPath>,
        key: T::GroupKey,
        permission: T::GroupPermission,
    },
    /// Transmit one MPDU ([`StaInput::TxDone`] follows).
    Transmit(StaTransmit<'id, T, K>),
    /// Transmit one MPDU under the pairwise key its link lends: EAPOL
    /// after the handshake, sent only while the link is up
    /// ([`StaInput::TxDone`] follows). It carries the Protected bit and
    /// the CCMP header's room.
    TransmitProtected {
        /// The MPDU.
        transmit: StaTransmit<'id, T, K>,
        /// The pairwise key, as [`Station::link`] lends it.
        key: &'a T::PairwiseKey,
    },
    /// The station associated with `Peer`: the rates of its frames start
    /// from what the association made known, the handshake's EAPOL
    /// included. A [`StaNotice::Disconnected`] ends it.
    Associated(Peer<T::RxMeta>),
    /// Tell the application what happened.
    Notify(StaNotice),
    /// Return one accepted application's request and its final outcome.
    CommandCompleted(StaCommandTerminal<'id, T::ConfigurationPath>),
}

/// One MPDU to transmit: its ticket, the buffer the station encoded it in,
/// its kind, its class and its rates.
#[must_use = "submit the operation or return its ticket and buffer"]
pub struct StaTransmit<'id, T: StaTokens, K> {
    /// Its epoch and purpose, moved with the operation through its terminal.
    pub context: StaTxContext<'id, T::EpochPath>,
    /// Its ticket.
    pub ticket: T::Ticket,
    /// The buffer holding the frame, from its header on.
    pub buffer: T::SmallBuffer,
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<K, T::Small>,
    /// What the frame is for, which the service turns into the port's
    /// coexistence priority.
    pub class: StaFrameClass,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
}

/// Reserve both owners before consuming a sequence or starting an attempt.
struct TxCredit<'id, T: StaTokens> {
    epoch: LinkEpoch<'id, T::EpochPath>,
    ticket: T::Ticket,
    buffer: T::SmallBuffer,
}

/// The identity of one station operation. No frame parsing or current-state
/// guess assigns a completion to a handshake, power transition or link.
#[derive(Debug)]
pub struct StaTxContext<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    epoch: LinkEpoch<'id, Path>,
    purpose: TxPurpose,
}

/// Opaque purpose retained with a station operation's owned epoch context.
pub struct StaTxPurpose(TxPurpose);

impl<'p, Path: PoolPath + 'p> oer_radio_port::LoanedContext<'p, Path::Identity>
    for StaTxContext<'p, Path>
where
    Path::Identity: 'p,
{
    type Owned = crate::epoch::EpochContext<Path, StaTxPurpose>;

    fn enclose(self) -> oer_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        self.epoch.context(StaTxPurpose(self.purpose))
    }

    fn restore(context: oer_radio_port::PortContext<'p, Path::Identity, Self::Owned>) -> Self {
        let (epoch, StaTxPurpose(purpose)) = context.into_parts();
        Self { epoch, purpose }
    }
}

impl<Path: PoolPath> StaTxContext<'_, Path> {
    /// Whether this operation used the probe rate record.
    pub const fn is_probe(&self) -> bool {
        matches!(
            self.purpose,
            TxPurpose::Probe | TxPurpose::Connected(Sending::Probe)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPurpose {
    Probe,
    Authentication { attempt: u16 },
    SaeAuthentication,
    Association,
    HandshakeResponse,
    HandshakeLast,
    Deauthentication,
    Connected(Sending),
}

/// What a station frame is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaFrameClass {
    /// A frame of the connection's setup or keys: Authentication,
    /// Association and EAPOL, which a coexisting radio should let through
    /// first.
    Connection(ConnectionFrame),
    /// Any other frame.
    Normal,
}

/// The kind of a station frame, as the port's frame kinds take it.
pub enum StaFrame<K, C> {
    /// An individual frame that solicits an ACK.
    Individual(oer_ieee80211_port::ClassSizedFrame<C, AckHeader, K>),
    /// A group frame.
    Group(oer_ieee80211_port::ClassSizedFrame<C, HeaderSpec<GroupAddress>, K>),
}

/// The rates a station frame goes at, which the service turns into the
/// port's retry plan on the phase's channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRatePlan {
    /// An individual management frame: the BSS's lowest basic rate,
    /// retried by the station's management limit.
    Management,
    /// A group frame (a broadcast Probe Request): one transmission at the
    /// band's lowest rate.
    Group,
    /// A Null that tells the access point the station's power state:
    /// [`NULL_TRANSMISSIONS`](crate::power_save::NULL_TRANSMISSIONS)
    /// transmissions at the BSS's lowest basic rate.
    Null,
}

/// A frame refused by admission or by the authenticated RSN protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRxError {
    Frame(oer_ieee80211_upper_mac::receive::RxDropReason),
    Security(RsnConnectedProcessError<oer_ieee80211_cipher::aes::SoftwareAesKeyUnwrapError>),
}

impl From<oer_ieee80211_upper_mac::receive::RxDropReason> for StaRxError {
    fn from(reason: oer_ieee80211_upper_mac::receive::RxDropReason) -> Self {
        Self::Frame(reason)
    }
}

impl From<RsnConnectedSupplicantError> for StaRxError {
    fn from(reason: RsnConnectedSupplicantError) -> Self {
        Self::Security(RsnConnectedProcessError::Supplicant(reason))
    }
}

impl From<RsnConnectedProcessError<oer_ieee80211_cipher::aes::SoftwareAesKeyUnwrapError>>
    for StaRxError
{
    fn from(
        reason: RsnConnectedProcessError<oer_ieee80211_cipher::aes::SoftwareAesKeyUnwrapError>,
    ) -> Self {
        Self::Security(reason)
    }
}

/// The station's connected link, as the data machine sees it while data
/// may go.
pub struct LinkView<'a, 'id, T: StaTokens> {
    /// The station's own address.
    pub address: OwnAddress,
    /// The connection identity, unchanged during sleep and channel absences.
    pub epoch: LinkEpoch<'id, T::EpochPath>,
    /// The access point.
    pub bssid: Bssid,
    /// The station's association identifier.
    pub association_id: u16,
    /// How data is sequenced on the BSS.
    pub mode: LinkMode<'a>,
    /// How data is protected.
    pub protection: LinkProtection<'a, T>,
}

/// How data is sequenced on a link: in the station's sequence spaces,
/// which its own frames share.
pub enum LinkMode<'a> {
    /// A QoS BSS: QoS Data, a sequence space per TID.
    Qos(&'a mut [StaSequenceCounter; 8]),
    /// A non-QoS BSS: Data in the station's sequence space, which its
    /// management frames share.
    NonQos(&'a mut StaSequenceCounter),
}

/// How data is protected on a link.
pub enum LinkProtection<'a, T: StaTokens> {
    /// An open network.
    Open,
    /// CCMP: the pairwise key data goes under, and the replay check of the
    /// association's keys that received data passes.
    Ccmp {
        /// The pairwise key.
        key: &'a T::PairwiseKey,
        /// The replay check.
        replay: &'a mut CcmpRxReplayEpoch,
    },
}

/// What the station tells the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaNotice {
    /// The station is connected to `bssid` on `channel`.
    Connected {
        /// The access point.
        bssid: Bssid,
        /// Its primary channel number.
        channel: u8,
    },
    /// The station left its network, or failed to join one.
    Disconnected(StaDisconnectReason),
}

/// Why the station is not connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectReason {
    InvalidEapol(oer_ieee80211_rsn::EapolParseError),
    RetainedSecurityIes(oer_ieee80211_rsn::frames::RsnFrameError),
    /// A required frame was refused before admission.
    TransmitRefused(TxRefusal),
    /// The application asked it to leave.
    Requested,
    /// No access point of the network answered the scan; `partial` when
    /// the scan ended early after cancelled absences.
    NotFound {
        /// The scan ended early.
        partial: bool,
    },
    /// The arbiter did not let the station on a channel.
    Refused(Refusal),
    /// A move's primary lease exchange was refused; the old lease is returned
    /// through normal channel release before another connection can begin.
    LeaseExchangeRefused,
    /// The arbiter told the station to leave its network.
    Arbiter(LeaveReason),
    /// The access point refused or never answered the authentication.
    AuthenticationFailed,
    /// The selected SAE exchange ended with its concrete cause.
    Sae(StaSaeFailure),
    /// The access point refused or never answered the association.
    AssociationFailed,
    /// The complete monotonic deadline of an operation cannot be represented.
    DeadlineOverflow,
    /// The access point deauthenticated or disassociated the station.
    Peer {
        /// The reason code it gave.
        reason_code: u16,
    },
    /// A frame the station had to send did not encode.
    Encode,
    /// The four-way handshake, or a group key's handshake, failed or timed
    /// out.
    HandshakeFailed,
    /// The peer did not confirm its protected association within SA Query timeout.
    SaQueryTimeout,
}

/// The scan list's channels at most.
const CHANNEL_CAPACITY: usize = 16;
/// The BSSs a scan keeps at most.
const SCAN_CAPACITY: usize = 16;

/// A new association's QoS sequence spaces.
const QOS_SEQUENCES: [StaSequenceCounter; 8] = [StaSequenceCounter::new(SequenceNumber::ZERO); 8];

/// The reason codes of the station's Deauthentication (IEEE 802.11-2020
/// Table 9-90).
const REASON_UNSPECIFIED: u16 = 1;
const REASON_LEAVING: u16 = 3;
const REASON_HANDSHAKE_TIMEOUT: u16 = 15;

/// Statically dispatched composition hooks retained across stop/restart.
/// The provider owns its entropy state; each nonce request must be fresh for
/// protected handshakes and PMF. Open-only compositions never request one.
pub trait StaCallbacks {
    /// Fresh randomness for each handshake and PMF SA Query procedure.
    fn nonce(&mut self) -> [u8; 32];
    /// Composition policy for the peer's complete HE Capabilities element.
    fn he_packet_padding(&self, capability: &[u8]) -> HePacketPadding;
}

/// What the station connects to and how.
///
/// A scan list is a contiguous sequence of checked channels. A hole that
/// could terminate a scan before its later channels is not representable:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_sta::machine::StaConfig;
/// fn holes<K, C: oer_ieee80211_sta::machine::StaCallbacks>(config: &mut StaConfig<K, C>) {
///     config.channels = [None; 16];
/// }
/// ```
///
/// A bounded, contiguous list uses the same checked channel values:
///
/// ```
/// use oer_ieee80211_sta::machine::StaConfig;
/// use oer_ieee80211_port::SupportedChannel;
/// fn one<K, C: oer_ieee80211_sta::machine::StaCallbacks>(config: &mut StaConfig<K, C>, channel: SupportedChannel<K>) {
///     config.channels = heapless::Vec::from_slice(&[channel]).unwrap();
/// }
/// ```
pub struct StaConfig<K, C: StaCallbacks> {
    /// The interface's own address.
    pub address: OwnAddress,
    /// The network's SSID.
    pub ssid: heapless::Vec<u8, MAX_SSID_LEN>,
    /// The channels the scan visits, in order.
    pub channels: heapless::Vec<SupportedChannel<K>, CHANNEL_CAPACITY>,
    /// How long the scan listens on each channel.
    pub dwell: Duration,
    /// The consecutive cancelled leases after which a scan ends with a
    /// partial result.
    pub skipped_absences: u8,
    /// The margin, in dB, within which a candidate on the channel of the
    /// access point beside the station wins over the strongest candidate.
    /// The composition chooses this policy for its deployment.
    pub access_point_margin_db: u8,
    /// The rates the Probe Request offers.
    pub supported_rates: [u8; 8],
    /// The capabilities the Association Request offers.
    pub association: &'static AssociationCapabilities,
    /// The nonzero number of beacon intervals advertised in Association.
    /// The composition also uses this value for its power-save policy.
    pub listen_interval: StationListenInterval,
    /// The PHY the station associates with.
    pub phy: PhyMode,
    /// Entropy and HE padding policy supplied by this composition.
    pub callbacks: C,
}

/// The receive rules a station uses, which its port must offer: checked
/// when the station's type is built, so a port without them builds no
/// station.
struct Rules<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Rules<K> {
    /// The network's frames alone.
    const MEMBER: SupportedFilter<K, StationRole> = {
        match SupportedFilter::new(ReceiveFilter::BSS_MEMBER) {
            Some(filter) => filter,
            None => panic!("a station's port receives its network's frames"),
        }
    };
    /// The network's frames and the other networks' management frames:
    /// the network's own channel in a background scan.
    const WIDENED: SupportedFilter<K, StationRole> = {
        match SupportedFilter::new(
            ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
        ) {
            Some(filter) => filter,
            None => panic!("a station's port hears the other networks beside its own"),
        }
    };
}

/// The station: its state and its tokens.
pub struct Station<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> {
    config: StaConfig<K, C>,
    security: StaSecurity,
    state: State<T, K>,
    keys: Keys<T>,
    configuration: Option<ConfigurePermission<'id, StationRole, T::ConfigurationPath>>,
    tickets: Pool<T::Ticket>,
    buffers: Pool<T::SmallBuffer>,
    /// The station's place for a lease, while it holds none.
    place: Option<LeasePlace<'id, T::GatePath>>,
    /// The lease the station holds, or gave back and awaits the end of.
    lease: Held<'id, K, T::GatePath>,
    /// The station's place for an absence, while it takes none.
    absence_place: Option<AbsencePlace<'id, T::GatePath>>,
    /// The absence the station holds.
    absence: Option<Lease<'id, K, T::GatePath>>,
    /// Each admitted application request reserves its own terminal cell.
    commands: Commands<'id, T::ConfigurationPath>,
    sequence: StaSequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
    pmksa: StaPmksaCache,
    /// The connection attempt that owns new TX operations.
    epoch: Option<LinkEpoch<'id, T::EpochPath>>,
    epochs: LinkEpochSource<'id, T::EpochPath>,
}

/// Resource bank of a terminal station. It cannot accept commands, receive
/// frames or issue protocol effects. Every remaining owner stays in this bank
/// until its caller retires it with the installation.
pub struct RetiredStation<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> {
    /// Authenticated successors returned by disable without protocol effects.
    deferred_eapol: DeferredEapol,
    config: StaConfig<K, C>,
    security: StaSecurity,
    epochs: LinkEpochSource<'id, T::EpochPath>,
    keys: Keys<T>,
    configuration: Option<ConfigurePermission<'id, StationRole, T::ConfigurationPath>>,
    tickets: Pool<T::Ticket>,
    buffers: Pool<T::SmallBuffer>,
    commands: Commands<'id, T::ConfigurationPath>,
    sequence: StaSequenceCounter,
    pmksa: StaPmksaCache,
    notice: Option<StaNotice>,
}

/// Terminal-only output; finalization never requests a port operation.
pub enum StaFinalOutput<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    CommandCompleted(StaCommandTerminal<'id, Path>),
    Notify(StaNotice),
}

/// A resumed machine and the terminal output which belonged to its prior run.
/// Its first actions reclaim retained keys and receive configuration; it cannot
/// admit a new Connect before those real port terminals return.
pub struct StationRestart<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> {
    station: Station<'id, T, K, C>,
    deferred_eapol: DeferredEapol,
    notice: Option<StaNotice>,
}

impl<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> StationRestart<'id, T, K, C> {
    /// Consume the restart into its machine and the previous run's outputs.
    pub fn into_parts(self) -> (Station<'id, T, K, C>, DeferredEapol, Option<StaNotice>) {
        (self.station, self.deferred_eapol, self.notice)
    }
}

impl<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> RetiredStation<'id, T, K, C> {
    /// Reuse this installation's owners, sequence and connection-epoch source.
    /// The stopped arbiter supplies both admission places by value. A partial
    /// return collection cannot enter this infallible transition.
    pub fn restart(self, places: StationPlaces<'id, T::GatePath>) -> StationRestart<'id, T, K, C> {
        let (place, absence_place) = places.into_parts();
        let Self {
            config,
            security,
            epochs,
            keys,
            configuration,
            tickets,
            buffers,
            commands,
            sequence,
            pmksa,
            notice,
            deferred_eapol,
        } = self;
        StationRestart {
            station: Station {
                config,
                security,
                epochs,
                keys,
                configuration,
                tickets,
                buffers,
                place: Some(place),
                lease: Held::None,
                absence_place: Some(absence_place),
                absence: None,
                commands,
                sequence,
                pmksa,
                scan: ScanTable::new(),
                epoch: None,
                state: State::Reclaiming(Reclaim::Keys),
            },
            notice,
            deferred_eapol,
        }
    }

    /// Return deferred frames and the remaining notice together with the
    /// resource bank. Consuming the bank prevents a second extraction of the
    /// same authenticated frame owner.
    pub fn into_outputs(self) -> (Self, DeferredEapol, Option<StaNotice>) {
        let Self {
            deferred_eapol,
            notice,
            ..
        } = self;
        (
            Self {
                deferred_eapol: DeferredEapol::Empty,
                notice: None,
                ..self
            },
            deferred_eapol,
            notice,
        )
    }

    /// Drain original accepted requests before the one lifecycle notice.
    pub fn poll_output(&mut self) -> Option<StaFinalOutput<'id, T::ConfigurationPath>> {
        self.commands
            .poll()
            .map(StaFinalOutput::CommandCompleted)
            .or_else(|| self.notice.take().map(StaFinalOutput::Notify))
    }

    /// Free frame credits retained before the installation is torn down.
    #[cfg(any(test, feature = "test-support"))]
    pub fn frame_credits(&self) -> (usize, usize) {
        (self.tickets.len(), self.buffers.len())
    }

    /// Pairwise owners, group owners (vacant or installed), and the group's
    /// mutation permission retained before installation teardown. Poisoned
    /// work may still own missing resources in the backend's retirement bank.
    #[cfg(any(test, feature = "test-support"))]
    pub fn key_owners(&self) -> (usize, usize, bool) {
        let keys = &self.keys;
        (
            usize::from(matches!(
                keys.pairwise,
                KeyCell::Free(_) | KeyCell::Installed(_)
            )),
            keys.group_slots
                .free
                .iter()
                .map(|_| ())
                .chain(keys.groups.owners().map(|_| ()))
                .count(),
            keys.group_permission.is_some(),
        )
    }

    /// Unexpected extra tokens observed before retirement.
    #[cfg(any(test, feature = "test-support"))]
    pub fn lost_tokens(&self) -> u32 {
        self.tickets.lost().combined(self.buffers.lost()).get()
    }
}

/// The station's lease.
enum Held<'id, K, Path: PoolPath> {
    /// None: the station holds its place.
    None,
    /// Held, from the arbiter's answer to its end.
    Lease(Lease<'id, K, Path>),
    /// Given back; its end's notice names it.
    Released(LeaseId<'id, Path>),
}

impl<'id, K, Path: PoolPath> Held<'id, K, Path> {
    fn id(&self) -> Option<LeaseId<'id, Path>> {
        match self {
            Self::None => None,
            Self::Lease(lease) => Some(lease.id()),
            Self::Released(id) => Some(*id),
        }
    }
}

/// A pairwise cell awaiting the response to its one installation.
#[derive(Debug)]
#[must_use = "return this token with the pairwise installation result"]
pub struct PendingPairwiseKey<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    cell: PairwiseCell,
    epoch: LinkEpoch<'id, Path>,
}

#[derive(Debug)]
struct PairwiseCell;

/// A group-key installation awaiting its response. Only Station creates this value;
/// moving it into the response proves which transition owns the installed key.
///
/// ```compile_fail,E0063
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>(key: T::GroupKey) -> StaInput<'id, T, K> {
///     StaInput::GroupKeyInstalled { key }
/// }
/// ```
///
/// It cannot be copied to manufacture a second response:
/// ```compile_fail,E0599
/// use oer_ieee80211_sta::machine::PendingGroupKey;
/// fn duplicate<'id>(pending: PendingGroupKey<'id>) -> (PendingGroupKey<'id>, PendingGroupKey<'id>) {
///     (pending.clone(), pending)
/// }
/// ```
///
/// With the original action's token, returning the key is well-typed:
/// ```
/// use oer_ieee80211_sta::machine::{PendingGroupKey, StaInput, StaTokens};
/// fn installed<'id, T: StaTokens, K>(pending: PendingGroupKey<'id, T::EpochPath>, key: T::GroupKey, permission: T::GroupPermission)
///     -> StaInput<'id, T, K> {
///     StaInput::GroupKeyInstalled { pending, key, permission }
/// }
/// ```
#[derive(Debug)]
#[must_use = "return this token with the group installation result"]
pub struct PendingGroupKey<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    epoch: LinkEpoch<'id, Path>,
    key_id: GroupKeyId,
    material: Ccmp128Key,
    place: GroupPlace,
    replay: GroupReplay,
}

/// The reserved successor of a GTK. It stays private until the port's
/// replacement terminal; the active replay epoch remains usable meanwhile.
///
/// The pending owner cannot complete two replacements:
/// ```compile_fail,E0382
/// use oer_ieee80211_sta::machine::PendingGroupReplacement;
/// fn duplicate<'id>(pending: PendingGroupReplacement<'id>) {
///     let first = pending;
///     let second = pending;
/// }
/// ```
/// The replacement input must return that owner:
/// ```compile_fail,E0063
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>(key: T::GroupKey) -> StaInput<'id, T, K> {
///     StaInput::GroupKeyReplaced { key }
/// }
/// ```
/// Its valid terminal moves both values once:
/// ```
/// use oer_ieee80211_sta::machine::{PendingGroupReplacement, StaInput, StaTokens};
/// fn returned<'id, T: StaTokens, K>(pending: PendingGroupReplacement<'id, T::EpochPath>, key: T::GroupKey, permission: T::GroupPermission)
///     -> StaInput<'id, T, K> {
///     StaInput::GroupKeyReplaced { pending, key, permission }
/// }
/// ```
#[must_use = "return this token with the matching group replacement terminal"]
pub struct PendingGroupReplacement<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    install: PendingGroupKey<'id, Path>,
    previous_id: GroupKeyId,
}

/// Replay and material custody of the existing installation being selected.
#[must_use = "return with the matching selection terminal"]
pub struct PendingGroupSelection<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    install: PendingGroupKey<'id, Path>,
}

/// A record cannot be reused until the physical removal terminal returns.
/// Its invariant installation and pool path belong to the station that issued
/// the action, independently of whether that connection has since left.
#[must_use = "return with the matching removal terminal"]
pub struct PendingGroupRemoval<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    key_id: GroupKeyId,
    place: GroupPlace,
    _material: Ccmp128Key,
    replay: GroupReplay,
    installation: core::marker::PhantomData<fn(&'id Path) -> &'id Path>,
}

/// Opaque connection context carried inside the installation's sole configure
/// permission. Its terminal returns the context with that same permission;
/// an old epoch releases admission without completing a transition of the
/// current connection. No service-side request table correlates this response.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_sta::machine::{StaInput, StaTokens};
/// fn forged<'id, T: StaTokens, K>() -> StaInput<'id, T, K> {
///     StaInput::Configured
/// }
/// ```
#[derive(Debug)]
pub struct StaConfigurationContext<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    epoch: Option<LinkEpoch<'id, Path>>,
}

impl<'p, Path: PoolPath + 'p> oer_radio_port::LoanedContext<'p, Path::Identity>
    for StaConfigurationContext<'p, Path>
where
    Path::Identity: 'p,
{
    type Owned = oer_radio_port::ContextChoice<(), crate::epoch::EpochContext<Path, ()>>;

    fn enclose(self) -> oer_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        match self.epoch {
            Some(epoch) => epoch.context(()).second(),
            None => oer_radio_port::PortContext::new(()).first(),
        }
    }

    fn restore(context: oer_radio_port::PortContext<'p, Path::Identity, Self::Owned>) -> Self {
        let epoch = match context.into_choice() {
            oer_radio_port::ContextChoice::First(_) => None,
            oer_radio_port::ContextChoice::Second(context) => Some(context.into_parts().0),
        };
        Self { epoch }
    }
}

type BankParts<P, G> = oer_ieee80211_port::association::KeyBankParts<P, G, KEY_BANK_CAPACITY>;

/// A refused bank retains all owners, including the element which did not fit.
enum KeyBankReunionError<P, G> {
    Incomplete {
        _parts: BankParts<P, G>,
    },
    Capacity {
        _parts: BankParts<P, G>,
        _pairwise: [Option<P>; KEY_BANK_CAPACITY],
        _group: [[Option<G>; KEY_BANK_CAPACITY]; 2],
    },
}

fn assemble_key_bank<P, G>(
    slot: P,
    mut pairwise: [Option<P>; KEY_BANK_CAPACITY],
    mut group: [[Option<G>; KEY_BANK_CAPACITY]; 2],
) -> Result<BankParts<P, G>, KeyBankReunionError<P, G>> {
    let mut parts = BankParts {
        pairwise: heapless::Vec::from_array([slot]),
        group: heapless::Vec::new(),
    };
    for source in &mut pairwise {
        if let Some(item) = source.take()
            && let Err(item) = parts.pairwise.push(item)
        {
            *source = Some(item);
            return Err(KeyBankReunionError::Capacity {
                _parts: parts,
                _pairwise: pairwise,
                _group: group,
            });
        }
    }
    for source in group.iter_mut().flatten() {
        if let Some(item) = source.take()
            && let Err(item) = parts.group.push(item)
        {
            *source = Some(item);
            return Err(KeyBankReunionError::Capacity {
                _parts: parts,
                _pairwise: pairwise,
                _group: group,
            });
        }
    }
    Ok(parts)
}

/// The state of a key cell, independent of the protocol's current state.
enum KeyCell<T: StaTokens> {
    Free(VacantPairwise<T>),
    Installing,
    Installed(T::PairwiseKey),
    Removing,
    /// Backend contract violation: every returned owner remains in custody.
    UnreturnedBank {
        _association: T::Association,
        _error: KeyBankReunionError<T::PairwiseSlot, T::GroupSlot>,
    },
}

/// Free slots are fungible credits from the installation's declared pool.
/// An install takes one; only its eventual removal returns one. Thus a full
/// pool proves that no group installation or removal remains outstanding.
struct GroupSlots<T: StaTokens> {
    free: Pool<T::GroupSlot>,
}

impl<T: StaTokens> GroupSlots<T> {
    fn is_full(&self) -> bool {
        self.free.len() == T::GROUP_SLOTS
    }
}

struct VacantPairwise<T: StaTokens> {
    slot: T::PairwiseSlot,
    association: T::AssociationStart,
}

struct Keys<T: StaTokens> {
    pairwise: KeyCell<T>,
    spare_pairwise: heapless::Vec<T::PairwiseSlot, KEY_BANK_CAPACITY>,
    spare_group: Pool<T::GroupSlot>,
    group_slots: GroupSlots<T>,
    group_permission: Option<T::GroupPermission>,
    groups: GroupKeys<T::GroupKey>,
}

type PairwiseAdmission<'id, T> = (
    <T as StaTokens>::Association,
    <T as StaTokens>::PairwiseSlot,
    PendingPairwiseKey<'id, <T as StaTokens>::EpochPath>,
);

impl<T: StaTokens> Keys<T> {
    fn new<const N: usize>(vacant: T::VacantAssociation, permission: T::GroupPermission) -> Self {
        const {
            if N != T::GROUP_SLOTS
                || N > T::TOTAL_GROUP_SLOTS
                || T::TOTAL_GROUP_SLOTS > KEY_BANK_CAPACITY
                || T::PAIRWISE_SLOTS == 0
                || T::PAIRWISE_SLOTS > KEY_BANK_CAPACITY
            {
                panic!("station collections must retain the complete key bank");
            }
        }
        let (association, slot, parts) = T::split_vacant(vacant);
        let (free, spare_group) = Pool::partition(parts.group, N);
        Self {
            pairwise: KeyCell::Free(VacantPairwise { slot, association }),
            spare_pairwise: parts.pairwise,
            spare_group,
            group_slots: GroupSlots { free },
            group_permission: Some(permission),
            groups: GroupKeys::new(),
        }
    }

    fn take_pairwise_slot<'id>(
        &mut self,
        epoch: LinkEpoch<'id, T::EpochPath>,
        spp: oer_ieee80211_wire::security::Spp,
    ) -> Option<PairwiseAdmission<'id, T>> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Installing) {
            KeyCell::Free(VacantPairwise { slot, association }) => Some((
                T::begin_association(association, spp),
                slot,
                PendingPairwiseKey {
                    cell: PairwiseCell,
                    epoch,
                },
            )),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    fn pairwise_installed(
        &mut self,
        pending: PendingPairwiseKey<'_, T::EpochPath>,
        key: T::PairwiseKey,
    ) {
        let PairwiseCell = pending.cell;
        self.pairwise = KeyCell::Installed(key);
    }

    fn take_pairwise_key(&mut self) -> Option<T::PairwiseKey> {
        match core::mem::replace(&mut self.pairwise, KeyCell::Removing) {
            KeyCell::Installed(key) => Some(key),
            cell => {
                self.pairwise = cell;
                None
            }
        }
    }

    fn group_installed(
        &mut self,
        pending: PendingGroupKey<'_, T::EpochPath>,
        key: T::GroupKey,
        permission: T::GroupPermission,
        current: bool,
        replay: Option<&mut CcmpRxReplayEpoch>,
        selected: bool,
    ) {
        let mut entry = InstalledGroup {
            place: pending.place,
            key_id: pending.key_id,
            material: pending.material,
            key,
            replay: pending.replay,
        };
        if current && let Some(replay) = replay {
            self.groups.activate(&mut entry, replay);
        }
        if selected {
            self.groups.restore(entry, current);
        } else {
            self.groups.install(entry, current);
        }
        self.group_permission = Some(permission);
    }

    fn take_group_key(
        &mut self,
        retiring: bool,
    ) -> Option<(T::GroupPermission, InstalledGroup<T::GroupKey>)> {
        let permission = self.group_permission.take()?;
        let entry = if retiring {
            self.groups.take_retiring()
        } else {
            self.groups.take_any()
        };
        match entry {
            Some(entry) => Some((permission, entry)),
            None => {
                self.group_permission = Some(permission);
                None
            }
        }
    }

    fn group_removed(
        &mut self,
        slot: T::GroupSlot,
        permission: T::GroupPermission,
        place: GroupPlace,
    ) {
        self.groups.release(place);
        self.group_slots.free.put(slot);
        self.group_permission = Some(permission);
    }

    fn current_group(&self) -> Option<GroupKeyId> {
        self.groups.current_id()
    }

    fn installed(&self) -> bool {
        matches!(self.pairwise, KeyCell::Installed(_)) && self.current_group().is_some()
    }

    fn clear(&self) -> bool {
        matches!(self.pairwise, KeyCell::Free(_))
            && self.group_slots.is_full()
            && self.group_permission.is_some()
    }
}

/// A new variant description contains no State, Connected or ConnectedRsn owner.
#[expect(
    clippy::large_enum_variant,
    reason = "sub-limit variant initializers stay affine without an allocator"
)]
enum Change<T: StaTokens, K> {
    Keep,
    Idle,
    Reclaiming(Reclaim),
    Scan(Scan<K>),
    Join(Join<T, K>),
    Handshake(NewHandshake<T, K>),
    Leave {
        reason: StaDisconnectReason,
        step: LeaveStep,
    },
    Report(StaDisconnectReason),
    Establish(NewConnection<T, K>),
    Confirm {
        replay: CcmpRxReplayEpoch,
        response: EapolResponse,
    },
}

struct NewConnection<T: StaTokens, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    qos_sequences: [StaSequenceCounter; 8],
    confirmation: Option<RsnStaResponseDeadline>,
}
impl<T: StaTokens, K> State<T, K> {
    fn take_deferred(&mut self) -> DeferredEapol {
        let deferred = match self {
            Self::Leave(leave) => &mut leave.deferred,
            Self::Connected(connected) => match connected.rsn.as_mut() {
                Some(rsn) => &mut rsn.deferred,
                None => return DeferredEapol::Empty,
            },
            _ => return DeferredEapol::Empty,
        };
        core::mem::replace(deferred, DeferredEapol::Empty)
    }

    #[inline(never)]
    fn confirm(&mut self, replay: CcmpRxReplayEpoch, response: EapolResponse) {
        let Self::Handshake(handshake) = self else {
            return;
        };
        let supplicant = handshake
            .supplicant
            .take()
            .ok_or(RsnStaSupplicantError::UnexpectedAction)
            .and_then(RsnStaSupplicant::into_connected);
        let supplicant = match supplicant {
            Ok(supplicant) => supplicant,
            Err(_) => {
                let deauthentication = Deauthentication {
                    bssid: handshake.bssid,
                    reason_code: REASON_UNSPECIFIED,
                    protected: handshake.management.is_some(),
                };
                self.leave(
                    StaDisconnectReason::HandshakeFailed,
                    leave_key_step(Some(deauthentication)),
                );
                return;
            }
        };
        let management = handshake.management.take();
        let new = NewConnection {
            bssid: handshake.bssid,
            channel: handshake.channel,
            association_id: handshake.association_id,
            qos: handshake.qos,
            peer: handshake.peer,
            qos_sequences: handshake.qos_sequences,
            confirmation: Some(handshake.deadline),
        };
        self.establish(new);
        if let Self::Connected(connected) = self {
            connected.rsn.initialize(|place| {
                oer_memory::init_in_place!(
                    place,
                    ConnectedRsn {
                        management: management,
                        supplicant: supplicant,
                        replay: replay,
                        message4: DuplicateMessage4::Send(response),
                        rekey: Rekey::None,
                        deferred: DeferredEapol::Empty,
                        dropped: crate::Counter::ZERO,
                        fault: None,
                    }
                )
            });
        }
    }

    #[inline(never)]
    fn apply(&mut self, change: Change<T, K>) {
        match change {
            Change::Keep => {}
            Change::Idle => *self = Self::Idle,
            Change::Reclaiming(reclaim) => *self = Self::Reclaiming(reclaim),
            Change::Scan(scan) => *self = Self::Scan(scan),
            Change::Join(join) => *self = Self::Join(join),
            Change::Handshake(new) => {
                *self = Self::Handshake(Handshake {
                    bssid: new.bssid,
                    channel: new.channel,
                    association_id: new.association_id,
                    qos: new.qos,
                    peer: new.peer,
                    supplicant: new.supplicant,
                    spp: new.spp,
                    pmk: new.pmk,
                    qos_sequences: new.qos_sequences,
                    deadline: new.deadline,
                    step: HandshakeStep::Await,
                    management: new.management,
                })
            }
            Change::Report(reason) => *self = Self::Report(reason),
            Change::Leave { reason, step } => self.leave(reason, step),
            Change::Confirm { replay, response } => self.confirm(replay, response),
            Change::Establish(new) => self.establish(new),
        }
    }
    #[inline(never)]
    fn leave(&mut self, reason: StaDisconnectReason, step: LeaveStep) {
        let deferred = self.take_deferred();
        *self = Self::Leave(oer_memory::InlinePlace::empty());
        if let Self::Leave(place) = self {
            place.initialize(|place| {
                oer_memory::init_in_place!(
                    place,
                    Leave {
                        reason: reason,
                        step: step,
                        deferred: deferred,
                    }
                )
            });
        }
    }

    #[inline(never)]
    fn establish(&mut self, new: NewConnection<T, K>) {
        *self = Self::Connected(oer_memory::InlinePlace::empty());
        if let Self::Connected(place) = self {
            place.initialize(|place| {
                oer_memory::init_in_place!(
                    place,
                    Connected {
                        bssid: new.bssid,
                        channel: new.channel,
                        association_id: new.association_id,
                        qos: new.qos,
                        peer: new.peer,
                        qos_sequences: new.qos_sequences,
                        confirmation: new.confirmation,
                        report: new
                            .confirmation
                            .is_none()
                            .then_some(UpstreamFact::Connected(new.channel)),
                        edca: false,
                        exchange: false,
                        power: PowerSaveView::Awake,
                        wake: false,
                        sending: false,
                        waking: false,
                        background: None,
                        connected_notice: new.confirmation.is_none(),
                        block_ack: None,
                        rsn: oer_memory::InlinePlace::empty(),
                    }
                )
            });
        }
    }
}

fn current_epoch<'id, T: StaTokens, K>(
    state: &State<T, K>,
    epoch: Option<LinkEpoch<'id, T::EpochPath>>,
) -> Option<LinkEpoch<'id, T::EpochPath>> {
    match state {
        State::Idle | State::Reclaiming(_) | State::Leave(_) | State::Report(_) => None,
        _ => epoch,
    }
}

/// The protocol state. Key-cell ownership remains in Station while these
/// states change, including while an installation response is outstanding.
#[expect(
    clippy::large_enum_variant,
    reason = "phase payloads stay resident and are never passed through transitions by value"
)]
enum State<T: StaTokens, K> {
    /// Not connected and not trying.
    Idle,
    /// A restarted service returns old key/configuration owners before Connect.
    Reclaiming(Reclaim),
    /// Visiting the scan list.
    Scan(Scan<K>),
    /// Joining the chosen access point.
    Join(Join<T, K>),
    /// Running the WPA2 four-way handshake.
    Handshake(Handshake<T, K>),
    /// Connected.
    Connected(oer_memory::InlinePlace<Connected<T, K>>),
    /// Leaving the network.
    Leave(oer_memory::InlinePlace<Leave>),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason),
}

#[derive(Clone, Copy)]
enum Reclaim {
    Keys,
    Configure,
    AwaitConfigured,
}

/// A visit of one scan channel.
struct Scan<K> {
    visit: usize,
    /// Consecutive cancelled leases.
    skipped: u8,
    step: ScanStep,
    /// The channel of the access point beside the station, as the last
    /// lease named it.
    access_point: Option<SupportedChannel<K>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanStep {
    /// Ask the arbiter for the visit's channel.
    NeedLease,
    /// The arbiter answers in the same step.
    AwaitAnswer,
    /// The lease is granted; the port moves at its start.
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    /// Listening until the lease's end.
    Listen,
}

/// The join of one access point.
struct Join<T: StaTokens, K> {
    access_point: ScanRecord,
    rsn: SelectedRsn,
    channel: SupportedChannel<K>,
    step: JoinStep<T::RxMeta>,
}

enum JoinStep<M> {
    NeedLease,
    AwaitAnswer,
    /// The access point beside the station followed a join too recently:
    /// the station asks again at this instant.
    RetryAt(Instant),
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    Authenticate(Authenticate),
    SaeSend {
        authentication: StaSaeAuthentication,
        transmission: StaSaeTransmission,
    },
    SaeAwait(StaSaeAuthentication),
    Associate(StaAssociationRuntime, AssociationKey),
    /// The association's EDCA parameters to set, after its notice.
    SetEdca {
        association_id: u16,
        peer: Peer<M>,
        key: AssociationKey,
    },
    NotifyAssociated {
        association_id: u16,
        peer: Peer<M>,
        key: AssociationKey,
    },
    StartLink {
        association_id: u16,
        peer: Peer<M>,
        key: AssociationKey,
    },
}

/// The access point as the association made it: what it negotiated, and
/// the metadata its Association Response arrived with, whose type is the
/// port's: rate selection starts from the Association Response, and one that needs a
/// signal measurement requires a port whose metadata carries one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Peer<M> {
    /// What the association negotiated.
    pub associated: StaAssociatedPeer,
    /// The Association Response's metadata.
    pub response: M,
}

enum AssociationKey {
    Open,
    Protected(Pmk),
}

enum Authenticate {
    NeedCredit(StaAuthenticationRuntime, AssociationKey),
    Exchange {
        runtime: StaAuthenticationExchange,
        stage: AuthStage,
        key: AssociationKey,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthStage {
    AwaitSent {
        attempt: u16,
        response_timeout: Duration,
    },
    AwaitResponse {
        until: Instant,
    },
}

/// The WPA2 four-way handshake with the joined access point.
struct NewHandshake<T: StaTokens, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    supplicant: Option<RsnStaSupplicant>,
    spp: oer_ieee80211_wire::security::Spp,
    pmk: Pmk,
    qos_sequences: [StaSequenceCounter; 8],
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    management: Option<ManagementProtection>,
}

struct Handshake<T: StaTokens, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    supplicant: Option<RsnStaSupplicant>,
    spp: oer_ieee80211_wire::security::Spp,
    pmk: Pmk,
    qos_sequences: [StaSequenceCounter; 8],
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    step: HandshakeStep,
    management: Option<ManagementProtection>,
}

impl<T: StaTokens, K> Handshake<T, K> {
    fn supplicant(&mut self) -> Result<&mut RsnStaSupplicant, RsnStaSupplicantError> {
        self.supplicant
            .as_mut()
            .ok_or(RsnStaSupplicantError::UnexpectedAction)
    }
}

/// The handshake's step; its values move from one step to the next, so the
/// handshake holds the largest of them in place.
enum HandshakeStep {
    /// Waiting for the access point's next message.
    Await,
    /// A message to send.
    Send(RsnTxFrame),
    /// Message 2's terminal.
    AwaitSent,
    /// The keys of Message 3 to install: the pairwise key, then the group
    /// key.
    Install(RsnStaKeyInstallRequest, EapolProtection),
    InstallGroup(RsnStaRetryInstall, CcmpRxReplayEpoch, EapolProtection),
    AwaitInstalled(RsnStaRetryInstall, CcmpRxReplayEpoch, EapolProtection),
}

/// The connected station.
struct Connected<T: StaTokens, K> {
    confirmation: Option<RsnStaResponseDeadline>,
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    /// The access point's BSS is a QoS BSS (it advertised WMM).
    qos: bool,
    /// The access point as the association made it.
    peer: Peer<T::RxMeta>,
    /// The association's QoS sequence spaces, one per TID: its EAPOL's and,
    /// lent through the link, its data's.
    qos_sequences: [StaSequenceCounter; 8],
    /// The upstream fact the arbiter has yet to hear.
    report: Option<UpstreamFact<K>>,
    /// The access point's beacons announced another EDCA set, already in
    /// `peer`, which the interface has yet to take.
    edca: bool,
    /// The primary lease to exchange after the port moved.
    exchange: bool,
    /// What the access point knows of the station's power state.
    power: PowerSaveView,
    /// A Null with PM=0 the station owes its access point.
    wake: bool,
    /// Admission: at most one connected control transmission at a time.
    sending: bool,
    /// A PM=0 transition is outstanding; beacons need not request another.
    waking: bool,
    /// The background scan, if one runs.
    background: Option<Background>,
    /// The application has yet to hear the station connected.
    connected_notice: bool,
    /// Parsed management to hand to the service before the next RX input.
    block_ack: Option<oer_ieee80211_wire::block_ack::frame::BlockAckAction>,
    /// The supplicant of a protected link and its group key's handshake.
    rsn: oer_memory::InlinePlace<ConnectedRsn>,
}

/// What the connected station's transmission in flight is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sending {
    /// A Null with PM=1 before an absence.
    Sleep,
    /// A Null with PM=0.
    Wake,
    /// A Probe Request of the background scan.
    Probe,
    /// Group Message 2.
    GroupResponse,
    /// Message 4 answering the exact completed Message 3.
    DuplicateMessage4,
    /// A protected SA Query Request or Response.
    SaQuery,
    /// A response to the AP's receive Block Ack negotiation.
    BlockAck,
}

/// A protected link's supplicant and its group key's handshake.
struct ConnectedRsn {
    /// First local failure; no further link work is admitted before leaving.
    fault: Option<StaDisconnectReason>,
    management: Option<ManagementProtection>,
    supplicant: RsnConnectedSupplicant,
    rekey: Rekey,
    message4: DuplicateMessage4,
    deferred: DeferredEapol,
    /// Ordinary peer-input drops, including invalid MICs and stale counters.
    dropped: crate::Counter,
    /// The replay check of the association's keys, which the data machine
    /// borrows through the link.
    replay: CcmpRxReplayEpoch,
}

impl ConnectedRsn {
    #[inline(never)]
    fn complete_install(
        &mut self,
        request: RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
    ) -> Result<(), StaDisconnectReason> {
        if let (Some(management), Some(igtk)) = (&mut self.management, request.igtk()) {
            management.install_igtk(igtk);
        }
        let frame = self
            .supplicant
            .complete_group_key_install(request, true)
            .map_err(|_| StaDisconnectReason::HandshakeFailed)?;
        self.rekey = Rekey::Send(EapolResponse { frame, protection });
        Ok(())
    }

    #[inline(never)]
    fn receive(
        &mut self,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
    ) -> Result<(), StaRxError> {
        if self.fault.is_some() {
            return Ok(());
        }
        match self.receive_eapol(eapol, protection) {
            Err(error) => match connected_rsn_fault(error) {
                Some(reason) => {
                    self.fault = Some(reason);
                    Ok(())
                }
                None => Err(error.into()),
            },
            Ok(()) => Ok(()),
        }
    }

    fn receive_eapol(
        &mut self,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
    ) -> Result<(), RsnConnectedSupplicantError> {
        use oer_ieee80211_rsn::EapolKeyMessage;
        let message = eapol
            .key_frame()
            .map_err(RsnConnectedSupplicantError::from)?
            .message();
        let action = match (message, protection) {
            (EapolKeyMessage::PairwiseMessage3, _) => self.supplicant.on_message3(eapol),
            (EapolKeyMessage::GroupMessage1, EapolProtection::Pairwise) => {
                self.supplicant.on_group_message1(eapol)
            }
            _ => Err(RsnConnectedSupplicantError::UnsupportedMessage),
        };
        let result = match action {
            Err(error) => Err(error),
            Ok(RsnConnectedAction::Retransmit(frame))
                if message == EapolKeyMessage::PairwiseMessage3 =>
            {
                if self.deferred.has_newer_message3(
                    frame
                        .key_frame()
                        .map_err(RsnConnectedSupplicantError::from)?
                        .replay_counter(),
                ) {
                    Err(RsnConnectedSupplicantError::StaleReplayCounter)
                } else {
                    if matches!(self.message4, DuplicateMessage4::None) {
                        self.message4 =
                            DuplicateMessage4::Send(EapolResponse { frame, protection });
                    }
                    Ok(())
                }
            }
            Ok(RsnConnectedAction::Retransmit(frame)) => {
                if self.deferred.has_newer_group(
                    frame
                        .key_frame()
                        .map_err(RsnConnectedSupplicantError::from)?
                        .replay_counter(),
                ) {
                    Err(RsnConnectedSupplicantError::StaleReplayCounter)
                } else {
                    // An exact duplicate coalesces with its current GM2.
                    if matches!(self.rekey, Rekey::None) {
                        self.rekey = Rekey::Send(EapolResponse { frame, protection });
                    }
                    Ok(())
                }
            }
            Ok(RsnConnectedAction::UnwrapGroupKeyData(request)) => {
                let current = match &self.rekey {
                    Rekey::Install(current, _) | Rekey::AwaitInstalled(current, _) => {
                        current.replay_counter()
                    }
                    _ => self.supplicant.replay_counter(),
                };
                if request.replay_counter() <= current {
                    Err(RsnConnectedSupplicantError::StaleReplayCounter)
                } else {
                    let (deferred, result) = if message == EapolKeyMessage::PairwiseMessage3 {
                        core::mem::replace(&mut self.deferred, DeferredEapol::Empty).message3(
                            PendingM3 {
                                request,
                                protection,
                            },
                        )
                    } else {
                        core::mem::replace(&mut self.deferred, DeferredEapol::Empty)
                            .group(PendingGroupM1 { request })
                    };
                    self.deferred = deferred;
                    if result.is_ok()
                        && message == EapolKeyMessage::PairwiseMessage3
                        && matches!(self.message4, DuplicateMessage4::Send(_))
                    {
                        // The AP has advanced; an unsent older M4 is obsolete.
                        self.message4 = DuplicateMessage4::None;
                    }
                    result
                }
            }
        };
        if let Err(error) = result {
            if let Some(reason) = connected_rsn_fault(error) {
                self.fault = Some(reason);
                return Err(error);
            }
            self.dropped.increment();
        }
        if let Err(reason) = self.advance_deferred() {
            self.fault = Some(reason);
        }
        result
    }

    /// Consume the smallest verified replay counter into installation work.
    /// M3 and GM1 advance the same frontier; their kind grants no priority.
    /// Invalid key data remains a counted protocol drop, not a local failure.
    #[inline(never)]
    fn advance_deferred(&mut self) -> Result<(), StaDisconnectReason> {
        if !matches!(self.rekey, Rekey::None)
            || matches!(self.message4, DuplicateMessage4::AwaitSent)
        {
            return Ok(());
        }
        loop {
            let (request, protection, rest) =
                match core::mem::replace(&mut self.deferred, DeferredEapol::Empty) {
                    DeferredEapol::Empty => {
                        self.deferred = DeferredEapol::Empty;
                        return Ok(());
                    }
                    DeferredEapol::Message3(PendingM3 {
                        request,
                        protection,
                    }) => (request, protection, DeferredEapol::Empty),
                    DeferredEapol::Both {
                        message3,
                        group: PendingGroupM1 { request },
                    } if request.replay_counter() < message3.replay_counter() => (
                        request,
                        EapolProtection::Pairwise,
                        DeferredEapol::Message3(message3),
                    ),
                    DeferredEapol::Both {
                        message3:
                            PendingM3 {
                                request,
                                protection,
                            },
                        group,
                    } => (request, protection, DeferredEapol::GroupMessage1(group)),
                    DeferredEapol::GroupMessage1(PendingGroupM1 { request }) => {
                        (request, EapolProtection::Pairwise, DeferredEapol::Empty)
                    }
                };
            self.deferred = rest;
            let wrapped = request
                .wrapped_key_data()
                .map_err(StaDisconnectReason::InvalidEapol)?;
            let unwrapped = software_aes128_key_unwrap(request.kek(), wrapped);
            match self
                .supplicant
                .complete_group_key_data_unwrap(request, unwrapped)
            {
                Ok(install) => {
                    self.rekey = Rekey::Install(install, protection);
                    return Ok(());
                }
                Err(oer_ieee80211_rsn::supplicant::RsnConnectedProcessError::Supplicant(error)) => {
                    if let Some(reason) = connected_rsn_fault(error) {
                        return Err(reason);
                    }
                    self.dropped.increment();
                }
                Err(_) => self.dropped.increment(),
            }
        }
    }
}

fn sae_derivation_failure(
    error: oer_ieee80211_rsn::sae::SaeError,
    peer: [u8; 6],
    pwe: oer_ieee80211_wire::security::SaePwe,
) -> StaDisconnectReason {
    StaDisconnectReason::Sae(match error {
        oer_ieee80211_rsn::sae::SaeError::Crypto => StaSaeFailure::Crypto { peer, pwe },
        error => StaSaeFailure::Protocol(error),
    })
}

fn connected_rsn_fault(error: RsnConnectedSupplicantError) -> Option<StaDisconnectReason> {
    match error {
        RsnConnectedSupplicantError::InvalidFrame(error) => {
            Some(StaDisconnectReason::InvalidEapol(error))
        }
        RsnConnectedSupplicantError::RetainedSecurityIes(error) => {
            Some(StaDisconnectReason::RetainedSecurityIes(error))
        }
        _ => None,
    }
}

/// At most one response to the completed M3 is retained. Further exact
/// duplicates coalesce while it waits or is in flight. A pending response
/// precedes the next group-handshake TX, after any current control TX ends.
#[expect(
    clippy::large_enum_variant,
    reason = "one bounded EAPOL response is owned in place; the station has no allocator"
)]
enum DuplicateMessage4 {
    None,
    Send(EapolResponse),
    AwaitSent,
}

#[derive(Clone, Copy)]
enum EapolProtection {
    Open,
    Pairwise,
}

struct EapolResponse {
    frame: RsnTxFrame,
    protection: EapolProtection,
}

/// A group key's handshake: one algorithm whatever the slots. With a free
/// group slot the new key goes in beside retained installations. A full
/// pool replaces the oldest non-current installation in one operation.
enum Rekey {
    None,
    /// The new key to install in a free slot or replace in an occupied slot.
    Install(
        RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>,
        EapolProtection,
    ),
    AwaitInstalled(
        RsnGroupKeyInstallRequest<DEFAULT_EAPOL_FRAME_CAPACITY>,
        EapolProtection,
    ),
    /// Group Message 2.
    Send(EapolResponse),
    AwaitSent,
}

/// A connected station's background scan: one visit per channel.
struct Background {
    visit: usize,
    skipped: u8,
    step: BackgroundStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackgroundStep {
    NeedAbsence,
    AwaitAnswer,
    /// Tell the access point the station leaves.
    NeedSleep,
    AwaitSleep,
    NeedReady,
    AwaitStart,
    NeedConfigure,
    AwaitConfigured,
    NeedProbe,
    Listen,
    /// Back home: the network's receive rules, then PM=0.
    NeedHome,
    AwaitHome,
    NeedWake,
    AwaitWake,
    /// The network's own channel: the widened receive rules, a probe and
    /// the dwell, then the network's rules again.
    AwaitWidened,
    HomeProbe {
        until: Instant,
    },
    HomeListen {
        until: Instant,
    },
    Narrow(Option<BackgroundScanEnd>),
    AwaitNarrowed(Option<BackgroundScanEnd>),
}

/// The station leaving its network, and why.
struct Leave {
    reason: StaDisconnectReason,
    step: LeaveStep,
    deferred: DeferredEapol,
}

/// A Deauthentication to send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Deauthentication {
    bssid: Bssid,
    reason_code: u16,
    protected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaveStep {
    /// The disabled port cannot execute cleanup. Its owner and every retained
    /// frame return through the terminal resource bank.
    Disabled,
    /// The absence's terminal proves the port returned to the home channel.
    AwaitAbsenceEnd(Option<Deauthentication>),
    /// Remove the installed keys; their slots come back with their
    /// removals. Then the Deauthentication, if any.
    RemoveKeys(Option<Deauthentication>),
    NeedDeauthentication(Deauthentication),
    AwaitDeauthentication,
    /// The pairwise key remains installed through the protected terminal.
    AwaitProtectedDeauthentication,
    NeedConfigure,
    AwaitConfigured,
    NeedRelease,
    /// The lease ends with its notice, which gives the place back.
    AwaitEnd,
}

/// The EAPOL ether type.
const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// IEEE 802.1X Port Access Entity group address.
const PAE_GROUP_ADDRESS: [u8; 6] = [0x01, 0x80, 0xc2, 0x00, 0x00, 0x03];

/// A checked management disconnect from the selected BSS.
fn peer_disconnect(frame: &[u8], local: OwnAddress, bssid: Bssid) -> Option<u16> {
    oer_ieee80211_wire::station::parse_sta_disconnect(frame, local, bssid)
        .map(|disconnect| disconnect.reason_code)
}

/// A group key's checked wire ID and temporal key.
fn group_key(install: &RsnKeyInstall) -> Option<(GroupKeyId, Ccmp128Key)> {
    let RsnKeyKind::Group { key_id, .. } = install.kind() else {
        return None;
    };
    Some((key_id, Ccmp128Key::new(install.key().as_bytes())))
}

/// The active replay frontiers stay available while a GTK command holds its
/// permission. Only its terminal exchanges the selected per-ID frontier.
fn active_replay<T: StaTokens, K>(state: &mut State<T, K>) -> Option<&mut CcmpRxReplayEpoch> {
    match state {
        State::Connected(connected) => connected.rsn.as_mut().map(|rsn| &mut rsn.replay),
        State::Handshake(handshake) => match &mut handshake.step {
            HandshakeStep::InstallGroup(_, replay, _)
            | HandshakeStep::AwaitInstalled(_, replay, _) => Some(replay),
            _ => None,
        },
        _ => None,
    }
}

/// What ended in the station's world, with the port's response evidence
/// `A` and lost-air evidence `D`.
enum Done<A, D> {
    Unsent {
        purpose: TxPurpose,
        reason: TxRefusal,
    },
    Command(StaCommand),
    Tick,
    /// The complete port terminal, including withdrawals and unknown outcomes.
    Tx {
        purpose: TxPurpose,
        completion: SingleOutcome<A, D>,
    },
    Configured,
}

/// The station's [`Done`] in the port types of `T`.
type StaDone<T> = Done<<T as StaTokens>::AckEvidence, <T as StaTokens>::AirDenied>;

/// An action a state owes.
enum Owed<'id, T: StaTokens, K> {
    BlockAck(PendingRxBlockAck<'id, T::EpochPath>),
    InstallPairwise {
        association: T::Association,
        pending: PendingPairwiseKey<'id, T::EpochPath>,
        slot: T::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    },
    InstallGroup {
        permission: T::GroupPermission,
        pending: PendingGroupKey<'id, T::EpochPath>,
        slot: T::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    ReplaceGroup {
        permission: T::GroupPermission,
        pending: PendingGroupReplacement<'id, T::EpochPath>,
        installed: T::GroupKey,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    RemovePairwise(T::PairwiseKey),
    SelectGroup {
        pending: PendingGroupSelection<'id, T::EpochPath>,
        installed: T::GroupKey,
        permission: T::GroupPermission,
    },
    RemoveGroup {
        pending: PendingGroupRemoval<'id, T::EpochPath>,
        key: T::GroupKey,
        permission: T::GroupPermission,
    },
    /// A request, with the place it takes.
    Request(ChannelRequest<K>, LeasePlace<'id, T::GatePath>),
    /// An absence, with the place it takes.
    Absence(ChannelRequest<K>, AbsencePlace<'id, T::GatePath>),
    Ready(LeaseId<'id, T::GatePath>),
    /// The primary lease to exchange.
    Exchange(Lease<'id, K, T::GatePath>),
    /// The primary lease to give back.
    Release(Lease<'id, K, T::GatePath>),
    Upstream(UpstreamFact<K>),
    Configure(
        StationReceive<K>,
        ConfigurePermission<
            'id,
            StationRole,
            T::ConfigurationPath,
            StaConfigurationContext<'id, T::EpochPath>,
        >,
    ),
    SetEdca(SupportedEdca<K>),
    Transmit(StaTransmit<'id, T, K>),
    /// Under the pairwise key of the connected link.
    TransmitProtected(StaTransmit<'id, T, K>),
    Associated(Peer<T::RxMeta>),
    Notify(StaNotice),
}

/// An action, an internal transition to drain, or an actual wait for input.
enum ActionProgress<A> {
    Action(A),
    Transition,
    Wait,
}

impl<A> From<Option<A>> for ActionProgress<A> {
    fn from(action: Option<A>) -> Self {
        match action {
            Some(action) => Self::Action(action),
            None => Self::Wait,
        }
    }
}

type ActionStep<'id, T, K> = (Change<T, K>, ActionProgress<Owed<'id, T, K>>);

/// The affine resources composition grants to one station. Epoch issuance,
/// frame pools, channel admission and key slots move into it together.
pub struct StationResources<
    'id,
    T: StaTokens,
    const TICKETS: usize,
    const BUFFERS: usize,
    const GROUPS: usize,
> {
    /// The installation's only station epoch source.
    pub epochs: LinkEpochSource<'id, T::EpochPath>,
    /// The interface's only receive-configuration permission, minted by the port.
    pub configure: ConfigurePermission<'id, StationRole, T::ConfigurationPath>,
    /// Ticket credits, each reserving a terminal.
    pub tickets: [T::Ticket; TICKETS],
    /// Fresh small allocations for protocol frames.
    pub buffers: [T::SmallBuffer; BUFFERS],
    /// Primary-channel lease admission.
    pub place: LeasePlace<'id, T::GatePath>,
    /// Temporary channel-absence admission.
    pub absence_place: AbsencePlace<'id, T::GatePath>,
    /// The interface association permission and its complete vacant key bank.
    pub keys: T::VacantAssociation,
    /// This station interface's unique GTK mutation permission.
    pub group_permission: T::GroupPermission,
}

/// The proof that a personal station's group-key capacity `GROUPS` is
/// nonzero: a protected station installs the GTK into one of these slots.
///
/// A composition creates it in a `const` item, where a zero capacity fails
/// the build:
///
/// ```
/// use oer_ieee80211_sta::machine::GroupCapacity;
/// const GROUPS: GroupCapacity<1> = GroupCapacity::new();
/// # let _ = GROUPS;
/// ```
///
/// ```compile_fail,E0080
/// use oer_ieee80211_sta::machine::GroupCapacity;
/// const GROUPS: GroupCapacity<0> = GroupCapacity::new();
/// ```
#[derive(Clone, Copy, Debug)]
pub struct GroupCapacity<const GROUPS: usize> {
    _proof: (),
}

impl<const GROUPS: usize> GroupCapacity<GROUPS> {
    /// The proof, checked when the constant is evaluated.
    ///
    /// The inline constant also rejects a zero capacity during code
    /// generation when called outside a constant expression.
    #[expect(
        clippy::new_without_default,
        reason = "the proof is made in a const item, not by a default value"
    )]
    pub const fn new() -> Self {
        const {
            if GROUPS == 0 {
                panic!("a protected station needs at least one group slot");
            }
        }
        Self { _proof: () }
    }
}

impl<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> Station<'id, T, K, C> {
    /// End this machine after the backend's disable/poison terminal. The caller
    /// first returns every backend terminal it received. Unlike ordinary Leave,
    /// this path owes no configuration, key removal or transmission to a port
    /// which can no longer execute it. Pending owners remain with the bank or
    /// the installation that must now be retired.
    pub fn finish(
        mut self,
        reason: StaDisconnectReason,
    ) -> (
        RetiredStation<'id, T, K, C>,
        StationPlaceReturns<'id, T::GatePath>,
    ) {
        let notice = self.finish_commands(reason);
        let Self {
            config,
            security,
            epochs,
            keys,
            configuration,
            tickets,
            buffers,
            place,
            lease: _,
            absence_place,
            absence: _,
            commands,
            sequence,
            pmksa,
            mut state,
            scan: _,
            epoch: _,
        } = self;
        (
            RetiredStation {
                deferred_eapol: state.take_deferred(),
                config,
                security,
                epochs,
                keys,
                configuration,
                tickets,
                buffers,
                commands,
                sequence,
                pmksa,
                notice,
            },
            StationPlaceReturns::new(place, absence_place),
        )
    }

    fn finish_commands(&mut self, reason: StaDisconnectReason) -> Option<StaNotice> {
        self.complete_commands();
        let notice = match self.state {
            State::Idle => None,
            State::Report(existing) => Some(StaNotice::Disconnected(existing)),
            _ => Some(StaNotice::Disconnected(reason)),
        };
        self.commands.complete(
            StaCommand::Connect,
            StaCommandOutcome::ConnectFailed(reason),
        );
        self.commands.complete(
            StaCommand::BackgroundScan,
            StaCommandOutcome::BackgroundScan {
                interrupted: Some(BackgroundScanEnd::LinkEnded(reason)),
            },
        );
        self.commands.complete(
            StaCommand::Disconnect,
            StaCommandOutcome::Disconnected(reason),
        );
        notice
    }

    /// Finalize a placed station without moving the complete machine or its
    /// scan table through a temporary. The returned owner has only terminal APIs.
    #[cfg(feature = "placement")]
    #[inline(never)]
    pub fn finish_placed<'a, 'b>(
        mut machine: oer_memory::Placed<'a, Self>,
        destination: &'b mut core::mem::MaybeUninit<RetiredStation<'id, T, K, C>>,
        reason: StaDisconnectReason,
    ) -> (
        oer_memory::Placed<'b, RetiredStation<'id, T, K, C>>,
        StationPlaceReturns<'id, T::GatePath>,
    ) {
        let notice = machine.finish_commands(reason);
        let (
            config,
            security,
            mut state,
            keys,
            configuration,
            tickets,
            buffers,
            place,
            lease,
            absence_place,
            absence,
            commands,
            sequence,
            scan,
            pmksa,
            epoch,
            epochs,
        ) = oer_memory::unpack_placed!(
            machine,
            Self {
                config,
                security,
                state,
                keys,
                configuration,
                tickets,
                buffers,
                place,
                lease,
                absence_place,
                absence,
                commands,
                sequence,
                scan,
                pmksa,
                epoch,
                epochs
            }
        );
        drop((lease, absence, scan, epoch));
        let deferred = state.take_deferred();
        drop(state);
        let retired = oer_memory::Placed::init(destination, |place| {
            oer_memory::init_in_place!(
                place,
                RetiredStation {
                    deferred_eapol: deferred,
                    config: config.read(),
                    security: security.read(),
                    epochs: epochs.read(),
                    keys: keys.read(),
                    configuration: configuration.read(),
                    tickets: tickets.read(),
                    buffers: buffers.read(),
                    commands: commands.read(),
                    sequence: sequence.read(),
                    pmksa: pmksa.read(),
                    notice: notice,
                }
            )
        });
        (
            retired,
            StationPlaceReturns::new(place.read(), absence_place.read()),
        )
    }

    /// Construct the open machine directly in caller-provided storage.
    #[cfg(feature = "placement")]
    pub fn init_open<'a, const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        config: StaConfig<K, C>,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
    ) -> (&'a mut Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config_in_place(storage, config, StaSecurity::Open, resources)
    }

    /// Build an open station; its backend may grant zero group slots.
    #[doc = include_str!("machine/resources_contracts.md")]
    pub fn new_open<const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        config: StaConfig<K, C>,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
    ) -> (Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config(config, StaSecurity::Open, resources)
    }

    /// Construct a personal station directly in caller-provided storage.
    #[cfg(feature = "placement")]
    pub fn init_personal<'a, const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        config: StaConfig<K, C>,
        password: SaePassword,
        pmk: Pmk,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
        _capacity: GroupCapacity<GROUPS>,
    ) -> (&'a mut Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config_in_place(
            storage,
            config,
            StaSecurity::Personal { password, pmk },
            resources,
        )
    }

    /// Build a personal station. `capacity` proves its group capacity nonzero.
    pub fn new_personal<const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        config: StaConfig<K, C>,
        password: SaePassword,
        pmk: Pmk,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
        _capacity: GroupCapacity<GROUPS>,
    ) -> (Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config(config, StaSecurity::Personal { password, pmk }, resources)
    }

    fn from_config<const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        config: StaConfig<K, C>,
        security: StaSecurity,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
    ) -> (Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        const {
            if TICKETS < 1 {
                panic!("a station needs at least one TX ticket");
            }
            if BUFFERS < 1 {
                panic!("a station needs at least one small buffer");
            }
            let _: (
                SupportedFilter<K, StationRole>,
                SupportedFilter<K, StationRole>,
            ) = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        }
        let StationResources {
            epochs,
            configure,
            tickets,
            buffers,
            place,
            absence_place,
            keys,
            group_permission,
        } = resources;
        let (commands, credits) = Commands::new();
        (
            Self {
                config,
                security,
                state: State::Idle,
                keys: Keys::new::<GROUPS>(keys, group_permission),
                configuration: Some(configure),
                tickets: Pool::new(tickets),
                buffers: Pool::new(buffers),
                place: Some(place),
                lease: Held::None,
                absence_place: Some(absence_place),
                absence: None,
                commands,
                sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
                scan: ScanTable::new(),
                pmksa: StaPmksaCache::new(),
                epoch: None,
                epochs,
            },
            credits,
        )
    }

    #[cfg(feature = "placement")]
    #[inline(never)]
    fn from_config_in_place<'a, const TICKETS: usize, const BUFFERS: usize, const GROUPS: usize>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        config: StaConfig<K, C>,
        security: StaSecurity,
        resources: StationResources<'id, T, TICKETS, BUFFERS, GROUPS>,
    ) -> (&'a mut Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        const {
            if TICKETS < 1 {
                panic!("a station needs at least one TX ticket");
            }
            if BUFFERS < 1 {
                panic!("a station needs at least one small buffer");
            }
            let _: (
                SupportedFilter<K, StationRole>,
                SupportedFilter<K, StationRole>,
            ) = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        }
        let StationResources {
            epochs,
            configure,
            tickets,
            buffers,
            place,
            absence_place,
            keys,
            group_permission,
        } = resources;
        let (commands, credits) = Commands::new();
        (
            oer_memory::init_in_place!(storage, Self {
                config: config,
                security: security,
                state: State::Idle,
                keys: Keys::new::<GROUPS>(keys, group_permission),
                configuration: Some(configure),
                tickets: Pool::new(tickets),
                buffers: Pool::new(buffers),
                place: Some(place),
                lease: Held::None,
                absence_place: Some(absence_place),
                absence: None,
                commands: commands,
                sequence: StaSequenceCounter::new(SequenceNumber::ZERO),
                scan => ScanTable::init_in_place,
                pmksa: StaPmksaCache::new(),
                epoch: None,
                epochs: epochs,
            }),
            credits,
        )
    }

    /// Tokens given back beyond the station's own: dropped, counted.
    pub fn lost_tokens(&self) -> u32 {
        self.tickets.lost().combined(self.buffers.lost()).get()
    }

    /// Whether an operation still belongs to this connection attempt.
    /// Resource return is unconditional; protocol and rate updates use this guard.
    pub fn is_current(&self, context: &StaTxContext<'id, T::EpochPath>) -> bool {
        Some(context.epoch) == self.epoch
    }

    /// Whether the station is connected.
    pub const fn is_connected(&self) -> bool {
        match &self.state {
            State::Connected(place) => match place.as_ref() {
                Some(connected) => connected.confirmation.is_none(),
                None => false,
            },
            _ => false,
        }
    }

    /// Startup is blocked on explicit cleanup actions and their port terminals.
    /// A service leaves application requests queued until this phase completes.
    pub const fn is_reclaiming(&self) -> bool {
        matches!(self.state, State::Reclaiming(_))
    }

    /// The access point of the associated station, as its association made
    /// it: during the handshake and once connected.
    pub const fn peer(&self) -> Option<&Peer<T::RxMeta>> {
        match &self.state {
            State::Handshake(handshake) => Some(&handshake.peer),
            State::Connected(place) => match place.as_ref() {
                Some(connected) => Some(&connected.peer),
                None => None,
            },
            _ => None,
        }
    }

    /// The connected link, which the data machine sends and receives on,
    /// while data may go: the station is home and awake, and a protected
    /// link holds its key. It lends the pairwise key, so only the station
    /// removes it, the replay check, and on a non-QoS BSS the station's
    /// sequence space.
    pub fn link(&mut self) -> Option<LinkView<'_, 'id, T>> {
        let State::Connected(connected) = &mut self.state else {
            return None;
        };
        let connected = connected.as_mut()?;
        if connected.confirmation.is_some() || self.absence.is_some() || connected.power.asleep() {
            return None;
        }
        let pairwise = match &self.keys.pairwise {
            KeyCell::Installed(key) => Some(key),
            KeyCell::Free(_)
            | KeyCell::Installing
            | KeyCell::Removing
            | KeyCell::UnreturnedBank { .. } => None,
        };
        let replay = match connected.rsn.as_mut() {
            Some(rsn) => Some(&mut rsn.replay),
            None => None,
        };
        let protection = match (pairwise, replay) {
            (Some(key), Some(replay)) => LinkProtection::Ccmp { key, replay },
            // A protected link without its key sends nothing.
            (None, Some(_)) | (Some(_), None) => return None,
            (None, None) => LinkProtection::Open,
        };
        Some(LinkView {
            epoch: self.epoch?,
            address: self.config.address,
            bssid: connected.bssid,
            association_id: connected.association_id,
            mode: if connected.qos {
                LinkMode::Qos(&mut connected.qos_sequences)
            } else {
                LinkMode::NonQos(&mut self.sequence)
            },
            protection,
        })
    }
    #[cfg(test)]
    /// The security policy the scan selects and the join associates with.
    const fn policy(&self) -> StaSecurityPolicy {
        match self.security {
            StaSecurity::Open => StaSecurityPolicy::Open,
            StaSecurity::Personal { .. } => StaSecurityPolicy::Wpa2Personal,
        }
    }

    /// Admit a request and reserve its terminal before changing protocol state.
    /// Refusal returns that same request immediately and owes no terminal.
    #[inline(never)]
    pub fn command(
        &mut self,
        now: Instant,
        request: StaRequest<'id, T::ConfigurationPath>,
    ) -> Result<(), StaCommandRefused<'id, T::ConfigurationPath>> {
        self.complete_commands();
        let command = request.command();
        let reason = match (command, &self.state) {
            (StaCommand::Connect, State::Idle) => None,
            (StaCommand::Connect, State::Connected(connected))
                if connected.confirmation.is_none() =>
            {
                Some(CommandRefusal::AlreadyConnected)
            }
            (StaCommand::Connect, _) => Some(CommandRefusal::Transitioning),
            (
                StaCommand::Disconnect,
                State::Connected(_) | State::Scan(_) | State::Join(_) | State::Handshake(_),
            ) => None,
            (StaCommand::Disconnect, State::Idle) => Some(CommandRefusal::NotConnected),
            (StaCommand::Disconnect, _) => Some(CommandRefusal::Transitioning),
            (StaCommand::BackgroundScan, State::Connected(connected))
                if connected.confirmation.is_none() =>
            {
                None
            }
            (StaCommand::BackgroundScan, _) => Some(CommandRefusal::NotConnected),
        };
        if let Some(reason) = reason {
            return Err(StaCommandRefused { request, reason });
        }
        self.commands.reserve(request);
        let (mut context, state) = self.transition();
        let change = context.step(state, now, Done::Command(command));
        state.apply(change);
        self.complete_commands();
        Ok(())
    }

    /// Record terminal outcomes before a later input can change their state.
    fn complete_commands(&mut self) {
        match &self.state {
            State::Connected(connected) if connected.confirmation.is_none() => {
                self.commands.complete(
                    StaCommand::Connect,
                    StaCommandOutcome::Connected {
                        bssid: connected.bssid,
                        channel: connected.channel.channel().number(),
                    },
                )
            }
            State::Leave(leave) => self.commands.complete(
                StaCommand::BackgroundScan,
                StaCommandOutcome::BackgroundScan {
                    interrupted: Some(BackgroundScanEnd::LinkEnded(leave.reason)),
                },
            ),
            State::Report(reason) => {
                self.commands.complete(
                    StaCommand::Connect,
                    StaCommandOutcome::ConnectFailed(*reason),
                );
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan {
                        interrupted: Some(BackgroundScanEnd::LinkEnded(*reason)),
                    },
                );
                self.commands.complete(
                    StaCommand::Disconnect,
                    StaCommandOutcome::Disconnected(*reason),
                );
            }
            _ => {}
        }
    }

    /// Take one input at `now`.
    pub fn input(&mut self, now: Instant, input: StaInput<'id, T, K>) {
        let (mut context, state) = self.transition();
        context.input_event(state, now, input);
        self.complete_commands();
    }

    /// Admit a received frame once, process station EAPOL/management, and
    /// deliver each remaining MSDU. Refusals retain their typed reason.
    ///
    /// Data cannot be admitted again through the old second RX entry point:
    ///
    /// ```compile_fail,E0425
    /// let _ = oer_ieee80211_sta::data::received;
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_sta::machine::{Station, StaTokens};
    /// use oer_ieee80211_port::Ieee80211Capabilities;
    /// fn receive<T: StaTokens, K: Ieee80211Capabilities, C: oer_ieee80211_sta::machine::StaCallbacks>(
    ///     station: &mut Station<'_, T, K, C>, now: oer_time::Instant,
    ///     frame: &[u8], meta: T::RxMeta,
    /// ) {
    ///     let _ = station.receive(now, frame, meta, |_| {});
    /// }
    /// ```
    #[inline(never)]
    pub fn receive<'frame>(
        &mut self,
        now: Instant,
        frame: &'frame [u8],
        meta: T::RxMeta,
        mut deliver: impl FnMut(oer_ieee80211_wire::data::EthernetFrameParts<'frame>),
    ) -> Result<(), StaRxError> {
        let (mut context, state) = self.transition();
        let result = context.receive_frame(state, now, frame, meta, &mut deliver);
        self.complete_commands();
        result
    }

    /// The next action to carry out at `now`, if any. `None` means a wait
    /// for an input, a returned resource, or the state's deadline.
    ///
    /// Internal transitions form an acyclic graph. Join completion enters
    /// Handshake/Connected; preparation failure enters Leave; Leave moves
    /// through RemoveKeys, NeedDeauthentication and NeedConfigure, or from
    /// NeedRelease to Report. The longest path is five steps: connected
    /// failure, an already ended absence, returned key slots, failed frame
    /// encoding, configuration. Every back edge requires an external
    /// completion or a new command. Associated is its own join step before
    /// either Connected or the first handshake action.
    #[must_use = "perform the returned action before waiting for another input"]
    #[inline(never)]
    pub fn poll_action(&mut self, now: Instant) -> Option<StaAction<'_, 'id, T, K>> {
        if matches!(self.keys.pairwise, KeyCell::UnreturnedBank { .. }) {
            return Some(StaAction::ContractViolation(
                StaKeyContractViolation::IncompleteBank,
            ));
        }
        let owed = loop {
            self.complete_commands();
            if let Some(terminal) = self.commands.poll() {
                return Some(StaAction::CommandCompleted(terminal));
            }
            let (mut context, state) = self.transition();
            let (change, progress) = context.owe(state, now);
            state.apply(change);
            match progress {
                ActionProgress::Action(owed) => break owed,
                ActionProgress::Transition => continue,
                ActionProgress::Wait => return None,
            }
        };
        Some(match owed {
            Owed::BlockAck(request) => StaAction::BlockAck(request),
            // Owed only while the station holds its primary lease; without
            // it the place stays, and nothing is asked.
            Owed::Absence(request, place) => match &self.lease {
                Held::Lease(home) => StaAction::RequestAbsence {
                    request,
                    home,
                    place,
                },
                _ => {
                    self.absence_place = Some(place);
                    return None;
                }
            },
            Owed::Ready(id) => StaAction::Ready(id),
            Owed::InstallPairwise {
                association,
                pending,
                slot,
                peer,
                key,
            } => StaAction::InstallPairwiseKey {
                association,
                pending,
                slot,
                peer,
                key,
            },
            Owed::InstallGroup {
                permission,
                pending,
                slot,
                key_id,
                key,
            } => match &self.keys.pairwise {
                KeyCell::Installed(pairwise) => StaAction::InstallGroupKey {
                    association: T::association(pairwise),
                    permission,
                    pending,
                    slot,
                    key_id,
                    key,
                },
                _ => {
                    self.keys.group_slots.free.put(slot);
                    self.keys.groups.release(pending.place);
                    self.keys.group_permission = Some(permission);
                    return Some(StaAction::ContractViolation(
                        StaKeyContractViolation::MissingAssociation,
                    ));
                }
            },
            Owed::RemovePairwise(key) => StaAction::RemovePairwiseKey(key),
            Owed::ReplaceGroup {
                permission,
                pending,
                installed,
                key_id,
                key,
            } => StaAction::ReplaceGroupKey {
                permission,
                pending,
                installed,
                key_id,
                key,
            },
            Owed::SelectGroup {
                pending,
                installed,
                permission,
            } => StaAction::SelectGroupKey {
                pending,
                installed,
                permission,
            },
            Owed::RemoveGroup {
                pending,
                key,
                permission,
            } => StaAction::RemoveGroupKey {
                pending,
                key,
                permission,
            },
            Owed::Exchange(lease) => StaAction::ExchangeLease(lease),
            Owed::Request(request, place) => StaAction::RequestChannel(request, place),
            Owed::Release(lease) => StaAction::ReleaseChannel(lease),
            Owed::Upstream(fact) => StaAction::Upstream(fact),
            Owed::Configure(receive, pending) => StaAction::Configure(receive, pending),
            Owed::SetEdca(edca) => StaAction::SetEdca(edca),
            Owed::Transmit(transmit) => StaAction::Transmit(transmit),
            // Owed only while the link is up in this very step, so the key
            // is there; without it the frame does not go and its tokens
            // come back.
            Owed::TransmitProtected(transmit) => match &self.keys.pairwise {
                KeyCell::Installed(key) => StaAction::TransmitProtected { transmit, key },
                _ => {
                    self.tickets.put(transmit.ticket);
                    self.buffers.put(transmit.buffer);
                    return None;
                }
            },
            Owed::Notify(notice) => StaAction::Notify(notice),
            Owed::Associated(peer) => StaAction::Associated(peer),
        })
    }

    /// When the station next needs a [`StaInput::Tick`], even without an
    /// event.
    pub fn poll_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Join(Join {
                step:
                    JoinStep::Authenticate(Authenticate::Exchange {
                        stage: AuthStage::AwaitResponse { until },
                        ..
                    }),
                ..
            }) => Some(*until),
            State::Join(Join {
                step: JoinStep::Associate(runtime, _),
                ..
            }) => {
                if self.tickets.is_empty() || self.buffers.is_empty() {
                    Some(runtime.response_deadline())
                } else {
                    Some(runtime.next_deadline())
                }
            }
            State::Join(Join {
                step: JoinStep::SaeSend { authentication, .. } | JoinStep::SaeAwait(authentication),
                ..
            }) => Some(authentication.next_deadline()),
            State::Join(Join {
                step: JoinStep::RetryAt(at),
                ..
            }) => Some(*at),
            State::Handshake(Handshake { deadline, .. }) => Some(deadline.deadline()),
            State::Connected(connected) => {
                if let Some(deadline) = connected.confirmation {
                    return Some(deadline.deadline());
                }
                let background =
                    connected
                        .background
                        .as_ref()
                        .and_then(|background| match background.step {
                            BackgroundStep::HomeProbe { until }
                            | BackgroundStep::HomeListen { until } => Some(until),
                            _ => None,
                        });
                let management = connected
                    .rsn
                    .as_ref()
                    .and_then(|rsn| rsn.management.as_ref())
                    .and_then(|management| {
                        management.deadline(
                            connected.sending
                                || self.tickets.is_empty()
                                || self.buffers.is_empty()
                                || !(self.absence.is_none()
                                    && !connected.power.asleep()
                                    && matches!(self.keys.pairwise, KeyCell::Installed(_))),
                        )
                    });
                crate::time::earliest(background, management)
            }
            _ => None,
        }
    }
    fn transition(&mut self) -> (TransitionContext<'_, 'id, T, K, C>, &mut State<T, K>) {
        (
            TransitionContext {
                config: &mut self.config,
                security: &mut self.security,
                keys: &mut self.keys,
                configuration: &mut self.configuration,
                tickets: &mut self.tickets,
                buffers: &mut self.buffers,
                place: &mut self.place,
                lease: &mut self.lease,
                absence_place: &mut self.absence_place,
                absence: &mut self.absence,
                commands: &mut self.commands,
                sequence: &mut self.sequence,
                scan: &mut self.scan,
                pmksa: &mut self.pmksa,
                epoch: &mut self.epoch,
                epochs: &mut self.epochs,
            },
            &mut self.state,
        )
    }
}

/// Disjoint resource loans used while the resident phase is mutably borrowed.
struct TransitionContext<'s, 'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> {
    config: &'s mut StaConfig<K, C>,
    security: &'s mut StaSecurity,
    keys: &'s mut Keys<T>,
    configuration: &'s mut Option<ConfigurePermission<'id, StationRole, T::ConfigurationPath>>,
    tickets: &'s mut Pool<T::Ticket>,
    buffers: &'s mut Pool<T::SmallBuffer>,
    /// The station's place for a lease, while it holds none.
    place: &'s mut Option<LeasePlace<'id, T::GatePath>>,
    /// The lease the station holds, or gave back and awaits the end of.
    lease: &'s mut Held<'id, K, T::GatePath>,
    /// The station's place for an absence, while it takes none.
    absence_place: &'s mut Option<AbsencePlace<'id, T::GatePath>>,
    /// The absence the station holds.
    absence: &'s mut Option<Lease<'id, K, T::GatePath>>,
    /// Each admitted application request reserves its own terminal cell.
    commands: &'s mut Commands<'id, T::ConfigurationPath>,
    sequence: &'s mut StaSequenceCounter,
    scan: &'s mut ScanTable<SCAN_CAPACITY>,
    pmksa: &'s mut StaPmksaCache,
    /// The connection attempt that owns new TX operations.
    epoch: &'s mut Option<LinkEpoch<'id, T::EpochPath>>,
    epochs: &'s mut LinkEpochSource<'id, T::EpochPath>,
}

impl<'s, 'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks>
    TransitionContext<'s, 'id, T, K, C>
{
    /// The security policy the scan selects and the join associates with.
    const fn policy(&self) -> StaSecurityPolicy {
        match *self.security {
            StaSecurity::Open => StaSecurityPolicy::Open,
            StaSecurity::Personal { .. } => StaSecurityPolicy::Wpa2Personal,
        }
    }

    fn ssid(&self) -> &[u8] {
        &self.config.ssid
    }

    fn channel(&self, visit: usize) -> Option<SupportedChannel<K>> {
        self.config.channels.get(visit).copied()
    }

    #[inline(never)]
    fn input_event(&mut self, state: &mut State<T, K>, now: Instant, input: StaInput<'id, T, K>) {
        let done = match input {
            StaInput::TxDone {
                context,
                ticket,
                buffer,
                completion,
            } => {
                // A transmission's tokens come back whatever the state.
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(T::renew));
                if Some(context.epoch) != *self.epoch {
                    return;
                }
                Done::Tx {
                    purpose: context.purpose,
                    completion,
                }
            }
            StaInput::Unsent {
                reason,
                context,
                ticket,
                buffer,
            } => {
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(T::renew));
                if Some(context.epoch) != *self.epoch {
                    return;
                }
                Done::Unsent {
                    purpose: context.purpose,
                    reason,
                }
            }
            StaInput::Tick => Done::Tick,
            StaInput::Configured(permission) => {
                let (permission, pending) = permission.into_parts();
                (*self.configuration) = Some(permission);
                if pending.epoch != current_epoch(state, *self.epoch) {
                    return;
                }
                Done::Configured
            }
            StaInput::Answer(answer) => {
                self.answered(state, answer);
                return;
            }
            StaInput::AbsenceAnswer(answer) => {
                self.absence_answered(state, answer);
                return;
            }
            StaInput::Exchanged(answer) => {
                match answer {
                    Ok(lease) => (*self.lease) = Held::Lease(lease),
                    Err(lease) => {
                        (*self.lease) = Held::Lease(lease);
                        if let State::Connected(connected) = state
                            && connected.confirmation.is_none()
                        {
                            let change = leave_connected(
                                connected,
                                StaDisconnectReason::LeaseExchangeRefused,
                                None,
                            );
                            state.apply(change);
                        }
                    }
                }
                return;
            }
            StaInput::PairwiseKeyInstalled { pending, key } => {
                let current = current_epoch(state, *self.epoch) == Some(pending.epoch);
                self.keys.pairwise_installed(pending, key);
                // Leave waits for every outstanding key installation to return.
                // An old response becomes a removal action there.
                if current {
                    self.keys_changed(state);
                }
                return;
            }
            StaInput::GroupKeyInstalled {
                pending,
                key,
                permission,
            } => {
                self.group_transition(state, pending, key, permission, None, false);
                return;
            }
            StaInput::GroupKeyReplaced {
                pending,
                key,
                permission,
            } => {
                self.group_transition(
                    state,
                    pending.install,
                    key,
                    permission,
                    Some(pending.previous_id),
                    false,
                );
                return;
            }
            StaInput::GroupKeySelected {
                pending,
                key,
                permission,
            } => {
                self.group_transition(state, pending.install, key, permission, None, true);
                return;
            }
            StaInput::PairwiseKeyRemoved { slot, association } => {
                let mut spare =
                    Pool::from_collection(core::mem::take(&mut self.keys.spare_pairwise));
                let parts = assemble_key_bank(
                    slot,
                    spare.take_all(),
                    [
                        self.keys.group_slots.free.take_all(),
                        self.keys.spare_group.take_all(),
                    ],
                )
                .and_then(|parts| {
                    T::collect_key_bank(parts)
                        .map_err(|parts| KeyBankReunionError::Incomplete { _parts: parts })
                });
                match parts {
                    Ok(bank) => {
                        let (association, slot, parts) = T::end_association(association, bank);
                        self.keys.pairwise = KeyCell::Free(VacantPairwise { association, slot });
                        self.keys.spare_pairwise = parts.pairwise;
                        let (free, spare) = Pool::partition(parts.group, T::GROUP_SLOTS);
                        self.keys.group_slots.free = free;
                        self.keys.spare_group = spare;
                    }
                    Err(error) => {
                        self.keys.pairwise = KeyCell::UnreturnedBank {
                            _association: association,
                            _error: error,
                        };
                    }
                }
                return;
            }
            StaInput::GroupKeyRemoved {
                pending,
                slot,
                permission,
            } => {
                if matches!(pending.replay, GroupReplay::Active)
                    && let Some(replay) = active_replay(state)
                {
                    replay.remove_group(pending.key_id);
                }
                self.keys.group_removed(slot, permission, pending.place);
                return;
            }
            StaInput::Arbiter(notice) => {
                self.noticed(state, notice);
                return;
            }
        };
        let change = self.step(state, now, done);
        state.apply(change);
    }

    /// The arbiter's answer.
    #[inline(never)]
    fn answered(&mut self, state: &mut State<T, K>, answer: Answer<'id, K, T::GatePath>) {
        let change = match answer {
            Answer::Lease(lease) => {
                let access_point = lease.access_point();
                *self.lease = Held::Lease(lease);
                match state {
                    State::Scan(scan) => {
                        scan.access_point = access_point;
                        scan.step = ScanStep::AwaitStart;
                    }
                    State::Join(join) => join.step = JoinStep::AwaitStart,
                    _ => {}
                }
                Change::Keep
            }
            Answer::Refused(refusal, place) => {
                *self.place = Some(place);
                match (&mut *state, refusal) {
                    (State::Join(join), Refusal::TooSoon { retry_at }) => {
                        join.step = JoinStep::RetryAt(retry_at);
                        Change::Keep
                    }
                    (State::Scan(_) | State::Join(_), _) => {
                        leave_unkeyed(StaDisconnectReason::Refused(refusal))
                    }
                    _ => Change::Keep,
                }
            }
        };
        state.apply(change);
    }

    #[inline(never)]
    fn group_transition(
        &mut self,
        state: &mut State<T, K>,
        pending: PendingGroupKey<'id, T::EpochPath>,
        key: T::GroupKey,
        permission: T::GroupPermission,
        replaced: Option<GroupKeyId>,
        selected: bool,
    ) {
        let current = current_epoch(state, *self.epoch) == Some(pending.epoch);
        let replay = if current { active_replay(state) } else { None };
        // An evicted installation of another ID ends here. Retained entries
        // of that ID still own their active state independently.
        let mut replay = replay;
        if let Some(old) = replaced
            && old != pending.key_id
            && !self.keys.groups.has_active(old)
            && let Some(replay) = &mut replay
        {
            replay.remove_group(old);
        }
        (*self.keys).group_installed(pending, key, permission, current, replay, selected);
        if current {
            self.group_installed(state);
        }
    }

    /// A key of the handshake is installed: once both are, Message 4
    /// follows.
    #[inline(never)]
    fn keys_changed(&mut self, state: &mut State<T, K>) {
        let change = match state {
            State::Handshake(handshake) if self.keys.installed() => {
                match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
                    HandshakeStep::AwaitInstalled(request, replay, protection) => {
                        finish_handshake_install(handshake, request, replay, protection)
                    }
                    step => {
                        handshake.step = step;
                        Change::Keep
                    }
                }
            }
            _ => Change::Keep,
        };
        state.apply(change);
    }

    /// A group key terminal completes only its own exchange stage. Other
    /// stages (including an encoded response waiting for credit) stay owned.
    fn group_installed(&mut self, state: &mut State<T, K>) {
        match state {
            State::Connected(connected) => finish_connected_install(connected),
            _ => self.keys_changed(state),
        }
    }

    /// The arbiter's answer to the station's absence.
    #[inline(never)]
    fn absence_answered(
        &mut self,
        state: &mut State<T, K>,
        answer: AbsenceAnswer<'id, K, T::GatePath>,
    ) {
        let connected = match state {
            State::Connected(connected) if connected.confirmation.is_none() => connected,
            _ => {
                if let AbsenceAnswer::Refused(_, place) = answer {
                    *self.absence_place = Some(place);
                }
                return;
            }
        };
        let Some(background) = &mut connected.background else {
            return;
        };
        match answer {
            AbsenceAnswer::Lease(lease) => {
                (*self.absence) = Some(lease);
                background.step = BackgroundStep::NeedSleep;
            }
            AbsenceAnswer::Refused(reason, place) => {
                (*self.absence_place) = Some(place);
                connected.background = None;
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan {
                        interrupted: Some(BackgroundScanEnd::AbsenceRefused(reason)),
                    },
                );
            }
        }
    }

    /// The background scan's step after its absence ended, `cancelled` or
    /// not.
    fn absence_over(&mut self, state: &mut State<T, K>, cancelled: bool) {
        (*self.absence) = None;
        let limit = self.config.skipped_absences;
        let connected = match state {
            State::Connected(connected) if connected.confirmation.is_none() => connected,
            _ => return,
        };
        let connected = &mut **connected;
        let Some(background) = &mut connected.background else {
            return;
        };
        background.step = if cancelled {
            background.skipped = background.skipped.saturating_add(1);
            if connected.power.asleep() {
                // A PM=1 may have reached the access point: wake first.
                BackgroundStep::NeedWake
            } else if background.skipped >= limit {
                connected.background = None;
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan {
                        interrupted: Some(BackgroundScanEnd::CancelledAbsences),
                    },
                );
                return;
            } else {
                BackgroundStep::NeedAbsence
            }
        } else {
            background.skipped = 0;
            BackgroundStep::NeedHome
        };
    }

    /// A notice of the arbiter.
    #[inline(never)]
    fn noticed(&mut self, state: &mut State<T, K>, notice: StationNotice<'id, K, T::GatePath>) {
        let change = match notice {
            StationNotice::AbsenceStarted(id) => {
                if self.absence.as_ref().map(Lease::id) == Some(id)
                    && let State::Connected(connected) = &mut *state
                    && connected.confirmation.is_none()
                    && let Some(background) = &mut connected.background
                    && background.step == BackgroundStep::AwaitStart
                {
                    background.step = BackgroundStep::NeedConfigure;
                }
                Change::Keep
            }
            StationNotice::AbsenceEnded(place) => {
                *self.absence_place = Some(place);
                self.absence_over(state, false);
                Change::Keep
            }
            StationNotice::AbsenceCancelled(_, place) => {
                *self.absence_place = Some(place);
                self.absence_over(state, true);
                Change::Keep
            }
            StationNotice::LeaseStarted(id) => {
                if self.lease.id() == Some(id) {
                    match &mut *state {
                        State::Scan(scan) if scan.step == ScanStep::AwaitStart => {
                            scan.skipped = 0;
                            scan.step = ScanStep::NeedConfigure;
                        }
                        State::Join(join) if matches!(join.step, JoinStep::AwaitStart) => {
                            join.step = JoinStep::NeedConfigure
                        }
                        _ => {}
                    }
                }
                Change::Keep
            }
            StationNotice::LeaseEnded(place) => {
                *self.lease = Held::None;
                *self.place = Some(place);
                self.lease_ended(state, false)
            }
            StationNotice::LeaseCancelled(_, place) => {
                *self.lease = Held::None;
                *self.place = Some(place);
                self.lease_ended(state, true)
            }
            StationNotice::Moved(channel) => {
                if let State::Connected(connected) = &mut *state
                    && connected.confirmation.is_none()
                {
                    connected.channel = channel;
                    connected.exchange = true;
                }
                Change::Keep
            }
            StationNotice::Leave(reason) => {
                let reason_code = match reason {
                    LeaveReason::Stopped | LeaveReason::PortFailed => None,
                    LeaveReason::Unservable => Some(REASON_LEAVING),
                };
                match &mut *state {
                    State::Connected(connected) => leave_connected(
                        connected,
                        StaDisconnectReason::Arbiter(reason),
                        reason_code,
                    ),
                    State::Handshake(handshake) => leave_keyed(
                        StaDisconnectReason::Arbiter(reason),
                        reason_code.map(|reason_code| Deauthentication {
                            bssid: handshake.bssid,
                            reason_code,
                            protected: handshake.management.is_some(),
                        }),
                    ),
                    State::Join(_) | State::Scan(_) => {
                        leave_unkeyed(StaDisconnectReason::Arbiter(reason))
                    }
                    _ => Change::Keep,
                }
            }
        };
        state.apply(change);
    }

    /// The state after the lease ended, at its own end or `cancelled`.
    fn lease_ended(&mut self, state: &mut State<T, K>, cancelled: bool) -> Change<T, K> {
        let lost = StaDisconnectReason::Arbiter(LeaveReason::PortFailed);
        match state {
            // A skipped absence: the same channel again, until too many in
            // a row end the scan with what it found.
            State::Scan(scan) if cancelled => {
                let skipped = scan.skipped.saturating_add(1);
                if skipped >= self.config.skipped_absences {
                    self.select(true, scan.access_point)
                } else {
                    Change::Scan(Scan {
                        skipped,
                        step: ScanStep::NeedLease,
                        visit: scan.visit,
                        access_point: scan.access_point,
                    })
                }
            }
            // The visit's listening is over: the next channel.
            State::Scan(scan) => self.visit(scan.visit.saturating_add(1), 0, scan.access_point),
            State::Leave(leave) if leave.step == LeaveStep::AwaitEnd => self.report(leave),
            // A terminal may arrive while an earlier leave action is still
            // owed. Only AwaitEnd proves that deauthentication and receive
            // shutdown completed; all preceding steps retain their payload.
            State::Leave(_) => Change::Keep,
            // The lease ended without the station's release: the port left;
            // the station leaves, its keys first.
            State::Connected(connected) => leave_connected(connected, lost, None),
            State::Handshake(_) => leave_keyed(lost, None),
            State::Join(_) => leave_unkeyed(lost),
            _ => Change::Keep,
        }
    }

    /// A leave reports once every key came back; until then it removes
    /// them.
    fn report(&mut self, leave: &mut Leave) -> Change<T, K> {
        if self.keys.clear() {
            Change::Report(leave.reason)
        } else {
            leave.step = LeaveStep::RemoveKeys(None);
            Change::Keep
        }
    }

    /// The state after `done` in `state`.
    #[inline(never)]
    fn step(&mut self, state: &mut State<T, K>, now: Instant, done: StaDone<T>) -> Change<T, K> {
        if let Done::Unsent { purpose, reason } = done {
            return self.unsent(state, now, purpose, reason);
        }
        match (state, done) {
            (State::Reclaiming(Reclaim::AwaitConfigured), Done::Configured) => Change::Idle,
            (State::Idle | State::Report(_), Done::Command(StaCommand::Connect)) => {
                (*self.epoch) = Some(self.epochs.begin());
                self.scan.clear();
                self.visit(0, 0, None)
            }
            (State::Connected(connected), Done::Command(StaCommand::Disconnect)) => {
                leave_connected(
                    connected,
                    StaDisconnectReason::Requested,
                    Some(REASON_LEAVING),
                )
            }
            (State::Scan(_), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(StaDisconnectReason::Requested)
            }
            (State::Join(_), Done::Command(StaCommand::Disconnect)) => {
                leave_unkeyed(StaDisconnectReason::Requested)
            }
            (State::Connected(connected), Done::Command(StaCommand::BackgroundScan))
                if connected.confirmation.is_none() && connected.background.is_none() =>
            {
                if now.checked_add(self.config.dwell).is_none() {
                    self.commands.complete(
                        StaCommand::BackgroundScan,
                        StaCommandOutcome::BackgroundScan {
                            interrupted: Some(BackgroundScanEnd::DeadlineOverflow),
                        },
                    );
                    return Change::Keep;
                }
                self.scan.clear();
                if self.channel(0).is_some() {
                    connected.background = Some(Background {
                        visit: 0,
                        skipped: 0,
                        step: BackgroundStep::NeedAbsence,
                    });
                } else {
                    self.commands.complete(
                        StaCommand::BackgroundScan,
                        StaCommandOutcome::BackgroundScan { interrupted: None },
                    );
                }
                Change::Keep
            }
            (State::Connected(connected), done) => self.confirming_step(connected, now, done),
            (State::Scan(scan), Done::Configured) if scan.step == ScanStep::AwaitConfigured => {
                scan.step = ScanStep::NeedProbe;
                Change::Keep
            }
            (State::Join(join), done) => self.join_step(join, now, done),
            (State::Handshake(handshake), Done::Command(StaCommand::Disconnect)) => {
                leave_handshake(handshake, REASON_LEAVING)
            }
            (State::Handshake(handshake), done) => match (&handshake.step, done) {
                (
                    HandshakeStep::AwaitSent,
                    Done::Tx {
                        purpose: TxPurpose::HandshakeResponse,
                        ..
                    },
                ) => {
                    handshake.step = HandshakeStep::Await;
                    Change::Keep
                }
                (_, Done::Tick)
                    if matches!(
                        handshake.deadline.poll(now),
                        RsnStaDeadlineEvent::Expired { .. }
                    ) =>
                {
                    leave_handshake(handshake, REASON_HANDSHAKE_TIMEOUT)
                }
                _ => Change::Keep,
            },
            (State::Leave(leave), done) => {
                leave.step = match (leave.step, done) {
                    (
                        LeaveStep::AwaitDeauthentication,
                        Done::Tx {
                            purpose: TxPurpose::Deauthentication,
                            ..
                        },
                    ) => LeaveStep::NeedConfigure,
                    (
                        LeaveStep::AwaitProtectedDeauthentication,
                        Done::Tx {
                            purpose: TxPurpose::Deauthentication,
                            ..
                        },
                    ) => LeaveStep::RemoveKeys(None),
                    (LeaveStep::AwaitConfigured, Done::Configured) => LeaveStep::NeedRelease,
                    (step, _) => step,
                };
                Change::Keep
            }
            (_, _) => Change::Keep,
        }
    }

    /// Refusal is a protocol input of its own. In particular, refusing M4
    /// never executes the transition for an accepted TX completion.
    #[inline(never)]
    fn unsent(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        purpose: TxPurpose,
        reason: TxRefusal,
    ) -> Change<T, K> {
        match (state, purpose) {
            (State::Connected(connected), TxPurpose::HandshakeLast)
                if connected.confirmation.is_some() =>
            {
                leave_connected(
                    connected,
                    StaDisconnectReason::HandshakeFailed,
                    Some(REASON_UNSPECIFIED),
                )
            }
            (
                State::Handshake(handshake),
                TxPurpose::HandshakeResponse | TxPurpose::HandshakeLast,
            ) => leave_handshake(handshake, REASON_UNSPECIFIED),
            (State::Join(join), TxPurpose::Authentication { attempt }) => {
                if matches!(join.step,
                    JoinStep::Authenticate(Authenticate::Exchange {
                        stage: AuthStage::AwaitSent { attempt: current, .. }, ..
                    }) if current == attempt
                ) {
                    leave_unkeyed(StaDisconnectReason::TransmitRefused(reason))
                } else {
                    Change::Keep
                }
            }
            (State::Join(_), TxPurpose::SaeAuthentication | TxPurpose::Association) => {
                leave_unkeyed(StaDisconnectReason::TransmitRefused(reason))
            }
            (State::Scan(_), TxPurpose::Probe) => {
                leave_unkeyed(StaDisconnectReason::TransmitRefused(reason))
            }
            (State::Connected(connected), purpose @ TxPurpose::Connected(_))
                if connected.confirmation.is_none() =>
            {
                self.connected_step(connected, now, Done::Unsent { purpose, reason })
            }
            (State::Leave(leave), TxPurpose::Deauthentication) => {
                leave.step = match leave.step {
                    LeaveStep::AwaitDeauthentication => LeaveStep::NeedConfigure,
                    LeaveStep::AwaitProtectedDeauthentication => LeaveStep::RemoveKeys(None),
                    step => step,
                };
                Change::Keep
            }
            (_, _) => Change::Keep,
        }
    }

    /// The installed-key phase still waiting for D4's delivered M4 terminal.
    #[inline(never)]
    fn confirming_step(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        done: StaDone<T>,
    ) -> Change<T, K> {
        let Some(deadline) = connected.confirmation else {
            return self.connected_step(connected, now, done);
        };
        match done {
            Done::Tick if matches!(deadline.poll(now), RsnStaDeadlineEvent::Expired { .. }) => {
                leave_connected(
                    connected,
                    StaDisconnectReason::HandshakeFailed,
                    Some(REASON_HANDSHAKE_TIMEOUT),
                )
            }
            Done::Command(StaCommand::Disconnect) => leave_connected(
                connected,
                StaDisconnectReason::Requested,
                Some(REASON_LEAVING),
            ),
            Done::Tx {
                purpose: TxPurpose::HandshakeLast,
                completion,
            } => {
                let change = self.connected_step(
                    connected,
                    now,
                    Done::Tx {
                        purpose: TxPurpose::Connected(Sending::DuplicateMessage4),
                        completion,
                    },
                );
                if matches!(change, Change::Keep)
                    && matches!(completion, TxOutcome::Ended(SingleEnd::Delivered { .. }))
                {
                    connected.confirmation = None;
                    connected.connected_notice = true;
                    connected.report = Some(UpstreamFact::Connected(connected.channel));
                }
                change
            }
            done => self.connected_step(connected, now, done),
        }
    }

    /// The connected state after `done`.
    fn connected_step(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        done: StaDone<T>,
    ) -> Change<T, K> {
        if matches!(
            done,
            Done::Tx {
                completion: TxOutcome::Withdrawn(oer_ieee80211_port::WithdrawCause::Disabled),
                ..
            }
        ) {
            return leave_disabled(connected);
        }
        match self.update_connected(connected, now, done) {
            Ok(()) => Change::Keep,
            Err(reason) => leave_connected(connected, reason, None),
        }
    }

    #[inline(never)]
    fn update_connected(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        done: StaDone<T>,
    ) -> Result<(), StaDisconnectReason> {
        if let Some(reason) = connected.rsn.as_ref().and_then(|rsn| rsn.fault) {
            return Err(reason);
        }
        let sent = match done {
            Done::Tx {
                purpose: TxPurpose::Connected(sending),
                completion,
            } => Some((
                sending,
                match completion {
                    TxOutcome::Ended(SingleEnd::Delivered { .. }) => NullOutcome::Delivered,
                    _ => NullOutcome::Unconfirmed,
                },
            )),
            Done::Unsent {
                purpose: TxPurpose::Connected(sending),
                ..
            } => Some((sending, NullOutcome::Unconfirmed)),
            _ => None,
        };
        if let Some((sending, outcome)) = sent {
            connected.sending = false;
            match sending {
                Sending::Wake => {
                    connected.waking = false;
                    connected
                        .power
                        .null_sent(StaPowerManagement::Active, outcome);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitWake
                    {
                        self.next_visit(background);
                    }
                }
                Sending::Sleep => {
                    connected
                        .power
                        .null_sent(StaPowerManagement::PowerSave, outcome);
                    if let Some(background) = &mut connected.background
                        && background.step == BackgroundStep::AwaitSleep
                    {
                        // Not delivered: no readiness; the arbiter cancels
                        // the absence at its start.
                        background.step = if outcome == NullOutcome::Delivered {
                            BackgroundStep::NeedReady
                        } else {
                            BackgroundStep::AwaitStart
                        };
                    }
                }
                Sending::GroupResponse | Sending::DuplicateMessage4 => {
                    if let Some(rsn) = connected.rsn.as_mut() {
                        if matches!(rsn.rekey, Rekey::AwaitSent) {
                            rsn.rekey = Rekey::None;
                        }
                        if matches!(rsn.message4, DuplicateMessage4::AwaitSent) {
                            rsn.message4 = DuplicateMessage4::None;
                        }
                        rsn.advance_deferred()?;
                    }
                }
                Sending::Probe | Sending::SaQuery | Sending::BlockAck => {}
            }
            if let Some(background) = &connected.background
                && background.step == BackgroundStep::NeedAbsence
                && background.skipped >= self.config.skipped_absences
            {
                connected.background = None;
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan {
                        interrupted: Some(BackgroundScanEnd::CancelledAbsences),
                    },
                );
            }
            self.background_done(connected);
            return Ok(());
        }
        let dwell = self.config.dwell;
        if let Some(background) = &mut connected.background {
            background.step = match (background.step, done) {
                (BackgroundStep::AwaitConfigured, Done::Configured) => BackgroundStep::NeedProbe,
                (BackgroundStep::AwaitHome, Done::Configured) => BackgroundStep::NeedWake,
                (BackgroundStep::AwaitWidened, Done::Configured) => match now.checked_add(dwell) {
                    Some(until) => BackgroundStep::HomeProbe { until },
                    None => BackgroundStep::Narrow(Some(BackgroundScanEnd::DeadlineOverflow)),
                },
                (
                    BackgroundStep::HomeProbe { until } | BackgroundStep::HomeListen { until },
                    Done::Tick,
                ) if now >= until => BackgroundStep::Narrow(None),
                (BackgroundStep::AwaitNarrowed(Some(reason)), Done::Configured) => {
                    connected.background = None;
                    self.commands.complete(
                        StaCommand::BackgroundScan,
                        StaCommandOutcome::BackgroundScan {
                            interrupted: Some(reason),
                        },
                    );
                    return Ok(());
                }
                (BackgroundStep::AwaitNarrowed(None), Done::Configured) => {
                    self.next_visit(background);
                    background.step
                }
                (step, _) => step,
            };
        }
        self.background_done(connected);
        Ok(())
    }

    /// The next visit of the background scan, or its end.
    fn next_visit(&mut self, background: &mut Background) {
        let visit = if background.skipped > 0 {
            background.visit
        } else {
            background.visit.saturating_add(1)
        };
        background.visit = visit;
        background.step = BackgroundStep::NeedAbsence;
    }

    /// End a background scan that ran out of channels or of tries.
    fn background_done(&mut self, connected: &mut Connected<T, K>) {
        if let Some(background) = &connected.background
            && background.step == BackgroundStep::NeedAbsence
        {
            if background.skipped >= self.config.skipped_absences {
                connected.background = None;
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan {
                        interrupted: Some(BackgroundScanEnd::CancelledAbsences),
                    },
                );
            } else if self.channel(background.visit).is_none() {
                connected.background = None;
                self.commands.complete(
                    StaCommand::BackgroundScan,
                    StaCommandOutcome::BackgroundScan { interrupted: None },
                );
            }
        }
    }

    /// The scan's visit of the channel at `visit`, or its end.
    fn visit(
        &mut self,
        visit: usize,
        skipped: u8,
        access_point: Option<SupportedChannel<K>>,
    ) -> Change<T, K> {
        match self.channel(visit) {
            Some(_) => Change::Scan(Scan {
                visit,
                skipped,
                step: ScanStep::NeedLease,
                access_point,
            }),
            None => self.select(false, access_point),
        }
    }

    /// The join of the best access point of the network the scan found:
    /// the strongest, unless one on the channel of the access point beside
    /// the station is within the margin of it.
    fn select(
        &mut self,
        partial: bool,
        access_point_channel: Option<SupportedChannel<K>>,
    ) -> Change<T, K> {
        let ssid = self.ssid();
        let policy = self.policy();
        let candidates = || {
            (*self.scan)
                .records()
                .filter(move |record| record.ssid_bytes() == ssid)
                .filter(move |record| select_association_rsn(record, policy).is_ok())
        };
        let Some(strongest) = candidates().max_by_key(|record| record.rssi).copied() else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        let margin = i16::from(self.config.access_point_margin_db);
        let beside = access_point_channel.and_then(|channel| {
            candidates()
                .filter(|record| record.channel == channel.channel().number())
                .filter(|record| {
                    i16::from(record.rssi) >= i16::from(strongest.rssi).saturating_sub(margin)
                })
                .max_by_key(|record| record.rssi)
                .copied()
        });
        let access_point = beside.unwrap_or(strongest);
        let Ok(rsn) = select_association_rsn(&access_point, policy) else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        let Some(channel) = self
            .config
            .channels
            .iter()
            .copied()
            .find(|channel| channel.channel().number() == access_point.channel)
        else {
            return leave_unkeyed(StaDisconnectReason::NotFound { partial });
        };
        Change::Join(Join {
            access_point,
            rsn,
            channel,
            step: JoinStep::NeedLease,
        })
    }

    fn begin_authentication(
        &mut self,
        join: &mut Join<T, K>,
        now: Instant,
    ) -> Result<JoinStep<T::RxMeta>, StaDisconnectReason> {
        let bssid = join.access_point.bssid;
        let key = match &(*self.security) {
            StaSecurity::Open => AssociationKey::Open,
            StaSecurity::Personal { password, pmk } => {
                if let AssociationSecurity::Rsn(RsnAssociation {
                    akm: AssociationAkm::Sae(pwe),
                    ..
                }) = join.rsn.security()
                {
                    if let Some(cached) = self.pmksa.resume(bssid, &self.config.ssid) {
                        join.rsn = join
                            .rsn
                            .with_pmkid(oer_ieee80211_wire::security::Pmkid(cached.pmkid()))
                            .ok_or(StaDisconnectReason::AuthenticationFailed)?;
                        AssociationKey::Protected(cached.pmk().duplicate())
                    } else {
                        let element = match pwe {
                            oer_ieee80211_wire::security::SaePwe::HuntingAndPecking => {
                                SaePasswordElement::hunting_and_pecking(
                                    password.as_bytes(),
                                    self.config.address.get(),
                                    bssid.get(),
                                )
                                .map_err(|error| sae_derivation_failure(error, bssid.get(), pwe))?
                            }
                            oer_ieee80211_wire::security::SaePwe::HashToElement => {
                                SaePasswordToken::derive(
                                    &self.config.ssid,
                                    password.as_bytes(),
                                    None,
                                )
                                .map_err(|error| sae_derivation_failure(error, bssid.get(), pwe))?
                                .password_element(self.config.address.get(), bssid.get())
                            }
                        };
                        let commit = SaeCommit::new(
                            &element,
                            self.config.callbacks.nonce(),
                            self.config.callbacks.nonce(),
                        )
                        .map_err(|error| sae_derivation_failure(error, bssid.get(), pwe))?;
                        let (authentication, transmission) =
                            StaSaeAuthentication::new(self.config.address, bssid, commit, pwe, now)
                                .map_err(StaDisconnectReason::Sae)?;
                        return Ok(JoinStep::SaeSend {
                            authentication,
                            transmission,
                        });
                    }
                } else {
                    AssociationKey::Protected(pmk.duplicate())
                }
            }
        };
        Ok(JoinStep::Authenticate(Authenticate::NeedCredit(
            StaAuthenticationRuntime::new(self.config.address, bssid),
            key,
        )))
    }

    fn sae_event(
        &mut self,
        join: &mut Join<T, K>,
        event: StaSaeEvent,
        now: Instant,
    ) -> Change<T, K> {
        join.step = match event {
            StaSaeEvent::Irrelevant(authentication) => JoinStep::SaeAwait(authentication),
            StaSaeEvent::Transmit {
                authentication,
                transmission,
            } => JoinStep::SaeSend {
                authentication,
                transmission,
            },
            StaSaeEvent::Authenticated(keys) => {
                let bssid = join.access_point.bssid;
                self.pmksa.insert(StaPmksa::new(
                    bssid,
                    self.config.ssid.clone(),
                    keys.pmk.duplicate(),
                    keys.pmkid,
                ));
                let runtime = match StaAssociationRuntime::new(
                    self.config.address,
                    bssid,
                    oer_ieee80211_wire::security::LinkProtection::Ccmp,
                    now,
                ) {
                    Ok(runtime) => runtime,
                    Err(_) => return leave_unkeyed(StaDisconnectReason::DeadlineOverflow),
                };
                JoinStep::Associate(runtime, AssociationKey::Protected(keys.pmk))
            }
            StaSaeEvent::Failed(reason) => return leave_unkeyed(StaDisconnectReason::Sae(reason)),
        };
        Change::Keep
    }

    fn join_step(&mut self, join: &mut Join<T, K>, now: Instant, done: StaDone<T>) -> Change<T, K> {
        join.step = match (
            core::mem::replace(&mut join.step, JoinStep::NeedLease),
            done,
        ) {
            (JoinStep::AwaitConfigured, Done::Configured) => {
                match self.begin_authentication(join, now) {
                    Ok(step) => step,
                    Err(reason) => return leave_unkeyed(reason),
                }
            }
            (
                JoinStep::SaeSend {
                    authentication,
                    transmission,
                },
                Done::Tick,
            ) => {
                if now >= authentication.next_deadline() {
                    return leave_unkeyed(StaDisconnectReason::Sae(StaSaeFailure::Timeout));
                }
                JoinStep::SaeSend {
                    authentication,
                    transmission,
                }
            }
            (JoinStep::SaeAwait(authentication), Done::Tick) => {
                match authentication.on_deadline(now) {
                    StaSaeEvent::Irrelevant(authentication) => JoinStep::SaeAwait(authentication),
                    StaSaeEvent::Failed(reason) => {
                        return leave_unkeyed(StaDisconnectReason::Sae(reason));
                    }
                    event => return self.sae_event(join, event, now),
                }
            }
            (
                JoinStep::Authenticate(Authenticate::Exchange {
                    runtime,
                    mut stage,
                    key,
                }),
                done,
            ) => {
                let timed_out = match (stage, done) {
                    (
                        AuthStage::AwaitSent {
                            attempt,
                            response_timeout,
                        },
                        Done::Tx {
                            purpose: TxPurpose::Authentication { attempt: returned },
                            completion: TxOutcome::Ended(SingleEnd::Delivered { .. }),
                        },
                    ) if attempt == returned => {
                        let Some(until) = now.checked_add(response_timeout) else {
                            return leave_unkeyed(StaDisconnectReason::DeadlineOverflow);
                        };
                        stage = AuthStage::AwaitResponse { until };
                        false
                    }
                    (
                        AuthStage::AwaitSent { attempt, .. },
                        Done::Tx {
                            purpose: TxPurpose::Authentication { attempt: returned },
                            ..
                        },
                    ) if attempt == returned => true,
                    (AuthStage::AwaitResponse { until }, Done::Tick) => now >= until,
                    _ => false,
                };
                let auth = if timed_out {
                    match runtime.response_timed_out() {
                        StaAuthenticationEvent::Retry { runtime, .. } => {
                            Authenticate::NeedCredit(runtime, key)
                        }
                        _ => return leave_unkeyed(StaDisconnectReason::AuthenticationFailed),
                    }
                } else {
                    Authenticate::Exchange {
                        runtime,
                        stage,
                        key,
                    }
                };
                JoinStep::Authenticate(auth)
            }
            (step, _) => step,
        };
        Change::Keep
    }
    fn has_link(&self, state: &State<T, K>) -> bool {
        let State::Connected(connected) = state else {
            return false;
        };
        if self.epoch.is_none()
            || connected.confirmation.is_some()
            || self.absence.is_some()
            || connected
                .rsn
                .as_ref()
                .is_some_and(|rsn| rsn.fault.is_some())
            || connected.power.asleep()
        {
            return false;
        }
        matches!(self.keys.pairwise, KeyCell::Installed(_)) == connected.rsn.as_ref().is_some()
    }

    #[inline(never)]
    fn receive_frame<'frame>(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        frame: &'frame [u8],
        meta: T::RxMeta,
        mut deliver: impl FnMut(oer_ieee80211_wire::data::EthernetFrameParts<'frame>),
    ) -> Result<(), StaRxError> {
        use oer_ieee80211_upper_mac::receive::{AdmittedData, RxDropReason};
        let header = match crate::receive::classify(frame)? {
            Received::Management(management) => {
                self.received_management(state, now, management, meta);
                return Ok(());
            }
            Received::Data(header) => header,
        };
        let (bssid, replay) = match &mut *state {
            State::Handshake(handshake) => (
                handshake.bssid,
                match &mut handshake.step {
                    HandshakeStep::InstallGroup(_, replay, _)
                    | HandshakeStep::AwaitInstalled(_, replay, _) => Some(replay),
                    _ => None,
                },
            ),
            State::Connected(connected) => (
                connected.bssid,
                connected.rsn.as_mut().map(|rsn| &mut rsn.replay),
            ),
            _ => return Err(RxDropReason::NotConnected.into()),
        };
        let admitted = AdmittedData::admit(
            header,
            self.config.address,
            bssid,
            crate::data::integrity(meta.common()),
            replay,
        )?;
        for parts in admitted.msdus()? {
            let parts = parts?;
            if parts.ether_type == EAPOL_ETHER_TYPE {
                if !matches!(*self.security, StaSecurity::Open)
                    && !admitted.protected()
                    && parts.destination != self.config.address.get()
                    && parts.destination != PAE_GROUP_ADDRESS
                {
                    continue;
                }
                let eapol =
                    OwnedEapolFrame::try_copy(RsnInterface::Station, bssid.get(), parts.payload)
                        .map_err(|_| RxDropReason::Payload)?;
                let protection = if admitted.protected() {
                    EapolProtection::Pairwise
                } else {
                    EapolProtection::Open
                };
                let change = match &mut *state {
                    State::Handshake(handshake) => {
                        self.handshake_eapol(handshake, now, eapol, protection)
                    }
                    State::Connected(connected) => {
                        Self::connected_eapol(connected, eapol, protection)?;
                        Change::Keep
                    }
                    _ => Change::Keep,
                };
                state.apply(change);
            } else {
                if !matches!(*self.security, StaSecurity::Open) && !admitted.protected() {
                    return Err(RxDropReason::Protection.into());
                }
                if !self.has_link(state) {
                    return Err(RxDropReason::NotConnected.into());
                }
                deliver(parts);
            }
        }
        Ok(())
    }

    /// Take a received frame.
    #[inline(never)]
    fn received_management(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        management: Management<'_>,
        meta: T::RxMeta,
    ) {
        let frame = management.frame();
        let change = match &mut *state {
            State::Scan(scan)
                if scan.step == ScanStep::Listen || scan.step == ScanStep::NeedProbe =>
            {
                observe_scan(&mut (*self.scan), now, management, &meta);
                Change::Keep
            }
            State::Join(join) => {
                match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
                    JoinStep::SaeAwait(authentication) => {
                        let event = authentication.observe_management_frame(frame, now);
                        self.sae_event(join, event, now)
                    }

                    JoinStep::Authenticate(Authenticate::NeedCredit(runtime, key)) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedCredit(runtime, key));
                        Change::Keep
                    }
                    JoinStep::Authenticate(Authenticate::Exchange {
                        mut runtime,
                        stage,
                        key,
                    }) => {
                        runtime.observe_received_frame();
                        match runtime.observe_management_frame(frame) {
                            StaAuthenticationEvent::Authenticated { .. } => {
                                match StaAssociationRuntime::new(
                                    self.config.address,
                                    join.access_point.bssid,
                                    match *self.security {
                                        StaSecurity::Open => {
                                            oer_ieee80211_wire::security::LinkProtection::Open
                                        }
                                        StaSecurity::Personal { .. } => {
                                            oer_ieee80211_wire::security::LinkProtection::Ccmp
                                        }
                                    },
                                    now,
                                ) {
                                    Ok(runtime) => {
                                        join.step = JoinStep::Associate(runtime, key);
                                        Change::Keep
                                    }
                                    Err(_) => leave_unkeyed(StaDisconnectReason::DeadlineOverflow),
                                }
                            }
                            StaAuthenticationEvent::Retry { runtime, .. } => {
                                join.step =
                                    JoinStep::Authenticate(Authenticate::NeedCredit(runtime, key));
                                Change::Keep
                            }
                            StaAuthenticationEvent::Failed { .. } => {
                                leave_unkeyed(StaDisconnectReason::AuthenticationFailed)
                            }
                            StaAuthenticationEvent::Irrelevant(runtime) => {
                                join.step = JoinStep::Authenticate(Authenticate::Exchange {
                                    runtime,
                                    stage,
                                    key,
                                });
                                Change::Keep
                            }
                        }
                    }
                    JoinStep::Associate(mut runtime, key) => {
                        runtime.observe_received_frame();
                        match runtime.observe_management_frame(frame, now) {
                            StaAssociationEvent::Associated { response, .. } => {
                                match StaAssociatedPeer::derive(
                                    &join.access_point,
                                    &response,
                                    self.config.phy,
                                    |capability| {
                                        self.config.callbacks.he_packet_padding(capability)
                                    },
                                ) {
                                    Ok(associated) => {
                                        join.step = JoinStep::NotifyAssociated {
                                            association_id: response.association_id,
                                            key,
                                            // The rate selection starts from the
                                            // response's metadata.
                                            peer: Peer {
                                                associated,
                                                response: meta,
                                            },
                                        };
                                        Change::Keep
                                    }
                                    Err(_) => leave_unkeyed(StaDisconnectReason::AssociationFailed),
                                }
                            }
                            StaAssociationEvent::Failed { failure, .. } => {
                                leave_unkeyed(association_failure(failure))
                            }
                            StaAssociationEvent::Irrelevant(runtime) => {
                                join.step = JoinStep::Associate(runtime, key);
                                Change::Keep
                            }
                        }
                    }
                    step => {
                        join.step = step;
                        Change::Keep
                    }
                }
            }
            State::Handshake(handshake) => {
                let protected = match handshake.supplicant() {
                    Ok(supplicant) => supplicant.management_protection(),
                    Err(_) => {
                        let change = leave_handshake(handshake, REASON_UNSPECIFIED);
                        state.apply(change);
                        return;
                    }
                };
                let disconnect = if protected {
                    handshake.management.as_mut().and_then(|management| {
                        match management.receive(frame, self.config.address, handshake.bssid) {
                            Some(ManagementEvent::Disconnect(reason)) => Some(reason),
                            _ => None,
                        }
                    })
                } else {
                    peer_disconnect(frame, self.config.address, handshake.bssid)
                };
                match disconnect {
                    Some(reason_code) => {
                        leave_keyed(StaDisconnectReason::Peer { reason_code }, None)
                    }
                    None => Change::Keep,
                }
            }
            State::Connected(connected) => {
                self.connected_received(connected, now, management, &meta)
            }
            _ => Change::Keep,
        };
        state.apply(change);
    }

    /// The connected state after a received frame.
    #[inline(never)]
    fn connected_received(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        management: Management<'_>,
        meta: &T::RxMeta,
    ) -> Change<T, K> {
        let frame = management.frame();
        let disconnect = match connected
            .rsn
            .as_mut()
            .and_then(|rsn| rsn.management.as_mut())
        {
            Some(management) => {
                match management.receive(frame, self.config.address, connected.bssid) {
                    Some(ManagementEvent::Disconnect(reason)) => Some(reason),
                    Some(ManagementEvent::Query(query)) => {
                        management.received_query(query);
                        None
                    }
                    Some(ManagementEvent::BlockAck(action)) => {
                        connected.block_ack = Some(action);
                        None
                    }
                    None => {
                        if matches!(
                            peer_disconnect(frame, self.config.address, connected.bssid),
                            Some(6 | 7)
                        ) && management
                            .start_query(now, || self.config.callbacks.nonce())
                            .is_err()
                        {
                            return leave_connected(
                                connected,
                                StaDisconnectReason::DeadlineOverflow,
                                Some(REASON_UNSPECIFIED),
                            );
                        }
                        None
                    }
                }
            }
            None => {
                if let Some(action) =
                    block_ack::unprotected_action(management, self.config.address, connected.bssid)
                {
                    connected.block_ack = Some(action);
                }
                peer_disconnect(frame, self.config.address, connected.bssid)
            }
        };
        if let Some(reason_code) = disconnect {
            // The access point ended the link: no Deauthentication back.
            return leave_connected(connected, StaDisconnectReason::Peer { reason_code }, None);
        }
        let step = connected
            .background
            .as_ref()
            .map(|background| background.step);
        let away = matches!(
            step,
            Some(BackgroundStep::NeedProbe | BackgroundStep::Listen)
        );
        let home = matches!(
            step,
            Some(BackgroundStep::HomeProbe { .. } | BackgroundStep::HomeListen { .. })
        );
        if away || home {
            observe_scan(&mut (*self.scan), now, management, meta);
            if away {
                return Change::Keep;
            }
        }
        if let Ok(beacon) = parse_sta_beacon(frame, connected.bssid.get(), connected.association_id)
        {
            if let Some(edca) = beacon.edca
                && edca != connected.peer.associated.edca
            {
                connected.peer.associated.edca = edca;
                connected.edca = true;
            }
            if connected.power.beacon(beacon.tim) == PowerSaveDecision::SendActive
                && !connected.waking
            {
                connected.wake = true;
            }
        }
        Change::Keep
    }

    /// A checked peer-input drop is reported at RX; it is not a lifecycle
    /// refusal and does not discard a previously authenticated successor.
    #[inline(never)]
    fn connected_eapol(
        connected: &mut Connected<T, K>,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
    ) -> Result<(), StaRxError> {
        let Some(rsn) = connected.rsn.as_mut() else {
            return Err(oer_ieee80211_upper_mac::receive::RxDropReason::Protection.into());
        };
        rsn.receive(eapol, protection)
    }

    fn configure(&mut self, receive: StationReceive<K>) -> Option<Owed<'id, T, K>> {
        let place = self.configuration.take()?;
        let epoch = if matches!(receive, StationReceive::Off) {
            None
        } else {
            *self.epoch
        };
        Some(Owed::Configure(
            receive,
            place.with_context(StaConfigurationContext { epoch }),
        ))
    }

    /// The action `state` owes, and the state that awaits its end.
    #[inline(never)]
    fn owe(&mut self, state: &mut State<T, K>, now: Instant) -> ActionStep<'id, T, K> {
        match state {
            State::Reclaiming(reclaim) => self.owe_reclaim(*reclaim),
            State::Report(reason) => {
                (*self.epoch) = None;
                (
                    Change::Idle,
                    ActionProgress::Action(Owed::Notify(StaNotice::Disconnected(*reason))),
                )
            }
            State::Scan(scan) => {
                let owed = match scan.step {
                    ScanStep::NeedLease if self.place.is_some() => {
                        match (self.channel(scan.visit), self.place.take()) {
                            (Some(channel), Some(place)) => {
                                scan.step = ScanStep::AwaitAnswer;
                                Some(Owed::Request(
                                    ChannelRequest {
                                        channel,
                                        purpose: Purpose::Scan {
                                            dwell: self.config.dwell,
                                        },
                                    },
                                    place,
                                ))
                            }
                            (_, place) => {
                                (*self.place) = place;
                                None
                            }
                        }
                    }
                    ScanStep::NeedConfigure if self.configuration.is_some() => {
                        scan.step = ScanStep::AwaitConfigured;
                        self.configure(StationReceive::OtherBssManagement)
                    }
                    ScanStep::NeedProbe => match self.encode_probe(TxPurpose::Probe) {
                        Ok(transmit) => {
                            scan.step = ScanStep::Listen;
                            Some(Owed::Transmit(transmit))
                        }
                        Err(Unsent::NoTokens) => None,
                        Err(Unsent::Encode) => {
                            return (
                                leave_unkeyed(StaDisconnectReason::Encode),
                                ActionProgress::Transition,
                            );
                        }
                        Err(Unsent::NoConnection) => {
                            return (
                                leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                    TxRefusal::LinkChanged,
                                )),
                                ActionProgress::Transition,
                            );
                        }
                    },
                    _ => None,
                };
                (Change::Keep, owed.into())
            }
            State::Join(join) => self.owe_join(join, now),
            State::Handshake(handshake) => self.owe_handshake(handshake),
            State::Connected(connected) => {
                let purpose = if connected.confirmation.is_some() {
                    TxPurpose::HandshakeLast
                } else {
                    TxPurpose::Connected(Sending::DuplicateMessage4)
                };
                self.owe_connected(connected, now, purpose)
            }
            State::Leave(leave) => self.owe_leave(leave),
            _ => (Change::Keep, ActionProgress::Wait),
        }
    }

    /// A restarted installation reclaims old keys before its receive-off
    /// terminal opens admission for a new connection.
    fn owe_reclaim(&mut self, reclaim: Reclaim) -> ActionStep<'id, T, K> {
        match reclaim {
            Reclaim::Keys => {
                if let Some(removal) = self.group_removal(false) {
                    (
                        Change::Reclaiming(Reclaim::Keys),
                        ActionProgress::Action(removal),
                    )
                } else if self.keys.group_slots.is_full()
                    && let Some(key) = self.keys.take_pairwise_key()
                {
                    (
                        Change::Reclaiming(Reclaim::Keys),
                        ActionProgress::Action(Owed::RemovePairwise(key)),
                    )
                } else if self.keys.clear() {
                    (
                        Change::Reclaiming(Reclaim::Configure),
                        ActionProgress::Transition,
                    )
                } else {
                    (Change::Reclaiming(Reclaim::Keys), ActionProgress::Wait)
                }
            }
            Reclaim::Configure => match self.configure(StationReceive::Off) {
                Some(owed) => (
                    Change::Reclaiming(Reclaim::AwaitConfigured),
                    ActionProgress::Action(owed),
                ),
                None => (Change::Reclaiming(Reclaim::Configure), ActionProgress::Wait),
            },
            Reclaim::AwaitConfigured => (Change::Reclaiming(reclaim), ActionProgress::Wait),
        }
    }

    /// The action the leave owes.
    fn owe_leave(&mut self, leave: &mut Leave) -> ActionStep<'id, T, K> {
        let owed = match leave.step {
            LeaveStep::AwaitAbsenceEnd(deauthentication) if self.absence.is_none() => {
                leave.step = leave_key_step(deauthentication);
                return (Change::Keep, ActionProgress::Transition);
            }
            LeaveStep::RemoveKeys(deauthentication) => {
                if let Some(removal) = self.group_removal(false) {
                    Some(removal)
                } else if self.keys.group_slots.is_full()
                    && let Some(key) = self.keys.take_pairwise_key()
                {
                    Some(Owed::RemovePairwise(key))
                } else if self.keys.clear() {
                    // Every key is back: the next step owes the action.
                    leave.step = match deauthentication {
                        Some(deauthentication) => LeaveStep::NeedDeauthentication(deauthentication),
                        None => LeaveStep::NeedConfigure,
                    };
                    return (Change::Keep, ActionProgress::Transition);
                } else {
                    None
                }
            }
            LeaveStep::NeedDeauthentication(Deauthentication {
                bssid,
                reason_code,
                protected,
            }) => {
                if protected && !matches!(self.keys.pairwise, KeyCell::Installed(_)) {
                    return (Change::Keep, ActionProgress::Wait);
                }
                match self.encode_management(
                    StaManagementSubtype::Deauthentication,
                    bssid,
                    &reason_code.to_le_bytes(),
                    TxPurpose::Deauthentication,
                    protected,
                ) {
                    Ok(transmit) => {
                        if protected {
                            leave.step = LeaveStep::AwaitProtectedDeauthentication;
                            Some(Owed::TransmitProtected(transmit))
                        } else {
                            leave.step = LeaveStep::AwaitDeauthentication;
                            Some(Owed::Transmit(transmit))
                        }
                    }
                    Err(Unsent::NoTokens) => None,
                    Err(Unsent::Encode | Unsent::NoConnection) => {
                        leave.step = if protected {
                            LeaveStep::RemoveKeys(None)
                        } else {
                            LeaveStep::NeedConfigure
                        };
                        return (Change::Keep, ActionProgress::Transition);
                    }
                }
            }
            LeaveStep::NeedConfigure if self.configuration.is_some() => {
                leave.step = LeaveStep::AwaitConfigured;
                self.configure(StationReceive::Off)
            }
            LeaveStep::NeedRelease => match core::mem::replace(&mut (*self.lease), Held::None) {
                Held::Lease(lease) => {
                    (*self.lease) = Held::Released(lease.id());
                    leave.step = LeaveStep::AwaitEnd;
                    Some(Owed::Release(lease))
                }
                // No lease to give back: done.
                held => {
                    (*self.lease) = held;
                    return (self.report(leave), ActionProgress::Transition);
                }
            },
            _ => None,
        };
        (Change::Keep, owed.into())
    }

    /// The handshake's state after a received frame.
    #[inline(never)]
    fn handshake_eapol(
        &mut self,
        handshake: &mut Handshake<T, K>,
        now: Instant,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
    ) -> Change<T, K> {
        if !matches!(
            handshake.step,
            HandshakeStep::Await | HandshakeStep::AwaitSent
        ) {
            return Change::Keep;
        }
        let received = match handshake.supplicant.as_mut() {
            Some(supplicant) => supplicant.on_frame(eapol, &handshake.pmk),
            None => Err(RsnStaSupplicantError::UnexpectedAction),
        };
        let action = match received {
            // Message 3's key data, unwrapped in the step: a few AES blocks.
            Ok(RsnStaSupplicantAction::UnwrapKeyData(request)) => {
                let wrapped = match request.wrapped_key_data() {
                    Ok(wrapped) => wrapped,
                    Err(error) => {
                        return leave_keyed(StaDisconnectReason::InvalidEapol(error), None);
                    }
                };
                let unwrapped = software_aes128_key_unwrap(request.kek(), wrapped);
                match handshake
                    .supplicant()
                    .map_err(oer_ieee80211_rsn::supplicant::RsnStaProcessError::from)
                    .and_then(|supplicant| supplicant.complete_key_data_unwrap(request, unwrapped))
                {
                    Ok(action) => action,
                    Err(oer_ieee80211_rsn::supplicant::RsnStaProcessError::Supplicant(
                        RsnStaSupplicantError::RetainedSecurityIes(error),
                    )) => {
                        return leave_keyed(StaDisconnectReason::RetainedSecurityIes(error), None);
                    }
                    Err(oer_ieee80211_rsn::supplicant::RsnStaProcessError::Supplicant(
                        RsnStaSupplicantError::InvalidFrame(error),
                    )) => return leave_keyed(StaDisconnectReason::InvalidEapol(error), None),
                    Err(_) => return leave_handshake(handshake, REASON_UNSPECIFIED),
                }
            }
            Ok(RsnStaSupplicantAction::Ready(action)) => action,
            Err(RsnStaSupplicantError::RetainedSecurityIes(error)) => {
                return leave_keyed(StaDisconnectReason::RetainedSecurityIes(error), None);
            }
            Err(RsnStaSupplicantError::InvalidFrame(error)) => {
                return leave_keyed(StaDisconnectReason::InvalidEapol(error), None);
            }
            // A peer frame the supplicant rejects changes nothing.
            Err(error) if error.is_peer_input_rejection() => RsnStaReady::None,
            Err(_) => return leave_handshake(handshake, REASON_UNSPECIFIED),
        };
        match action {
            RsnStaReady::Transmit(message) => {
                let Some(deadline) =
                    RsnStaResponseDeadline::start(RsnStaResponseWait::Message3, now)
                else {
                    return leave_keyed(
                        StaDisconnectReason::DeadlineOverflow,
                        Some(Deauthentication {
                            bssid: handshake.bssid,
                            reason_code: REASON_UNSPECIFIED,
                            protected: false,
                        }),
                    );
                };
                handshake.step = HandshakeStep::Send(message);
                handshake.deadline = deadline;
            }
            RsnStaReady::InstallKeys(request) => {
                handshake.step = HandshakeStep::Install(request, protection);
            }
            RsnStaReady::RetryInstall(_) | RsnStaReady::Deauthenticate => {
                return leave_handshake(handshake, REASON_UNSPECIFIED);
            }
            RsnStaReady::None => {}
        }
        Change::Keep
    }

    /// The action the handshake owes.
    #[inline(never)]
    fn owe_handshake(&mut self, handshake: &mut Handshake<T, K>) -> ActionStep<'id, T, K> {
        let (step, owed) = match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
            HandshakeStep::Send(message) => {
                match self.encode_eapol(
                    handshake.bssid,
                    message.as_bytes(),
                    TxPurpose::HandshakeResponse,
                ) {
                    Ok(transmit) => (HandshakeStep::AwaitSent, Some(Owed::Transmit(transmit))),
                    Err(Unsent::NoTokens) => (HandshakeStep::Send(message), None),
                    Err(Unsent::Encode | Unsent::NoConnection) => {
                        return (
                            leave_handshake(handshake, REASON_UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            HandshakeStep::Install(request, protection) => {
                let protected = match handshake.supplicant() {
                    Ok(supplicant) => supplicant.management_protection(),
                    Err(_) => {
                        return (
                            leave_handshake(handshake, REASON_UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                };
                let Ok(replay) =
                    CcmpRxReplayEpoch::pairwise(*request.pairwise().receive_sequence())
                else {
                    return (
                        leave_handshake(handshake, REASON_UNSPECIFIED),
                        ActionProgress::Transition,
                    );
                };
                let peer = handshake.bssid.individual();
                match (*self.epoch)
                    .and_then(|epoch| self.keys.take_pairwise_slot(epoch, handshake.spp))
                {
                    Some((association, slot, pending)) => {
                        let (pairwise, group) = request.into_parts();
                        let key = pairwise.into_key();
                        if protected && handshake.management.is_none() {
                            handshake.management =
                                Some(ManagementProtection::new(Ccmp128Key::new(key.as_bytes())));
                        }
                        (
                            HandshakeStep::InstallGroup(group, replay, protection),
                            Some(Owed::InstallPairwise {
                                association,
                                pending,
                                slot,
                                peer,
                                key,
                            }),
                        )
                    }
                    None => (HandshakeStep::Install(request, protection), None),
                }
            }
            HandshakeStep::InstallGroup(request, replay, protection)
                if !matches!(self.keys.pairwise, KeyCell::Installed(_)) =>
            {
                // PairwiseKeyInstalled returns the sole association owner.
                // Its typed response unlocks GTK installation; D4 is unchanged.
                (
                    HandshakeStep::InstallGroup(request, replay, protection),
                    None,
                )
            }
            HandshakeStep::InstallGroup(request, replay, protection) => {
                match self.begin_group_change(request.group()) {
                    Ok(ActionProgress::Transition) => {
                        return (
                            finish_handshake_install(handshake, request, replay, protection),
                            ActionProgress::Transition,
                        );
                    }
                    Ok(ActionProgress::Action(owed)) => (
                        HandshakeStep::AwaitInstalled(request, replay, protection),
                        Some(owed),
                    ),
                    Ok(ActionProgress::Wait) => (
                        HandshakeStep::InstallGroup(request, replay, protection),
                        None,
                    ),
                    Err(()) => {
                        return (
                            leave_handshake(handshake, REASON_UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            step => (step, None),
        };
        handshake.step = step;
        (Change::Keep, owed.into())
    }

    /// An EAPOL frame to `bssid` in a data frame, in a buffer.
    fn encode_eapol(
        &mut self,
        bssid: Bssid,
        eapol: &[u8],
        purpose: TxPurpose,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        Ok(self.encode_reserved(
            credit,
            |frame| {
                oer_ieee80211_wire::station::StaDataFrame {
                    source: address,
                    bssid,
                    destination: bssid.get(),
                    sequence_number,
                    ether_type: EAPOL_ETHER_TYPE,
                    payload: eapol,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Connection(ConnectionFrame::Eapol),
            purpose,
            StaRatePlan::Management,
        )?)
    }

    /// The action the connected state owes.
    #[inline(never)]
    fn owe_connected(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        pairwise_purpose: TxPurpose,
    ) -> ActionStep<'id, T, K> {
        if let Some(reason) = connected.rsn.as_ref().and_then(|rsn| rsn.fault) {
            return (
                leave_connected(connected, reason, None),
                ActionProgress::Transition,
            );
        }
        if connected
            .rsn
            .as_ref()
            .and_then(|rsn| rsn.management.as_ref())
            .is_some_and(|management| management.timed_out(now))
        {
            return (
                leave_connected(
                    connected,
                    StaDisconnectReason::SaQueryTimeout,
                    Some(REASON_UNSPECIFIED),
                ),
                ActionProgress::Transition,
            );
        }
        if let Some(action) = connected.block_ack.take()
            && let Some(epoch) = (*self.epoch)
        {
            let request = PendingRxBlockAck::new(
                epoch,
                connected.bssid,
                action,
                connected.qos && connected.peer.associated.phy != PhyMode::Legacy,
            );
            return (
                Change::Keep,
                ActionProgress::Action(Owed::BlockAck(request)),
            );
        }
        if connected.connected_notice {
            connected.connected_notice = false;
            let notice = StaNotice::Connected {
                bssid: connected.bssid,
                channel: connected.channel.channel().number(),
            };
            return (Change::Keep, ActionProgress::Action(Owed::Notify(notice)));
        }
        if let Some(fact) = connected.report.take() {
            return (Change::Keep, ActionProgress::Action(Owed::Upstream(fact)));
        }
        if core::mem::take(&mut connected.edca)
            && let Some(edca) = SupportedEdca::new(oer_ieee80211_port::EdcaParameterSet::from_wmm(
                connected.peer.associated.edca,
            ))
        {
            return (Change::Keep, ActionProgress::Action(Owed::SetEdca(edca)));
        }
        if connected.exchange {
            match core::mem::replace(&mut (*self.lease), Held::None) {
                Held::Lease(lease) => {
                    connected.exchange = false;
                    return (Change::Keep, ActionProgress::Action(Owed::Exchange(lease)));
                }
                held => (*self.lease) = held,
            }
        }
        if let Some(owed) = self
            .owe_sa_query(connected, now)
            .or_else(|| self.owe_duplicate_message4(connected, pairwise_purpose))
            .or_else(|| self.owe_rekey(connected, pairwise_purpose))
        {
            return match owed {
                Ok(owed) => (Change::Keep, ActionProgress::Action(owed)),
                Err(reason) => (
                    leave_connected(connected, reason, Some(REASON_UNSPECIFIED)),
                    ActionProgress::Transition,
                ),
            };
        }
        match self.owe_background(connected, now) {
            Ok(progress) => (Change::Keep, progress),
            Err(reason) => (
                leave_connected(connected, reason, Some(REASON_UNSPECIFIED)),
                ActionProgress::Transition,
            ),
        }
    }

    fn owe_sa_query(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        if connected.sending || !self.link_up(&connected.power, &(*self.keys)) {
            return None;
        }
        let management = connected.rsn.as_mut()?.management.as_mut()?;
        let query = management.pending(now)?;
        match self.encode_management(
            StaManagementSubtype::Action,
            connected.bssid,
            &query.encode(),
            TxPurpose::Connected(Sending::SaQuery),
            true,
        ) {
            Ok(transmit) => {
                management.submitted(query);
                connected.sending = true;
                Some(Ok(Owed::TransmitProtected(transmit)))
            }
            Err(Unsent::NoTokens) => None,
            Err(Unsent::Encode) => Some(Err(StaDisconnectReason::Encode)),
            Err(Unsent::NoConnection) => Some(Err(StaDisconnectReason::TransmitRefused(
                TxRefusal::LinkChanged,
            ))),
        }
    }

    fn owe_duplicate_message4(
        &mut self,
        connected: &mut Connected<T, K>,
        purpose: TxPurpose,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        if connected.sending || !self.link_up(&connected.power, &(*self.keys)) {
            return None;
        }
        let rsn = connected.rsn.as_mut()?;
        let DuplicateMessage4::Send(message) = &rsn.message4 else {
            return None;
        };
        let qos = connected.qos.then_some(&mut connected.qos_sequences);
        match self.encode_eapol_response(connected.bssid, qos, message, purpose) {
            Ok(owed) => {
                rsn.message4 = DuplicateMessage4::AwaitSent;
                connected.sending = true;
                Some(Ok(owed))
            }
            Err(Unsent::NoTokens) => None,
            Err(Unsent::Encode) => Some(Err(StaDisconnectReason::Encode)),
            Err(Unsent::NoConnection) => Some(Err(StaDisconnectReason::TransmitRefused(
                TxRefusal::LinkChanged,
            ))),
        }
    }

    fn encode_eapol_response(
        &mut self,
        bssid: Bssid,
        qos: Option<&mut [StaSequenceCounter; 8]>,
        response: &EapolResponse,
        purpose: TxPurpose,
    ) -> Result<Owed<'id, T, K>, Unsent> {
        match response.protection {
            EapolProtection::Open => self
                .encode_eapol(bssid, response.frame.as_bytes(), purpose)
                .map(Owed::Transmit),
            EapolProtection::Pairwise => self
                .encode_protected_eapol(bssid, qos, response.frame.as_bytes(), purpose)
                .map(Owed::TransmitProtected),
        }
    }

    /// The action of a group key's handshake, if one is owed; a failure's
    /// reason to leave.
    fn begin_group_change(
        &mut self,
        request: &RsnKeyInstall,
    ) -> Result<ActionProgress<Owed<'id, T, K>>, ()> {
        let (key_id, material) = group_key(request).ok_or(())?;
        if self
            .keys
            .groups
            .current_material(key_id, material.as_bytes())
        {
            return Ok(ActionProgress::Transition);
        }
        let Some(epoch) = *self.epoch else {
            return Ok(ActionProgress::Wait);
        };
        let Some(permission) = self.keys.group_permission.take() else {
            return Ok(ActionProgress::Wait);
        };
        if let Some(InstalledGroup {
            place,
            key_id,
            material,
            key,
            replay,
        }) = self.keys.groups.take_matching(key_id, material.as_bytes())
        {
            let install = PendingGroupKey {
                epoch,
                key_id,
                material,
                place,
                replay,
            };
            return Ok(ActionProgress::Action(Owed::SelectGroup {
                pending: PendingGroupSelection { install },
                installed: key,
                permission,
            }));
        }
        let replay = match oer_ieee80211_upper_mac::ccmp::CcmpRxReplayState::from_receive_sequence(
            *request.receive_sequence(),
        ) {
            Ok(replay) => GroupReplay::Retained(replay),
            Err(_) => {
                self.keys.group_permission = Some(permission);
                return Err(());
            }
        };
        if let Some(slot) = self.keys.group_slots.free.take() {
            if let Some(place) = self.keys.groups.reserve() {
                return Ok(ActionProgress::Action(Owed::InstallGroup {
                    pending: PendingGroupKey {
                        epoch,
                        key_id,
                        material: Ccmp128Key::new(material.as_bytes()),
                        place,
                        replay,
                    },
                    slot,
                    key_id,
                    key: material,
                    permission,
                }));
            }
            self.keys.group_slots.free.put(slot);
        }
        let Some(InstalledGroup {
            place,
            key_id: previous_id,
            key: installed,
            ..
        }) = self.keys.groups.replace()
        else {
            self.keys.group_permission = Some(permission);
            return Ok(ActionProgress::Wait);
        };
        Ok(ActionProgress::Action(Owed::ReplaceGroup {
            pending: PendingGroupReplacement {
                install: PendingGroupKey {
                    epoch,
                    key_id,
                    material: Ccmp128Key::new(material.as_bytes()),
                    place,
                    replay,
                },
                previous_id,
            },
            installed,
            key_id,
            key: material,
            permission,
        }))
    }

    fn group_removal(&mut self, retiring: bool) -> Option<Owed<'id, T, K>> {
        let (
            permission,
            InstalledGroup {
                place,
                key_id,
                material,
                key,
                replay,
            },
        ) = self.keys.take_group_key(retiring)?;
        Some(Owed::RemoveGroup {
            pending: PendingGroupRemoval {
                place,
                key_id,
                _material: material,
                replay,
                installation: core::marker::PhantomData,
            },
            key,
            permission,
        })
    }

    #[inline(never)]
    fn owe_rekey(
        &mut self,
        connected: &mut Connected<T, K>,
        pairwise_purpose: TxPurpose,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        // Only a completed install repeats; it becomes Send, so there are at most two iterations.
        loop {
            let rsn = connected.rsn.as_mut()?;
            let (rekey, owed) = match core::mem::replace(&mut rsn.rekey, Rekey::None) {
                Rekey::Install(request, protection) => {
                    match self.begin_group_change(request.group()) {
                        Ok(ActionProgress::Transition) => {
                            if let Err(reason) = rsn.complete_install(request, protection) {
                                return Some(Err(reason));
                            }
                            continue;
                        }
                        Ok(ActionProgress::Action(owed)) => {
                            (Rekey::AwaitInstalled(request, protection), Some(owed))
                        }
                        Ok(ActionProgress::Wait) => (Rekey::Install(request, protection), None),
                        Err(()) => return Some(Err(StaDisconnectReason::HandshakeFailed)),
                    }
                }
                Rekey::Send(message)
                    if !connected.sending && self.link_up(&connected.power, &(*self.keys)) =>
                {
                    let message_kind = match message.frame.key_frame() {
                        Ok(frame) => frame.message(),
                        Err(error) => return Some(Err(StaDisconnectReason::InvalidEapol(error))),
                    };
                    let qos = connected.qos.then_some(&mut connected.qos_sequences);
                    match self.encode_eapol_response(
                        connected.bssid,
                        qos,
                        &message,
                        if message_kind == oer_ieee80211_rsn::EapolKeyMessage::PairwiseMessage4 {
                            pairwise_purpose
                        } else {
                            TxPurpose::Connected(Sending::GroupResponse)
                        },
                    ) {
                        Ok(owed) => {
                            connected.sending = true;
                            (Rekey::AwaitSent, Some(owed))
                        }
                        Err(Unsent::NoTokens) => (Rekey::Send(message), None),
                        Err(Unsent::Encode) => return Some(Err(StaDisconnectReason::Encode)),
                        Err(Unsent::NoConnection) => {
                            return Some(Err(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )));
                        }
                    }
                }
                rekey => (rekey, None),
            };
            rsn.rekey = rekey;
            return owed.map(Ok);
        }
    }

    /// The background scan's action, or a Null the access point is owed.
    fn owe_background(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
    ) -> Result<ActionProgress<Owed<'id, T, K>>, StaDisconnectReason> {
        // Admission and action polling can occur at different instants. Validate
        // again before changing receive rules or handing an absence to the arbiter.
        if connected
            .background
            .as_ref()
            .is_some_and(|background| background.step == BackgroundStep::NeedAbsence)
            && now.checked_add(self.config.dwell).is_none()
        {
            connected.background = None;
            self.commands.complete(
                StaCommand::BackgroundScan,
                StaCommandOutcome::BackgroundScan {
                    interrupted: Some(BackgroundScanEnd::DeadlineOverflow),
                },
            );
            return Ok(ActionProgress::Transition);
        }
        let bssid = connected.bssid;
        let home = connected.channel;
        let absence_id = self.absence.as_ref().map(Lease::id);
        let mut owed = None;
        let mut power = None;
        if let Some(background) = &mut connected.background {
            let step = background.step;
            background.step = match step {
                BackgroundStep::NeedAbsence => match self.channel(background.visit) {
                    // The network's own channel: no absence.
                    Some(channel) if channel == home => {
                        owed = self.configure(bss_rules(bssid, Rules::<K>::WIDENED));
                        if owed.is_some() {
                            BackgroundStep::AwaitWidened
                        } else {
                            step
                        }
                    }
                    Some(channel)
                        if self.absence_place.is_some()
                            && matches!(*self.lease, Held::Lease(_)) =>
                    {
                        owed = self.absence_place.take().map(|place| {
                            Owed::Absence(
                                ChannelRequest {
                                    channel,
                                    purpose: Purpose::Scan {
                                        dwell: self.config.dwell,
                                    },
                                },
                                place,
                            )
                        });
                        BackgroundStep::AwaitAnswer
                    }
                    _ => step,
                },
                BackgroundStep::NeedSleep if !connected.sending => {
                    power = Some((StaPowerManagement::PowerSave, Sending::Sleep));
                    BackgroundStep::AwaitSleep
                }
                BackgroundStep::NeedReady => {
                    owed = absence_id.map(Owed::Ready);
                    BackgroundStep::AwaitStart
                }
                BackgroundStep::NeedConfigure if self.configuration.is_some() => {
                    owed = self.configure(StationReceive::OtherBssManagement);
                    BackgroundStep::AwaitConfigured
                }
                BackgroundStep::NeedProbe if !connected.sending => {
                    owed = self.probe(&mut connected.sending)?;
                    if owed.is_some() {
                        BackgroundStep::Listen
                    } else {
                        BackgroundStep::NeedProbe
                    }
                }
                BackgroundStep::NeedHome if self.configuration.is_some() => {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitHome
                }
                BackgroundStep::NeedWake if !connected.sending => {
                    power = Some((StaPowerManagement::Active, Sending::Wake));
                    BackgroundStep::AwaitWake
                }
                BackgroundStep::HomeProbe { until } if !connected.sending => {
                    owed = self.probe(&mut connected.sending)?;
                    if owed.is_some() {
                        BackgroundStep::HomeListen { until }
                    } else {
                        BackgroundStep::HomeProbe { until }
                    }
                }
                BackgroundStep::Narrow(reason) if self.configuration.is_some() => {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitNarrowed(reason)
                }
                step => step,
            };
        } else if connected.wake && !connected.power.asleep() && !connected.sending {
            // The access point buffers for an awake station: PM=0 again.
            connected.wake = false;
            power = Some((StaPowerManagement::Active, Sending::Wake));
        }
        if let Some((power_management, sending)) = power {
            match self.encode_null(bssid, power_management) {
                Ok(transmit) => {
                    connected.sending = true;
                    connected.waking = sending == Sending::Wake;
                    owed = Some(Owed::Transmit(transmit));
                }
                // No token now: the step waits for the next poll.
                Err(Unsent::NoTokens) => {
                    if let Some(background) = &mut connected.background {
                        background.step = match background.step {
                            BackgroundStep::AwaitSleep => BackgroundStep::NeedSleep,
                            BackgroundStep::AwaitWake => BackgroundStep::NeedWake,
                            step => step,
                        };
                    } else {
                        connected.wake = true;
                    }
                }
                Err(Unsent::Encode) => return Err(StaDisconnectReason::Encode),
                Err(Unsent::NoConnection) => {
                    return Err(StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged));
                }
            }
        }
        Ok(owed.into())
    }

    /// A Probe Request of the background scan, marked in flight.
    fn probe(
        &mut self,
        sending: &mut bool,
    ) -> Result<Option<Owed<'id, T, K>>, StaDisconnectReason> {
        match self.encode_probe(TxPurpose::Connected(Sending::Probe)) {
            Ok(transmit) => {
                *sending = true;
                Ok(Some(Owed::Transmit(transmit)))
            }
            Err(Unsent::NoTokens) => Ok(None),
            Err(Unsent::Encode) => Err(StaDisconnectReason::Encode),
            Err(Unsent::NoConnection) => {
                Err(StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged))
            }
        }
    }

    /// A Null to `bssid` with `power_management`, in a buffer.
    fn encode_null(
        &mut self,
        bssid: Bssid,
        power_management: StaPowerManagement,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        let transmit = self.encode_reserved(
            credit,
            |frame| {
                StaNullDataFrame {
                    station_address: address,
                    bssid,
                    sequence_number,
                    power_management,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Normal,
            TxPurpose::Connected(match power_management {
                StaPowerManagement::Active => Sending::Wake,
                StaPowerManagement::PowerSave => Sending::Sleep,
            }),
            StaRatePlan::Null,
        )?;
        Ok(transmit)
    }

    #[inline(never)]
    fn owe_join(&mut self, join: &mut Join<T, K>, now: Instant) -> ActionStep<'id, T, K> {
        let connect = ChannelRequest {
            channel: join.channel,
            purpose: Purpose::Connect,
        };
        let (step, owed) = match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
            JoinStep::RetryAt(at) if now >= at => match self.place.take() {
                Some(place) => (JoinStep::AwaitAnswer, Some(Owed::Request(connect, place))),
                None => (JoinStep::RetryAt(at), None),
            },
            JoinStep::NeedLease => match self.place.take() {
                Some(place) => (JoinStep::AwaitAnswer, Some(Owed::Request(connect, place))),
                None => (JoinStep::NeedLease, None),
            },
            JoinStep::NeedConfigure if self.configuration.is_some() => {
                let receive = bss_rules(join.access_point.bssid, Rules::<K>::MEMBER);
                (JoinStep::AwaitConfigured, self.configure(receive))
            }
            JoinStep::SaeSend {
                authentication,
                transmission,
            } => {
                if now >= authentication.next_deadline() {
                    return (
                        leave_unkeyed(StaDisconnectReason::Sae(StaSaeFailure::Timeout)),
                        ActionProgress::Transition,
                    );
                }
                let credit = match self.reserve_transmit() {
                    Ok(credit) => credit,
                    Err(Unsent::NoTokens) => {
                        join.step = JoinStep::SaeSend {
                            authentication,
                            transmission,
                        };
                        return (Change::Keep, ActionProgress::Wait);
                    }
                    Err(Unsent::Encode) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                    Err(Unsent::NoConnection) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )),
                            ActionProgress::Transition,
                        );
                    }
                };
                let source = self.config.address;
                let bssid = join.access_point.bssid;
                let sequence_number = self.sequence.take();
                let transmit = self.encode_reserved(
                    credit,
                    |frame| {
                        oer_ieee80211_wire::station::SaeAuthenticationFrame {
                            source,
                            bssid,
                            sequence_number,
                            transaction: transmission.transaction,
                            status_code: transmission.status_code,
                            body: transmission.body(),
                        }
                        .encode(frame)
                        .ok()
                    },
                    StaFrameClass::Connection(ConnectionFrame::Authentication),
                    TxPurpose::SaeAuthentication,
                    StaRatePlan::Management,
                );
                match transmit {
                    Ok(transmit) => (
                        JoinStep::SaeAwait(authentication),
                        Some(Owed::Transmit(transmit)),
                    ),
                    Err(EncodeError) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            JoinStep::Authenticate(Authenticate::NeedCredit(runtime, key)) => {
                let credit = match self.reserve_transmit() {
                    Ok(credit) => credit,
                    Err(Unsent::NoTokens) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedCredit(runtime, key));
                        return (Change::Keep, ActionProgress::Wait);
                    }
                    Err(Unsent::Encode) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                    Err(Unsent::NoConnection) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )),
                            ActionProgress::Transition,
                        );
                    }
                };
                let (runtime, attempt) = runtime.begin_attempt(&mut (*self.sequence));
                let bssid = join.access_point.bssid;
                let address = self.config.address;
                let encoded = self.encode_reserved(
                    credit,
                    |frame| {
                        OpenAuthenticationRequest {
                            source: address,
                            bssid,
                            sequence_number: attempt.sequence_number,
                        }
                        .encode(frame)
                        .ok()
                    },
                    StaFrameClass::Connection(ConnectionFrame::Authentication),
                    TxPurpose::Authentication {
                        attempt: attempt.ordinal,
                    },
                    StaRatePlan::Management,
                );
                match encoded {
                    Ok(transmit) => {
                        let auth = Authenticate::Exchange {
                            runtime,
                            key,
                            stage: AuthStage::AwaitSent {
                                attempt: attempt.ordinal,
                                response_timeout: attempt.response_timeout,
                            },
                        };
                        (JoinStep::Authenticate(auth), Some(Owed::Transmit(transmit)))
                    }
                    Err(EncodeError) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            JoinStep::Associate(runtime, key) => {
                if now >= runtime.response_deadline() {
                    return (
                        leave_unkeyed(StaDisconnectReason::AssociationFailed),
                        ActionProgress::Transition,
                    );
                }
                let credit = match self.reserve_transmit() {
                    Ok(credit) => credit,
                    Err(Unsent::NoTokens) => {
                        join.step = JoinStep::Associate(runtime, key);
                        return (Change::Keep, ActionProgress::Wait);
                    }
                    Err(Unsent::Encode) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::Encode),
                            ActionProgress::Transition,
                        );
                    }
                    Err(Unsent::NoConnection) => {
                        return (
                            leave_unkeyed(StaDisconnectReason::TransmitRefused(
                                TxRefusal::LinkChanged,
                            )),
                            ActionProgress::Transition,
                        );
                    }
                };
                match runtime.poll(now, &mut (*self.sequence)) {
                    StaAssociationPoll::Transmit { runtime, attempt } => {
                        let capabilities = self.config.association;
                        let phy = self.config.phy;
                        let address = self.config.address;
                        let listen_interval = self.config.listen_interval.get();
                        let access_point = &join.access_point;
                        let rsn = &join.rsn;
                        let encoded = self.encode_reserved(
                            credit,
                            |frame| {
                                AssociationRequest {
                                    source: address,
                                    access_point,
                                    sequence_number: attempt.sequence_number,
                                    listen_interval,
                                    phy,
                                    security: rsn,
                                    power_capability: None,
                                    he_ul_mu_power: None,
                                }
                                .encode(frame, capabilities)
                                .ok()
                            },
                            StaFrameClass::Connection(ConnectionFrame::Association),
                            TxPurpose::Association,
                            StaRatePlan::Management,
                        );
                        match encoded {
                            Ok(transmit) => (
                                JoinStep::Associate(runtime, key),
                                Some(Owed::Transmit(transmit)),
                            ),
                            Err(EncodeError) => {
                                return (
                                    leave_unkeyed(StaDisconnectReason::Encode),
                                    ActionProgress::Transition,
                                );
                            }
                        }
                    }
                    StaAssociationPoll::Idle(runtime) => {
                        self.return_credit(credit);
                        (JoinStep::Associate(runtime, key), None)
                    }
                    StaAssociationPoll::Failed { failure, .. } => {
                        self.return_credit(credit);
                        return (
                            leave_unkeyed(association_failure(failure)),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            // The next state owes its first action at once.
            JoinStep::NotifyAssociated {
                association_id,
                peer,
                key,
            } => (
                JoinStep::SetEdca {
                    association_id,
                    peer,
                    key,
                },
                Some(Owed::Associated(peer)),
            ),
            // A set the backend cannot apply leaves the interface's
            // previous one.
            JoinStep::SetEdca {
                association_id,
                peer,
                key,
            } => {
                let next = JoinStep::StartLink {
                    association_id,
                    peer,
                    key,
                };
                match SupportedEdca::new(oer_ieee80211_port::EdcaParameterSet::from_wmm(
                    peer.associated.edca,
                )) {
                    Some(edca) => (next, Some(Owed::SetEdca(edca))),
                    None => {
                        join.step = next;
                        return (Change::Keep, ActionProgress::Transition);
                    }
                }
            }
            JoinStep::StartLink {
                association_id,
                peer,
                key,
            } => {
                let state = self.joined(join, association_id, peer, key, now);
                return (state, ActionProgress::Transition);
            }
            step => (step, None),
        };
        join.step = step;
        (Change::Keep, owed.into())
    }

    /// The associated station: its handshake, or, on an open network,
    /// connected.
    #[inline(never)]
    fn joined(
        &mut self,
        join: &mut Join<T, K>,
        association_id: u16,
        peer: Peer<T::RxMeta>,
        key: AssociationKey,
        now: Instant,
    ) -> Change<T, K> {
        let bssid = join.access_point.bssid;
        let channel = join.channel;
        let qos = !join.access_point.wmm_ie_bytes().is_empty();
        let pmk = match key {
            AssociationKey::Protected(pmk) => pmk,
            AssociationKey::Open => {
                return Change::Establish(NewConnection {
                    bssid,
                    channel,
                    association_id,
                    qos,
                    peer,
                    qos_sequences: QOS_SEQUENCES,
                    confirmation: None,
                });
            }
        };
        let Some(deadline) = RsnStaResponseDeadline::start(RsnStaResponseWait::Message1, now)
        else {
            return leave_keyed(
                StaDisconnectReason::DeadlineOverflow,
                Some(Deauthentication {
                    bssid,
                    reason_code: REASON_UNSPECIFIED,
                    protected: false,
                }),
            );
        };
        let supplicant = RsnStaSupplicant::try_new(
            self.config.address.get(),
            bssid.get(),
            self.config.callbacks.nonce(),
            join.rsn.as_bytes(),
            join.access_point.rsn_ie_bytes(),
            join.access_point.rsnxe_bytes(),
        );
        match supplicant {
            Ok(supplicant) => Change::Handshake(NewHandshake {
                spp: match join.rsn.security() {
                    AssociationSecurity::Rsn(rsn) => rsn.spp,
                    AssociationSecurity::Open => {
                        return leave_unkeyed(StaDisconnectReason::AssociationFailed);
                    }
                },
                bssid,
                channel,
                association_id,
                qos,
                peer,
                supplicant: Some(supplicant),
                pmk,
                qos_sequences: QOS_SEQUENCES,
                deadline,
                management: None,
            }),
            _ => leave_keyed(
                StaDisconnectReason::HandshakeFailed,
                Some(Deauthentication {
                    bssid,
                    reason_code: REASON_UNSPECIFIED,
                    protected: false,
                }),
            ),
        }
    }

    /// A ticket and a small buffer, if the station holds both.
    fn tokens(&mut self) -> Option<(T::Ticket, T::SmallBuffer)> {
        let ticket = self.tickets.take()?;
        match self.buffers.take() {
            Some(buffer) => Some((ticket, buffer)),
            None => {
                self.tickets.put(ticket);
                None
            }
        }
    }

    fn reserve_transmit(&mut self) -> Result<TxCredit<'id, T>, Unsent> {
        let epoch = self.epoch.ok_or(Unsent::NoConnection)?;
        let (ticket, buffer) = self.tokens().ok_or(Unsent::NoTokens)?;
        Ok(TxCredit {
            epoch,
            ticket,
            buffer,
        })
    }

    fn return_credit(&mut self, credit: TxCredit<'id, T>) {
        self.tickets.put(credit.ticket);
        self.buffers.put(credit.buffer);
    }

    /// Encode with both owners already reserved, after the caller may have
    /// consumed a sequence or started an attempt.
    fn encode_reserved(
        &mut self,
        credit: TxCredit<'id, T>,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
        class: StaFrameClass,
        purpose: TxPurpose,
        plan: StaRatePlan,
    ) -> Result<StaTransmit<'id, T, K>, EncodeError> {
        let TxCredit {
            epoch,
            ticket,
            mut buffer,
        } = credit;
        let frame = encode(buffer.bytes_mut()).and_then(|len| {
            let bytes = buffer.bytes_mut().split_at_checked(len)?.0;
            let length = MpduLength::new(len)?;
            if matches!(plan, StaRatePlan::Group) {
                let header = HeaderSpec::from_mac_header(
                    oer_ieee80211_wire::frame::MacHeader::decode(bytes)?,
                    GroupAddress::new,
                )?;
                oer_ieee80211_port::ClassSizedFrame::new(SizedFrame::new(header, length)?)
                    .ok()
                    .map(StaFrame::Group)
            } else {
                let header = HeaderSpec::from_mac_header(
                    oer_ieee80211_wire::frame::MacHeader::decode(bytes)?,
                    IndividualAddress::new,
                )?;
                oer_ieee80211_port::ClassSizedFrame::new(SizedFrame::new(
                    AckHeader::new(header)?,
                    length,
                )?)
                .ok()
                .map(StaFrame::Individual)
            }
        });
        match frame {
            Some(frame) => Ok(StaTransmit {
                context: StaTxContext { epoch, purpose },
                ticket,
                buffer,
                frame,
                class,
                plan,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(EncodeError)
            }
        }
    }

    /// Whether the connected link is up: the station is home and awake,
    /// and holds its pairwise key ([`Self::link`]'s rule).
    fn link_up(&self, power: &PowerSaveView, keys: &Keys<T>) -> bool {
        self.absence.is_none() && !power.asleep() && matches!(keys.pairwise, KeyCell::Installed(_))
    }

    /// An EAPOL frame of the connected station under its pairwise key: the
    /// Protected bit and the CCMP header's room, which the backend fills;
    /// QoS Data at user priority 7 on a QoS BSS.
    ///
    /// On a QoS BSS it takes TID 7's number, which data at priority 7
    /// shares. Allocating from TID 0 instead would leave holes in that TID's
    /// sequence under its Block Ack.
    fn encode_protected_eapol(
        &mut self,
        bssid: Bssid,
        qos: Option<&mut [StaSequenceCounter; 8]>,
        eapol: &[u8],
        purpose: TxPurpose,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        const PRIORITY: u8 = 7;
        let credit = self.reserve_transmit()?;
        let peer_qos = qos.is_some();
        let sequence_number = match qos {
            Some(sequences) => {
                let [_, _, _, _, _, _, _, voice] = sequences;
                voice.take()
            }
            None => self.sequence.take(),
        };
        let frame = oer_ieee80211_wire::station::StaProtectedDataFrame {
            source: self.config.address,
            bssid,
            destination: bssid.get(),
            sequence_number,
            user_priority: PRIORITY,
            peer_qos,
            ccmp_header: [0; 8],
            ether_type: EAPOL_ETHER_TYPE,
            payload: eapol,
        };
        Ok(self.encode_reserved(
            credit,
            |buffer| frame.encode(buffer).ok(),
            StaFrameClass::Connection(ConnectionFrame::Eapol),
            purpose,
            StaRatePlan::Management,
        )?)
    }

    /// A Probe Request reserves credit before consuming its sequence.
    fn encode_probe(&mut self, purpose: TxPurpose) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let sequence_number = self.sequence.take();
        let rates = self.config.supported_rates;
        let address = self.config.address.get();
        let ssid = self.config.ssid.clone();
        Ok(self.encode_reserved(
            credit,
            |output| {
                ProbeRequest {
                    destination: [0xff; 6],
                    source: address,
                    bssid: [0xff; 6],
                    sequence_number,
                    ssid: &ssid,
                    supported_rates: &rates,
                }
                .encode(output)
                .ok()
            },
            StaFrameClass::Normal,
            purpose,
            StaRatePlan::Group,
        )?)
    }

    /// A management frame of `subtype` to `bssid` in a buffer.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: Bssid,
        body: &[u8],
        purpose: TxPurpose,
        protected: bool,
    ) -> Result<StaTransmit<'id, T, K>, Unsent> {
        let credit = self.reserve_transmit()?;
        let address = self.config.address;
        let sequence_number = self.sequence.take();
        Ok(self.encode_reserved(
            credit,
            |frame| {
                if protected {
                    return oer_ieee80211_wire::station::StaProtectedManagementFrame {
                        subtype,
                        source: address,
                        bssid,
                        sequence_number,
                        ccmp_header: [0; 8],
                        body,
                    }
                    .encode(frame)
                    .ok();
                }
                StaManagementFrame {
                    subtype,
                    source: address,
                    bssid,
                    sequence_number,
                    body,
                }
                .encode(frame)
                .ok()
            },
            StaFrameClass::Normal,
            purpose,
            StaRatePlan::Management,
        )?)
    }
}

/// The network's receive rules of `filter` for `bssid`.
fn bss_rules<K>(bssid: Bssid, filter: SupportedFilter<K, StationRole>) -> StationReceive<K> {
    StationReceive::Bss {
        bssid: bssid.individual(),
        filter,
    }
}

/// Leave a network the station holds no key of: receive off, the lease
/// back.
fn association_failure(failure: StaAssociationFailure) -> StaDisconnectReason {
    match failure {
        StaAssociationFailure::DeadlineOverflow => StaDisconnectReason::DeadlineOverflow,
        _ => StaDisconnectReason::AssociationFailed,
    }
}

fn leave_unkeyed<T: StaTokens, K>(reason: StaDisconnectReason) -> Change<T, K> {
    Change::Leave {
        reason,
        step: LeaveStep::NeedConfigure,
    }
}
fn leave_key_step(deauthentication: Option<Deauthentication>) -> LeaveStep {
    match deauthentication {
        Some(deauthentication) if deauthentication.protected => {
            LeaveStep::NeedDeauthentication(deauthentication)
        }
        other => LeaveStep::RemoveKeys(other),
    }
}
fn leave_keyed<T: StaTokens, K>(
    reason: StaDisconnectReason,
    deauthentication: Option<Deauthentication>,
) -> Change<T, K> {
    Change::Leave {
        reason,
        step: leave_key_step(deauthentication),
    }
}
fn leave_handshake<T: StaTokens, K>(handshake: &Handshake<T, K>, reason_code: u16) -> Change<T, K> {
    let reason = if reason_code == REASON_LEAVING {
        StaDisconnectReason::Requested
    } else {
        StaDisconnectReason::HandshakeFailed
    };
    leave_keyed(
        reason,
        Some(Deauthentication {
            bssid: handshake.bssid,
            reason_code,
            protected: handshake.management.is_some(),
        }),
    )
}
fn leave_connected<T: StaTokens, K>(
    connected: &Connected<T, K>,
    reason: StaDisconnectReason,
    reason_code: Option<u16>,
) -> Change<T, K> {
    let bssid = connected.bssid;
    let protected = connected
        .rsn
        .as_ref()
        .is_some_and(|rsn| rsn.management.is_some());
    Change::Leave {
        reason,
        step: LeaveStep::AwaitAbsenceEnd(reason_code.map(|reason_code| Deauthentication {
            bssid,
            reason_code,
            protected,
        })),
    }
}
fn leave_disabled<T: StaTokens, K>(_connected: &Connected<T, K>) -> Change<T, K> {
    Change::Leave {
        reason: StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged),
        step: LeaveStep::Disabled,
    }
}
#[inline(never)]
fn finish_connected_install<T: StaTokens, K>(connected: &mut Connected<T, K>) {
    if let Some(rsn) = connected.rsn.as_mut() {
        let rekey = core::mem::replace(&mut rsn.rekey, Rekey::None);
        match rekey {
            Rekey::AwaitInstalled(request, protection) => {
                if rsn.complete_install(request, protection).is_err() {
                    rsn.dropped.increment();
                }
            }
            rekey => rsn.rekey = rekey,
        }
    }
}

fn finish_handshake_install<T: StaTokens, K>(
    handshake: &mut Handshake<T, K>,
    request: RsnStaRetryInstall,
    replay: CcmpRxReplayEpoch,
    protection: EapolProtection,
) -> Change<T, K> {
    if let (Some(management), Some(igtk)) = (&mut handshake.management, request.igtk()) {
        management.install_igtk(igtk);
    }
    match handshake.supplicant().and_then(|supplicant| {
        supplicant.complete_retry_install::<DEFAULT_EAPOL_FRAME_CAPACITY>(request, true)
    }) {
        Ok(RsnStaReady::Transmit(message)) => Change::Confirm {
            replay,
            response: EapolResponse {
                frame: message,
                protection,
            },
        },
        _ => leave_handshake(handshake, REASON_UNSPECIFIED),
    }
}
/// Why a frame was not handed out.
#[derive(Debug)]
enum Unsent {
    NoConnection,
    /// The station holds no ticket or buffer now.
    NoTokens,
    /// The frame did not encode.
    Encode,
}

/// Encoding can fail after credit is reserved; acquiring credit cannot.
#[derive(Debug)]
struct EncodeError;

impl From<EncodeError> for Unsent {
    fn from(_: EncodeError) -> Self {
        Self::Encode
    }
}

#[cfg(test)]
mod tests;
