//! The station: one sans-IO machine for the whole lifecycle of an
//! infrastructure station, from its scan to its disconnect.
//!
//! The machine takes the port's events, the channel arbiter's answers and
//! notices, its commands and time as [`StaInput`] values, and leaves what
//! the port and the arbiter must do as [`StaAction`] values, in their own
//! terms (`oer-protocols-ieee80211-contracts-port`, `oer-protocols-ieee80211-core-arbiter`): a thin
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
//! # Types
//!
//! The machine is generic over the types of its values ([`StaTypes`]): the
//! arbiter's path, the brand of the station's own command credits and the
//! port's evidence of a received frame and of a transmission's end. It holds
//! nothing the port has a limited number of. It encodes each frame into a
//! buffer of its own and lends it to the action that carries it out at once
//! ([`StaTransmit`]); the frame's end comes back with its tag
//! ([`StaInput::FrameEnded`]). It keeps each key's progress (none,
//! installing, installed, removing), and its actions name the key by its
//! place and material, not by an owner.
//! Management requests use the configured station address as transmitter;
//! the selected BSSID identifies the receiver and the BSS.
//!
//! The station has one place for each purpose of its frames: a frame waits
//! while the frame of its own purpose is out, never for another purpose's.
//! The four orders of its keys and its configuration are states: one group
//! key operation at a time; the pairwise key removed only once the group
//! table is empty and the link came back; one receive configuration at a
//! time. Leave removes every GTK before the PTK, under the original D4
//! deadline for any pending installation.
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
use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
/// Why an EAPOL-Key packet is not one, as [`StaDisconnectReason::InvalidEapol`]
/// carries it.
pub use oer_protocols_ieee80211_wire::eapol::EapolParseError;
use oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL;
use oer_protocols_ieee80211_wire::status::reason_code;

use crate::events::{ConnectStep, RsnDrop, ScanOutcome, StaEvent};
use oer_portable_events::{EventSink, report};
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, ConnectionFrame, GroupAddress, HeaderSpec, Ieee80211Capabilities, MpduLength,
    ReceiveFilter, RxMetadata, SingleEnd, SingleOutcome, SizedFrame, Station as StationRole,
    StationReceive, SupportedChannel, SupportedEdca, SupportedFilter, TxOutcome,
};
use oer_protocols_ieee80211_core_arbiter::{
    AbsenceAnswer, AbsencePlace, Answer, ChannelRequest, Lease, LeaseId, LeasePlace, LeaveReason,
    Purpose, Refusal, StationNotice, StationPlaceReturns, StationPlaces, UpstreamFact,
};
use oer_protocols_ieee80211_core_security_rsn::{
    DEFAULT_EAPOL_FRAME_CAPACITY, OwnedEapolFrame, Pmk, RsnInterface,
    frames::RsnTxFrame,
    keys::{RsnKeyInstall, RsnKeyKind},
    sae::{SaeCommit, SaePassword, SaePasswordElement, SaePasswordToken},
    supplicant::{
        RsnConnectedAction, RsnConnectedProcessError, RsnConnectedSupplicant,
        RsnConnectedSupplicantError, RsnGroupKeyFrame, RsnGroupKeyInstallRequest,
        RsnStaDeadlineEvent, RsnStaKeyInstallRequest, RsnStaProcessError, RsnStaReady,
        RsnStaResponseDeadline, RsnStaResponseWait, RsnStaRetryInstall, RsnStaSupplicant,
        RsnStaSupplicantError,
    },
};
use oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding;
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_core_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_protocols_ieee80211_wire::address::{Bssid, IndividualAddress, OwnAddress};
use oer_protocols_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
use {
    crate::{
        scan::{BssObservation, ScanTable},
        security::{StaSecurityPolicy, select_association_rsn},
    },
    oer_protocols_ieee80211_core_upper_mac::{phy_mode::NegotiatedPhy, sequence::SequenceCounter},
    oer_protocols_ieee80211_wire::{
        management::{BROADCAST_ADDRESS, MAX_SSID_LEN, ProbeRequest},
        scan::ScanRecord,
        security::{AssociationAkm, AssociationSecurity, RsnAssociation},
        sequence::SequenceNumber,
        station::{
            AssociationCapabilities, AssociationRequest, OpenAuthenticationRequest, SelectedRsn,
            StaManagementFrame, StaManagementSubtype,
        },
        station_beacon::parse_sta_beacon,
        station_power_save::{StaNullDataFrame, StaPowerManagement},
    },
};
use {
    oer_protocols_ieee80211_contracts_security::Ccmp128Key,
    oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch,
};

use crate::association::{StaAssociatedPeer, StaHePower, StaPhyRequest, negotiate};
use crate::receive::{Management, Received, observe_scan};
// The machine's tests build receive metadata through `use super::*`.
use oer_portable_time::{Duration, Instant};
#[cfg(test)]
use oer_protocols_ieee80211_contracts_port::RxEvidence;

use crate::epoch::{LinkEpoch, LinkEpochs};
use crate::join::sae::{StaSaeAuthentication, StaSaeEvent, StaSaeFailure, StaSaeTransmission};
use crate::link::{FrameNumber, LinkAction, LinkHandover, LinkReturn, RxLink};
use oer_protocols_ieee80211_core_datapath::numbers::{LinkSpace, TxNumbers};
use oer_protocols_ieee80211_core_security_rsn::pmksa::{Pmksa, PmksaCache};

/// The station's PMKSA cache: per access point and SSID.
type StaPmksaCache = PmksaCache<Bssid, heapless::Vec<u8, MAX_SSID_LEN>>;
use crate::pool::Pool;
use crate::request::StationListenInterval;
use oer_protocols_radio_port::{PoolPath, Root};
mod block_ack;
mod commands;
mod deferred_eapol;
mod group_keys;
mod management;
mod tx_block_ack;
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
pub use group_keys::StaGroupPlace;
use group_keys::{
    GroupArrival, GroupKeys, GroupRemoval, GroupRole, Installed as InstalledGroup,
    Replay as GroupReplay,
};
use management::{ManagementEvent, ManagementProtection};
use tx_block_ack::TxBlockAckOwed;
pub use tx_block_ack::{StaTxBlockAck, StaTxBlockAckPolicy, StaTxBlockAckPolicyError};

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
    /// A frame's bytes no longer held the frame its description named.
    Layout(oer_protocols_ieee80211_contracts_port::StorageTooShort),
    /// The port refused the frame: its key was issued by an earlier
    /// installation of the port
    /// ([`WithdrawCause::StalePermission`](oer_protocols_ieee80211_contracts_port::WithdrawCause::StalePermission)).
    /// The link cannot send under it again.
    StaleKey,
}

/// The types of a station's values: what it shares with its arbiter and
/// its application, and what the port tells of a frame. None of them is an
/// owner the port has a limited number of.
pub trait StaTypes {
    /// Component owning the station's channel arbiter and lease admission.
    type GatePath: PoolPath;
    /// Installation component owning this interface's command credits.
    type ConfigurationPath: PoolPath;
    /// What the port observed of a solicited response.
    type AckEvidence: Copy;
    /// The port's evidence that coexistence denied an attempt the air.
    type AirDenied: Copy;
    /// The metadata the port gives a received frame.
    type RxMeta: RxMetadata;
}

/// The network one connection joins: its SSID and how it protects its link.
/// The station keeps the profile across reconnections until the next one
/// replaces it ([`Station::set_profile`]); secrets erase themselves on drop.
pub struct StaProfile<K> {
    /// The network's SSID.
    pub ssid: heapless::Vec<u8, MAX_SSID_LEN>,
    /// How the network protects its link.
    pub security: StaSecurity,
    /// What the station asks this network's access points for: the formats
    /// it admits and its widest width; a 40 MHz join takes one of the
    /// configuration's wide channels.
    pub phy: StaPhyRequest<K>,
}

impl<K> core::fmt::Debug for StaProfile<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StaProfile")
            .field("ssid", &self.ssid)
            .field("policy", &self.security.policy())
            .field("phy", &self.phy)
            .finish_non_exhaustive()
    }
}

/// A profile the station did not take: the same profile, which erases its
/// secrets when dropped, and why.
pub struct StaProfileRefused<K> {
    pub profile: StaProfile<K>,
    pub reason: CommandRefusal,
}

impl<K> core::fmt::Debug for StaProfileRefused<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StaProfileRefused")
            .field("profile", &self.profile)
            .field("reason", &self.reason)
            .finish()
    }
}

/// How the station protects its link.
pub enum StaSecurity {
    /// An open network.
    Open,
    /// Personal credentials: the password supports SAE and the prederived
    /// PMK supports PSK. Selection prefers SAE whenever the BSS offers it.
    Personal { password: SaePassword, pmk: Pmk },
    /// A WPA2 PSK without a retained password or SAE upgrade.
    Wpa2Personal { pmk: Pmk },
}

impl StaSecurity {
    /// The security policy the scan selects and the join associates with.
    pub const fn policy(&self) -> StaSecurityPolicy {
        match self {
            Self::Open => StaSecurityPolicy::Open,
            Self::Personal { .. } => StaSecurityPolicy::Wpa2Personal,
            Self::Wpa2Personal { .. } => StaSecurityPolicy::Wpa2Psk,
        }
    }
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
pub enum StaInput<'id, T: StaTypes, K> {
    /// A deadline the station named passed.
    Tick,
    /// A frame the station handed out ended: the frame of its tag's purpose
    /// is no longer out.
    FrameEnded {
        /// The frame's tag, as its action named it.
        tag: StaTxTag,
        /// How it ended.
        end: StaFrameEnd<T::AckEvidence, T::AirDenied>,
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
    /// The receive configuration the station set ([`StaAction::Configure`])
    /// took effect.
    Configured,
    /// The pairwise key operation the station asked for
    /// ([`StaAction::InstallPairwiseKey`], [`StaAction::RemovePairwiseKey`])
    /// ended with its outcome.
    PairwiseKeyEnded(StaKeyOutcome),
    /// The group key operation the station asked for at `place` ended:
    /// an installation, a replacement, a selection or a removal.
    GroupKeyEnded {
        /// The group table's place the operation named.
        place: StaGroupPlace,
        /// How it ended.
        outcome: StaKeyOutcome,
    },
}

/// How a key operation the station asked for ended.
///
/// An installation, replacement or selection that expired ends the
/// connection attempt it belongs to as a failed handshake
/// ([`StaDisconnectReason::HandshakeFailed`]); the station then holds no key
/// it asked for there. A removal that expired ends the same way as one done:
/// the station holds that key no longer and does not wait for it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaKeyOutcome {
    /// The operation took effect.
    Done,
    /// The operation did not take effect within its bound.
    Expired,
}

/// What a connect's scan covered: every channel, or it ended early after
/// cancelled absences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanCoverage {
    Complete,
    Partial,
}

/// How a frame the station handed out ended.
#[derive(Clone, Copy, Debug)]
pub enum StaFrameEnd<A, D> {
    /// It went: its outcome on the air.
    Sent(SingleOutcome<A, D>),
    /// It was refused before admission: no transmission terminal exists.
    Refused(TxRefusal),
}

/// One action of the station, which the service carries out at once.
/// Discarding a produced action is diagnosed by `unused_must_use`:
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use oer_protocols_ieee80211_core_roles_sta::machine::{StaAction, StaTypes};
/// fn ignore<'a, 'id, T: StaTypes, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>) {
///     next();
/// }
/// ```
/// The consumer may transfer it to the component that performs it:
/// ```
/// #![deny(unused_must_use)]
/// use oer_protocols_ieee80211_core_roles_sta::machine::{StaAction, StaTypes};
/// fn forward<'a, 'id, T: StaTypes, K: 'a>(next: impl FnOnce() -> StaAction<'a, 'id, T, K>)
///     -> StaAction<'a, 'id, T, K> {
///     next()
/// }
/// ```
#[must_use = "carry out the action"]
#[expect(
    clippy::large_enum_variant,
    reason = "the key-unwrap request owns its bounded buffer by value; the station has no allocator"
)]
pub enum StaAction<'a, 'id, T: StaTypes, K> {
    /// Tell the link's holder what the link does now
    /// ([`crate::link`]): take it, hold or release its data, a transmit
    /// agreement's change, or give it back ([`Station::link_returned`]).
    Link(LinkAction),
    /// A validated Block Ack action of the current BSS, for its RX bank owner.
    BlockAck(PendingRxBlockAck),
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
    Configure(StationReceive<K>),
    /// Set the EDCA parameters the station's interface contends with: the
    /// access point's, from its association on and whenever its beacons
    /// announce another set. No input follows.
    SetEdca(SupportedEdca<K>),
    /// Set the station interface's HE state, in the vendor station's two
    /// stages: before authentication, an HE20 join's HE BSS (the color the
    /// scan heard) or `Off` for any other join; after the Association
    /// Response and before the link's data, the HE20 association. `Off`
    /// again at the leave of an HE BSS and before a restart's reclaim
    /// finds one left. No input follows.
    SetHe(oer_protocols_ieee80211_contracts_port::StationHe),
    /// Install the pairwise key ([`StaInput::PairwiseKeyEnded`]).
    InstallPairwiseKey {
        /// The access point.
        peer: IndividualAddress,
        /// The temporal key.
        key: Ccmp128Key,
        /// The association's SPP A-MSDU decision.
        spp: oer_protocols_ieee80211_wire::security::Spp,
    },
    /// Install a group key at `place` ([`StaInput::GroupKeyEnded`]).
    InstallGroupKey {
        /// The group table's place.
        place: StaGroupPlace,
        /// Its key index.
        key_id: GroupKeyId,
        /// The temporal key.
        key: Ccmp128Key,
    },
    /// Replace the key at `place` in one operation; only its end switches
    /// the station's replay lane ([`StaInput::GroupKeyEnded`]).
    ReplaceGroupKey {
        /// The group table's place of the key it replaces.
        place: StaGroupPlace,
        /// The successor's key index.
        key_id: GroupKeyId,
        /// The successor's material.
        key: Ccmp128Key,
    },
    /// Choose the retained installation at `place` without writing material
    /// or resetting replay ([`StaInput::GroupKeyEnded`]).
    SelectGroupKey {
        /// The group table's place.
        place: StaGroupPlace,
        /// Its key index.
        key_id: GroupKeyId,
    },
    /// Remove the pairwise key ([`StaInput::PairwiseKeyEnded`]).
    RemovePairwiseKey,
    /// Remove the group key at `place` ([`StaInput::GroupKeyEnded`]).
    RemoveGroupKey {
        /// The group table's place.
        place: StaGroupPlace,
    },
    /// Transmit one MPDU ([`StaInput::FrameEnded`] follows).
    Transmit(StaTransmit<'a, K>),
    /// Transmit one MPDU the station numbered itself under its pairwise
    /// key, which is the engine's: a protected Deauthentication after the
    /// link came back ([`StaInput::FrameEnded`] follows). It carries the
    /// Protected bit and the CCMP header's room. A protected frame of a
    /// handed-over link goes as [`Self::Transmit`] with its
    /// [`FrameNumber::Link`].
    TransmitProtected(StaTransmit<'a, K>),
    /// The station associated with `Peer`: the rates of its frames start
    /// from what the association made known, the handshake's EAPOL
    /// included. A [`StaNotice::Disconnected`] ends it.
    Associated(Peer<T::RxMeta>),
    /// Tell the application what happened.
    Notify(StaNotice),
    /// Return one accepted application's request and its final outcome.
    CommandCompleted(StaCommandTerminal<'id, T::ConfigurationPath>),
}

/// The capacity of the station's own frame buffer: the longest frame it
/// encodes, without its FCS.
pub const STA_FRAME_CAPACITY: usize = 512;

/// The station's own frame buffer: each frame is encoded here and lent to
/// the action that carries it out in the same step.
struct FrameBuffer {
    bytes: [u8; STA_FRAME_CAPACITY],
    /// The bytes an encoding may use: a test narrows it to make one fail.
    #[cfg(test)]
    limit: usize,
}

impl FrameBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; STA_FRAME_CAPACITY],
            #[cfg(test)]
            limit: STA_FRAME_CAPACITY,
        }
    }

    /// Whether a frame of `length` bytes fits the buffer as the test set it.
    const fn fits(&self, length: usize) -> bool {
        #[cfg(test)]
        let fits = length <= self.limit;
        #[cfg(not(test))]
        let fits = length <= STA_FRAME_CAPACITY;
        fits
    }
}

/// One MPDU to transmit: the station's buffer holding it, its kind, its
/// class and its rates.
#[must_use = "submit the frame; its end comes back with its tag"]
pub struct StaTransmit<'a, K> {
    /// Its epoch and purpose, which its end names.
    pub tag: StaTxTag,
    /// The station's buffer holding the frame from its header on; `frame`
    /// names its length. The link's holder writes its Sequence Number in
    /// place ([`crate::link::number`]); a BlockAckReq has no bytes.
    pub bytes: &'a mut [u8],
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<K>,
    /// What the frame is for, which the service turns into the port's
    /// coexistence priority.
    pub class: StaFrameClass,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
    /// The access category it contends in: [`Self::ACCESS_CATEGORY`] for a
    /// management frame, the admitted one of its user priority for EAPOL.
    pub access_category: WmmAccessCategory,
    /// Who numbers it: the station, or the link's holder as it submits it.
    pub number: FrameNumber,
}

/// A frame the station encoded into its buffer, owed until its action.
struct Encoded<K> {
    tag: StaTxTag,
    frame: StaFrame<K>,
    class: StaFrameClass,
    plan: StaRatePlan,
    access_category: WmmAccessCategory,
    number: FrameNumber,
}

/// The identity of one station frame: the connection it was made in and
/// what it is for. No frame parsing or current-state guess assigns an end
/// to a handshake, power transition or link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxTag {
    epoch: LinkEpoch,
    purpose: TxPurpose,
}

impl StaTxTag {
    /// Whether this frame used the probe rate record.
    pub const fn is_probe(&self) -> bool {
        matches!(
            self.purpose,
            TxPurpose::Probe | TxPurpose::Connected(Sending::Probe)
        )
    }

    /// The rate record whose selection this frame's end teaches: a probe's,
    /// or the control frames'.
    pub const fn frame_rates(&self) -> FrameRates {
        if self.is_probe() {
            FrameRates::Probe
        } else {
            FrameRates::Control
        }
    }

    /// The TID whose next Sequence Number this frame names: an ADDBA
    /// Request, for which the link's holder held the TID's data
    /// ([`LinkAction::Sequence`]).
    pub const fn names_sequence(&self) -> Option<WmmUserPriority> {
        match self.purpose {
            TxPurpose::Connected(Sending::AddbaRequest { tid }) => WmmUserPriority::new(tid),
            _ => None,
        }
    }

    /// The connection the frame was made in.
    pub const fn epoch(&self) -> LinkEpoch {
        self.epoch
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

impl TxPurpose {
    /// The purpose's place among the station's frames out.
    const fn place(self) -> u8 {
        match self {
            Self::Probe => 1 << 0,
            Self::Authentication { .. } => 1 << 1,
            Self::SaeAuthentication => 1 << 2,
            Self::Association => 1 << 3,
            Self::HandshakeResponse => 1 << 4,
            Self::HandshakeLast => 1 << 5,
            Self::Deauthentication => 1 << 6,
            Self::Connected(_) => 1 << 7,
        }
    }
}

/// The one place of every frame of the connected station.
const CONNECTED_PLACE: TxPurpose = TxPurpose::Connected(Sending::Probe);

/// The station's frames out: one place a purpose, taken from the frame's
/// action to its end.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FramesOut(u8);

impl FramesOut {
    const fn holds(self, purpose: TxPurpose) -> bool {
        self.0 & purpose.place() != 0
    }

    fn take(&mut self, purpose: TxPurpose) {
        self.0 |= purpose.place();
    }

    fn release(&mut self, purpose: TxPurpose) {
        self.0 &= !purpose.place();
    }
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
pub enum StaFrame<K> {
    /// An individual frame that solicits an ACK.
    Individual(SizedFrame<AckHeader, K>),
    /// A group frame.
    Group(SizedFrame<HeaderSpec<GroupAddress>, K>),
    /// A BlockAckReq of a transmit agreement, which the port encodes and
    /// which solicits a BlockAck.
    BlockAckRequest {
        /// The agreement's recipient.
        receiver: IndividualAddress,
        /// The station's address.
        transmitter: [u8; 6],
        /// The agreement's TID.
        tid: oer_protocols_ieee80211_contracts_port::Tid,
        /// Where the recipient's window moves.
        starting_sequence: SequenceNumber,
    },
}

/// The rates a station frame goes at, which the service turns into the
/// port's retry plan on the port's channel.
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

impl StaFrameClass {
    /// The coexistence class the port gives the frame: the connection's
    /// setup and keys go first, any other frame at its access category.
    pub const fn coex_operation(self) -> oer_protocols_ieee80211_contracts_port::TxOperation {
        match self {
            Self::Connection(frame) => {
                oer_protocols_ieee80211_contracts_port::TxOperation::Connection(frame)
            }
            Self::Normal => oer_protocols_ieee80211_contracts_port::TxOperation::Data,
        }
    }
}

impl StaRatePlan {
    /// The retry plan of an individual frame at this plan's rates, as
    /// `rates` select them; `carried` turns a rate into the port's. `None`
    /// when no stage of the selection is carried.
    pub fn individual<R, M: RxMetadata, A, BA>(
        self,
        rates: &impl RateSelection<M, A, BA>,
        carried: impl Fn(oer_protocols_ieee80211_contracts_port::PhyRate) -> Option<R>,
    ) -> Option<oer_protocols_ieee80211_contracts_port::RetryPlan<R>> {
        match self {
            // A Null's retries are its plan's.
            Self::Null => rates
                .plan(FrameRates::Control, carried)
                .map(|plan| plan.limit(crate::power_save::NULL_TRANSMISSIONS)),
            Self::Management | Self::Group => rates.plan(FrameRates::Control, carried),
        }
    }

    /// The one rate of a group frame: the first stage of the probe record.
    pub fn group<R, M: RxMetadata, A, BA>(
        rates: &impl RateSelection<M, A, BA>,
        carried: impl Fn(oer_protocols_ieee80211_contracts_port::PhyRate) -> Option<R>,
    ) -> Option<R> {
        rates
            .plan(FrameRates::Probe, carried)
            .map(|plan| plan.into_first().rate)
    }
}

impl<K: Ieee80211Capabilities> StaFrame<K> {
    /// The frame on the port's channel at the rates of `plan`, as `rates`
    /// select them; `carried` turns a rate into the port's. `None` when no
    /// stage of the selection is carried: the frame does not go. A
    /// BlockAckReq keeps only the plan's non-HT stages.
    pub fn rated<'p, M: RxMetadata, A, BA>(
        self,
        plan: StaRatePlan,
        rates: &impl RateSelection<M, A, BA>,
        carried: impl Fn(
            oer_protocols_ieee80211_contracts_port::PhyRate,
        ) -> Option<oer_protocols_ieee80211_contracts_port::PortRate<'p, K>>,
    ) -> Option<oer_protocols_ieee80211_contracts_port::TxFrame<'p, K>> {
        match self {
            Self::Individual(frame) => plan.individual(rates, carried).map(|plan| {
                oer_protocols_ieee80211_contracts_port::TxFrame::Individual { frame, plan }
            }),
            Self::Group(frame) => StaRatePlan::group(rates, carried)
                .map(|rate| oer_protocols_ieee80211_contracts_port::TxFrame::Group { frame, rate }),
            Self::BlockAckRequest {
                receiver,
                transmitter,
                tid,
                starting_sequence,
            } => {
                let plan = plan
                    .individual(rates, carried)?
                    .filter_map(|rate| rate.legacy().ok())?;
                Some(
                    oer_protocols_ieee80211_contracts_port::TxFrame::BlockAckRequest {
                        receiver,
                        transmitter,
                        tid,
                        starting_sequence,
                        plan,
                    },
                )
            }
        }
    }
}

impl<K: Ieee80211Capabilities> StaTransmit<'_, K> {
    /// Station management frames contend in the Voice queue; admission
    /// control governs QoS data only.
    pub const ACCESS_CATEGORY: WmmAccessCategory = WmmAccessCategory::Voice;
    /// A station frame goes without an RTS/CTS or CTS-to-self exchange.
    pub const PROTECTION: oer_protocols_ieee80211_contracts_port::Protection =
        oer_protocols_ieee80211_contracts_port::Protection::None;
    /// A station frame goes at the port's calibrated power.
    pub const POWER: oer_protocols_ieee80211_contracts_port::SupportedPower<K> =
        oer_protocols_ieee80211_core_datapath::calibrated_power::<K>();
}

/// What a received frame was to the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaReceived {
    /// It delivered data MSDUs and nothing else: the station owes nothing
    /// it did not owe before, and no deadline came earlier.
    Data,
    /// It was an input to the station's state, a management frame or a key
    /// exchange message: the next [`Station::poll_action`] may owe an action.
    Input,
}

/// What data taken for transmission was to the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTraffic {
    /// It was recorded and nothing else: the station owes nothing it did
    /// not owe before, and no deadline came earlier.
    Recorded,
    /// It changed the station's transmit agreements, a negotiation queued
    /// or an inactivity deadline set: the next [`Station::poll_action`] may
    /// owe an action.
    Input,
}

/// A frame refused by admission or by the authenticated RSN protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRxError {
    Frame(oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason),
    Security(RsnConnectedProcessError),
}

impl From<oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason> for StaRxError {
    fn from(reason: oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason) -> Self {
        Self::Frame(reason)
    }
}

impl From<RsnConnectedSupplicantError> for StaRxError {
    fn from(reason: RsnConnectedSupplicantError) -> Self {
        Self::Security(RsnConnectedProcessError::Supplicant(reason))
    }
}

/// What an EAPOL frame of the link was to the station
/// ([`Station::eapol`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaEapol {
    /// The station took it.
    Taken,
    /// The station's RSN failed: the link gives no more data before the
    /// station leaves.
    Faulted,
}

/// What the station tells the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaNotice {
    /// The station is connected to `bssid` on `channel` in `phy`.
    Connected {
        /// The access point.
        bssid: Bssid,
        /// Its primary channel number.
        channel: u8,
        /// The PHY the station associated in.
        phy: oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode,
    },
    /// The station left its network, or failed to join one.
    Disconnected(StaDisconnectReason),
}

/// Why the station is not connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectReason {
    InvalidEapol(EapolParseError),
    RetainedSecurityIes(oer_protocols_ieee80211_core_security_rsn::frames::RsnFrameError),
    /// A required frame was refused before admission.
    TransmitRefused(TxRefusal),
    /// The application asked it to leave.
    Requested,
    /// No access point of the network answered the scan, which covered
    /// every channel or ended early after cancelled absences.
    NotFound {
        scan: ScanCoverage,
    },
    /// Access points of the network answered, and none offers a format the
    /// profile's request admits; `scan` as for [`Self::NotFound`].
    NoAdmittedFormat {
        scan: ScanCoverage,
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
    /// The access point answered an HE20 request without setting up the HE
    /// association: its Association Response lacks, repeats or malforms an
    /// HE element, or no longer admits HE20. The station keeps no other
    /// link in its place.
    HeNotGranted,
    /// The complete monotonic deadline of an operation cannot be represented.
    DeadlineOverflow,
    /// The access point deauthenticated or disassociated the station.
    Peer {
        /// The reason code it gave.
        reason_code: u16,
    },
    /// A frame the station had to send did not encode.
    Encode,
    /// The BSS's admission control closes every access category, so the
    /// station's EAPOL has none to contend in.
    NoAdmittedAccessCategory,
    /// The four-way handshake, or a group key's handshake, failed or timed
    /// out.
    HandshakeFailed,
    /// The peer did not confirm its protected association within SA Query timeout.
    SaQueryTimeout,
}

/// The scan list's channels at most.
const CHANNEL_CAPACITY: usize = 16;
/// The 40 MHz channels a join may tune at most: a secondary above and one
/// below for every scanned channel.
pub const WIDE_CHANNEL_CAPACITY: usize = 2 * CHANNEL_CAPACITY;
/// The BSSs a scan keeps at most.
const SCAN_CAPACITY: usize = 16;

/// A new association's QoS sequence spaces.
const QOS_SEQUENCES: [SequenceCounter; 8] = [SequenceCounter::new(SequenceNumber::ZERO); 8];

/// Statically dispatched composition hooks retained across stop/restart.
/// The provider owns its entropy state; each nonce request must be fresh for
/// protected handshakes and PMF. Open-only compositions never request one.
pub trait StaCallbacks {
    /// The synchronous AES-128 engine of every AES operation of the station,
    /// in its step: the EAPOL-Key data unwrap (RFC 3394), the AES-CMAC
    /// EAPOL-Key MIC (PSK-SHA256, SAE), and the CCMP and BIP of protected
    /// management frames. The composition chooses it, a software block or
    /// the chip's accelerator. A refused key or block fails the operation as
    /// a handled error: an unwrap like corrupted key data, a received
    /// management frame like one that does not verify.
    type AesEngine: oer_portable_crypto::block::Aes128Engine;
    /// Fresh randomness for each handshake and PMF SA Query procedure.
    fn nonce(&mut self) -> [u8; 32];
    /// The engine for the next AES operation.
    fn aes_engine(&mut self) -> &mut Self::AesEngine;
    /// Composition policy for the peer's complete HE Capabilities element.
    fn he_packet_padding(&self, capability: &[u8]) -> HePacketPadding;
}

/// What the station connects to and how.
///
/// A scan list is a contiguous sequence of checked channels. A hole that
/// could terminate a scan before its later channels is not representable:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_core_roles_sta::machine::StaConfig;
/// fn holes<K, C: oer_protocols_ieee80211_core_roles_sta::machine::StaCallbacks>(config: &mut StaConfig<K, C>) {
///     config.channels = [None; 16];
/// }
/// ```
///
/// A bounded, contiguous list uses the same checked channel values:
///
/// ```
/// use oer_protocols_ieee80211_core_roles_sta::machine::{ScanChannel, StaConfig};
/// fn one<K, C: oer_protocols_ieee80211_core_roles_sta::machine::StaCallbacks>(config: &mut StaConfig<K, C>, channel: ScanChannel<K>) {
///     config.channels = heapless::Vec::from_slice(&[channel]).unwrap();
/// }
/// ```
/// A channel the scan visits, with the power elements an HE Association
/// Request carries on it: the Power Capability states the powers of the
/// current channel (IEEE Std 802.11-2020, 9.4.2.14), so each channel has its
/// own, which the composition computes from that channel's ceiling when it
/// builds the configuration. They are the scanned primary's because HE is
/// offered at 20 MHz only: an HE join is on the channel it was scanned on.
pub struct ScanChannel<K> {
    pub channel: SupportedChannel<K>,
    pub he_power: StaHePower,
}

impl<K> Clone for ScanChannel<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for ScanChannel<K> {}

pub struct StaConfig<K, C: StaCallbacks> {
    /// The interface's own address.
    pub address: OwnAddress,
    /// The channels the scan visits, in order, each with the power elements
    /// of an HE join on it.
    pub channels: heapless::Vec<ScanChannel<K>, CHANNEL_CAPACITY>,
    /// The 40 MHz channels an HT40 join may tune: the one whose primary is
    /// the access point's scanned channel and whose secondary its HT
    /// Operation names. Without one, the join stays on the scanned channel.
    pub wide_channels: heapless::Vec<SupportedChannel<K>, WIDE_CHANNEL_CAPACITY>,
    /// How long the scan listens on each channel, from the moment its
    /// receive rules are in place, within the visit's lease.
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
    /// The station's transmit Block Ack originator, which the composition
    /// builds from its policy; `None` negotiates no transmit agreement.
    pub tx_block_ack: Option<StaTxBlockAck>,
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

/// The station: its state and the progress of what it asked of the port.
pub struct Station<'id, T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks> {
    config: StaConfig<K, C>,
    profile: Option<StaProfile<K>>,
    state: State<T, K>,
    keys: Keys,
    configuration: Configuration,
    /// The buffer each frame is encoded into.
    frame: FrameBuffer,
    /// The station's frames out, one place a purpose.
    out: FramesOut,
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
    sequence: SequenceCounter,
    scan: ScanTable<SCAN_CAPACITY>,
    pmksa: StaPmksaCache,
    /// The connection attempt that owns new frames.
    epoch: Option<LinkEpoch>,
    epochs: LinkEpochs,
    /// Who holds the connected link.
    link: LinkCustody,
    /// Whether the port holds an HE BSS or association the station set
    /// ([`StaAction::SetHe`]), which it sets `Off` at its leave; every join
    /// sets the port's HE state again before authentication.
    he: bool,
}

/// Whether the link's next change includes the changes of the transmit
/// agreements: they wait for [`Station::poll_action`], and are due there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgreementChanges {
    Wait,
    Due,
}

/// Who holds the station's connected link ([`crate::link`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkCustody {
    /// The station: it numbers its own frames and admits its own data.
    Station,
    /// The link's holder: whether the station last told it the link
    /// carries data, and each TID's transmit window it last told it.
    Lent {
        carriage: crate::link::Carriage,
        windows: [Option<oer_protocols_ieee80211_core_datapath::plane::TxWindow>; 8],
    },
    /// The station asked for it back.
    Returning,
}

impl LinkCustody {
    /// Whether the link's holder numbers the link's frames.
    const fn lent(self) -> bool {
        !matches!(self, Self::Station)
    }
}

/// The station's receive configuration as it asked for it: one at a time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Configuration {
    /// None is being set.
    Idle,
    /// One is being set, for the connection attempt it names: an end of an
    /// earlier attempt's completes no transition of the current one.
    Setting(Option<LinkEpoch>),
}

/// Terminal-only output; finalization never requests a port operation.
pub enum StaFinalOutput<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    CommandCompleted(StaCommandTerminal<'id, Path>),
    Notify(StaNotice),
}

/// What a finished station keeps for its restart: the frames its RSN
/// deferred and the notice of its run.
struct Finished {
    deferred: DeferredEapol,
    notice: FinalNotice,
}

/// The notice a finished run owes its application.
#[derive(Clone, Copy, Debug)]
enum FinalNotice {
    /// None: the station was idle.
    None,
    /// Owed.
    Owed(StaNotice),
    /// Handed out ([`Station::poll_output`]).
    Given,
}

impl FinalNotice {
    /// The notice still owed, which is then given.
    fn give(&mut self) -> Option<StaNotice> {
        match core::mem::replace(self, Self::Given) {
            Self::Owed(notice) => Some(notice),
            other => {
                *self = other;
                None
            }
        }
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

/// A group key installation the station asked for, kept by the station
/// until its end: the key's index, material and place, and its replay lane.
#[derive(Debug)]
pub(crate) struct PendingGroupKey {
    pub(crate) epoch: LinkEpoch,
    key_id: GroupKeyId,
    material: Ccmp128Key,
    place: StaGroupPlace,
    /// The key's replay lane: its initial frontier until its end switches
    /// the lane to it.
    replay: PendingLane,
}

/// A pending group key's replay lane.
#[derive(Debug)]
enum PendingLane {
    /// As the station keeps it: already active, or the frontier the key
    /// starts from.
    Station(GroupReplay),
    /// The link's holder switched the key ID's lane to the key at its end
    /// and took this frontier from it, for the installation of that ID
    /// which stays.
    Switched(Option<oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState>),
}

impl PendingGroupKey {
    /// The key's end switches its key ID's lane of `replay` to the key,
    /// once.
    pub(crate) fn activate(&mut self, replay: &mut CcmpRxReplayEpoch) {
        let PendingLane::Station(GroupReplay::Retained(next)) =
            core::mem::replace(&mut self.replay, PendingLane::Station(GroupReplay::Active))
        else {
            // It was already RX-active under this ID, or switched; selecting
            // it changes neither counters nor the per-ID replay frontier.
            return;
        };
        self.replay = PendingLane::Switched(replay.exchange_group(self.key_id, next));
    }
}

/// The successor of a GTK. Only the replacement's end switches the active
/// replay lane; the old one stays usable meanwhile.
#[derive(Debug)]
pub(crate) struct PendingGroupReplacement {
    pub(crate) install: PendingGroupKey,
    /// The replaced installation's key ID, whose replay lane the end ends
    /// when it is another ID and no installation of it stays active.
    retired: Option<GroupKeyId>,
}

impl PendingGroupReplacement {
    /// The key ID whose replay lane the replacement's end ends.
    pub(crate) const fn retired_lane(&self) -> Option<GroupKeyId> {
        self.retired
    }
}

/// The existing installation being selected.
#[derive(Debug)]
pub(crate) struct PendingGroupSelection {
    pub(crate) install: PendingGroupKey,
}

/// A group key being removed: its place is not reused before the removal's
/// end.
#[derive(Debug)]
pub(crate) struct PendingGroupRemoval {
    pub(crate) key_id: GroupKeyId,
    place: StaGroupPlace,
    _material: Ccmp128Key,
    replay: GroupReplay,
}

impl PendingGroupRemoval {
    /// Whether the removed key's replay lane is active: its end ends the
    /// lane.
    pub(crate) const fn active(&self) -> bool {
        matches!(self.replay, GroupReplay::Active)
    }
}

/// The pairwise key's progress, as the station asked for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairwiseKey {
    /// None is installed.
    None,
    /// Its installation for the connection attempt it names is out.
    Installing(LinkEpoch),
    /// It is installed.
    Installed,
    /// Its removal is out.
    Removing,
}

/// The group table's operation: one at a time.
#[derive(Debug)]
enum GroupOperation {
    Idle,
    Install(PendingGroupKey),
    Replace(PendingGroupReplacement),
    Select(PendingGroupSelection),
    Remove(PendingGroupRemoval),
}

impl GroupOperation {
    /// The place the operation in progress names.
    const fn place(&self) -> Option<StaGroupPlace> {
        match self {
            Self::Idle => None,
            Self::Install(pending) => Some(pending.place),
            Self::Replace(pending) => Some(pending.install.place),
            Self::Select(pending) => Some(pending.install.place),
            Self::Remove(pending) => Some(pending.place),
        }
    }
}

struct Keys {
    pairwise: PairwiseKey,
    group: GroupOperation,
    groups: GroupKeys,
    /// The group keys the station's port holds at once.
    capacity: usize,
}

impl Keys {
    fn new(capacity: usize) -> Self {
        Self {
            pairwise: PairwiseKey::None,
            group: GroupOperation::Idle,
            groups: GroupKeys::new(),
            capacity,
        }
    }

    fn group_installed(
        &mut self,
        pending: PendingGroupKey,
        role: GroupRole,
        replay: Option<&mut CcmpRxReplayEpoch>,
        arrival: GroupArrival,
    ) {
        let mut pending = pending;
        // A link's holder switched the lane already, the station otherwise.
        if role == GroupRole::Current
            && let Some(replay) = replay
        {
            pending.activate(replay);
        }
        let PendingGroupKey {
            place,
            key_id,
            material,
            replay,
            ..
        } = pending;
        let replay = match replay {
            PendingLane::Station(replay) => replay,
            PendingLane::Switched(previous) => {
                if let Some(previous) = previous {
                    self.groups.retain(key_id, previous);
                }
                GroupReplay::Active
            }
        };
        let entry = InstalledGroup {
            place,
            key_id,
            material,
            replay,
        };
        match arrival {
            GroupArrival::Selected => self.groups.restore(entry, role),
            GroupArrival::Installed => self.groups.install(entry, role),
        }
    }

    /// An installed group key to remove, while no group operation is out.
    fn take_group_key(&mut self, removal: GroupRemoval) -> Option<InstalledGroup> {
        if !matches!(self.group, GroupOperation::Idle) {
            return None;
        }
        match removal {
            #[cfg(test)]
            GroupRemoval::Retiring => self.groups.take_retiring(),
            GroupRemoval::Any => self.groups.take_any(),
        }
    }

    fn current_group(&self) -> Option<GroupKeyId> {
        self.groups.current_id()
    }

    /// Whether the station's pairwise key is installed.
    const fn keyed(&self) -> bool {
        matches!(self.pairwise, PairwiseKey::Installed)
    }

    fn installed(&self) -> bool {
        self.keyed() && self.current_group().is_some()
    }

    /// Whether the group table holds no key and no group operation is out.
    fn groups_empty(&self) -> bool {
        matches!(self.group, GroupOperation::Idle) && self.groups.is_empty()
    }

    fn clear(&self) -> bool {
        matches!(self.pairwise, PairwiseKey::None) && self.groups_empty()
    }

    /// The group table's places the station may still fill.
    #[cfg(test)]
    fn free_group_places(&self) -> usize {
        self.capacity.saturating_sub(self.groups.occupied())
    }
}

/// A new variant description contains no State, Connected or ConnectedRsn owner.
#[expect(
    clippy::large_enum_variant,
    reason = "sub-limit variant initializers stay affine without an allocator"
)]
enum Change<T: StaTypes, K> {
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

struct NewConnection<T: StaTypes, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    qos_sequences: [SequenceCounter; 8],
    confirmation: Option<RsnStaResponseDeadline>,
}
impl<T: StaTypes, K> State<T, K> {
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
                    reason_code: reason_code::UNSPECIFIED,
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
            connected.rsn = Some(ConnectedRsn {
                management,
                supplicant,
                replay: Some(replay),
                message4: DuplicateMessage4::Send(response),
                rekey: Rekey::None,
                deferred: DeferredEapol::Empty,
                fault: None,
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
        *self = Self::Leave(Leave {
            reason,
            step,
            deferred,
        });
    }

    #[inline(never)]
    fn establish(&mut self, new: NewConnection<T, K>) {
        *self = Self::Connected(Connected {
            bssid: new.bssid,
            channel: new.channel,
            association_id: new.association_id,
            qos: new.qos,
            peer: new.peer,
            qos_sequences: new.qos_sequences,
            sequence: TidSequence::Unasked,
            confirmation: new.confirmation,
            report: new
                .confirmation
                .is_none()
                .then_some(UpstreamFact::Connected(new.channel)),
            edca: Owing::Clear,
            exchange: Owing::Clear,
            power: PowerSaveView::Awake,
            wake: Owing::Clear,
            transmission: Transmission::Free,
            background: None,
            connected_notice: if new.confirmation.is_none() {
                Owing::Owed
            } else {
                Owing::Clear
            },
            block_ack: None,
            tx_block_ack: TxAgreements::Unstarted,
            rsn: None,
        });
    }
}

fn current_epoch<T: StaTypes, K>(
    state: &State<T, K>,
    epoch: Option<LinkEpoch>,
) -> Option<LinkEpoch> {
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
enum State<T: StaTypes, K> {
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
    Connected(Connected<T, K>),
    /// Leaving the network.
    Leave(Leave),
    /// Telling the application why it is not connected, then idle.
    Report(StaDisconnectReason),
    /// The port stopped: the run ended ([`Station::finish`]) and the
    /// station waits for its restart.
    Finished(Finished),
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
    /// The receive rules are in place; the visit listens until `until`.
    NeedProbe {
        until: Instant,
    },
    /// Listening until `until`, or the lease's end if that comes first.
    Listen {
        until: Instant,
    },
    /// The window ended: the lease goes back.
    NeedRelease,
    /// The lease is given back and its end is due.
    AwaitEnd,
}

/// The join of one access point.
/// A candidate of the network's access points: its observation and what the
/// join decided of it before any frame.
#[derive(Clone, Copy)]
struct Candidate<K> {
    observation: BssObservation,
    rsn: SelectedRsn,
    channel: SupportedChannel<K>,
    phy: NegotiatedPhy,
}

struct Join<T: StaTypes, K> {
    access_point: ScanRecord,
    /// The ERP protection the access point's ERP elements ask for.
    erp: oer_protocols_ieee80211_wire::protection::ErpProtection,
    rsn: SelectedRsn,
    channel: SupportedChannel<K>,
    /// The PHY the join associates with, on `channel`, decided once from the
    /// access point's scan record with what its request carries.
    phy: NegotiatedPhy,
    /// Whether the join set the port's HE state before authentication: the
    /// HE BSS of an HE20 join, `Off` for another.
    he_set: bool,
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
    /// An HE20 association to install in the port, after its EDCA
    /// parameters and before the link's data.
    SetHe {
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
    NeedSend(StaAuthenticationRuntime, AssociationKey),
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
struct NewHandshake<T: StaTypes, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    supplicant: Option<RsnStaSupplicant>,
    spp: oer_protocols_ieee80211_wire::security::Spp,
    pmk: Pmk,
    qos_sequences: [SequenceCounter; 8],
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    management: Option<ManagementProtection>,
}

struct Handshake<T: StaTypes, K> {
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    qos: bool,
    peer: Peer<T::RxMeta>,
    supplicant: Option<RsnStaSupplicant>,
    spp: oer_protocols_ieee80211_wire::security::Spp,
    pmk: Pmk,
    qos_sequences: [SequenceCounter; 8],
    /// The deadline of the message the station waits for.
    deadline: RsnStaResponseDeadline,
    step: HandshakeStep,
    management: Option<ManagementProtection>,
}

impl<T: StaTypes, K> Handshake<T, K> {
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

/// The next Sequence Number of a TID the station's ADDBA Request names,
/// while the link's holder numbers the TID ([`LinkAction::Sequence`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TidSequence {
    /// No ADDBA Request waits for a number.
    Unasked,
    /// The station asked the holder for the TID's next number.
    Asked(WmmUserPriority),
    /// The holder told it: the TID's space holds it, and the holder takes
    /// no data of the TID until the request goes.
    Fresh(WmmUserPriority),
}

/// The connected station.
struct Connected<T: StaTypes, K> {
    confirmation: Option<RsnStaResponseDeadline>,
    bssid: Bssid,
    channel: SupportedChannel<K>,
    association_id: u16,
    /// The access point's BSS is a QoS BSS (it advertised WMM).
    qos: bool,
    /// The access point as the association made it.
    peer: Peer<T::RxMeta>,
    /// Each TID's next Sequence Number of the association: the spaces
    /// themselves while the station holds its link; once the link's holder
    /// has them, as the holder last reported them after data
    /// ([`Station::tx_traffic`]), advanced by the station's own frames of
    /// the TID, so its ADDBA Request names the next number of the TID.
    qos_sequences: [SequenceCounter; 8],
    /// The TID whose next Sequence Number the station asked the link's
    /// holder for its ADDBA Request.
    sequence: TidSequence,
    /// The upstream fact the arbiter has yet to hear.
    report: Option<UpstreamFact<K>>,
    /// The access point's beacons announced another EDCA set, already in
    /// `peer`, which the interface has yet to take.
    edca: Owing,
    /// The primary lease to exchange after the port moved.
    exchange: Owing,
    /// What the access point knows of the station's power state.
    power: PowerSaveView,
    /// A Null with PM=0 the station owes its access point.
    wake: Owing,
    /// Admission: at most one connected control transmission at a time.
    transmission: Transmission,
    /// The background scan, if one runs.
    background: Option<Background>,
    /// The application has yet to hear the station connected.
    connected_notice: Owing,
    /// Parsed management to hand to the service before the next RX input.
    block_ack: Option<oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction>,
    /// Whether the transmit Block Ack originator started this association.
    tx_block_ack: TxAgreements,
    /// The supplicant of a protected link and its group key's handshake.
    rsn: Option<ConnectedRsn>,
}

/// Whether the connected station still owes something: a notice, an
/// exchange or a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Owing {
    Owed,
    Clear,
}

/// The connected station's one control transmission at a time: none, one,
/// or a Null with PM=0, whose outstanding transition beacons need not
/// request again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transmission {
    Free,
    Busy,
    Waking,
}

/// Whether the transmit Block Ack originator started the association's
/// agreements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxAgreements {
    Unstarted,
    Started,
}

/// How a lease or an absence ended: at its own end, or cancelled before it
/// began.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseEnd {
    Ended,
    Cancelled,
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
    /// An ADDBA Request negotiating the station's transmit agreement of `tid`.
    AddbaRequest { tid: u8 },
    /// A DELBA ending one of the station's transmit agreements.
    Delba,
    /// A BlockAckReq moving the recipient's window of `tid`.
    BlockAckRequest { tid: u8 },
}

/// The DELBA body that ends the station's own transmit agreement of `tid`
/// after its inactivity: the station is its originator (IEEE Std
/// 802.11-2020, 9.6.5.5, 10.25.5).
fn own_delba(
    tid: u8,
) -> Result<
    [u8; oer_protocols_ieee80211_wire::block_ack::frame::DELBA_ACTION_BODY_LEN],
    oer_protocols_ieee80211_wire::block_ack::frame::AddbaRequestError,
> {
    oer_protocols_ieee80211_wire::block_ack::frame::encode_delba(
        tid,
        oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Originator,
        reason_code::TIMEOUT,
    )
}

/// The station's transmit agreements in `connected`'s association, started
/// afresh at their first use in it.
fn tx_agreements<'c, T: StaTypes, K>(
    tx: &'c mut Option<StaTxBlockAck>,
    connected: &mut Connected<T, K>,
) -> Option<&'c mut StaTxBlockAck> {
    let tx = tx.as_mut()?;
    if connected.tx_block_ack == TxAgreements::Unstarted {
        connected.tx_block_ack = TxAgreements::Started;
        tx.associate(
            connected.qos
                && connected.peer.associated.phy.format()
                    != oer_protocols_ieee80211_contracts_port::PhyFormat::NonHt,
        );
    }
    Some(tx)
}

/// Hand a received Block Ack action to the transmit agreements, or keep it
/// for the receive agreements.
fn route_block_ack<T: StaTypes, K>(
    tx: &mut Option<StaTxBlockAck>,
    connected: &mut Connected<T, K>,
    action: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction,
    now: Instant,
) {
    if !tx_agreements(tx, connected).is_some_and(|tx| tx.on_action(action, now)) {
        connected.block_ack = Some(action);
    }
}

/// The protection of the connected station's robust management frames:
/// protected once the association protects management frames.
fn management_frame_protection<T: StaTypes, K>(
    connected: &Connected<T, K>,
) -> oer_protocols_ieee80211_wire::frame::FrameProtection {
    if connected
        .rsn
        .as_ref()
        .is_some_and(|rsn| rsn.management.is_some())
    {
        oer_protocols_ieee80211_wire::frame::FrameProtection::Protected
    } else {
        oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected
    }
}

/// Whether the connected link carries data now: confirmed, home, awake and,
/// on a protected link, keyed.
fn data_link_up<T: StaTypes, K, A>(
    connected: &Connected<T, K>,
    absence: Option<&A>,
    keys: &Keys,
) -> bool {
    connected.confirmation.is_none()
        && absence.is_none()
        && !connected.power.asleep()
        && (connected.rsn.as_ref().is_none() || keys.keyed())
}

/// The data plane's state of `connected`'s link, the station at `address`:
/// its header template (under the pairwise key once `protection` is
/// CCMP's), the admission
/// its access point's EDCA set names, the retry of the transmit agreements
/// `tx` and the access point's A-MPDU limits; no agreement is operational
/// yet. `None` for a BSSID the station sends no data to.
fn handover_data<T: StaTypes, K>(
    address: OwnAddress,
    bssid: Bssid,
    connected: &Connected<T, K>,
    protection: oer_protocols_ieee80211_wire::security::LinkProtection,
    tx: Option<&StaTxBlockAck>,
) -> Option<oer_protocols_ieee80211_core_datapath::plane::DataLink> {
    let header = oer_protocols_ieee80211_wire::station::StaDataHeaderTemplate::new(
        address,
        bssid,
        if connected.qos {
            oer_protocols_ieee80211_wire::data::PeerQos::Qos
        } else {
            oer_protocols_ieee80211_wire::data::PeerQos::NonQos
        },
        protection,
    )?;
    let parameters = oer_protocols_ieee80211_wire::ht::HtAmpduParameters::from_bytes([connected
        .peer
        .associated
        .ht_ampdu_parameters]);
    let ht =
        oer_protocols_ieee80211_contracts_port::MpduStartSpacing::new(parameters.spacing().get())
            .map(
                |spacing| oer_protocols_ieee80211_core_datapath::plane::HtLimits {
                    spacing,
                    max_length: parameters.max_ampdu_length(),
                },
            );
    Some(oer_protocols_ieee80211_core_datapath::plane::DataLink {
        header,
        admission: AcAdmission::from_edca(connected.peer.associated.edca),
        retry: tx.map(StaTxBlockAck::retry_members),
        ht,
        windows: [None; 8],
        protection: connected.peer.associated.protection,
    })
}

/// The next TID whose operational transmit agreement in `connected`'s
/// association is not the window in `told`, once the agreements changed;
/// `told` takes it.
fn agreement_change<T: StaTypes, K>(
    tx: &mut Option<StaTxBlockAck>,
    connected: &Connected<T, K>,
    told: &mut [Option<oer_protocols_ieee80211_core_datapath::plane::TxWindow>; 8],
) -> Option<LinkAction> {
    let tx = tx
        .as_mut()
        .filter(|_| connected.tx_block_ack == TxAgreements::Started)?;
    if !tx.changed() {
        return None;
    }
    for tid in WmmUserPriority::ALL {
        let window = tx.operational(tid.value()).map(|agreement| {
            oer_protocols_ieee80211_core_datapath::plane::TxWindow {
                head: agreement.starting_sequence,
                size: agreement.window,
            }
        });
        let heard = tid.select_mut(told);
        if *heard != window {
            *heard = window;
            return Some(LinkAction::Agreement { tid, window });
        }
    }
    tx.settled();
    None
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
    /// The replay check of the association's keys, while the station holds
    /// its link; the link's holder keeps it from the handover to the
    /// return.
    replay: Option<CcmpRxReplayEpoch>,
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
            .complete_group_key_install(request)
            .map_err(|_| StaDisconnectReason::HandshakeFailed)?;
        self.rekey = Rekey::Send(EapolResponse { frame, protection });
        Ok(())
    }

    #[inline(never)]
    fn receive<E: oer_portable_crypto::block::Aes128Engine>(
        &mut self,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
        engine: &mut E,
    ) -> Result<(), StaRxError> {
        if self.fault.is_some() {
            return Ok(());
        }
        match self.receive_eapol(eapol, protection, engine) {
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

    fn receive_eapol<E: oer_portable_crypto::block::Aes128Engine>(
        &mut self,
        eapol: OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
        engine: &mut E,
    ) -> Result<(), RsnConnectedSupplicantError> {
        use oer_protocols_ieee80211_core_security_rsn::EapolKeyMessage;
        let message = eapol
            .key_frame()
            .map_err(RsnConnectedSupplicantError::from)?
            .message();
        let action = match (message, protection) {
            (EapolKeyMessage::PairwiseMessage3, _) => self.supplicant.on_message3(eapol, engine),
            (EapolKeyMessage::GroupMessage1, EapolProtection::Pairwise) => {
                self.supplicant.on_group_message1(eapol, engine)
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
            Ok(RsnConnectedAction::GroupKeyData(request)) => {
                let current = match &self.rekey {
                    Rekey::Install(current, _) | Rekey::AwaitInstalled(current, _) => {
                        current.replay_counter()
                    }
                    Rekey::Unwrap(current, _) => current.replay_counter(),
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
        // A refusal returns to the caller, which counts it.
        if let Err(error) = result
            && let Some(reason) = connected_rsn_fault(error)
        {
            self.fault = Some(reason);
            return Err(error);
        }
        self.advance_deferred();
        result
    }

    /// Move the smallest verified replay counter into unwrap work.
    /// M3 and GM1 advance the same frontier; their kind grants no priority.
    #[inline(never)]
    fn advance_deferred(&mut self) {
        if !matches!(self.rekey, Rekey::None)
            || matches!(self.message4, DuplicateMessage4::AwaitSent)
        {
            return;
        }
        let (request, protection, rest) =
            match core::mem::replace(&mut self.deferred, DeferredEapol::Empty) {
                DeferredEapol::Empty => return,
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
        self.rekey = Rekey::Unwrap(request, protection);
    }

    /// Unwrap the key data of an authenticated frame over `engine` and
    /// prepare its installation. Key data that is too long, does not unwrap
    /// or does not parse is a counted protocol drop; the next deferred frame
    /// follows.
    fn prepare_install(
        &mut self,
        frame: RsnGroupKeyFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        protection: EapolProtection,
        engine: &mut impl oer_portable_crypto::block::Aes128Engine,
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<(), StaDisconnectReason> {
        match self.supplicant.prepare_group_key_install(frame, engine) {
            Ok(install) => {
                self.rekey = Rekey::Install(install, protection);
                return Ok(());
            }
            Err(RsnConnectedProcessError::Supplicant(error)) => {
                if let Some(reason) = connected_rsn_fault(error) {
                    return Err(reason);
                }
                report(events, || StaEvent::RsnInputDropped(RsnDrop::EapolRefused));
            }
            Err(RsnConnectedProcessError::KeyUnwrap(_)) => report(events, || {
                StaEvent::RsnInputDropped(RsnDrop::KeyUnwrapFailed)
            }),
            Err(RsnConnectedProcessError::KeyDataTooLong) => report(events, || {
                StaEvent::RsnInputDropped(RsnDrop::KeyDataTooLong)
            }),
        }
        self.advance_deferred();
        Ok(())
    }
}

fn sae_derivation_failure(
    error: oer_protocols_ieee80211_core_security_rsn::sae::SaeError,
    peer: [u8; 6],
    pwe: oer_protocols_ieee80211_wire::security::SaePwe,
) -> StaDisconnectReason {
    StaDisconnectReason::Sae(match error {
        oer_protocols_ieee80211_core_security_rsn::sae::SaeError::Crypto => StaSaeFailure::Crypto {
            peer,
            pwe: pwe.into(),
        },
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

/// How the MPDU that carried an EAPOL frame was protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolProtection {
    /// It came open.
    Open,
    /// It came under the pairwise key.
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
    /// An authenticated M3 or GM1 whose key data the station unwraps in its
    /// next action turn.
    Unwrap(
        RsnGroupKeyFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        EapolProtection,
    ),
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

/// A checked management disconnect from the selected BSS.
fn peer_disconnect(frame: &[u8], local: OwnAddress, bssid: Bssid) -> Option<u16> {
    oer_protocols_ieee80211_wire::station::parse_sta_disconnect(frame, local, bssid)
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
fn active_replay<T: StaTypes, K>(state: &mut State<T, K>) -> Option<&mut CcmpRxReplayEpoch> {
    match state {
        State::Connected(connected) => connected.rsn.as_mut().and_then(|rsn| rsn.replay.as_mut()),
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
type StaDone<T> = Done<<T as StaTypes>::AckEvidence, <T as StaTypes>::AirDenied>;

/// An action a state owes.
enum Owed<'id, T: StaTypes, K> {
    BlockAck(PendingRxBlockAck),
    InstallPairwise {
        peer: IndividualAddress,
        key: Ccmp128Key,
        spp: oer_protocols_ieee80211_wire::security::Spp,
    },
    InstallGroup {
        place: StaGroupPlace,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    ReplaceGroup {
        place: StaGroupPlace,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    RemovePairwise,
    SelectGroup {
        place: StaGroupPlace,
        key_id: GroupKeyId,
    },
    RemoveGroup(StaGroupPlace),
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
    Configure(StationReceive<K>),
    SetEdca(SupportedEdca<K>),
    SetHe(oer_protocols_ieee80211_contracts_port::StationHe),
    /// A frame in the station's buffer.
    Transmit(Encoded<K>),
    /// A frame in the station's buffer, under the pairwise key.
    TransmitProtected(Encoded<K>),
    /// Ask the link's holder for the TID's next Sequence Number.
    LinkSequence(WmmUserPriority),
    /// Tell the link's holder the link carries data again: an ADDBA Request
    /// whose number the holder told does not go.
    LinkRelease,
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

/// What composition grants one station: its places with the channel
/// arbiter.
pub struct StationResources<'id, T: StaTypes> {
    /// Primary-channel lease admission.
    pub place: LeasePlace<'id, T::GatePath>,
    /// Temporary channel-absence admission.
    pub absence_place: AbsencePlace<'id, T::GatePath>,
}

/// The proof that a personal station's group-key capacity `GROUPS` is
/// nonzero: a protected station installs the GTK into one of these slots.
///
/// A composition creates it in a `const` item, where a zero capacity fails
/// the build:
///
/// ```
/// use oer_protocols_ieee80211_core_roles_sta::machine::GroupCapacity;
/// const GROUPS: GroupCapacity<1> = GroupCapacity::new();
/// # let _ = GROUPS;
/// ```
///
/// ```compile_fail,E0080
/// use oer_protocols_ieee80211_core_roles_sta::machine::GroupCapacity;
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
                panic!("a station needs a group slot for any protected profile");
            }
        }
        Self { _proof: () }
    }
}

impl<'id, T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks> Station<'id, T, K, C> {
    /// End this run after the port's stop: every accepted command ends and
    /// the station's places go back to the arbiter. The station owes no
    /// configuration, key removal or frame to a port which stopped; it keeps
    /// its profile, its configuration and its keys' progress for
    /// [`Self::restart`], and its terminal outputs for [`Self::poll_output`].
    pub fn finish(&mut self, reason: StaDisconnectReason) -> StationPlaceReturns<'id, T::GatePath> {
        let notice = match self.finish_commands(reason) {
            Some(notice) => FinalNotice::Owed(notice),
            None => FinalNotice::None,
        };
        let deferred = self.state.take_deferred();
        self.state = State::Finished(Finished { deferred, notice });
        self.lease = Held::None;
        self.absence = None;
        self.epoch = None;
        // A frame out is still the port's: only its end frees its place.
        self.link = LinkCustody::Station;
        StationPlaceReturns::new(self.place.take(), self.absence_place.take())
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

    /// A finished station's terminal outputs: its accepted requests' ends,
    /// then the one lifecycle notice.
    pub fn poll_output(&mut self) -> Option<StaFinalOutput<'id, T::ConfigurationPath>> {
        if let Some(terminal) = self.commands.poll() {
            return Some(StaFinalOutput::CommandCompleted(terminal));
        }
        match &mut self.state {
            State::Finished(finished) => finished.notice.give().map(StaFinalOutput::Notify),
            _ => None,
        }
    }

    /// The frames the RSN of a finished station deferred, once.
    pub fn take_deferred(&mut self) -> DeferredEapol {
        match &mut self.state {
            State::Finished(finished) => {
                core::mem::replace(&mut finished.deferred, DeferredEapol::Empty)
            }
            _ => DeferredEapol::Empty,
        }
    }

    /// Start the next run of a finished station in place, with the places
    /// the arbiter gave back. It first removes the keys and the receive
    /// configuration the last run left, then takes a Connect; it gives back
    /// the frames its RSN deferred and a notice its last run did not hand
    /// out. A station that is not finished stays as it is.
    pub fn restart(
        &mut self,
        places: StationPlaces<'id, T::GatePath>,
    ) -> (DeferredEapol, Option<StaNotice>) {
        let (place, absence_place) = places.into_parts();
        self.place = Some(place);
        self.absence_place = Some(absence_place);
        let State::Finished(finished) =
            core::mem::replace(&mut self.state, State::Reclaiming(Reclaim::Keys))
        else {
            return (DeferredEapol::Empty, None);
        };
        self.scan.clear();
        // The next run's MAC starts without an HE association.
        self.he = false;
        let mut notice = finished.notice;
        (finished.deferred, notice.give())
    }

    /// Construct the station directly in caller-provided storage. It has no
    /// profile until [`Self::set_profile`]; `capacity` proves the group-key
    /// slots every protected profile needs.
    #[cfg(feature = "placement")]
    pub fn init<'a, const GROUPS: usize>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        config: StaConfig<K, C>,
        resources: StationResources<'id, T>,
        _capacity: GroupCapacity<GROUPS>,
    ) -> (&'a mut Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config_in_place::<GROUPS>(storage, config, resources)
    }

    /// Build the station. It has no profile until [`Self::set_profile`];
    /// `capacity` proves the group-key slots every protected profile needs.
    #[doc = include_str!("machine/resources_contracts.md")]
    pub fn new<const GROUPS: usize>(
        config: StaConfig<K, C>,
        resources: StationResources<'id, T>,
        _capacity: GroupCapacity<GROUPS>,
    ) -> (Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        Self::from_config::<GROUPS>(config, resources)
    }

    fn from_config<const GROUPS: usize>(
        config: StaConfig<K, C>,
        resources: StationResources<'id, T>,
    ) -> (Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        const {
            let _: (
                SupportedFilter<K, StationRole>,
                SupportedFilter<K, StationRole>,
            ) = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        }
        let StationResources {
            place,
            absence_place,
        } = resources;
        let (commands, credits) = Commands::new();
        (
            Self {
                config,
                profile: None,
                state: State::Idle,
                keys: Keys::new(GROUPS),
                configuration: Configuration::Idle,
                frame: FrameBuffer::new(),
                out: FramesOut::default(),
                place: Some(place),
                lease: Held::None,
                absence_place: Some(absence_place),
                absence: None,
                commands,
                sequence: SequenceCounter::new(SequenceNumber::ZERO),
                scan: ScanTable::new(),
                pmksa: StaPmksaCache::new(),
                epoch: None,
                epochs: LinkEpochs::new(),
                link: LinkCustody::Station,
                he: false,
            },
            credits,
        )
    }

    #[cfg(feature = "placement")]
    #[inline(never)]
    fn from_config_in_place<'a, const GROUPS: usize>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        config: StaConfig<K, C>,
        resources: StationResources<'id, T>,
    ) -> (&'a mut Self, StaCommandCredits<'id, T::ConfigurationPath>) {
        const {
            let _: (
                SupportedFilter<K, StationRole>,
                SupportedFilter<K, StationRole>,
            ) = (Rules::<K>::MEMBER, Rules::<K>::WIDENED);
        }
        let StationResources {
            place,
            absence_place,
        } = resources;
        let (commands, credits) = Commands::new();
        (
            oer_portable_memory::init_in_place!(storage, Self {
                config: config,
                profile: None,
                state: State::Idle,
                keys: Keys::new(GROUPS),
                configuration: Configuration::Idle,
                frame: FrameBuffer::new(),
                out: FramesOut::default(),
                place: Some(place),
                lease: Held::None,
                absence_place: Some(absence_place),
                absence: None,
                commands: commands,
                sequence: SequenceCounter::new(SequenceNumber::ZERO),
                scan => ScanTable::init_in_place,
                pmksa: StaPmksaCache::new(),
                epoch: None,
                epochs: LinkEpochs::new(),
                link: LinkCustody::Station,
                he: false,
            }),
            credits,
        )
    }

    /// Whether an operation still belongs to this connection attempt.
    /// Resource return is unconditional; protocol and rate updates use this guard.
    pub fn is_current(&self, context: &StaTxTag) -> bool {
        Some(context.epoch) == self.epoch
    }

    /// Whether the station is connected.
    pub const fn is_connected(&self) -> bool {
        match &self.state {
            State::Connected(connected) => connected.confirmation.is_none(),
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
            State::Connected(connected) => Some(&connected.peer),
            _ => None,
        }
    }

    /// The link's next change its holder applies before it carries data
    /// again ([`crate::link`]): its handover, a hold or release of its data,
    /// or its return. A change of a transmit agreement waits for
    /// [`Self::poll_action`], as each of these does too.
    pub fn poll_link(&mut self) -> Option<LinkAction> {
        self.owe_link(AgreementChanges::Wait)
    }

    /// The link's next change: once connected the station hands its link
    /// over; it holds the link's data while it sleeps, is away or its keys
    /// are not confirmed and releases it again; with `agreements` due it
    /// adds each change of a TID's transmit agreement; once it left, it asks
    /// for the link back.
    fn owe_link(&mut self, agreements: AgreementChanges) -> Option<LinkAction> {
        let protection = self.profile.as_ref().map_or(
            oer_protocols_ieee80211_wire::security::LinkProtection::Open,
            |profile| profile.security.policy().link_protection(),
        );
        let connected = match &mut self.state {
            State::Connected(connected) => Some(connected),
            _ => None,
        };
        match (self.link, connected) {
            (LinkCustody::Station, Some(connected)) => {
                let epoch = self.epoch?;
                // A protected link goes once its key is installed, with its
                // replay check; the key itself is the engine's.
                let (header_protection, replay) = match connected.rsn.as_mut() {
                    Some(rsn) => {
                        if !self.keys.keyed() || rsn.replay.is_none() {
                            return None;
                        }
                        (
                            oer_protocols_ieee80211_wire::security::LinkProtection::Ccmp,
                            rsn.replay.take(),
                        )
                    }
                    None => (
                        oer_protocols_ieee80211_wire::security::LinkProtection::Open,
                        None,
                    ),
                };
                let address = self.config.address;
                let bssid = connected.bssid;
                let data = handover_data(
                    address,
                    bssid,
                    connected,
                    header_protection,
                    self.config.tx_block_ack.as_ref(),
                );
                if let Some(tx) = self.config.tx_block_ack.as_mut() {
                    tx.mark_changed();
                }
                let numbers = TxNumbers::new(
                    self.sequence,
                    connected.qos.then_some(connected.qos_sequences),
                );
                self.link = LinkCustody::Lent {
                    carriage: crate::link::Carriage::Held,
                    windows: [None; 8],
                };
                Some(LinkAction::HandOver(LinkHandover::new(
                    epoch,
                    data,
                    numbers,
                    RxLink::new(address, bssid, protection, replay),
                )))
            }
            (LinkCustody::Lent { .. }, None) => {
                self.link = LinkCustody::Returning;
                Some(LinkAction::Return)
            }
            (
                LinkCustody::Lent {
                    mut carriage,
                    mut windows,
                },
                Some(connected),
            ) => {
                let now = if data_link_up(connected, self.absence.as_ref(), &self.keys) {
                    crate::link::Carriage::Carries
                } else {
                    crate::link::Carriage::Held
                };
                let action = if now != carriage {
                    carriage = now;
                    Some(match now {
                        crate::link::Carriage::Carries => LinkAction::Release,
                        crate::link::Carriage::Held => LinkAction::Hold,
                    })
                } else if agreements == AgreementChanges::Due {
                    agreement_change(&mut self.config.tx_block_ack, connected, &mut windows)
                } else {
                    None
                };
                self.link = LinkCustody::Lent { carriage, windows };
                action
            }
            (LinkCustody::Station | LinkCustody::Returning, _) => None,
        }
    }

    /// Take the link its holder gave back: the station numbers its own
    /// frames again from where the link left them and, still connected,
    /// holds its replay check again.
    pub fn link_returned(&mut self, link: LinkReturn) {
        let LinkReturn { station, tids, rx } = link;
        self.sequence = station;
        let replay = rx.into_replay();
        if let State::Connected(connected) = &mut self.state {
            if let Some(tids) = tids {
                connected.qos_sequences = tids;
            }
            if let Some(rsn) = connected.rsn.as_mut() {
                rsn.replay = replay;
            }
            connected.sequence = TidSequence::Unasked;
        }
        self.link = LinkCustody::Station;
    }

    /// The frame `encoded` in the station's buffer, lent to its action.
    fn hand_out(&mut self, encoded: Encoded<K>) -> StaTransmit<'_, K> {
        let Encoded {
            tag,
            frame,
            class,
            plan,
            access_category,
            number,
        } = encoded;
        StaTransmit {
            tag,
            bytes: self.frame.bytes.as_mut_slice(),
            frame,
            class,
            plan,
            access_category,
            number,
        }
    }

    /// The record of the group key operation at `place`, whose end the
    /// link's holder applies to its replay epoch first
    /// ([`LinkHandover::group_terminal`]).
    pub(crate) fn group_lane(
        &mut self,
        place: StaGroupPlace,
    ) -> Option<crate::link::GroupLane<'_>> {
        if self.keys.group.place() != Some(place) {
            return None;
        }
        Some(match &mut self.keys.group {
            GroupOperation::Idle => return None,
            GroupOperation::Install(pending) => crate::link::GroupLane::Installed(pending),
            GroupOperation::Replace(pending) => crate::link::GroupLane::Replaced(pending),
            GroupOperation::Select(pending) => crate::link::GroupLane::Selected(pending),
            GroupOperation::Remove(pending) => crate::link::GroupLane::Removed(pending),
        })
    }

    /// The link's holder tells the next Sequence Number of `tid` the
    /// station asked for ([`LinkAction::Sequence`]): the station's ADDBA
    /// Request of `tid` names it. An answer the station no longer waits for
    /// changes nothing.
    pub fn tx_sequence(&mut self, tid: WmmUserPriority, next: SequenceNumber) {
        let State::Connected(connected) = &mut self.state else {
            return;
        };
        if connected.sequence == TidSequence::Asked(tid) {
            *tid.select_mut(&mut connected.qos_sequences) = SequenceCounter::new(next);
            connected.sequence = TidSequence::Fresh(tid);
        }
    }

    /// Take an EAPOL frame its link admitted (`payload`, its EAPOL packet,
    /// from an MPDU of `protection`). [`StaEapol::Faulted`] once the
    /// station's RSN failed: its link gives no more data before it leaves.
    ///
    /// # Errors
    ///
    /// The station is not connected, its link is open, the packet does not
    /// fit, or its RSN refused the frame.
    #[inline(never)]
    pub fn eapol(
        &mut self,
        payload: &[u8],
        protection: EapolProtection,
    ) -> Result<StaEapol, StaRxError> {
        let result = (|| {
            let State::Connected(connected) = &mut self.state else {
                return Err(
                    oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason::NotConnected
                        .into(),
                );
            };
            let eapol =
                OwnedEapolFrame::try_copy(RsnInterface::Station, connected.bssid.get(), payload)
                    .map_err(|_| {
                        oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason::Payload
                    })?;
            let Some(rsn) = connected.rsn.as_mut() else {
                return Err(
                    oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason::Protection
                        .into(),
                );
            };
            rsn.receive(eapol, protection, self.config.callbacks.aes_engine())?;
            Ok(if rsn.fault.is_some() {
                StaEapol::Faulted
            } else {
                StaEapol::Taken
            })
        })();
        self.complete_commands();
        result
    }

    #[cfg(test)]
    /// The security policy of the current profile, if any.
    fn policy(&self) -> Option<StaSecurityPolicy> {
        self.profile
            .as_ref()
            .map(|profile| profile.security.policy())
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
            (StaCommand::Connect, State::Idle) if self.profile.is_none() => {
                Some(CommandRefusal::NoProfile)
            }
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

    /// Name the network the next Connect joins. Only an idle station takes
    /// a profile; otherwise it returns the same profile with the refusal a
    /// Connect would get now. The previous profile's secrets erase on drop,
    /// and the PMKSA cache forgets the previous network's associations.
    #[expect(
        clippy::result_large_err,
        reason = "a refused profile returns by value so its secrets stay with their owner"
    )]
    pub fn set_profile(&mut self, profile: StaProfile<K>) -> Result<(), StaProfileRefused<K>> {
        self.complete_commands();
        let reason = match &self.state {
            State::Idle => None,
            State::Connected(connected) if connected.confirmation.is_none() => {
                Some(CommandRefusal::AlreadyConnected)
            }
            _ => Some(CommandRefusal::Transitioning),
        };
        if let Some(reason) = reason {
            return Err(StaProfileRefused { profile, reason });
        }
        if let Some(previous) = self.profile.replace(profile) {
            self.pmksa.forget_context(&previous.ssid);
        }
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
                        phy: connected.peer.associated.phy,
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
    pub fn input(
        &mut self,
        now: Instant,
        input: StaInput<'id, T, K>,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let (mut context, state) = self.transition();
        context.input_event(state, now, input, events);
        context.report_step(state, now, events);
        self.complete_commands();
    }

    /// Take a received frame the station's link does not admit: a
    /// management frame, or a data frame before the link exists, whose
    /// EAPOL the handshake takes once admitted. Its data goes nowhere: a
    /// connected station's data is its link's ([`LinkHandover::receive`]).
    /// Refusals retain their typed reason.
    ///
    /// Data cannot be admitted again through the old second RX entry point:
    ///
    /// ```compile_fail,E0425
    /// let _ = oer_protocols_ieee80211_core_roles_sta::data::received;
    /// ```
    ///
    /// ```
    /// use oer_protocols_ieee80211_core_roles_sta::machine::{Station, StaTypes};
    /// use oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities;
    /// fn receive<T: StaTypes, K: Ieee80211Capabilities, C: oer_protocols_ieee80211_core_roles_sta::machine::StaCallbacks>(
    ///     station: &mut Station<'_, T, K, C>, now: oer_portable_time::Instant,
    ///     frame: &[u8], meta: T::RxMeta,
    /// ) {
    ///     let _ = station.receive(now, frame, meta, &mut ());
    /// }
    /// ```
    #[inline(never)]
    pub fn receive(
        &mut self,
        now: Instant,
        frame: &[u8],
        meta: T::RxMeta,
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<StaReceived, StaRxError> {
        let (mut context, state) = self.transition();
        let result = context.receive_frame(state, now, frame, meta, events);
        context.report_step(state, now, events);
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
    pub fn poll_action(
        &mut self,
        now: Instant,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<StaAction<'_, 'id, T, K>> {
        let owed = loop {
            self.complete_commands();
            if let Some(terminal) = self.commands.poll() {
                return Some(StaAction::CommandCompleted(terminal));
            }
            // The link's change comes first, also after a transition of
            // this call: a station that left asks for its link back before
            // it owes anything that needs it.
            if let Some(link) = self.owe_link(AgreementChanges::Due) {
                return Some(StaAction::Link(link));
            }
            let (mut context, state) = self.transition();
            // The step is the call's own return place, read in place: an
            // unchanged state's wait, the connected link's every turn,
            // moves neither change nor action.
            let mut step = context.owe(state, now, events);
            let changed = !matches!(step.0, Change::Keep);
            if changed {
                state.apply(core::mem::replace(&mut step.0, Change::Keep));
            }
            context.report_step(state, now, events);
            match step.1 {
                ActionProgress::Action(owed) => break owed,
                ActionProgress::Transition => continue,
                ActionProgress::Wait if changed => {
                    return self.owe_link(AgreementChanges::Due).map(StaAction::Link);
                }
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
            Owed::InstallPairwise { peer, key, spp } => {
                StaAction::InstallPairwiseKey { peer, key, spp }
            }
            Owed::InstallGroup { place, key_id, key } => {
                StaAction::InstallGroupKey { place, key_id, key }
            }
            Owed::RemovePairwise => StaAction::RemovePairwiseKey,
            Owed::ReplaceGroup { place, key_id, key } => {
                StaAction::ReplaceGroupKey { place, key_id, key }
            }
            Owed::SelectGroup { place, key_id } => StaAction::SelectGroupKey { place, key_id },
            Owed::RemoveGroup(place) => StaAction::RemoveGroupKey { place },
            Owed::Exchange(lease) => StaAction::ExchangeLease(lease),
            Owed::Request(request, place) => StaAction::RequestChannel(request, place),
            Owed::Release(lease) => StaAction::ReleaseChannel(lease),
            Owed::Upstream(fact) => StaAction::Upstream(fact),
            Owed::Configure(receive) => StaAction::Configure(receive),
            Owed::SetEdca(edca) => StaAction::SetEdca(edca),
            Owed::SetHe(he) => StaAction::SetHe(he),
            Owed::Transmit(encoded) => StaAction::Transmit(self.hand_out(encoded)),
            // A frame of the handed-over link goes under the link's key; one
            // the station numbered goes under its installed key. Owed only
            // while that key is there; without it the frame does not go and
            // its purpose's place is free again.
            Owed::TransmitProtected(encoded) => match encoded.number {
                FrameNumber::Link { .. } => StaAction::Transmit(self.hand_out(encoded)),
                FrameNumber::Open if self.keys.keyed() => {
                    StaAction::TransmitProtected(self.hand_out(encoded))
                }
                FrameNumber::Open => {
                    self.out.release(encoded.tag.purpose);
                    return None;
                }
            },
            Owed::LinkSequence(tid) => StaAction::Link(LinkAction::Sequence(tid)),
            Owed::LinkRelease => StaAction::Link(LinkAction::Release),
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
                // While its last request is out, only the response bound
                // runs.
                if self.out.holds(TxPurpose::Association) {
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
            State::Scan(Scan {
                step: ScanStep::NeedProbe { until } | ScanStep::Listen { until },
                ..
            }) => Some(*until),
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
                            connected.transmission != Transmission::Free
                                || self.out.holds(CONNECTED_PLACE)
                                || !(self.absence.is_none()
                                    && !connected.power.asleep()
                                    && self.keys.keyed()),
                        )
                    });
                let can_send = (connected.transmission == Transmission::Free)
                    && data_link_up(connected, self.absence.as_ref(), &self.keys)
                    && !self.out.holds(CONNECTED_PLACE);
                let tx_block_ack = self
                    .config
                    .tx_block_ack
                    .as_ref()
                    .filter(|_| connected.tx_block_ack == TxAgreements::Started)
                    .and_then(|tx| tx.deadline(can_send));
                crate::time::earliest(crate::time::earliest(background, management), tx_block_ack)
            }
            _ => None,
        }
    }

    /// Data of `tid` was taken for the connected link at `now`, and the
    /// link's holder numbers the TID's next MPDU `next`: the station's next
    /// ADDBA Request of the TID names it. An idle TID negotiates its
    /// transmit agreement, a live one stays active.
    pub fn tx_traffic(
        &mut self,
        tid: WmmUserPriority,
        next: SequenceNumber,
        now: Instant,
    ) -> StaTraffic {
        let State::Connected(connected) = &mut self.state else {
            return StaTraffic::Recorded;
        };
        if connected.qos {
            *tid.select_mut(&mut connected.qos_sequences) = SequenceCounter::new(next);
        }
        if tx_agreements(&mut self.config.tx_block_ack, connected)
            .is_some_and(|tx| tx.traffic(tid.value(), now))
        {
            StaTraffic::Input
        } else {
            StaTraffic::Recorded
        }
    }

    /// The data plane dropped a frame of `tid` under its transmit agreement
    /// and moved its window to `head`: the access point is owed a
    /// BlockAckReq moving its own window there.
    pub fn tx_block_ack_request(
        &mut self,
        tid: WmmUserPriority,
        head: oer_protocols_ieee80211_wire::sequence::SequenceNumber,
    ) {
        let State::Connected(connected) = &mut self.state else {
            return;
        };
        if let Some(tx) = tx_agreements(&mut self.config.tx_block_ack, connected) {
            tx.request(tid.value(), head);
        }
    }

    fn transition(&mut self) -> (TransitionContext<'_, 'id, T, K, C>, &mut State<T, K>) {
        (
            TransitionContext {
                config: &mut self.config,
                profile: &mut self.profile,
                keys: &mut self.keys,
                configuration: &mut self.configuration,
                frame: &mut self.frame,
                out: &mut self.out,
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
                link: &mut self.link,
                he: &mut self.he,
            },
            &mut self.state,
        )
    }
}

/// Disjoint resource loans used while the resident phase is mutably borrowed.
struct TransitionContext<'s, 'id, T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks> {
    config: &'s mut StaConfig<K, C>,
    profile: &'s mut Option<StaProfile<K>>,
    keys: &'s mut Keys,
    configuration: &'s mut Configuration,
    /// The buffer each frame is encoded into.
    frame: &'s mut FrameBuffer,
    /// The station's frames out, one place a purpose.
    out: &'s mut FramesOut,
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
    sequence: &'s mut SequenceCounter,
    scan: &'s mut ScanTable<SCAN_CAPACITY>,
    pmksa: &'s mut StaPmksaCache,
    /// The connection attempt that owns new TX operations.
    epoch: &'s mut Option<LinkEpoch>,
    epochs: &'s mut LinkEpochs,
    /// Who holds the connected link.
    link: &'s mut LinkCustody,
    /// Whether the port holds an HE association the station set.
    he: &'s mut bool,
}

impl<'s, 'id, T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks>
    TransitionContext<'s, 'id, T, K, C>
{
    // Connect is admitted only with a profile (`CommandRefusal::NoProfile`),
    // and only Idle replaces it: every connection phase below reads the
    // profile its Connect was admitted with.

    /// The security policy the scan selects and the join associates with.
    fn policy(&self) -> StaSecurityPolicy {
        self.profile
            .as_ref()
            .map_or(StaSecurityPolicy::Open, |profile| profile.security.policy())
    }

    /// Whether the profile protects its link.
    fn link_protection(&self) -> oer_protocols_ieee80211_wire::security::LinkProtection {
        self.policy().link_protection()
    }

    fn ssid(&self) -> &[u8] {
        self.profile
            .as_ref()
            .map_or(&[], |profile| profile.ssid.as_slice())
    }

    fn channel(&self, visit: usize) -> Option<SupportedChannel<K>> {
        self.config
            .channels
            .get(visit)
            .map(|scanned| scanned.channel)
    }

    /// Report `state`'s step of a connect at `now`.
    #[inline(always)]
    fn report_step(
        &self,
        state: &State<T, K>,
        now: Instant,
        events: &mut impl EventSink<StaEvent>,
    ) {
        report(events, || {
            let (step, channel) = self.connect_step(state);
            StaEvent::ConnectStep {
                at: now,
                step,
                channel,
            }
        });
    }

    /// `state`'s step of a connect and the channel it is on.
    fn connect_step(&self, state: &State<T, K>) -> (ConnectStep, Option<u8>) {
        match state {
            State::Scan(scan) => (
                match scan.step {
                    ScanStep::NeedLease | ScanStep::AwaitAnswer => ConnectStep::ScanAsk,
                    ScanStep::AwaitStart => ConnectStep::ScanTune,
                    ScanStep::NeedConfigure | ScanStep::AwaitConfigured => {
                        ConnectStep::ScanConfigure
                    }
                    ScanStep::NeedProbe { .. } => ConnectStep::ScanProbe,
                    ScanStep::Listen { .. } | ScanStep::NeedRelease | ScanStep::AwaitEnd => {
                        ConnectStep::ScanListen
                    }
                },
                self.channel(scan.visit)
                    .map(|channel| channel.channel().number()),
            ),
            State::Join(join) => (
                match join.step {
                    JoinStep::NeedLease | JoinStep::AwaitAnswer | JoinStep::RetryAt(_) => {
                        ConnectStep::JoinAsk
                    }
                    JoinStep::AwaitStart => ConnectStep::JoinTune,
                    JoinStep::NeedConfigure | JoinStep::AwaitConfigured => {
                        ConnectStep::JoinConfigure
                    }
                    JoinStep::Authenticate(_)
                    | JoinStep::SaeSend { .. }
                    | JoinStep::SaeAwait(_) => ConnectStep::Authenticate,
                    JoinStep::Associate(..) => ConnectStep::Associate,
                    JoinStep::SetEdca { .. }
                    | JoinStep::SetHe { .. }
                    | JoinStep::NotifyAssociated { .. }
                    | JoinStep::StartLink { .. } => ConnectStep::StartLink,
                },
                Some(join.channel.channel().number()),
            ),
            State::Handshake(_) => (ConnectStep::Handshake, None),
            State::Connected(_) => (ConnectStep::Connected, None),
            State::Idle
            | State::Reclaiming(_)
            | State::Leave(_)
            | State::Report(_)
            | State::Finished(_) => (ConnectStep::Rest, None),
        }
    }

    #[inline(never)]
    fn input_event(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        input: StaInput<'id, T, K>,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let done = match input {
            StaInput::FrameEnded { tag, end } => {
                // The purpose's place is free again whatever the state.
                self.out.release(tag.purpose);
                if Some(tag.epoch) != *self.epoch {
                    return;
                }
                if tag.purpose == TxPurpose::Probe {
                    report(events, || StaEvent::ScanProbeEnded {
                        at: now,
                        end: match &end {
                            StaFrameEnd::Sent(completion) => {
                                crate::events::ScanProbeEnd::of(completion)
                            }
                            StaFrameEnd::Refused(_) => crate::events::ScanProbeEnd::NotTransmitted,
                        },
                    });
                }
                match end {
                    StaFrameEnd::Sent(completion) => Done::Tx {
                        purpose: tag.purpose,
                        completion,
                    },
                    StaFrameEnd::Refused(reason) => Done::Unsent {
                        purpose: tag.purpose,
                        reason,
                    },
                }
            }
            StaInput::Tick => Done::Tick,
            StaInput::Configured => {
                let Configuration::Setting(epoch) =
                    core::mem::replace(self.configuration, Configuration::Idle)
                else {
                    return;
                };
                if epoch != current_epoch(state, *self.epoch) {
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
            StaInput::PairwiseKeyEnded(outcome) => {
                match (self.keys.pairwise, outcome) {
                    (PairwiseKey::Installing(epoch), StaKeyOutcome::Done) => {
                        self.keys.pairwise = PairwiseKey::Installed;
                        // Leave waits for every outstanding key installation
                        // to end; an old one's key is removed there.
                        if current_epoch(state, *self.epoch) == Some(epoch) {
                            self.keys_changed(state);
                        }
                    }
                    (PairwiseKey::Installing(epoch), StaKeyOutcome::Expired) => {
                        self.keys.pairwise = PairwiseKey::None;
                        if current_epoch(state, *self.epoch) == Some(epoch) {
                            self.key_expired(state);
                        }
                    }
                    (PairwiseKey::Removing, _) => self.keys.pairwise = PairwiseKey::None,
                    (PairwiseKey::None | PairwiseKey::Installed, _) => {}
                }
                return;
            }
            StaInput::GroupKeyEnded {
                place,
                outcome: StaKeyOutcome::Expired,
            } => {
                if self.keys.group.place() != Some(place) {
                    return;
                }
                let epoch = match core::mem::replace(&mut self.keys.group, GroupOperation::Idle) {
                    GroupOperation::Idle => return,
                    GroupOperation::Install(pending) => {
                        self.keys.groups.release(pending.place, events);
                        pending.epoch
                    }
                    GroupOperation::Replace(pending) => {
                        // The replaced key went with the place.
                        self.keys.groups.release(pending.install.place, events);
                        pending.install.epoch
                    }
                    GroupOperation::Select(pending) => {
                        // The selected installation stays, not current.
                        let epoch = pending.install.epoch;
                        (*self.keys).group_installed(
                            pending.install,
                            GroupRole::Other,
                            None,
                            GroupArrival::Selected,
                        );
                        epoch
                    }
                    GroupOperation::Remove(pending) => {
                        self.group_removed(state, pending, events);
                        return;
                    }
                };
                if current_epoch(state, *self.epoch) == Some(epoch) {
                    self.key_expired(state);
                }
                return;
            }
            StaInput::GroupKeyEnded {
                place,
                outcome: StaKeyOutcome::Done,
            } => {
                if self.keys.group.place() != Some(place) {
                    return;
                }
                match core::mem::replace(&mut self.keys.group, GroupOperation::Idle) {
                    GroupOperation::Idle => {}
                    GroupOperation::Install(pending) => {
                        self.group_transition(
                            state,
                            pending,
                            None,
                            GroupArrival::Installed,
                            events,
                        );
                    }
                    GroupOperation::Replace(pending) => {
                        let retired = pending.retired_lane();
                        self.group_transition(
                            state,
                            pending.install,
                            retired,
                            GroupArrival::Installed,
                            events,
                        );
                    }
                    GroupOperation::Select(pending) => {
                        self.group_transition(
                            state,
                            pending.install,
                            None,
                            GroupArrival::Selected,
                            events,
                        );
                    }
                    GroupOperation::Remove(pending) => self.group_removed(state, pending, events),
                }
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

    /// A group key's removal ended, done or expired: the station holds the
    /// key no longer.
    fn group_removed(
        &mut self,
        state: &mut State<T, K>,
        pending: PendingGroupRemoval,
        events: &mut impl EventSink<StaEvent>,
    ) {
        if pending.active()
            && let Some(replay) = active_replay(state)
        {
            replay.remove_group(pending.key_id);
        }
        self.keys.groups.release(pending.place, events);
    }

    /// A key installation of the current connection attempt did not take:
    /// the attempt ends as a failed handshake.
    #[inline(never)]
    fn key_expired(&mut self, state: &mut State<T, K>) {
        let change = match state {
            State::Handshake(handshake) => leave_keyed(
                StaDisconnectReason::HandshakeFailed,
                Some(Deauthentication {
                    bssid: handshake.bssid,
                    reason_code: reason_code::UNSPECIFIED,
                    // A protected frame needs the pairwise key the station
                    // still holds.
                    protected: handshake.management.is_some() && self.keys.keyed(),
                }),
            ),
            State::Connected(connected) => leave_connected(
                connected,
                StaDisconnectReason::HandshakeFailed,
                Some(reason_code::UNSPECIFIED),
            ),
            _ => Change::Keep,
        };
        state.apply(change);
    }

    #[inline(never)]
    fn group_transition(
        &mut self,
        state: &mut State<T, K>,
        pending: PendingGroupKey,
        retired: Option<GroupKeyId>,
        arrival: GroupArrival,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let current = current_epoch(state, *self.epoch) == Some(pending.epoch);
        let role = if current {
            GroupRole::Current
        } else {
            GroupRole::Other
        };
        // While the link's holder keeps the replay check, it applied the
        // end to it already.
        let replay = if current { active_replay(state) } else { None };
        let mut replay = replay;
        if let Some(old) = retired
            && let Some(replay) = &mut replay
        {
            replay.remove_group(old);
        }
        (*self.keys).group_installed(pending, role, replay, arrival);
        if current {
            self.group_installed(state, events);
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
                        finish_handshake_install(
                            handshake,
                            request,
                            replay,
                            protection,
                            self.config.callbacks.aes_engine(),
                        )
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
    /// stages (including a response waiting for its frame's place) stay as they are.
    fn group_installed(&mut self, state: &mut State<T, K>, events: &mut impl EventSink<StaEvent>) {
        match state {
            State::Connected(connected) => finish_connected_install(connected, events),
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

    /// The background scan's step after its absence ended (`end`), cancelled or
    /// not.
    fn absence_over(&mut self, state: &mut State<T, K>, end: LeaseEnd) {
        (*self.absence) = None;
        let limit = self.config.skipped_absences;
        let connected = match state {
            State::Connected(connected) if connected.confirmation.is_none() => connected,
            _ => return,
        };
        let Some(background) = &mut connected.background else {
            return;
        };
        background.step = if end == LeaseEnd::Cancelled {
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
                self.absence_over(state, LeaseEnd::Ended);
                Change::Keep
            }
            StationNotice::AbsenceCancelled(_, place) => {
                *self.absence_place = Some(place);
                self.absence_over(state, LeaseEnd::Cancelled);
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
                self.lease_ended(state, LeaseEnd::Ended)
            }
            StationNotice::LeaseCancelled(_, place) => {
                *self.lease = Held::None;
                *self.place = Some(place);
                self.lease_ended(state, LeaseEnd::Cancelled)
            }
            StationNotice::Moved(channel) => {
                if let State::Connected(connected) = &mut *state
                    && connected.confirmation.is_none()
                {
                    connected.channel = channel;
                    connected.exchange = Owing::Owed;
                }
                Change::Keep
            }
            StationNotice::Leave(reason) => {
                let reason_code = match reason {
                    LeaveReason::Stopped | LeaveReason::PortFailed => None,
                    LeaveReason::Unservable => Some(reason_code::DEAUTH_LEAVING),
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

    /// The state after the lease ended (`end`): at its own end or cancelled.
    fn lease_ended(&mut self, state: &mut State<T, K>, end: LeaseEnd) -> Change<T, K> {
        let lost = StaDisconnectReason::Arbiter(LeaveReason::PortFailed);
        match state {
            // A skipped absence: the same channel again, until too many in
            // a row end the scan with what it found.
            State::Scan(scan) if end == LeaseEnd::Cancelled => {
                let skipped = scan.skipped.saturating_add(1);
                if skipped >= self.config.skipped_absences {
                    self.select(ScanCoverage::Partial, scan.access_point)
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
                    Some(reason_code::DEAUTH_LEAVING),
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
                match now.checked_add(self.config.dwell) {
                    Some(until) => {
                        scan.step = ScanStep::NeedProbe { until };
                        Change::Keep
                    }
                    None => leave_unkeyed(StaDisconnectReason::DeadlineOverflow),
                }
            }
            (State::Join(join), done) => self.join_step(join, now, done),
            (State::Handshake(handshake), Done::Command(StaCommand::Disconnect)) => {
                leave_handshake(handshake, reason_code::DEAUTH_LEAVING)
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
                    leave_handshake(handshake, reason_code::FOUR_WAY_HANDSHAKE_TIMEOUT)
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
                    Some(reason_code::UNSPECIFIED),
                )
            }
            (
                State::Handshake(handshake),
                TxPurpose::HandshakeResponse | TxPurpose::HandshakeLast,
            ) => leave_handshake(handshake, reason_code::UNSPECIFIED),
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
                    Some(reason_code::FOUR_WAY_HANDSHAKE_TIMEOUT),
                )
            }
            Done::Command(StaCommand::Disconnect) => leave_connected(
                connected,
                StaDisconnectReason::Requested,
                Some(reason_code::DEAUTH_LEAVING),
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
                    connected.connected_notice = Owing::Owed;
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
                completion: TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled
                ),
                ..
            }
        ) {
            return leave_disabled(connected);
        }
        // A key of an earlier installation is refused for good: every later
        // frame under it would be refused too, so the link ends, without a
        // Deauthentication under that key.
        if matches!(
            done,
            Done::Tx {
                completion: TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::StalePermission
                ),
                ..
            }
        ) {
            return leave_connected(
                connected,
                StaDisconnectReason::TransmitRefused(TxRefusal::StaleKey),
                None,
            );
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
            connected.transmission = Transmission::Free;
            match sending {
                Sending::Wake => {
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
                        rsn.advance_deferred();
                    }
                }
                Sending::AddbaRequest { tid } => {
                    if outcome != NullOutcome::Delivered
                        && let Some(tx) = self.config.tx_block_ack.as_mut()
                    {
                        tx.request_failed(tid, now);
                    }
                }
                Sending::BlockAckRequest { tid } => {
                    if outcome != NullOutcome::Delivered
                        && let Some(tx) = self.config.tx_block_ack.as_mut()
                    {
                        tx.request_unanswered(tid);
                    }
                }
                Sending::Probe | Sending::SaQuery | Sending::BlockAck | Sending::Delba => {}
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
            None => self.select(ScanCoverage::Complete, access_point),
        }
    }

    /// The join of the best access point of the network the scan found:
    /// the strongest, unless one on the channel of the access point beside
    /// the station is within the margin of it.
    fn select(
        &mut self,
        scan: ScanCoverage,
        access_point_channel: Option<SupportedChannel<K>>,
    ) -> Change<T, K> {
        let ssid = self.ssid();
        let policy = self.policy();
        let Some(request) = self.profile.as_ref().map(|profile| profile.phy) else {
            return leave_unkeyed(StaDisconnectReason::NotFound { scan });
        };
        let channels = &self.config.channels;
        let wide = &self.config.wide_channels;
        // A network's access points: its SSID and a security selection the
        // station's policy admits, on a channel the station scans.
        let network = || {
            (*self.scan).observations().filter_map(move |observation| {
                if observation.record.ssid_bytes() != ssid {
                    return None;
                }
                let rsn = select_association_rsn(&observation.record, policy).ok()?;
                let scanned = channels
                    .iter()
                    .copied()
                    .find(|scanned| scanned.channel.channel().number() == observation.channel)?;
                Some((observation, rsn, scanned))
            })
        };
        // Its candidates: those that offer a format the request admits, with
        // the decision taken once from their scan records.
        let candidates = || {
            network().filter_map(move |(observation, rsn, scanned)| {
                let (channel, phy) = negotiate(
                    &request,
                    scanned.he_power,
                    &observation.record,
                    wide,
                    scanned.channel,
                )?;
                Some(Candidate {
                    observation: *observation,
                    rsn,
                    channel,
                    phy,
                })
            })
        };
        let Some(strongest) = candidates().max_by_key(|candidate| candidate.observation.rssi)
        else {
            let reason = if network().next().is_some() {
                StaDisconnectReason::NoAdmittedFormat { scan }
            } else {
                StaDisconnectReason::NotFound { scan }
            };
            return leave_unkeyed(reason);
        };
        let margin = i16::from(self.config.access_point_margin_db);
        // Within the margin of the strongest: a known level no more than the
        // margin below it; an unknown level only beside an unknown strongest.
        let within_margin = |rssi: Option<i8>| match (rssi, strongest.observation.rssi) {
            (Some(rssi), Some(strongest)) => {
                i16::from(rssi) >= i16::from(strongest).saturating_sub(margin)
            }
            (_, None) => true,
            (None, Some(_)) => false,
        };
        let beside = access_point_channel.and_then(|channel| {
            candidates()
                .filter(|candidate| candidate.observation.channel == channel.channel().number())
                .filter(|candidate| within_margin(candidate.observation.rssi))
                .max_by_key(|candidate| candidate.observation.rssi)
        });
        let Candidate {
            observation: selected,
            rsn,
            channel,
            phy,
        } = beside.unwrap_or(strongest);
        let access_point = selected.record;
        Change::Join(Join {
            access_point,
            erp: selected.erp,
            rsn,
            channel,
            phy,
            he_set: false,
            step: JoinStep::NeedLease,
        })
    }

    fn begin_authentication(
        &mut self,
        join: &mut Join<T, K>,
        now: Instant,
    ) -> Result<JoinStep<T::RxMeta>, StaDisconnectReason> {
        let bssid = join.access_point.bssid;
        let Some(profile) = self.profile.as_ref() else {
            return Err(StaDisconnectReason::AuthenticationFailed);
        };
        let key = match &profile.security {
            StaSecurity::Open => AssociationKey::Open,
            StaSecurity::Wpa2Personal { pmk } => AssociationKey::Protected(pmk.duplicate()),
            StaSecurity::Personal { password, pmk } => {
                if let AssociationSecurity::Rsn(RsnAssociation {
                    akm: AssociationAkm::Sae(pwe),
                    ..
                }) = join.rsn.security()
                {
                    if let Some(cached) = self.pmksa.resume(bssid, &profile.ssid) {
                        join.rsn = join
                            .rsn
                            .with_pmkid(oer_protocols_ieee80211_wire::security::Pmkid(
                                cached.pmkid(),
                            ))
                            .ok_or(StaDisconnectReason::AuthenticationFailed)?;
                        AssociationKey::Protected(cached.pmk().duplicate())
                    } else {
                        let element = match pwe {
                            oer_protocols_ieee80211_wire::security::SaePwe::HuntingAndPecking => {
                                SaePasswordElement::hunting_and_pecking(
                                    password.as_bytes(),
                                    self.config.address.get(),
                                    bssid.get(),
                                )
                                .map_err(|error| sae_derivation_failure(error, bssid.get(), pwe))?
                            }
                            oer_protocols_ieee80211_wire::security::SaePwe::HashToElement => {
                                SaePasswordToken::derive(&profile.ssid, password.as_bytes(), None)
                                    .map_err(|error| {
                                        sae_derivation_failure(error, bssid.get(), pwe)
                                    })?
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
        Ok(JoinStep::Authenticate(Authenticate::NeedSend(
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
                let Some(profile) = self.profile.as_ref() else {
                    return leave_unkeyed(StaDisconnectReason::AuthenticationFailed);
                };
                self.pmksa.insert(Pmksa::new(
                    bssid,
                    profile.ssid.clone(),
                    keys.pmk.duplicate(),
                    keys.pmkid,
                ));
                let runtime = match StaAssociationRuntime::new(
                    self.config.address,
                    bssid,
                    oer_protocols_ieee80211_wire::security::LinkProtection::Ccmp,
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
                            Authenticate::NeedSend(runtime, key)
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
    #[inline(never)]
    fn receive_frame(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        frame: &[u8],
        meta: T::RxMeta,
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<StaReceived, StaRxError> {
        use oer_protocols_ieee80211_core_upper_mac::receive::{AdmittedData, RxDropReason};
        let header = match crate::receive::classify(frame)? {
            Received::Management(management) => {
                self.received_management(state, now, management, meta, events);
                return Ok(StaReceived::Input);
            }
            Received::Data(header) => header,
        };
        // Before its link exists only the handshake takes data: its EAPOL.
        let State::Handshake(handshake) = &mut *state else {
            return Err(RxDropReason::NotConnected.into());
        };
        let bssid = handshake.bssid;
        let replay = match &mut handshake.step {
            HandshakeStep::InstallGroup(_, replay, _)
            | HandshakeStep::AwaitInstalled(_, replay, _) => Some(replay),
            _ => None,
        };
        let admitted = AdmittedData::admit(
            header,
            self.config.address,
            bssid,
            crate::data::integrity(meta.common()),
            replay,
        )?;
        let mut received = StaReceived::Data;
        for msdu in crate::link::msdus(&admitted, self.config.address, self.link_protection())? {
            match msdu? {
                None => {}
                Some(crate::link::LinkMsdu::Eapol {
                    payload,
                    protection,
                }) => {
                    let eapol =
                        OwnedEapolFrame::try_copy(RsnInterface::Station, bssid.get(), payload)
                            .map_err(|_| RxDropReason::Payload)?;
                    let change = match &mut *state {
                        State::Handshake(handshake) => {
                            self.handshake_eapol(handshake, now, eapol, protection)
                        }
                        _ => Change::Keep,
                    };
                    state.apply(change);
                    received = StaReceived::Input;
                }
                // The handshake's link carries no data.
                Some(crate::link::LinkMsdu::Data(_)) => {
                    return Err(RxDropReason::NotConnected.into());
                }
            }
        }
        Ok(received)
    }

    /// Take a received frame.
    #[inline(never)]
    fn received_management(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        management: Management<'_>,
        meta: T::RxMeta,
        events: &mut impl EventSink<StaEvent>,
    ) {
        let frame = management.frame();
        let change = match &mut *state {
            State::Scan(scan)
                if matches!(
                    scan.step,
                    ScanStep::Listen { .. } | ScanStep::NeedProbe { .. }
                ) =>
            {
                // A connect's scan keeps its table for its own network.
                let ssid = self
                    .profile
                    .as_ref()
                    .map_or(&[][..], |profile| profile.ssid.as_slice());
                let subtype = management.subtype();
                let addressing = if management.header().receiver() == self.config.address.get() {
                    crate::events::Addressing::ToUs
                } else {
                    crate::events::Addressing::Overheard
                };
                let observation =
                    observe_scan(&mut (*self.scan), now, management, &meta, Some(ssid));
                report_scan(events, observation);
                report(events, || StaEvent::ScanHeard {
                    at: now,
                    heard: crate::receive::scan_heard(subtype, addressing, &meta, observation),
                });
                Change::Keep
            }
            State::Join(join) => {
                match core::mem::replace(&mut join.step, JoinStep::NeedLease) {
                    JoinStep::SaeAwait(authentication) => {
                        let event = authentication.observe_management_frame(frame, now);
                        self.sae_event(join, event, now)
                    }

                    JoinStep::Authenticate(Authenticate::NeedSend(runtime, key)) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedSend(runtime, key));
                        Change::Keep
                    }
                    JoinStep::Authenticate(Authenticate::Exchange {
                        runtime,
                        stage,
                        key,
                    }) => match runtime.observe_management_frame(frame) {
                        StaAuthenticationEvent::Authenticated { .. } => {
                            match StaAssociationRuntime::new(
                                self.config.address,
                                join.access_point.bssid,
                                self.link_protection(),
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
                                JoinStep::Authenticate(Authenticate::NeedSend(runtime, key));
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
                    },
                    JoinStep::Associate(runtime, key) => {
                        match runtime.observe_management_frame(frame, now) {
                            StaAssociationEvent::Associated { response, .. } => {
                                match StaAssociatedPeer::derive(
                                    &join.access_point,
                                    join.erp,
                                    &response,
                                    join.phy.mode(),
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
                                    Err(error) => leave_unkeyed(if error.he() {
                                        StaDisconnectReason::HeNotGranted
                                    } else {
                                        StaDisconnectReason::AssociationFailed
                                    }),
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
                    Ok(supplicant) => supplicant.group_management().is_some(),
                    Err(_) => {
                        let change = leave_handshake(handshake, reason_code::UNSPECIFIED);
                        state.apply(change);
                        return;
                    }
                };
                let disconnect = if protected {
                    handshake.management.as_mut().and_then(|management| {
                        match management.receive(
                            self.config.callbacks.aes_engine(),
                            frame,
                            self.config.address,
                            handshake.bssid,
                        ) {
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
                self.connected_received(connected, now, management, &meta, events)
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
        events: &mut impl EventSink<StaEvent>,
    ) -> Change<T, K> {
        let frame = management.frame();
        let disconnect = match connected
            .rsn
            .as_mut()
            .and_then(|rsn| rsn.management.as_mut())
        {
            Some(management) => {
                match management.receive(
                    self.config.callbacks.aes_engine(),
                    frame,
                    self.config.address,
                    connected.bssid,
                ) {
                    Some(ManagementEvent::Disconnect(reason)) => Some(reason),
                    Some(ManagementEvent::Query(query)) => {
                        management.received_query(query);
                        None
                    }
                    Some(ManagementEvent::BlockAck(action)) => {
                        route_block_ack(&mut self.config.tx_block_ack, connected, action, now);
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
                                Some(reason_code::UNSPECIFIED),
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
                    route_block_ack(&mut self.config.tx_block_ack, connected, action, now);
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
            let observation = observe_scan(&mut (*self.scan), now, management, meta, None);
            report_scan(events, observation);
            if away {
                return Change::Keep;
            }
        }
        if let Ok(beacon) = parse_sta_beacon(frame, connected.bssid.get(), connected.association_id)
        {
            if let Some(edca) = beacon.observation.edca
                && edca != connected.peer.associated.edca
            {
                connected.peer.associated.edca = edca;
                connected.edca = Owing::Owed;
            }
            if connected.power.beacon(beacon.observation.tim) == PowerSaveDecision::SendActive
                && connected.transmission != Transmission::Waking
            {
                connected.wake = Owing::Owed;
            }
        }
        Change::Keep
    }

    /// Set `receive`, while no other configuration is being set.
    fn configure(&mut self, receive: StationReceive<K>) -> Option<Owed<'id, T, K>> {
        if !matches!(*self.configuration, Configuration::Idle) {
            return None;
        }
        let epoch = if matches!(receive, StationReceive::Off) {
            None
        } else {
            *self.epoch
        };
        *self.configuration = Configuration::Setting(epoch);
        Some(Owed::Configure(receive))
    }

    /// The action `state` owes, and the state that awaits its end.
    #[inline(never)]
    fn owe(
        &mut self,
        state: &mut State<T, K>,
        now: Instant,
        events: &mut impl EventSink<StaEvent>,
    ) -> ActionStep<'id, T, K> {
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
                if matches!(
                    scan.step,
                    ScanStep::NeedProbe { until } | ScanStep::Listen { until } if now >= until
                ) {
                    scan.step = ScanStep::NeedRelease;
                }
                let owed = match scan.step {
                    ScanStep::NeedRelease => {
                        match core::mem::replace(&mut (*self.lease), Held::None) {
                            Held::Lease(lease) => {
                                (*self.lease) = Held::Released(lease.id());
                                scan.step = ScanStep::AwaitEnd;
                                Some(Owed::Release(lease))
                            }
                            // No lease to give back: its end was reported.
                            held => {
                                (*self.lease) = held;
                                let next = scan.visit.saturating_add(1);
                                let access_point = scan.access_point;
                                return (
                                    self.visit(next, 0, access_point),
                                    ActionProgress::Transition,
                                );
                            }
                        }
                    }
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
                    ScanStep::NeedConfigure
                        if matches!(*self.configuration, Configuration::Idle) =>
                    {
                        scan.step = ScanStep::AwaitConfigured;
                        self.configure(StationReceive::OtherBssManagement)
                    }
                    ScanStep::NeedProbe { until } => match self.encode_probe(TxPurpose::Probe) {
                        Ok(transmit) => {
                            scan.step = ScanStep::Listen { until };
                            report(events, || StaEvent::ScanProbeBuilt { at: now });
                            Some(Owed::Transmit(transmit))
                        }
                        Err(Unsent::Out) => None,
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
                self.owe_connected(connected, now, purpose, events)
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
                if let Some(removal) = self.group_removal(GroupRemoval::Any) {
                    (
                        Change::Reclaiming(Reclaim::Keys),
                        ActionProgress::Action(removal),
                    )
                } else if let Some(owed) = self.pairwise_removal() {
                    (
                        Change::Reclaiming(Reclaim::Keys),
                        ActionProgress::Action(owed),
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
            Reclaim::Configure if *self.he => {
                *self.he = false;
                (
                    Change::Reclaiming(Reclaim::Configure),
                    ActionProgress::Action(Owed::SetHe(
                        oer_protocols_ieee80211_contracts_port::StationHe::Off,
                    )),
                )
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

    /// The pairwise key's removal once every group key is gone and the link
    /// came back.
    fn pairwise_removal(&mut self) -> Option<Owed<'id, T, K>> {
        if !self.keys.groups_empty() || self.link.lent() || !self.keys.keyed() {
            return None;
        }
        self.keys.pairwise = PairwiseKey::Removing;
        Some(Owed::RemovePairwise)
    }

    /// The action the leave owes.
    fn owe_leave(&mut self, leave: &mut Leave) -> ActionStep<'id, T, K> {
        let owed = match leave.step {
            LeaveStep::AwaitAbsenceEnd(deauthentication) if self.absence.is_none() => {
                leave.step = leave_key_step(deauthentication);
                return (Change::Keep, ActionProgress::Transition);
            }
            LeaveStep::RemoveKeys(deauthentication) => {
                if let Some(removal) = self.group_removal(GroupRemoval::Any) {
                    Some(removal)
                } else if let Some(owed) = self.pairwise_removal() {
                    Some(owed)
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
                // A protected Deauthentication goes under the station's
                // installed key, once the link came back.
                if protected && (self.link.lent() || !self.keys.keyed()) {
                    return (Change::Keep, ActionProgress::Wait);
                }
                match self.encode_management(
                    StaManagementSubtype::Deauthentication,
                    bssid,
                    &reason_code.to_le_bytes(),
                    TxPurpose::Deauthentication,
                    if protected {
                        oer_protocols_ieee80211_wire::frame::FrameProtection::Protected
                    } else {
                        oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected
                    },
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
                    Err(Unsent::Out) => None,
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
            LeaveStep::NeedConfigure if *self.he => {
                *self.he = false;
                Some(Owed::SetHe(
                    oer_protocols_ieee80211_contracts_port::StationHe::Off,
                ))
            }
            LeaveStep::NeedConfigure if matches!(*self.configuration, Configuration::Idle) => {
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
        // Message 3's key data is unwrapped here, in the step that takes it.
        let received = match handshake.supplicant.as_mut() {
            Some(supplicant) => {
                supplicant.on_frame(eapol, &handshake.pmk, self.config.callbacks.aes_engine())
            }
            None => Err(RsnStaSupplicantError::UnexpectedAction.into()),
        };
        let action = match received {
            Ok(action) => action,
            Err(RsnStaProcessError::Supplicant(RsnStaSupplicantError::RetainedSecurityIes(
                error,
            ))) => {
                return leave_keyed(StaDisconnectReason::RetainedSecurityIes(error), None);
            }
            Err(RsnStaProcessError::Supplicant(RsnStaSupplicantError::InvalidFrame(error))) => {
                return leave_keyed(StaDisconnectReason::InvalidEapol(error), None);
            }
            // A peer frame the supplicant rejects changes nothing.
            Err(error) if error.is_peer_input_rejection() => RsnStaReady::None,
            Err(_) => return leave_handshake(handshake, reason_code::UNSPECIFIED),
        };
        handshake_ready(handshake, now, action, protection)
    }

    /// The action the handshake owes.
    #[inline(never)]
    fn owe_handshake(&mut self, handshake: &mut Handshake<T, K>) -> ActionStep<'id, T, K> {
        let (step, owed) = match core::mem::replace(&mut handshake.step, HandshakeStep::Await) {
            HandshakeStep::Send(message) => {
                match self.encode_eapol(
                    handshake.bssid,
                    AcAdmission::from_edca(handshake.peer.associated.edca),
                    message.as_bytes(),
                    TxPurpose::HandshakeResponse,
                ) {
                    Ok(transmit) => (HandshakeStep::AwaitSent, Some(Owed::Transmit(transmit))),
                    Err(EapolUnsent::Unsent(Unsent::Out)) => (HandshakeStep::Send(message), None),
                    Err(EapolUnsent::NoAdmittedAccessCategory) => {
                        return (
                            leave_handshake_because(
                                handshake,
                                StaDisconnectReason::NoAdmittedAccessCategory,
                                reason_code::UNSPECIFIED,
                            ),
                            ActionProgress::Transition,
                        );
                    }
                    Err(EapolUnsent::Unsent(Unsent::Encode | Unsent::NoConnection)) => {
                        return (
                            leave_handshake(handshake, reason_code::UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                }
            }
            HandshakeStep::Install(request, protection) => {
                let protected = match handshake.supplicant() {
                    Ok(supplicant) => supplicant.group_management().is_some(),
                    Err(_) => {
                        return (
                            leave_handshake(handshake, reason_code::UNSPECIFIED),
                            ActionProgress::Transition,
                        );
                    }
                };
                let Ok(replay) =
                    CcmpRxReplayEpoch::pairwise(*request.pairwise().receive_sequence())
                else {
                    return (
                        leave_handshake(handshake, reason_code::UNSPECIFIED),
                        ActionProgress::Transition,
                    );
                };
                let peer = handshake.bssid.individual();
                match (*self.epoch).filter(|_| self.keys.pairwise == PairwiseKey::None) {
                    Some(epoch) => {
                        self.keys.pairwise = PairwiseKey::Installing(epoch);
                        let (pairwise, group) = request.into_parts();
                        let key = pairwise.into_key();
                        if protected && handshake.management.is_none() {
                            handshake.management =
                                Some(ManagementProtection::new(Ccmp128Key::new(key.as_bytes())));
                        }
                        (
                            HandshakeStep::InstallGroup(group, replay, protection),
                            Some(Owed::InstallPairwise {
                                peer,
                                key,
                                spp: handshake.spp,
                            }),
                        )
                    }
                    None => (HandshakeStep::Install(request, protection), None),
                }
            }
            HandshakeStep::InstallGroup(request, replay, protection) if !self.keys.keyed() => {
                // The pairwise key's end unlocks GTK installation; D4 is
                // unchanged.
                (
                    HandshakeStep::InstallGroup(request, replay, protection),
                    None,
                )
            }
            HandshakeStep::InstallGroup(request, replay, protection) => {
                match self.begin_group_change(request.group()) {
                    Ok(ActionProgress::Transition) => {
                        return (
                            finish_handshake_install(
                                handshake,
                                request,
                                replay,
                                protection,
                                self.config.callbacks.aes_engine(),
                            ),
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
                            leave_handshake(handshake, reason_code::UNSPECIFIED),
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

    /// An EAPOL frame to `bssid` in a data frame, in a buffer, contending in
    /// the access category `admission` leaves its user priority 7.
    fn encode_eapol(
        &mut self,
        bssid: Bssid,
        admission: AcAdmission,
        eapol: &[u8],
        purpose: TxPurpose,
    ) -> Result<Encoded<K>, EapolUnsent> {
        let priority = eapol_priority(admission)?;
        let epoch = self.reserve_transmit(purpose)?;
        let address = self.config.address;
        let (sequence_number, number) =
            self.station_number(oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected);
        Ok(self
            .encode_reserved(
                epoch,
                |frame| {
                    oer_protocols_ieee80211_wire::station::StaDataFrame {
                        source: address,
                        bssid,
                        destination: bssid.get(),
                        sequence_number,
                        ether_type: ETHER_TYPE_EAPOL,
                        payload: eapol,
                    }
                    .encode(frame)
                    .ok()
                },
                StaFrameClass::Connection(ConnectionFrame::Eapol),
                purpose,
                StaRatePlan::Management,
            )
            .map(|encoded| Encoded {
                access_category: priority.access_category(),
                number,
                ..encoded
            })?)
    }

    /// The action the connected state owes.
    #[inline(never)]
    fn owe_connected(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        pairwise_purpose: TxPurpose,
        events: &mut impl EventSink<StaEvent>,
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
                    Some(reason_code::UNSPECIFIED),
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
                connected.qos
                    && connected.peer.associated.phy.format()
                        != oer_protocols_ieee80211_contracts_port::PhyFormat::NonHt,
            );
            return (
                Change::Keep,
                ActionProgress::Action(Owed::BlockAck(request)),
            );
        }
        if connected.connected_notice == Owing::Owed {
            connected.connected_notice = Owing::Clear;
            let notice = StaNotice::Connected {
                bssid: connected.bssid,
                channel: connected.channel.channel().number(),
                phy: connected.peer.associated.phy,
            };
            return (Change::Keep, ActionProgress::Action(Owed::Notify(notice)));
        }
        if let Some(fact) = connected.report.take() {
            return (Change::Keep, ActionProgress::Action(Owed::Upstream(fact)));
        }
        if core::mem::replace(&mut connected.edca, Owing::Clear) == Owing::Owed
            && let Some(edca) = SupportedEdca::new(
                oer_protocols_ieee80211_contracts_port::EdcaParameterSet::from_wmm(
                    connected.peer.associated.edca,
                ),
            )
        {
            return (Change::Keep, ActionProgress::Action(Owed::SetEdca(edca)));
        }
        if connected.exchange == Owing::Owed {
            match core::mem::replace(&mut (*self.lease), Held::None) {
                Held::Lease(lease) => {
                    connected.exchange = Owing::Clear;
                    return (Change::Keep, ActionProgress::Action(Owed::Exchange(lease)));
                }
                held => (*self.lease) = held,
            }
        }
        // Each source in its own place, in order: an empty one moves no
        // action through the next.
        let owed = |connected: &mut Connected<T, K>, owed| match owed {
            Ok(owed) => (Change::Keep, ActionProgress::Action(owed)),
            Err(reason) => (
                leave_connected(connected, reason, Some(reason_code::UNSPECIFIED)),
                ActionProgress::Transition,
            ),
        };
        if let Some(action) = self.owe_sa_query(connected, now) {
            return owed(connected, action);
        }
        if let Some(action) = self.owe_tx_block_ack(connected, now, events) {
            return owed(connected, action);
        }
        if let Some(action) = self.owe_duplicate_message4(connected, pairwise_purpose) {
            return owed(connected, action);
        }
        if let Some(action) = self.owe_rekey(connected, pairwise_purpose, events) {
            return owed(connected, action);
        }
        // Without a background scan and an owed wake, the background owes
        // nothing: the connected link's every turn ends here, its action's
        // place untouched.
        if connected.background.is_none()
            && !(connected.wake == Owing::Owed
                && !connected.power.asleep()
                && (connected.transmission == Transmission::Free))
        {
            return (Change::Keep, ActionProgress::Wait);
        }
        match self.owe_background(connected, now) {
            Ok(progress) => (Change::Keep, progress),
            Err(reason) => (
                leave_connected(connected, reason, Some(reason_code::UNSPECIFIED)),
                ActionProgress::Transition,
            ),
        }
    }

    fn owe_sa_query(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        if connected.transmission != Transmission::Free
            || !self.link_up(&connected.power, &(*self.keys))
        {
            return None;
        }
        let management = connected.rsn.as_mut()?.management.as_mut()?;
        let query = management.pending(now)?;
        match self.encode_management(
            StaManagementSubtype::Action,
            connected.bssid,
            &query.encode(),
            TxPurpose::Connected(Sending::SaQuery),
            oer_protocols_ieee80211_wire::frame::FrameProtection::Protected,
        ) {
            Ok(transmit) => {
                management.submitted(query);
                connected.transmission = Transmission::Busy;
                Some(Ok(Owed::TransmitProtected(transmit)))
            }
            Err(Unsent::Out) => None,
            Err(Unsent::Encode) => Some(Err(StaDisconnectReason::Encode)),
            Err(Unsent::NoConnection) => Some(Err(StaDisconnectReason::TransmitRefused(
                TxRefusal::LinkChanged,
            ))),
        }
    }

    /// The ADDBA Request or DELBA the station's transmit agreements owe.
    fn owe_tx_block_ack(
        &mut self,
        connected: &mut Connected<T, K>,
        now: Instant,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        let can_send = (connected.transmission == Transmission::Free)
            && data_link_up(connected, self.absence.as_ref(), &(*self.keys))
            && !self.out.holds(CONNECTED_PLACE);
        // An ADDBA Request whose number the link's holder told goes first;
        // one that waits for it owes nothing else of the agreements.
        let fresh = match connected.sequence {
            TidSequence::Unasked => None,
            TidSequence::Asked(_) => return None,
            TidSequence::Fresh(_) if !can_send => return None,
            TidSequence::Fresh(tid) => {
                connected.sequence = TidSequence::Unasked;
                Some(tid)
            }
        };
        let lent = self.link.lent();
        let tx = tx_agreements(&mut self.config.tx_block_ack, connected)?;
        if fresh.is_none() && !tx.may_owe(now) {
            return None;
        }
        enum Body {
            Delba(
                u8,
                [u8; oer_protocols_ieee80211_wire::block_ack::frame::DELBA_ACTION_BODY_LEN],
            ),
            Addba(
                u8,
                [u8; oer_protocols_ieee80211_wire::block_ack::frame::ADDBA_ACTION_BODY_LEN],
            ),
        }
        let owed = match fresh {
            Some(tid) => TxBlockAckOwed::Negotiate { tid: tid.value() },
            None => tx.poll(now, can_send)?,
        };
        let body = match owed {
            TxBlockAckOwed::Request { tid, start } => {
                return self.owe_block_ack_request(connected, tid, start, events);
            }
            TxBlockAckOwed::Delba { tid } => {
                let Ok(delba) = own_delba(tid) else {
                    return Some(Err(StaDisconnectReason::Encode));
                };
                Body::Delba(tid, delba)
            }
            TxBlockAckOwed::Negotiate { tid } => {
                // While the link's holder numbers the TID, it tells the
                // TID's next number first and holds the TID's data until
                // the request goes.
                if lent && fresh.is_none() {
                    let priority = WmmUserPriority::new(tid)?;
                    connected.sequence = TidSequence::Asked(priority);
                    return Some(Ok(Owed::LinkSequence(priority)));
                }
                let starting_sequence = connected.qos_sequences.get(usize::from(tid))?.peek();
                match tx.begin(tid, starting_sequence, now) {
                    Ok(request) => Body::Addba(tid, request.body),
                    Err(_) => {
                        // Its attempt is spent; the next waits the interval.
                        tx.request_failed(tid, now);
                        // The holder takes the TID's data again.
                        return fresh.map(|_| Ok(Owed::LinkRelease));
                    }
                }
            }
        };
        let (frame, sending): (&[u8], _) = match &body {
            Body::Delba(_, delba) => (delba, Sending::Delba),
            Body::Addba(tid, request) => (request, Sending::AddbaRequest { tid: *tid }),
        };
        let protection = management_frame_protection(connected);
        match self.encode_management(
            StaManagementSubtype::Action,
            connected.bssid,
            frame,
            TxPurpose::Connected(sending),
            protection,
        ) {
            Ok(transmit) => {
                connected.transmission = Transmission::Busy;
                if let (Body::Delba(tid, _), Some(tx)) = (&body, self.config.tx_block_ack.as_mut())
                {
                    tx.delba_sent(*tid);
                }
                Some(Ok(match protection {
                    oer_protocols_ieee80211_wire::frame::FrameProtection::Protected => {
                        Owed::TransmitProtected(transmit)
                    }
                    oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected => {
                        Owed::Transmit(transmit)
                    }
                }))
            }
            Err(Unsent::Out) => {
                if let (Sending::AddbaRequest { tid }, Some(tx)) =
                    (sending, self.config.tx_block_ack.as_mut())
                {
                    tx.request_failed(tid, now);
                }
                // The holder takes the TID's data again.
                fresh.map(|_| Ok(Owed::LinkRelease))
            }
            Err(Unsent::Encode) => Some(Err(StaDisconnectReason::Encode)),
            Err(Unsent::NoConnection) => Some(Err(StaDisconnectReason::TransmitRefused(
                TxRefusal::LinkChanged,
            ))),
        }
    }

    /// A BlockAckReq of the agreement of `tid`, moving the recipient's
    /// window to `start`, as a connected station's frame.
    fn owe_block_ack_request(
        &mut self,
        connected: &mut Connected<T, K>,
        tid: u8,
        start: SequenceNumber,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        let (Some(receiver), Some(port_tid), Some(priority)) = (
            IndividualAddress::new(connected.bssid.get()),
            oer_protocols_ieee80211_contracts_port::Tid::new(tid),
            WmmUserPriority::new(tid),
        ) else {
            return Some(Err(StaDisconnectReason::Encode));
        };
        let purpose = TxPurpose::Connected(Sending::BlockAckRequest { tid });
        let epoch = match self.reserve_transmit(purpose) {
            Ok(epoch) => epoch,
            Err(Unsent::NoConnection) => {
                return Some(Err(StaDisconnectReason::TransmitRefused(
                    TxRefusal::LinkChanged,
                )));
            }
            Err(Unsent::Out | Unsent::Encode) => return None,
        };
        if let Some(tx) = self.config.tx_block_ack.as_mut() {
            tx.request_sent(tid);
        }
        report(events, || StaEvent::BlockAckRequestSent);
        connected.transmission = Transmission::Busy;
        // The port encodes a BlockAckReq: nothing of it is in the buffer.
        self.out.take(purpose);
        Some(Ok(Owed::Transmit(Encoded {
            tag: StaTxTag { epoch, purpose },
            frame: StaFrame::BlockAckRequest {
                receiver,
                transmitter: self.config.address.get(),
                tid: port_tid,
                starting_sequence: start,
            },
            class: StaFrameClass::Normal,
            plan: StaRatePlan::Management,
            // The request contends with the data whose window it moves.
            access_category: priority.access_category(),
            // A control frame has no Sequence Control.
            number: FrameNumber::Open,
        })))
    }

    fn owe_duplicate_message4(
        &mut self,
        connected: &mut Connected<T, K>,
        purpose: TxPurpose,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        if connected.transmission != Transmission::Free
            || !self.link_up(&connected.power, &(*self.keys))
        {
            return None;
        }
        let rsn = connected.rsn.as_mut()?;
        let DuplicateMessage4::Send(message) = &rsn.message4 else {
            return None;
        };
        let admission = AcAdmission::from_edca(connected.peer.associated.edca);
        let qos = connected.qos.then_some(&mut connected.qos_sequences);
        match self.encode_eapol_response(connected.bssid, qos, admission, message, purpose) {
            Ok(owed) => {
                rsn.message4 = DuplicateMessage4::AwaitSent;
                connected.transmission = Transmission::Busy;
                Some(Ok(owed))
            }
            Err(unsent) => unsent.disconnect_reason().map(Err),
        }
    }

    fn encode_eapol_response(
        &mut self,
        bssid: Bssid,
        qos: Option<&mut [SequenceCounter; 8]>,
        admission: AcAdmission,
        response: &EapolResponse,
        purpose: TxPurpose,
    ) -> Result<Owed<'id, T, K>, EapolUnsent> {
        match response.protection {
            EapolProtection::Open => self
                .encode_eapol(bssid, admission, response.frame.as_bytes(), purpose)
                .map(Owed::Transmit),
            EapolProtection::Pairwise => self
                .encode_protected_eapol(bssid, qos, admission, response.frame.as_bytes(), purpose)
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
        // One group operation at a time.
        if !matches!(self.keys.group, GroupOperation::Idle) {
            return Ok(ActionProgress::Wait);
        }
        if let Some(InstalledGroup {
            place,
            key_id,
            material,
            replay,
            ..
        }) = self.keys.groups.take_matching(key_id, material.as_bytes())
        {
            self.keys.group = GroupOperation::Select(PendingGroupSelection {
                install: PendingGroupKey {
                    epoch,
                    key_id,
                    material,
                    place,
                    replay: PendingLane::Station(replay),
                },
            });
            return Ok(ActionProgress::Action(Owed::SelectGroup { place, key_id }));
        }
        let replay =
            match oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState::from_receive_sequence(
                *request.receive_sequence(),
            ) {
                Ok(replay) => GroupReplay::Retained(replay),
                Err(_) => return Err(()),
            };
        if self.keys.groups.occupied() < self.keys.capacity
            && let Some(place) = self.keys.groups.reserve()
        {
            self.keys.group = GroupOperation::Install(PendingGroupKey {
                epoch,
                key_id,
                material: Ccmp128Key::new(material.as_bytes()),
                place,
                replay: PendingLane::Station(replay),
            });
            return Ok(ActionProgress::Action(Owed::InstallGroup {
                place,
                key_id,
                key: material,
            }));
        }
        let Some(InstalledGroup {
            place,
            key_id: previous_id,
            ..
        }) = self.keys.groups.replace()
        else {
            return Ok(ActionProgress::Wait);
        };
        // An evicted installation of another ID ends its lane with the
        // end; retained entries of that ID still own their active state
        // independently. The operation keeps the entries as they are now
        // until its end.
        let retired = (previous_id != key_id && !self.keys.groups.has_active(previous_id))
            .then_some(previous_id);
        self.keys.group = GroupOperation::Replace(PendingGroupReplacement {
            install: PendingGroupKey {
                epoch,
                key_id,
                material: Ccmp128Key::new(material.as_bytes()),
                place,
                replay: PendingLane::Station(replay),
            },
            retired,
        });
        Ok(ActionProgress::Action(Owed::ReplaceGroup {
            place,
            key_id,
            key: material,
        }))
    }

    fn group_removal(&mut self, removal: GroupRemoval) -> Option<Owed<'id, T, K>> {
        let InstalledGroup {
            place,
            key_id,
            material,
            replay,
            ..
        } = self.keys.take_group_key(removal)?;
        self.keys.group = GroupOperation::Remove(PendingGroupRemoval {
            place,
            key_id,
            _material: material,
            replay,
        });
        Some(Owed::RemoveGroup(place))
    }

    #[inline(never)]
    fn owe_rekey(
        &mut self,
        connected: &mut Connected<T, K>,
        pairwise_purpose: TxPurpose,
        events: &mut impl EventSink<StaEvent>,
    ) -> Option<Result<Owed<'id, T, K>, StaDisconnectReason>> {
        // A completed install repeats as Send; a refused unwrap repeats with
        // the next of the two deferred frames.
        loop {
            let rsn = connected.rsn.as_mut()?;
            // Only these states owe anything; the others, the connected
            // link's every turn, stay where they are.
            if !matches!(
                rsn.rekey,
                Rekey::Unwrap(..) | Rekey::Install(..) | Rekey::Send(..)
            ) {
                return None;
            }
            let (rekey, owed) = match core::mem::replace(&mut rsn.rekey, Rekey::None) {
                // The key data is unwrapped here, in the step; its install
                // follows in the next turn of this loop.
                Rekey::Unwrap(frame, protection) => {
                    if let Err(reason) = rsn.prepare_install(
                        frame,
                        protection,
                        self.config.callbacks.aes_engine(),
                        events,
                    ) {
                        return Some(Err(reason));
                    }
                    continue;
                }
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
                    if (connected.transmission == Transmission::Free)
                        && self.link_up(&connected.power, &(*self.keys)) =>
                {
                    let message_kind = match message.frame.key_frame() {
                        Ok(frame) => frame.message(),
                        Err(error) => return Some(Err(StaDisconnectReason::InvalidEapol(error))),
                    };
                    let admission = AcAdmission::from_edca(connected.peer.associated.edca);
                    let qos = connected.qos.then_some(&mut connected.qos_sequences);
                    match self.encode_eapol_response(
                        connected.bssid,
                        qos,
                        admission,
                        &message,
                        if message_kind
                            == oer_protocols_ieee80211_core_security_rsn::EapolKeyMessage::PairwiseMessage4
                        {
                            pairwise_purpose
                        } else {
                            TxPurpose::Connected(Sending::GroupResponse)
                        }
                    ) {
                        Ok(owed) => {
                            connected.transmission = Transmission::Busy;
                            (Rekey::AwaitSent, Some(owed))
                        }
                        Err(unsent) => match unsent.disconnect_reason() {
                            Some(reason) => return Some(Err(reason)),
                            None => (Rekey::Send(message), None),
                        },
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
                BackgroundStep::NeedSleep if (connected.transmission == Transmission::Free) => {
                    power = Some((StaPowerManagement::PowerSave, Sending::Sleep));
                    BackgroundStep::AwaitSleep
                }
                BackgroundStep::NeedReady => {
                    owed = absence_id.map(Owed::Ready);
                    BackgroundStep::AwaitStart
                }
                BackgroundStep::NeedConfigure
                    if matches!(*self.configuration, Configuration::Idle) =>
                {
                    owed = self.configure(StationReceive::OtherBssManagement);
                    BackgroundStep::AwaitConfigured
                }
                BackgroundStep::NeedProbe if (connected.transmission == Transmission::Free) => {
                    owed = self.probe(&mut connected.transmission)?;
                    if owed.is_some() {
                        BackgroundStep::Listen
                    } else {
                        BackgroundStep::NeedProbe
                    }
                }
                BackgroundStep::NeedHome if matches!(*self.configuration, Configuration::Idle) => {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitHome
                }
                BackgroundStep::NeedWake if (connected.transmission == Transmission::Free) => {
                    power = Some((StaPowerManagement::Active, Sending::Wake));
                    BackgroundStep::AwaitWake
                }
                BackgroundStep::HomeProbe { until }
                    if (connected.transmission == Transmission::Free) =>
                {
                    owed = self.probe(&mut connected.transmission)?;
                    if owed.is_some() {
                        BackgroundStep::HomeListen { until }
                    } else {
                        BackgroundStep::HomeProbe { until }
                    }
                }
                BackgroundStep::Narrow(reason)
                    if matches!(*self.configuration, Configuration::Idle) =>
                {
                    owed = self.configure(bss_rules(bssid, Rules::<K>::MEMBER));
                    BackgroundStep::AwaitNarrowed(reason)
                }
                step => step,
            };
        } else if connected.wake == Owing::Owed
            && !connected.power.asleep()
            && (connected.transmission == Transmission::Free)
        {
            // The access point buffers for an awake station: PM=0 again.
            connected.wake = Owing::Clear;
            power = Some((StaPowerManagement::Active, Sending::Wake));
        }
        if let Some((power_management, sending)) = power {
            match self.encode_null(bssid, power_management) {
                Ok(transmit) => {
                    connected.transmission = if sending == Sending::Wake {
                        Transmission::Waking
                    } else {
                        Transmission::Busy
                    };
                    owed = Some(Owed::Transmit(transmit));
                }
                // Its purpose's frame is still out: the step waits for its end.
                Err(Unsent::Out) => {
                    if let Some(background) = &mut connected.background {
                        background.step = match background.step {
                            BackgroundStep::AwaitSleep => BackgroundStep::NeedSleep,
                            BackgroundStep::AwaitWake => BackgroundStep::NeedWake,
                            step => step,
                        };
                    } else {
                        connected.wake = Owing::Owed;
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
        transmission: &mut Transmission,
    ) -> Result<Option<Owed<'id, T, K>>, StaDisconnectReason> {
        match self.encode_probe(TxPurpose::Connected(Sending::Probe)) {
            Ok(transmit) => {
                *transmission = Transmission::Busy;
                Ok(Some(Owed::Transmit(transmit)))
            }
            Err(Unsent::Out) => Ok(None),
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
    ) -> Result<Encoded<K>, Unsent> {
        let purpose = TxPurpose::Connected(match power_management {
            StaPowerManagement::Active => Sending::Wake,
            StaPowerManagement::PowerSave => Sending::Sleep,
        });
        let epoch = self.reserve_transmit(purpose)?;
        let address = self.config.address;
        let (sequence_number, number) =
            self.station_number(oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected);
        let transmit = self.encode_reserved(
            epoch,
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
            purpose,
            StaRatePlan::Null,
        )?;
        Ok(Encoded { number, ..transmit })
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
            // Before authentication, as the vendor station's connect does
            // (`libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`): an HE20 join's
            // HE BSS with the color the scan heard, `Off` for any other.
            JoinStep::NeedConfigure if !join.he_set => {
                use oer_protocols_ieee80211_contracts_port::{HeBssColor, StationHe};
                let he = if matches!(join.phy, NegotiatedPhy::He { .. }) {
                    match oer_protocols_ieee80211_wire::he::parse_he_operation(
                        join.access_point.he_operation_ie_bytes(),
                    )
                    .ok()
                    .as_ref()
                    .and_then(HeBssColor::of)
                    {
                        Some(color) => StationHe::Bss(color),
                        None => {
                            return (
                                leave_unkeyed(StaDisconnectReason::HeNotGranted),
                                ActionProgress::Transition,
                            );
                        }
                    }
                } else {
                    StationHe::Off
                };
                join.he_set = true;
                *self.he = matches!(he, StationHe::Bss(_));
                (JoinStep::NeedConfigure, Some(Owed::SetHe(he)))
            }
            JoinStep::NeedConfigure if matches!(*self.configuration, Configuration::Idle) => {
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
                let epoch = match self.reserve_transmit(TxPurpose::SaeAuthentication) {
                    Ok(epoch) => epoch,
                    Err(Unsent::Out) => {
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
                    epoch,
                    |frame| {
                        oer_protocols_ieee80211_wire::station::SaeAuthenticationFrame {
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
            JoinStep::Authenticate(Authenticate::NeedSend(runtime, key)) => {
                // Every authentication attempt shares one place.
                let epoch = match self.reserve_transmit(TxPurpose::Authentication { attempt: 0 }) {
                    Ok(epoch) => epoch,
                    Err(Unsent::Out) => {
                        join.step = JoinStep::Authenticate(Authenticate::NeedSend(runtime, key));
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
                    epoch,
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
                let epoch = match self.reserve_transmit(TxPurpose::Association) {
                    Ok(epoch) => epoch,
                    Err(Unsent::Out) => {
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
                        let phy = join.phy.elements(capabilities);
                        let address = self.config.address;
                        let listen_interval = self.config.listen_interval.get();
                        let access_point = &join.access_point;
                        let rsn = &join.rsn;
                        let encoded = self.encode_reserved(
                            epoch,
                            |frame| {
                                AssociationRequest {
                                    source: address,
                                    access_point,
                                    sequence_number: attempt.sequence_number,
                                    listen_interval,
                                    phy,
                                    security: rsn,
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
                    StaAssociationPoll::Idle(runtime) => (JoinStep::Associate(runtime, key), None),
                    StaAssociationPoll::Failed { failure, .. } => {
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
                let next = if peer.associated.he_association.is_some() {
                    JoinStep::SetHe {
                        association_id,
                        peer,
                        key,
                    }
                } else {
                    JoinStep::StartLink {
                        association_id,
                        peer,
                        key,
                    }
                };
                match SupportedEdca::new(
                    oer_protocols_ieee80211_contracts_port::EdcaParameterSet::from_wmm(
                        peer.associated.edca,
                    ),
                ) {
                    Some(edca) => (next, Some(Owed::SetEdca(edca))),
                    None => {
                        join.step = next;
                        return (Change::Keep, ActionProgress::Transition);
                    }
                }
            }
            JoinStep::SetHe {
                association_id,
                peer,
                key,
            } => {
                let he = peer.associated.he_association.map_or(
                    oer_protocols_ieee80211_contracts_port::StationHe::Off,
                    oer_protocols_ieee80211_contracts_port::StationHe::He,
                );
                (
                    JoinStep::StartLink {
                        association_id,
                        peer,
                        key,
                    },
                    Some(Owed::SetHe(he)),
                )
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
                    reason_code: reason_code::UNSPECIFIED,
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
                    reason_code: reason_code::UNSPECIFIED,
                    protected: false,
                }),
            ),
        }
    }

    /// The epoch of a frame of `purpose`, when the station is in a
    /// connection attempt and its purpose's place is free.
    fn reserve_transmit(&mut self, purpose: TxPurpose) -> Result<LinkEpoch, Unsent> {
        let epoch = self.epoch.ok_or(Unsent::NoConnection)?;
        if self.out.holds(purpose) {
            return Err(Unsent::Out);
        }
        Ok(epoch)
    }

    /// Encode a frame of `purpose` into the station's buffer, after the
    /// caller may have consumed a sequence or started an attempt; its
    /// purpose's place is taken until its end.
    fn encode_reserved(
        &mut self,
        epoch: LinkEpoch,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
        class: StaFrameClass,
        purpose: TxPurpose,
        plan: StaRatePlan,
    ) -> Result<Encoded<K>, EncodeError> {
        let length =
            encode(self.frame.bytes.as_mut_slice()).filter(|length| self.frame.fits(*length));
        let bytes = self.frame.bytes.as_slice();
        let frame = length
            .and_then(|length| sta_frame(bytes.split_at_checked(length)?.0, plan))
            .ok_or(EncodeError)?;
        self.out.take(purpose);
        Ok(Encoded {
            tag: StaTxTag { epoch, purpose },
            frame,
            class,
            plan,
            access_category: StaTransmit::<'_, K>::ACCESS_CATEGORY,
            number: FrameNumber::Open,
        })
    }

    /// Whether the connected link is up: the station is home and awake,
    /// and holds its pairwise key, itself or lent to its link.
    fn link_up(&self, power: &PowerSaveView, keys: &Keys) -> bool {
        self.absence.is_none() && !power.asleep() && keys.keyed()
    }

    /// The Sequence Number of a station frame of the station's own space:
    /// the station's next while it holds its link; zero once the link's
    /// holder has the link, which numbers it as it submits it
    /// (`protected`, under the link's key).
    fn station_number(
        &mut self,
        protection: oer_protocols_ieee80211_wire::frame::FrameProtection,
    ) -> (SequenceNumber, FrameNumber) {
        if self.link.lent() {
            (
                SequenceNumber::ZERO,
                FrameNumber::Link {
                    space: LinkSpace::Station,
                    protected: protection
                        == oer_protocols_ieee80211_wire::frame::FrameProtection::Protected,
                },
            )
        } else {
            (self.sequence.take(), FrameNumber::Open)
        }
    }

    /// An EAPOL frame of the connected station under its pairwise key: the
    /// Protected bit and the CCMP header's room, which the backend fills;
    /// QoS Data at the user priority `admission` leaves priority 7 on a QoS
    /// BSS.
    ///
    /// On a QoS BSS it takes that priority's TID number, which data at the
    /// same priority shares. Allocating from TID 0 instead would leave holes
    /// in that TID's sequence under its Block Ack.
    fn encode_protected_eapol(
        &mut self,
        bssid: Bssid,
        qos: Option<&mut [SequenceCounter; 8]>,
        admission: AcAdmission,
        eapol: &[u8],
        purpose: TxPurpose,
    ) -> Result<Encoded<K>, EapolUnsent> {
        let priority = eapol_priority(admission)?;
        let epoch = self.reserve_transmit(purpose)?;
        let peer_qos = if qos.is_some() {
            oer_protocols_ieee80211_wire::data::PeerQos::Qos
        } else {
            oer_protocols_ieee80211_wire::data::PeerQos::NonQos
        };
        // A TID's next number is the station's to write while it holds the
        // link; its holder writes it once it has the link.
        let (sequence_number, number) = match qos {
            Some(sequences) => (
                priority.select_mut(sequences).take(),
                if self.link.lent() {
                    FrameNumber::Link {
                        space: LinkSpace::Tid(priority),
                        protected: true,
                    }
                } else {
                    FrameNumber::Open
                },
            ),
            None => {
                self.station_number(oer_protocols_ieee80211_wire::frame::FrameProtection::Protected)
            }
        };
        let frame = oer_protocols_ieee80211_wire::station::StaProtectedDataFrame {
            source: self.config.address,
            bssid,
            destination: bssid.get(),
            sequence_number,
            user_priority: priority.value(),
            peer_qos,
            ccmp_header: [0; 8],
            ether_type: ETHER_TYPE_EAPOL,
            payload: eapol,
        };
        Ok(self
            .encode_reserved(
                epoch,
                |buffer| frame.encode(buffer).ok(),
                StaFrameClass::Connection(ConnectionFrame::Eapol),
                purpose,
                StaRatePlan::Management,
            )
            .map(|encoded| Encoded {
                access_category: priority.access_category(),
                number,
                ..encoded
            })?)
    }

    /// A Probe Request takes its place before consuming its sequence.
    fn encode_probe(&mut self, purpose: TxPurpose) -> Result<Encoded<K>, Unsent> {
        let epoch = self.reserve_transmit(purpose)?;
        let (sequence_number, number) =
            self.station_number(oer_protocols_ieee80211_wire::frame::FrameProtection::Unprotected);
        let rates = self.config.supported_rates;
        let address = self.config.address.get();
        let ssid = self
            .profile
            .as_ref()
            .map(|profile| profile.ssid.clone())
            .unwrap_or_default();
        Ok(self
            .encode_reserved(
                epoch,
                |output| {
                    ProbeRequest {
                        destination: BROADCAST_ADDRESS,
                        source: address,
                        bssid: BROADCAST_ADDRESS,
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
            )
            .map(|encoded| Encoded { number, ..encoded })?)
    }

    /// A management frame of `subtype` to `bssid` in a buffer.
    fn encode_management(
        &mut self,
        subtype: StaManagementSubtype,
        bssid: Bssid,
        body: &[u8],
        purpose: TxPurpose,
        protection: oer_protocols_ieee80211_wire::frame::FrameProtection,
    ) -> Result<Encoded<K>, Unsent> {
        let epoch = self.reserve_transmit(purpose)?;
        let address = self.config.address;
        let (sequence_number, number) = self.station_number(protection);
        Ok(self
            .encode_reserved(
                epoch,
                |frame| {
                    if protection == oer_protocols_ieee80211_wire::frame::FrameProtection::Protected
                    {
                        return oer_protocols_ieee80211_wire::station::StaProtectedManagementFrame {
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
            )
            .map(|encoded| Encoded { number, ..encoded })?)
    }
}

/// The description of the station frame `bytes` hold, from its header on,
/// that goes by `plan`: a group frame for a group plan, an acknowledged
/// individual frame otherwise. `None` for a frame whose header or length
/// the port cannot take.
pub fn sta_frame<K: Ieee80211Capabilities>(bytes: &[u8], plan: StaRatePlan) -> Option<StaFrame<K>> {
    let length = MpduLength::new(bytes.len())?;
    if matches!(plan, StaRatePlan::Group) {
        let header = HeaderSpec::from_mac_header(
            oer_protocols_ieee80211_wire::frame::MacHeader::decode(bytes)?,
            GroupAddress::new,
        )?;
        SizedFrame::new(header, length).map(StaFrame::Group)
    } else {
        let header = HeaderSpec::from_mac_header(
            oer_protocols_ieee80211_wire::frame::MacHeader::decode(bytes)?,
            IndividualAddress::new,
        )?;
        SizedFrame::new(AckHeader::new(header)?, length).map(StaFrame::Individual)
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

fn leave_unkeyed<T: StaTypes, K>(reason: StaDisconnectReason) -> Change<T, K> {
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
fn leave_keyed<T: StaTypes, K>(
    reason: StaDisconnectReason,
    deauthentication: Option<Deauthentication>,
) -> Change<T, K> {
    Change::Leave {
        reason,
        step: leave_key_step(deauthentication),
    }
}
/// The handshake's state after the supplicant's resolved effect.
fn handshake_ready<T: StaTypes, K>(
    handshake: &mut Handshake<T, K>,
    now: Instant,
    action: RsnStaReady<DEFAULT_EAPOL_FRAME_CAPACITY>,
    protection: EapolProtection,
) -> Change<T, K> {
    match action {
        RsnStaReady::Transmit(message) => {
            let Some(deadline) = RsnStaResponseDeadline::start(RsnStaResponseWait::Message3, now)
            else {
                return leave_keyed(
                    StaDisconnectReason::DeadlineOverflow,
                    Some(Deauthentication {
                        bssid: handshake.bssid,
                        reason_code: reason_code::UNSPECIFIED,
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
            return leave_handshake(handshake, reason_code::UNSPECIFIED);
        }
        RsnStaReady::None => {}
    }
    Change::Keep
}

fn leave_handshake<T: StaTypes, K>(handshake: &Handshake<T, K>, reason_code: u16) -> Change<T, K> {
    let reason = if reason_code == reason_code::DEAUTH_LEAVING {
        StaDisconnectReason::Requested
    } else {
        StaDisconnectReason::HandshakeFailed
    };
    leave_handshake_because(handshake, reason, reason_code)
}
/// Leave a handshake for `reason`, deauthenticating with `reason_code`.
fn leave_handshake_because<T: StaTypes, K>(
    handshake: &Handshake<T, K>,
    reason: StaDisconnectReason,
    reason_code: u16,
) -> Change<T, K> {
    leave_keyed(
        reason,
        Some(Deauthentication {
            bssid: handshake.bssid,
            reason_code,
            protected: handshake.management.is_some(),
        }),
    )
}
fn leave_connected<T: StaTypes, K>(
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
fn leave_disabled<T: StaTypes, K>(_connected: &Connected<T, K>) -> Change<T, K> {
    Change::Leave {
        reason: StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged),
        step: LeaveStep::Disabled,
    }
}
#[inline(never)]
fn finish_connected_install<T: StaTypes, K>(
    connected: &mut Connected<T, K>,
    events: &mut impl EventSink<StaEvent>,
) {
    if let Some(rsn) = connected.rsn.as_mut() {
        let rekey = core::mem::replace(&mut rsn.rekey, Rekey::None);
        match rekey {
            Rekey::AwaitInstalled(request, protection) => {
                if rsn.complete_install(request, protection).is_err() {
                    report(events, || {
                        StaEvent::RsnInputDropped(RsnDrop::GroupInstallFailed)
                    });
                }
            }
            rekey => rsn.rekey = rekey,
        }
    }
}

fn finish_handshake_install<T: StaTypes, K, E: oer_portable_crypto::block::Aes128Engine>(
    handshake: &mut Handshake<T, K>,
    request: RsnStaRetryInstall,
    replay: CcmpRxReplayEpoch,
    protection: EapolProtection,
    engine: &mut E,
) -> Change<T, K> {
    if let (Some(management), Some(igtk)) = (&mut handshake.management, request.igtk()) {
        management.install_igtk(igtk);
    }
    match handshake.supplicant().and_then(|supplicant| {
        supplicant.complete_retry_install::<DEFAULT_EAPOL_FRAME_CAPACITY, _>(request, engine)
    }) {
        Ok(RsnStaReady::Transmit(message)) => Change::Confirm {
            replay,
            response: EapolResponse {
                frame: message,
                protection,
            },
        },
        _ => leave_handshake(handshake, reason_code::UNSPECIFIED),
    }
}
/// Why a frame was not handed out.
#[derive(Debug)]
enum Unsent {
    NoConnection,
    /// The frame of its purpose is still out: the station's one place for
    /// it is taken until that frame's end.
    Out,
    /// The frame did not encode.
    Encode,
}

/// Why an EAPOL frame was not handed out.
#[derive(Debug)]
enum EapolUnsent {
    /// Admission control closes every access category: EAPOL has none to
    /// contend in. Nothing was reserved or numbered.
    NoAdmittedAccessCategory,
    Unsent(Unsent),
}

impl From<Unsent> for EapolUnsent {
    fn from(unsent: Unsent) -> Self {
        Self::Unsent(unsent)
    }
}

impl From<EncodeError> for EapolUnsent {
    fn from(error: EncodeError) -> Self {
        Self::Unsent(error.into())
    }
}

impl EapolUnsent {
    /// The reason a connected station leaves for a refusal other than its
    /// purpose's frame still out, which only defers the frame.
    fn disconnect_reason(self) -> Option<StaDisconnectReason> {
        match self {
            Self::NoAdmittedAccessCategory => Some(StaDisconnectReason::NoAdmittedAccessCategory),
            Self::Unsent(Unsent::Out) => None,
            Self::Unsent(Unsent::Encode) => Some(StaDisconnectReason::Encode),
            Self::Unsent(Unsent::NoConnection) => {
                Some(StaDisconnectReason::TransmitRefused(TxRefusal::LinkChanged))
            }
        }
    }
}

/// The user priority of the station's EAPOL: 7, or the canonical priority of
/// the access category admission control leaves it. A BSS that closes every
/// category leaves EAPOL none.
fn eapol_priority(admission: AcAdmission) -> Result<WmmUserPriority, EapolUnsent> {
    admission
        .admitted_user_priority(WmmUserPriority::UP7)
        .ok_or(EapolUnsent::NoAdmittedAccessCategory)
}

/// Encoding can fail after the frame's place is taken; taking it cannot.
#[derive(Debug)]
struct EncodeError;

/// Report what the scan table did with a frame; an ignored frame is no
/// event.
#[inline(always)]
fn report_scan(events: &mut impl EventSink<StaEvent>, observation: crate::scan::ScanObservation) {
    use crate::scan::ScanObservation;
    let outcome = match observation {
        ScanObservation::Ignored => return,
        ScanObservation::Inserted { .. } => ScanOutcome::Inserted,
        ScanObservation::Updated { .. } => ScanOutcome::Updated,
        ScanObservation::Duplicate { .. } => ScanOutcome::Duplicate,
        ScanObservation::Dropped => ScanOutcome::Dropped,
    };
    report(events, || StaEvent::ScanBssObserved(outcome));
}

impl From<EncodeError> for Unsent {
    fn from(_: EncodeError) -> Self {
        Self::Encode
    }
}

#[cfg(test)]
mod tests;
