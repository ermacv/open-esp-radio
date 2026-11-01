//! The hardware side of the ESP32-S31 lower-MAC backend.
//!
//! [`LowerMacCore`] is the state the backend's driver owns by value: the interfaces, the installed keys, the receive Block Ack
//! banks, and the one published attempt of each EDCA queue. It drives the
//! ESP32-S31 register seams through a caller-supplied [`LowerMacHardware`]
//! and never waits: the runtime owns the event queues, the interrupt
//! entries, the publication deadlines, the software queue of each access
//! category ahead of the core and the asynchronous PHY retune an `Enable`
//! needs, and serves the phase contract
//! (`oer_ieee80211_contracts_port::phase`) over this core.
//!
//! # Checked operations
//!
//! Submission consumes a branded free queue and an admission ticket. Key
//! installation returns an affine `KeyHandle`; `admit_key` borrows that live
//! handle and consumes a ticket to issue one `KeyUse`. Station GTK installation
//! returns `ReceiveKeyHandle`, which has only a consuming removal path and
//! cannot enter TX admission or numbering. The installed material
//! cannot be removed while any attempt holds a right. `MpduValues::checked`
//! validates the caller's untrusted request before its fields become private.
//! Hardware failures retain the submitted owners, including quarantine.
//!
//! `CorePools<N>` issues the construction authority and exactly `N` TX
//! tickets. `KeyUseCount<N>` has a width checked against that same pool in
//! const evaluation; only consuming a ticket/right can increment/decrement it.
//! The backend uses its phase ticket count for that same `N`. A
//! protected MPDU or A-MPDU plan owns its right, whose selector identifies the
//! still-programmed CCMP material; publication never searches by a copied
//! key index. Removal consumes the handle and completes at its FIFO position
//! when there are no rights, or immediately after the last TX terminal.
//! Poisoned retains rights and DMA owners and never reports `KeyRemoved`.
//!
//! # Lifecycle and command barriers
//!
//! Construction returns a [`CoreOwner`] holding the core and phase in place.
//! [`CoreOwner::view`] lends the phase's view. Enable consumes a
//! [`DisabledView`], returning an [`EnabledView`] or a [`RetuningView`].
//! Disable consumes the enabled view; [`DisablingView::finish`] returns a
//! disabled view and emits one terminal after all publications ended. A
//! poisoned bank retains its owners. Only disabled views can select a
//! channel; [`CoreOwner::dismantle`] recovers only a disabled bank whose
//! transmitter is idle. Gate types survive disable and retune.
//!
//! Closing consumes the enabled view and holds new attempts. A drained
//! published prefix produces a [`ClosedGate`] view and blocks hardware
//! queues. Opening consumes this evidence; publication failure returns every
//! held owner and retains the published prefix in [`FailedGateView`].
//!
//! Receive configuration consumes a disabled or enabled view into
//! [`ConfiguringView`]. The barrier withdraws its role's held frames and
//! waits for that role's publications, then returns the original phase's
//! view. Later FIFO commands cannot publish through the barrier. The other
//! interface's frames retain their own completion paths.
//!
//! Direct owners and the port use these same view methods. Borrow a fresh
//! view after every asynchronous wait. [`CommandView`] projects an idle
//! phase's shared non-lifecycle commands without exposing TX admission or a
//! lifecycle transition.
//!
//! # Transmit buffers
//!
//! A transmit buffer is a whole ordinary TX slot ([`TxSlot`]): the pinned
//! descriptor and source buffer the hardware reads, whose MPDU follows the
//! TX metadata word. The caller holds the slot, encodes the MPDU into it
//! and submits it with the attempt; it comes back with the attempt's
//! completion ([`LowerMacSink::tx_completed`]). The caller supplies the PN
//! allocated above the port; [`LowerMacCore::write_ccmp_header`] only formats
//! it with the installed key's ID. A [`CcmpHeaderWriteError`] prevents TX
//! preparation. Receive-only key handles are rejected at compile time.
//!
//! # One publication per attempt, one attempt per EDCA queue
//!
//! An attempt is published through
//! [`OrdinaryTxOwner::start_queued_single_attempt`]: exactly one descriptor
//! publication at the submitted rate with the caller's protection, backoff
//! and power ceiling. Its completion, a collision detach or the hardware
//! timeout abort ends it; the driver carries known-result retries out by
//! submitting the same MPDU again (`MpduValues::retry`).
//! Retry marking uses the checked management/data `FrameControl` and the
//! encoded MPDU's length. An incomplete or unsupported field returns
//! `InvalidRetryHeader` with the original slot before publication; neither
//! slot capacity nor a failed buffer borrow can silently skip the marking.
//!
//! The chip's pure completion decoder returns `Observed(status)` or
//! `NoEffect`, or a `CompletionFault`. Only Observed constructs completed
//! metadata and consumes the published attempt through `finish(status)`.
//! NoEffect retains the same descriptor, deadline and attempt until its real
//! result or hardware timeout. SecurityKeyError and InvalidStatus propagate
//! through `LowerMacFault::Tx` to the existing poisoned-install path; they
//! return no TX terminal or slot. TimedOut ends the attempt with an unknown
//! result, and the backend never republishes it.
//!
//! The S31 MAC has four ordinary EDCA queues (voice, video, best effort,
//! background), each with its own descriptor, completion bank and latched
//! completion, timeout and collision state
//! (`pac/src/wifi/mac/tx/queue.rs`); the vendor publishes them concurrently
//! (`ppTxPkt` maps user priorities onto them) and handles their collisions
//! and timeouts per queue (`lmacProcessCollisions`,
//! `lmacProcessAllTxTimeout`). The core holds one attempt per queue
//! within the core; the driver's software queue holds the
//! next ones.
//!
//! The MAC interrupt's task events do not name a queue, so one
//! [`LowerMacCore::service`] call offers the same edge to every published
//! queue, and each claims only its own queue's state; an edge no queue
//! claims is ignored, and a lost one still ends in the publication
//! deadline's quarantine. A timeout abort forces the MAC-wide CCA for its
//! 16 µs settle, so aborts run one at a time: a queue whose timeout arrives
//! while another settles keeps its latched timeout and is aborted right
//! after that settle ends.
//!
//! # Limits
//!
//! The S31 draws its backoff in software, at most
//! `MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS` slots. Coexistence maps `TxOperation::Data` onto the static
//! per-access-category priority of
//! [`LegacyTxQueue::vendor_data_packet_priority`]; a
//! `TxOperation::Connection` frame takes the connection-frame priorities
//! while the coexistence owner's reconnect policy is on and that static
//! priority otherwise. An
//! individually addressed frame without acknowledgement is published at legacy
//! rates only, the one program with a response field; hardware has not
//! confirmed its on-air behaviour. A published attempt cannot be withdrawn: the
//! S31 abort path needs the queue's hardware timeout edge. A-MPDU transmission
//! and access-point TBTT schedules are not implemented yet.

use oer_ieee80211_wire::address::AddressFlags;
use oer_ieee80211_wire::frame::{FrameControlFields, FrameType};

use core::pin::Pin;
use oer_radio_port::{PoolPath, Root};

use oer_events::{EventSink, report};
use oer_time::Clock;

/// The contention a publication selected: the head's VIF and access
/// category, the complete record it drew from, the current window including
/// retry expansion, and the drawn backoff. Values only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentionDraw {
    pub role: VifRole,
    pub category: WmmAccessCategory,
    pub parameters: oer_ieee80211_contracts_port::EdcaAcParameters,
    pub window_slots: u16,
    pub backoff_slots: u16,
}

/// A receiver of the publication observations the S31 driver reports; it
/// lives in the driver's static queues from preparation to retirement.
pub trait Observations: EventSink<ContentionDraw> + 'static {}
impl<O: EventSink<ContentionDraw> + 'static> Observations for O {}

use crate::mac::tsf_relation::TsfTimingError;
use oer_esp32s31_hal::types::{MacPti, StaTbttSchedule};
use oer_ieee80211_contracts_port::TsfGeneration;
use oer_ieee80211_contracts_port::tsf::TsfInstant;

use crate::mac::{
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    crypto::{
        ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot,
        StaPairwiseCcmpSlot, install_ap_group_ccmp, install_ap_pairwise_ccmp,
        install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    init::{
        MacSnifferHardware, StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware,
        StaNoiseFloorHardware,
    },
    portable,
    rx::{
        NormalizedRxFrame,
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{
        HeEdcaTxopLimit, HtAmpduDensity, LegacyTxQueue, TxPhyRate, TxSlot,
        ampdu::{HeAmpduPolicy, HtAmpduHardware},
        runtime::he_txop_limit,
    },
};
use crate::ordinary_tx::MacTxPlan;
use crate::station_tsf::{StationTsf, StationTsfHardware};
use oer_esp32s31_hal::types::MacInterface;
use oer_ieee80211_contracts_port::{
    BandSet, BeaconTimingCapabilities, BlockAckReport, Channel, ConnectionFrame, LinkMetric,
    LowerMacCapabilities, PhyFormatSet, PhyRate, Protection, RateSupport, ReceiveFilter,
    RxEvidence, RxMeta, RxMetadata, SignalMetadata, TbttEvent, TxOperation, TxStatus, VifRole,
    VifRoleSet, VifTsf, WidthSet,
};
use oer_ieee80211_wire::{
    ccmp::CCMP_HEADER_LEN,
    channel::WifiChannel,
    management::BROADCAST_ADDRESS,
    phy::{HeMcs, HtMcs},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};

pub mod ampdu;
pub mod association;
mod resources;
pub use resources::{CoreAuthority, CorePools, CoreResources, FreeQueue, TxTicket};
mod lifecycle;
pub use lifecycle::{
    ClosedGate, ClosingGate, CommandView, ConfigurationProgress, ConfigureRefused, ConfiguringView,
    CoreGate, CoreOwner, CoreRetained, CoreView, DisabledView, DisablingView, EnabledView,
    FailedGateView, GateClosure, LifecycleStart, OpenGate, RetuningView, VacantView,
};

mod keys;
pub use keys::{
    KeyAdmissionError, KeyAdmissionRefused, KeyHandle, KeyRemoved, KeyUse, QuarantinedAdmission,
    ReceiveKeyHandle, RemovingKey, TransmitKeyScope, TxAdmission,
};
use keys::{KeyUseCount, TransmitKeyToken};

pub use ampdu::{
    AmpduBacking, AmpduBackingSource, AmpduFormat, AmpduOutcome, AmpduPlan, AmpduProgress,
    AmpduPublication, ESP32S31_AMPDU_MAX_LENGTH, Esp32s31AmpduBuffer, Esp32s31AmpduOwner, NoAmpdu,
    PublishedAmpdu, esp32s31_ampdu_capabilities,
};

use crate::{
    ampdu_tx::AmpduTxRoleAdapter,
    ordinary_tx::{
        MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS, OrdinaryTxError, OrdinaryTxIdleOwner,
        OrdinaryTxInterface, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan,
        QuarantinedSingleAttempt, QueuedSingleAttempt, QueuedSingleAttemptFailed,
        QueuedSingleAttemptProgress, QueuedSingleAttemptRefused, SingleAttempt,
        SingleAttemptProtection, TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE, WifiTxEntropy,
        WifiTxPowerProfile,
    },
    tx::WifiTxWake,
};

/// The ordinary EDCA queues, each holding one attempt in flight.
pub const LOWER_MAC_TX_QUEUES: usize = 4;

/// Interfaces the core configures at once: the station context (MAC
/// interface zero) and the access-point context (interface one).
pub const LOWER_MAC_VIFS: u8 = 2;

const RESOURCES: oer_ieee80211_contracts_port::MacResourceLimits =
    ESP32S31_MAC_SERVICE_CAPABILITIES.resources;

/// Keys installed at once: the station's pairwise and group slots and the
/// access point's pairwise and group slots the driver exposes.
pub const LOWER_MAC_KEY_SLOTS: usize = (RESOURCES.station_pairwise_ccmp_slots
    + RESOURCES.station_group_ccmp_slots
    + RESOURCES.access_point_pairwise_ccmp_slots
    + RESOURCES.access_point_group_ccmp_slots) as usize;

const CCMP_128_KEY_BYTES: usize = 16;
/// Frame Control, Duration and Address 1: the bytes a subframe's check
/// reads.
const MIN_FRAME_LENGTH: usize = 10;
const RX_BLOCK_ACK_BANKS: usize = RESOURCES.rx_block_ack_entries as usize;

/// The longest PSDU (MPDU, MIC and FCS) every transmit program carries:
/// the legacy program's 12-bit length field
/// (the `signal` check of `MacLegacyTxProgram::new`, which
/// `TxSlot::submit_legacy` turns into `TxError::Invalid`); the HE program
/// takes up to 0x3ff7.
const MAX_PROGRAM_PSDU: usize = 0x0fff;
/// Light-sleep wake lead the vendor power manager publishes beside the
/// station TBTT lead (`g_pm_cfg[16]`, as
/// `protocols/ieee80211/core/roles/sta/src/modem_sleep.rs` records it in
/// `TBTT_WAKE_WINDOW_MICROS`). The port
/// reports TBTTs only; it keeps the vendor relation between the two leads.
const STATION_TBTT_WAKE_WINDOW_MICROS: u16 = 1_500;

/// The receive rules the station context provides: its link policy (six)
/// for the BSS-member rules, and the ESP-NOW policy (six, mode two) that
/// also admits management frames of other BSSs.
const STATION_RECEIVE_FILTERS: ReceiveFilter =
    ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT);

/// One received MPDU's metadata on the ESP32-S31: the portable fields, and
/// the frame's RSSI from its receive prefix and the PHY's noise floor when
/// it was received, which every frame carries. The S31 therefore measures
/// every frame's signal ([`SignalMetadata`]), and a rate selection that
/// needs it can use this port's metadata directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxMeta {
    common: RxMeta,
    rssi_dbm: i8,
    noise_floor_dbm: i8,
}

impl Esp32s31RxMeta {
    /// The same metadata with the frame's receive stamp, which the runtime
    /// converts from the MAC local time.
    pub const fn with_timestamp(
        mut self,
        timestamp: RxEvidence<oer_ieee80211_contracts_port::Ieee80211Stamp>,
    ) -> Self {
        self.common.timestamp = timestamp;
        self
    }
}

impl RxMetadata for Esp32s31RxMeta {
    fn common(&self) -> &RxMeta {
        &self.common
    }
}

impl SignalMetadata for Esp32s31RxMeta {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
    }
}

/// What the ESP32-S31 lower MAC accepts through this core, with TX slots of
/// `buffer_size` bytes.
///
/// The rates are those `TxPhyRate::try_from(PhyRate)` accepts: DSSS/CCK and
/// OFDM, HT MCS 0-7 at 20 and 40 MHz, and HE SU MCS 0-9 at 20 MHz with BCC
/// or LDPC and DCM; the DCM MCS limits are checked at submission. The
/// services are exactly the hardware-owned operations of
/// [`ESP32S31_MAC_SERVICE_CAPABILITIES`]. The longest MPDU leaves room for
/// the TX metadata word, the CCMP MIC and the FCS in the four-byte-aligned
/// descriptor capacity, and with the MIC and the FCS fits the transmit
/// programs' PSDU length field (the legacy program's 12 bits). Each ordinary
/// EDCA queue holds one attempt. 5 GHz is absent from the ESP32-S31.
pub const fn esp32s31_lower_mac_capabilities(buffer_size: usize) -> LowerMacCapabilities {
    let buffered =
        (buffer_size & !3).saturating_sub(TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE);
    // The transmit programs' length fields bound a PSDU too.
    let programmable = MAX_PROGRAM_PSDU - TX_CCMP_MIC_SIZE - TX_FCS_SIZE;
    let usable = if buffered < programmable {
        buffered
    } else {
        programmable
    };
    LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: true,
            he_ldpc: true,
            spatial_streams: 1,
        },
        services: ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services(),
        vifs: LOWER_MAC_VIFS,
        tx_queues: LOWER_MAC_TX_QUEUES as u8,
        max_mpdu_length: if usable > u16::MAX as usize {
            u16::MAX
        } else {
            usable as u16
        },
        tx_power_ceiling_min_dbm: Some(0),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: STATION_RECEIVE_FILTERS,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::PROBE_REQUESTS),
        rx_block_ack_max_tid: RESOURCES.rx_block_ack_max_tid,
        rx_block_ack_max_window: RESOURCES.rx_block_ack_max_window,
        // The bounds `EdcaContentionParameters::new` checks.
        edca_max_aifsn: 0x0f,
        edca_max_cw_exponent: crate::mac::edca::MAX_HARDWARE_ECW_EXPONENT,
        edca_max_txop_limit_units_32_us: u16::MAX,
    }
}

/// The station TSF is read, set and scheduled; the access-point TSF only
/// restarts from zero.
pub const ESP32S31_BEACON_TIMING_CAPABILITIES: BeaconTimingCapabilities =
    BeaconTimingCapabilities {
        tsf_read: VifRoleSet::STATION,
        tsf_set: VifRoleSet::STATION,
        tsf_restart: VifRoleSet::STATION.union(VifRoleSet::ACCESS_POINT),
        tbtt: VifRoleSet::STATION,
    };

/// The station TBTT schedule of the MAC's interface-zero timer, whose event
/// fires on the power interrupt (`MacPowerInterruptObservation::sta_tbtt`).
pub trait StationTbttHardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule);
    fn stop_station_tbtt(&mut self);
}

/// The power-save block of every ordinary TX queue.
pub trait TxGateHardware {
    fn set_power_save_tx_block(&mut self, blocked: bool);
}

/// The beacon receive priority registers (`hal_set_rx_beacon_pti`,
/// `hal_clear_rx_beacon_pti`).
pub trait RxBeaconPriorityHardware {
    /// Receive beacons at `pti`, the beacon and the shared priority alike.
    fn set_rx_beacon_pti(&mut self, pti: MacPti);
    /// Withdraw the beacon receive priority request.
    fn clear_rx_beacon_pti(&mut self);
}

/// The coexistence priorities of the shared radio the core shares its RF
/// with, as it last published them.
/// The core retains a concrete `C: RadioCoexPriorities` chosen by its image;
/// this policy is statically dispatched, including on the P2 publication path.
pub trait RadioCoexPriorities {
    /// The priority of the beacon-window event (`coex_pti_get(0)`), which
    /// beacon reception asks for the air with.
    fn beacon_window_pti(&self) -> MacPti;

    /// The priorities `frame` carries: while the coexistence owner's
    /// reconnect policy is on, the priorities of its event (46's as the
    /// packet priority, and the lesser of it and the slice event's as the
    /// scheduler priority); `None` keeps the ordinary priorities of the
    /// frame's access category, as outside the policy and for a Probe
    /// Request, which only requests the air.
    ///
    /// SOURCE(esp32s31): complete pinned `libpp[pp.o]::pp_coex_tx_request`
    /// and `libpp[hal_mac.o]::mac_tx_set_pti`.
    fn connection_frame_priorities(
        &self,
        frame: ConnectionFrame,
    ) -> Option<ConnectionFramePriorities>;
}

/// The packet and scheduler priorities of a connection frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionFramePriorities {
    pub packet: u8,
    pub scheduler: u8,
}

/// The priority count the vendor publishes beside a connection frame's
/// priority under the reconnect policy.
const CONNECTION_FRAME_PRIORITY_COUNT: u16 = 4_000;

/// Every register seam the core drives; [`HtAmpduHardware`] includes the
/// ordinary queues' `TxHardware`.
pub trait LowerMacHardware:
    HtAmpduHardware
    + CcmpKeyHardware
    + RxBlockAckHardware
    + StaApRegisterHardware
    + StaLinkRxPolicyHardware
    + StaNoiseFloorHardware
    + StaEspNowRxPolicyHardware
    + MacSnifferHardware
    + ApRxPolicyHardware
    + ApTsfHardware
    + StationTsfHardware
    + StationTbttHardware
    + TxGateHardware
    + RxBeaconPriorityHardware
{
}

impl<H> LowerMacHardware for H where
    H: HtAmpduHardware
        + CcmpKeyHardware
        + RxBlockAckHardware
        + StaApRegisterHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + StaEspNowRxPolicyHardware
        + MacSnifferHardware
        + ApRxPolicyHardware
        + ApTsfHardware
        + StationTsfHardware
        + StationTbttHardware
        + TxGateHardware
        + RxBeaconPriorityHardware
{
}

/// Receiver of the events one core call produces.
pub trait LowerMacSink<
    'core,
    'slot,
    const BUFFER_SIZE: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
>
{
    /// The attempt of `queue`, submitted at `rate`, ended as `end`: its
    /// slot comes back with it.
    fn tx_completed(
        &mut self,
        queue: FreeQueue<'core, Path>,
        ticket: TxTicket<'core, Path>,
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        end: PublicationEnd,
        rate: PhyRate,
    );
    /// A detached aggregate ended, returning its queue and admission credit.
    fn ampdu_completed(
        &mut self,
        queue: FreeQueue<'core, Path>,
        ticket: TxTicket<'core, Path>,
        end: PublicationEnd,
    );
    /// The last admitted use returned and the programmed key was cleared.
    fn key_removed(&mut self, removed: KeyRemoved<'core, Path>);
    /// A disable the core started has ended: no attempt is left.
    fn disabled(&mut self);
    /// The station's TBTT event announced the TBTT `event` names.
    fn tbtt(&mut self, event: TbttEvent);
}

/// Why the backend's state is unknown: the port is poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacFault {
    /// The ordinary TX owner failed or quarantined its descriptor.
    Tx(OrdinaryTxError),
    /// A receive Block Ack bank did not read back as programmed.
    RxBlockAckReadback,
    /// The hardware refused a checked key.
    Key(CryptoKeyError),
    /// The hardware refused a checked receive Block Ack agreement.
    RxBlockAck(S31RxBlockAckAgreementError),
    /// An agreement found no free bank, which a slot of the backend's
    /// shared pool always holds.
    NoRxBlockAckBank,
    /// The port's owner abandoned the backend: a fail-stop or a dropped
    /// owner. Only a reset restores it.
    Abandoned,
    /// The MPDU protection or interface does not match its admitted key right.
    KeyUseMismatch,
    /// A key owner named no position of its core's key bank.
    KeyBank,
}

/// How one attempt ended, as the ESP32-S31 observed it. The backend maps it
/// onto the portable completion. A detached hardware timeout is
/// `TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)`: whether
/// its PPDU reached the air is unknown. An executor watchdog without a
/// qualified detach instead poisons and quarantines the publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationEnd {
    /// The MSDU aged before publication; no hardware attempt was aborted.
    Expired {
        /// Original aggregate member positions; bit zero for a single.
        msdus: u64,
    },
    /// Its completion arrived: the hardware's status, the ACK's SNR and a
    /// BlockAck the attempt solicited.
    Completed {
        status: TxStatus,
        ack_snr_db: Option<i8>,
        block_ack: Option<BlockAckReport>,
    },
    /// An internal collision detached it: the publication did not reach
    /// the air.
    Collision,
    /// The queue's hardware timeout and its abort: whether the PPDU reached
    /// the air is unknown.
    TimedOut,
    /// Held behind the closed gate or a disable, it was never published.
    Withdrawn,
}

/// One interface's configuration: its role, address, BSSID and the frames
/// it receives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreVif {
    pub role: VifRole,
    pub address: [u8; 6],
    pub bssid: Option<[u8; 6]>,
    pub receive: ReceiveFilter,
}

impl CoreVif {
    /// The interface's BSS: its BSSID, or an access point's address.
    fn bss(&self) -> Option<[u8; 6]> {
        match self.role {
            VifRole::Station => self.bssid,
            VifRole::AccessPoint => Some(self.address),
        }
    }

    /// Whether the receive filter admits `frame`, an MPDU without FCS: the
    /// hardware policies pass a superset of the requested rules, which
    /// this narrows.
    fn admits(&self, frame: &[u8]) -> bool {
        let filter = self.receive;
        let Some(address1) = frame_address(frame, 4) else {
            return false;
        };
        let control = FrameControlFields::from_bytes([frame[0], frame[1]]);
        let frame_type = control.kind();
        let subtype = control.subtype();
        if frame_type == FrameType::Control {
            return filter.contains(ReceiveFilter::OWN_CONTROL) && address1 == self.address;
        }
        let management = frame_type == FrameType::Management;
        if management
            && matches!(subtype, SUBTYPE_BEACON | SUBTYPE_PROBE_RESPONSE)
            && filter.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT)
        {
            return true;
        }
        if management
            && subtype == SUBTYPE_PROBE_REQUEST
            && filter.contains(ReceiveFilter::PROBE_REQUESTS)
        {
            let bssid = frame_bssid(frame);
            return (address1 == self.address || address1 == BROADCAST_ADDRESS)
                && (bssid == Some(BROADCAST_ADDRESS) || bssid == self.bss() && bssid.is_some());
        }
        if filter.contains(ReceiveFilter::OWN_UNICAST) && address1 == self.address {
            return true;
        }
        let Some(bss) = self.bss() else {
            return false;
        };
        let in_bss = frame_bssid(frame).is_some_and(|bssid| bssid == bss);
        if management && subtype == SUBTYPE_BEACON {
            return in_bss && filter.contains(ReceiveFilter::OWN_BSS_BEACONS);
        }
        AddressFlags::from_bytes([address1[0]]).group()
            && in_bss
            && filter.contains(ReceiveFilter::OWN_BSS_GROUP)
    }
}

const SUBTYPE_PROBE_REQUEST: u16 = 4;
const SUBTYPE_PROBE_RESPONSE: u16 = 5;
const SUBTYPE_BEACON: u16 = 8;

fn frame_address(frame: &[u8], offset: usize) -> Option<[u8; 6]> {
    frame.get(offset..offset + 6)?.try_into().ok()
}

/// The BSSID a management or data frame carries.
fn frame_bssid(frame: &[u8]) -> Option<[u8; 6]> {
    let control = FrameControlFields::from_bytes([*frame.first()?, *frame.get(1)?]);
    match control.kind() {
        FrameType::Management => frame_address(frame, 16),
        FrameType::Data => match control.ds() {
            0b00 => frame_address(frame, 16),
            // From the DS: Address 2 is the BSSID.
            0b10 => frame_address(frame, 10),
            // To the DS: Address 1 is the BSSID.
            0b01 => frame_address(frame, 4),
            _ => None,
        },
        _ => None,
    }
}

/// Why a fresh MPDU has not received a CCMP header and must not be
/// submitted as numbered protected TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpHeaderWriteError {
    /// Software cannot currently write the slot's buffer.
    Unavailable,
    /// The CCMP header does not fit after the metadata and MAC header,
    /// including when the offset overflows.
    InsufficientSpace,
}

/// A receive Block Ack agreement whose values a caller's types guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckValues {
    pub role: VifRole,
    pub peer: [u8; 6],
    pub tid: u8,
    pub start_sequence: SequenceNumber,
    pub window: u8,
}

/// The beacon receive priority ([`LowerMacCore::set_rx_beacon_priority`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBeaconPriority {
    /// The beacon-window priority the shared radio's owner sets.
    BeaconWindow,
    /// Priority zero.
    Zero,
    /// No beacon receive priority request.
    Cleared,
}

/// Configuration of one core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LowerMacConfig {
    /// The station address the cold start published; the station seams have
    /// no transaction that changes it.
    pub station_address: [u8; 6],
    /// The channel the radio is tuned to.
    pub channel: WifiChannel,
    /// Executor watchdog of one publication.
    pub publication_timeout: oer_time::Duration,
    /// The epoch of the core's TSF relations, a number no other owner of a
    /// TSF relation took.
    pub tsf_epoch: u32,
}

/// The owners a disabled, idle [`LowerMacCore`] is built from, which
/// [`CoreOwner::dismantle`] returns.
pub struct LowerMacCoreParts<
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
    const TICKETS: usize = 4,
> {
    authority: CoreAuthority<'core, Path, TICKETS>,
    tx: OrdinaryTxIdleOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: LowerMacConfig,
    coex: &'slot C,
    keys: KeyBank<'core, Path, TICKETS>,
}

enum KeyToken {
    StaPairwise(StaPairwiseCcmpSlot),
    StaGroup(StaGroupCcmpSlot),
    ApPairwise(ApPairwiseCcmpSlot),
    ApGroup(ApGroupCcmpSlot),
}

impl<
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> LowerMacCoreParts<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>
{
    pub(crate) fn into_transmitter(self) -> OrdinaryTxIdleOwner<'slot, P, E, T, BUFFER_SIZE> {
        self.tx
    }
}

impl KeyToken {
    fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        match self {
            Self::StaPairwise(slot) => slot.clear(hardware),
            Self::StaGroup(slot) => slot.clear(hardware),
            Self::ApPairwise(slot) => slot.clear(hardware),
            Self::ApGroup(slot) => slot.clear(hardware),
        }
    }
}

struct KeyBank<'core, Path: PoolPath, const TICKETS: usize> {
    installed: [Option<CheckedKeyScope>; LOWER_MAC_KEY_SLOTS],
    uses: [KeyUseCount<TICKETS>; LOWER_MAC_KEY_SLOTS],
    removals: [Option<RemovingKey<'core, Path>>; LOWER_MAC_KEY_SLOTS],
}

impl<Path: PoolPath, const TICKETS: usize> KeyBank<'_, Path, TICKETS> {
    const fn new() -> Self {
        Self {
            installed: [None; LOWER_MAC_KEY_SLOTS],
            uses: [const { KeyUseCount::new() }; LOWER_MAC_KEY_SLOTS],
            removals: [const { None }; LOWER_MAC_KEY_SLOTS],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckEntry {
    role: VifRole,
    peer: [u8; 6],
    tid: u8,
}

/// What one queue's attempt holds.
enum Work<'slot, const BUFFER_SIZE: usize> {
    /// An MPDU in its slot, not yet published: the transmit gate is
    /// closed.
    Held {
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        plan: OrdinaryTxPlan,
        single: SingleAttempt,
    },
    Published(QueuedSingleAttempt<'slot, BUFFER_SIZE>),
    Quarantined {
        _owner: QuarantinedSingleAttempt<'slot, BUFFER_SIZE>,
    },
}

impl<const BUFFER_SIZE: usize> Work<'_, BUFFER_SIZE> {
    const fn held(&self) -> bool {
        matches!(self, Self::Held { .. })
    }

    /// The pending deadline of a published attempt, and whether it is the
    /// end of a timeout abort's settle.
    const fn deadline(&self) -> Option<(oer_time::Instant, bool)> {
        match self {
            Self::Published(queued) => Some((queued.deadline(), queued.abort_settling())),
            Self::Held { .. } | Self::Quarantined { .. } => None,
        }
    }
}

/// The attempt occupying one queue.
struct Attempt<'core, 'slot, const BUFFER_SIZE: usize, Path: PoolPath> {
    queue: FreeQueue<'core, Path>,
    role: VifRole,
    admission: TxAdmission<'core, Path>,
    /// The rate of its publication, which its completion reports.
    rate: PhyRate,
    admitted_at: oer_time::Instant,
    work: Work<'slot, BUFFER_SIZE>,
}

/// The running station TBTT schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StationTbtt {
    first: u64,
    interval: core::num::NonZeroU64,
}

impl StationTbtt {
    /// The TBTT an event at station TSF `now` announces: the first one at
    /// or after `now`, since the event fires its lead before it.
    fn announced(self, now: u64) -> u64 {
        let Some(after) = now.checked_sub(self.first).filter(|&after| after != 0) else {
            return self.first;
        };
        // A nonzero divisor: the quotient of `after - 1` plus one is the
        // ceiling of `after / interval`, without a division-by-zero path.
        let periods = ((after - 1) / self.interval).saturating_add(1);
        self.first
            .saturating_add(periods.saturating_mul(self.interval.get()))
    }
}

/// The ESP32-S31 lower-MAC state the backend's driver owns: the hardware
/// side of the port, one published attempt per EDCA queue.
/// Diagnostic callers cannot borrow the nested transmitter:
/// ```compile_fail,E0599
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// use oer_esp32s31_ieee80211_driver::lower_mac::LowerMacCore;
/// use oer_esp32s31_ieee80211_driver::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};
/// fn borrow_tx<P: WifiTxPowerProfile, E: WifiTxEntropy, T: oer_time::Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, const N: usize>(
///     core: &LowerMacCore<'_, '_, P, E, T, C, N>,
/// ) {
///     core.tx();
/// }
/// ```
/// A diagnostic can read the next publication deadline directly:
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// use oer_esp32s31_ieee80211_driver::lower_mac::LowerMacCore;
/// use oer_esp32s31_ieee80211_driver::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};
/// fn deadline<P: WifiTxPowerProfile, E: WifiTxEntropy, T: oer_time::Timer, C: oer_esp32s31_ieee80211_driver::lower_mac::RadioCoexPriorities, const N: usize>(
///     core: &LowerMacCore<'_, '_, P, E, T, C, N>,
/// ) -> Option<oer_time::Instant> {
///     core.next_deadline()
/// }
/// ```
pub struct LowerMacCore<
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
    const TICKETS: usize = 4,
> {
    authority: CoreAuthority<'core, Path, TICKETS>,
    tx: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: LowerMacConfig,
    fault: Option<LowerMacFault>,
    /// The configured channel; the radio is tuned to it unless a retune is
    /// pending.
    channel: WifiChannel,
    tuned: WifiChannel,
    vifs: [Option<CoreVif>; LOWER_MAC_VIFS as usize],
    keys: KeyBank<'core, Path, TICKETS>,
    rx_block_acks: [Option<RxBlockAckEntry>; RX_BLOCK_ACK_BANKS],
    monitor: bool,
    tbtt: Option<StationTbtt>,
    /// The owner of the station TSF writes and its relation.
    station_tsf: StationTsf,
    /// The relation of the access-point TSF, which only restarts.
    access_point_tsf: crate::mac::ap_tsf::AccessPointTsf,
    /// MSDUs retired for age, separately from hardware attempt outcomes.
    expired_msdus: u32,
    /// The attempt of each ordinary queue, by its hardware index
    /// ([`LegacyTxQueue::hardware_index`]).
    queues: [Option<Attempt<'core, 'slot, BUFFER_SIZE, Path>>; LOWER_MAC_TX_QUEUES],
    /// The coexistence priorities the shared radio's owner sets: the beacon
    /// window's and a connection frame's.
    coex: &'slot C,
}

impl<
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// A disabled core over an idle ordinary TX owner. `coex` is the shared
    /// radio the core shares its RF with: beacon reception and connection
    /// frames ask for the air at its priorities.
    pub fn new(
        authority: CoreAuthority<'core, Path, TICKETS>,
        tx: OrdinaryTxIdleOwner<'slot, P, E, T, BUFFER_SIZE>,
        config: LowerMacConfig,
        coex: &'slot C,
    ) -> CoreOwner<Self> {
        CoreOwner::prepared(Self::assemble(authority, tx, config, coex))
    }
    pub(in crate::lower_mac) fn assemble(
        authority: CoreAuthority<'core, Path, TICKETS>,
        tx: OrdinaryTxIdleOwner<'slot, P, E, T, BUFFER_SIZE>,
        config: LowerMacConfig,
        coex: &'slot C,
    ) -> Self {
        Self {
            authority,
            tx: tx.owner,
            config,
            fault: None,
            channel: config.channel,
            tuned: config.channel,
            vifs: [None; LOWER_MAC_VIFS as usize],
            keys: KeyBank::new(),
            rx_block_acks: [None; RX_BLOCK_ACK_BANKS],
            monitor: false,
            tbtt: None,
            station_tsf: StationTsf::new(config.tsf_epoch),
            access_point_tsf: crate::mac::ap_tsf::AccessPointTsf::new(config.tsf_epoch),
            queues: [const { None }; LOWER_MAC_TX_QUEUES],
            expired_msdus: 0,
            coex,
        }
    }

    /// A disabled core over the owners a dismantled core returned: the
    /// state of the core they came from is gone, as at a radio start.
    pub fn from_parts(
        parts: LowerMacCoreParts<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
    ) -> CoreOwner<Self> {
        let LowerMacCoreParts {
            authority,
            tx,
            config,
            coex,
            keys,
        } = parts;
        let mut core = Self::assemble(authority, tx, config, coex);
        core.keys = keys;
        CoreOwner::prepared(core)
    }

    /// The channel the radio is tuned to: the configured one unless an
    /// `Enable` still awaits its retune.
    pub const fn tuned_channel(&self) -> WifiChannel {
        self.tuned
    }
    pub(crate) fn replace_power_profile(&mut self, power: P) {
        self.tx.replace_power_profile(power);
    }

    /// The channel received frames are reported on.
    pub fn channel(&self) -> Channel {
        Channel::from_wifi_channel(self.channel)
    }

    /// The image clock used by the core's internal deadlines.
    fn now(&self) -> oer_time::Instant {
        self.tx.now()
    }

    /// The earliest held MSDU expiry, published attempt deadline or timeout
    /// abort settle, which the runtime turns into [`WifiTxWake::Deadline`].
    /// While an abort settles, no other queue's deadline comes before its
    /// end: that queue's abort waits for it.
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        let deadlines = || {
            self.queues.iter().flatten().filter_map(|attempt| {
                attempt.work.deadline().or_else(|| {
                    if !attempt.work.held() {
                        return None;
                    }
                    let queue = LegacyTxQueue::from_access_category(attempt.queue.category());
                    let lifetime = self
                        .tx
                        .policy()
                        .access_policy(attempt.role, queue)
                        .msdu_lifetime();
                    Some((attempt.admitted_at.saturating_add(lifetime), false))
                })
            })
        };
        let settle = deadlines()
            .filter(|(_, settling)| *settling)
            .map(|(deadline, _)| deadline)
            .min();
        deadlines()
            .map(|(deadline, _)| settle.map_or(deadline, |settle| deadline.max(settle)))
            .min()
    }

    /// MSDUs completed with Expired, separately from hardware outcomes.
    pub const fn expired_msdus(&self) -> u32 {
        self.expired_msdus
    }

    fn msdu_expired(
        &self,
        role: VifRole,
        category: WmmAccessCategory,
        admitted_at: oer_time::Instant,
    ) -> bool {
        let lifetime = self
            .tx
            .policy()
            .access_policy(role, LegacyTxQueue::from_access_category(category))
            .msdu_lifetime();
        self.now()
            .checked_duration_since(admitted_at)
            .is_some_and(|age| age >= lifetime)
    }

    /// Draw for the head frame of `access_category`, using its VIF's record
    /// and the AC queue's single contention state. The backend calls this
    /// at publication and carries the draw into the single-attempt values;
    /// `observations` receives the selection, built only when it is enabled.
    pub fn draw_backoff(
        &mut self,
        role: VifRole,
        access_category: WmmAccessCategory,
        observations: &mut impl EventSink<ContentionDraw>,
    ) -> u16 {
        let queue = LegacyTxQueue::from_access_category(access_category);
        let backoff = self.tx.contention_publication(role, queue).1;
        #[cfg(all(oer_trace, oer_debug_control))]
        crate::port::trace::contention(self.tx.policy(), role, access_category, backoff);
        report(observations, || {
            let (parameters, window_slots) =
                self.tx.policy().contention_record(role, access_category);
            ContentionDraw {
                role,
                category: access_category,
                parameters,
                window_slots,
                backoff_slots: backoff,
            }
        });
        backoff
    }
    #[inline(never)]
    pub(crate) fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.tx
            .reset_terminal_exchange(role, LegacyTxQueue::from_access_category(category));
    }

    /// Whether the queue of `access_category` holds an attempt.
    #[cfg(test)]
    fn queue_busy(&self, access_category: WmmAccessCategory) -> bool {
        let queue = LegacyTxQueue::from_access_category(access_category);
        self.queues[usize::from(queue.hardware_index())].is_some()
    }

    fn attempts(&self) -> impl Iterator<Item = &Attempt<'core, 'slot, BUFFER_SIZE, Path>> {
        self.queues.iter().flatten()
    }

    /// Whether a queue's timeout abort forces CCA now.
    fn abort_settling(&self) -> bool {
        self.attempts().any(|attempt| {
            attempt
                .work
                .deadline()
                .is_some_and(|(_, settling)| settling)
        })
    }

    /// Write the caller's already allocated packet number into a fresh MPDU.
    /// The core above the port owns the per-key PN allocator; retries retain
    /// the original header. This operation owns no counter and cannot exhaust
    /// or reset the installation's packet-number space.
    ///
    /// # Errors
    ///
    /// Insufficient header space leaves the buffer unchanged. Only a free
    /// slot and a transmit-capable installed key authorize this write.
    pub fn write_ccmp_header(
        &self,
        key: &KeyHandle<'core, Path>,
        slot: &mut crate::mac::tx::TxSlotFree<'_, BUFFER_SIZE>,
        header_len: usize,
        packet_number: oer_ieee80211_wire::ccmp::CcmpPacketNumber,
    ) -> Result<(), CcmpHeaderWriteError> {
        let buffer = slot
            .buffer_mut()
            .map_err(|_| CcmpHeaderWriteError::Unavailable)?;
        let at = TX_METADATA_SIZE
            .checked_add(header_len)
            .ok_or(CcmpHeaderWriteError::InsufficientSpace)?;
        let place = buffer
            .get_mut(at..)
            .and_then(|remaining| remaining.get_mut(..CCMP_HEADER_LEN))
            .ok_or(CcmpHeaderWriteError::InsufficientSpace)?;
        let key_id = match key.scope {
            TransmitKeyScope::StationPairwise { .. }
            | TransmitKeyScope::AccessPointPairwise { .. } => {
                oer_ieee80211_wire::ccmp::CcmpKeyId::PAIRWISE
            }
            TransmitKeyScope::AccessPointGroup { key_id } => key_id.group_key_id().key_id(),
        };
        let header = oer_ieee80211_wire::ccmp::CcmpHeader::new(packet_number, key_id).encode();
        place.copy_from_slice(&header);
        Ok(())
    }

    /// Publish an MPDU whose values and state a caller's types guarantee
    /// ([`MpduValues`] states which), or hold it behind a closed gate: one
    /// call, so no other operation comes between its plan and its
    /// publication. `retry` marks a transmission of an MPDU the plan sends
    /// again: its Retry bit is set and its CCMP header kept.
    ///
    /// # Errors
    ///
    /// A fault of the transmit owner, with the slot.
    #[allow(clippy::result_large_err, reason = "the slot goes back by value")]
    fn submit_checked<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        queue: FreeQueue<'core, Path>,
        admission: TxAdmission<'core, Path>,
        values: MpduValues<'slot, BUFFER_SIZE>,
        publish: bool,
    ) -> Result<(), SubmitFault<'core, 'slot, BUFFER_SIZE, Path>> {
        let MpduValues {
            role,
            admitted_at,
            slot,
            len,
            no_ack,
            rate,
            reported,
            protection,
            power_ceiling_dbm,
            backoff_slots,
            operation,
            retry,
        } = values;
        let mut slot = slot.into_slot();
        if let Some(fault) = self.fault {
            return Err(SubmitFault {
                queue,
                admission,
                fault,
                slot,
            });
        }
        let access_category = queue.category();
        let matches_admission = queued_free_buffer(slot.as_mut()).and_then(|buffer| {
            if retry {
                crate::ordinary_tx::mark_retry(buffer, len)?;
            }
            Ok(admission.matches_frame(
                role,
                buffer
                    .get(TX_METADATA_SIZE..)
                    .and_then(|frame| frame.get(..len)),
            ))
        });
        match matches_admission {
            Ok(true) => {}
            Ok(false) => {
                return Err(SubmitFault {
                    queue,
                    admission,
                    slot,
                    fault: LowerMacFault::KeyUseMismatch,
                });
            }
            Err(error) => {
                return Err(SubmitFault {
                    queue,
                    admission,
                    slot,
                    fault: LowerMacFault::Tx(error),
                });
            }
        }
        let common = self.plan_common(
            access_category,
            role,
            rate,
            protection,
            &admission,
            power_ceiling_dbm,
            backoff_slots,
        );
        let planned = self.plan_mpdu(common, len, access_category, operation, no_ack);
        let index = usize::from(planned.queue.hardware_index());
        let work = if publish && !self.msdu_expired(role, access_category, admitted_at) {
            match self
                .tx
                .start_queued_single_attempt(hardware, slot, planned.plan, planned.single)
            {
                Ok(queued) => Work::Published(queued),
                Err(QueuedSingleAttemptRefused { error, slot }) => {
                    return Err(SubmitFault {
                        queue,
                        admission,
                        fault: LowerMacFault::Tx(error),
                        slot,
                    });
                }
            }
        } else {
            Work::Held {
                slot,
                plan: planned.plan,
                single: planned.single,
            }
        };
        self.queues[index] = Some(Attempt {
            queue,
            role,
            admission,
            rate: reported,
            admitted_at,
            work,
        });
        Ok(())
    }

    /// What every attempt shares, planned from values its caller's types
    /// guarantee: `key`, when one, is installed.
    #[allow(clippy::too_many_arguments, reason = "the fields every attempt shares")]
    fn plan_common(
        &self,
        access_category: WmmAccessCategory,
        role: VifRole,
        rate: TxPhyRate,
        protection: Protection,
        admission: &TxAdmission<'core, Path>,
        power_ceiling_dbm: Option<i8>,
        backoff_slots: u16,
    ) -> CommonAdmission {
        let (hardware_key_selector, hardware_mic_length) = admission.hardware();
        CommonAdmission {
            queue: LegacyTxQueue::from_access_category(access_category),
            role,
            rate,
            hardware_key_selector,
            hardware_mic_length,
            single: SingleAttempt {
                protection: match protection {
                    Protection::None => SingleAttemptProtection::None,
                    Protection::RtsCts { rate } => SingleAttemptProtection::RtsCts {
                        rate: crate::mac::portable::legacy_from_phy_rate(rate),
                    },
                    Protection::CtsToSelf { rate } => SingleAttemptProtection::CtsToSelf {
                        rate: crate::mac::portable::legacy_from_phy_rate(rate),
                    },
                },
                backoff_slots,
                power_ceiling_dbm,
                no_ack: false,
            },
        }
    }

    /// Plan one MPDU's publication from its common values.
    fn plan_mpdu(
        &self,
        common: CommonAdmission,
        length: usize,
        access_category: WmmAccessCategory,
        operation: TxOperation,
        no_ack: bool,
    ) -> Admission {
        let queue = common.queue;
        // The static priority vendor data encapsulation assigns the access
        // category, unless the coexistence owner raises a connection frame.
        let raised = match operation {
            TxOperation::Connection(frame) => self.coex.connection_frame_priorities(frame),
            TxOperation::Data => None,
        };
        let (scheduler_priority, packet_priority, priority_count) = match raised {
            Some(connection) => (
                connection.scheduler,
                connection.packet,
                CONNECTION_FRAME_PRIORITY_COUNT,
            ),
            None => (
                queue.vendor_data_scheduler_priority(),
                queue.vendor_data_packet_priority(),
                1,
            ),
        };
        let plan = OrdinaryTxPlan {
            frame_length: length,
            descriptor_capacity: None,
            exchange: MacTxPlan {
                access_category,
                initial_rate: common.rate,
                publication_limit: 1,
                publication_timeout: self.config.publication_timeout,
            },
            hardware_mic_length: common.hardware_mic_length,
            hardware_key_selector: common.hardware_key_selector,
            interface: match common.role {
                VifRole::Station => OrdinaryTxInterface::Station,
                VifRole::AccessPoint => OrdinaryTxInterface::AccessPoint,
            },
            scheduler_priority,
            packet_priority,
            priority_count,
        };
        Admission {
            queue,
            plan,
            single: SingleAttempt {
                no_ack,
                ..common.single
            },
        }
    }

    /// Plan one aggregate while retaining its queue and encryption admission.
    /// A refusal returns both owners and leaves the subframes in the buffer.
    pub fn plan_ampdu<S: AmpduBacking, const SLOTS: usize>(
        &self,
        queue: FreeQueue<'core, Path>,
        admission: TxAdmission<'core, Path>,
        mut values: AmpduValues,
        buffer: &mut Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    ) -> Result<AmpduPlan<'core, Path>, AmpduPlanningRefused<'core, Path>> {
        values.access_category = queue.category();
        match self.ampdu_settings(values, &admission, buffer) {
            Ok(settings) => Ok(AmpduPlan {
                queue,
                admission,
                settings,
            }),
            Err(reason) => Err(AmpduPlanningRefused {
                queue,
                admission,
                reason,
            }),
        }
    }

    /// Publish a detached aggregate's terminal and return its owner. A
    /// quarantined aggregate has no such projection or key-release operation.
    #[allow(
        clippy::result_large_err,
        reason = "a poisoned core retains the full aggregate and key right"
    )]
    pub fn complete_ampdu<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
        B,
        const SLOTS: usize,
    >(
        &mut self,
        hardware: &mut H,
        completed: ampdu::CompletedAmpdu<'core, 'slot, B, SLOTS, Path>,
        sink: &mut K,
    ) -> Result<
        (
            ampdu::Esp32s31AmpduOwner<'slot, B, SLOTS>,
            [oer_time::Instant; SLOTS],
        ),
        ampdu::AmpduFault<'core, 'slot, B, SLOTS, Path>,
    > {
        if self.fault.is_some() {
            return Err(ampdu::AmpduFault::retain_completed(completed));
        }
        let ampdu::CompletedAmpdu {
            outcome,
            owner,
            queue,
            admission,
            admitted_at,
        } = completed;
        match admission {
            TxAdmission::Open(ticket) => sink.ampdu_completed(queue, ticket, outcome.end()),
            TxAdmission::Protected(key) => {
                let (index, ticket) = self.release_key(key);
                sink.ampdu_completed(queue, ticket, outcome.end());
                self.finish_key_removal(hardware, index, sink);
            }
        }
        Ok((owner, admitted_at))
    }

    /// Return an unpublished expired aggregate's queue and ticket exactly
    /// once. Its buffer, live members and admission samples remain together.
    #[allow(
        clippy::result_large_err,
        reason = "a poisoned core retains every unpublished owner"
    )]
    pub fn complete_expired_ampdu<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
        S: AmpduBacking,
        const SLOTS: usize,
    >(
        &mut self,
        hardware: &mut H,
        expired: ampdu::ExpiredAmpdu<'core, 'slot, S, SLOTS, Path>,
        sink: &mut K,
    ) -> Result<
        Esp32s31AmpduBuffer<'slot, S, SLOTS>,
        ampdu::ExpiredAmpdu<'core, 'slot, S, SLOTS, Path>,
    > {
        if self.fault.is_some() {
            return Err(expired);
        }
        let (buffer, plan, msdus) = expired.into_parts();
        self.expired_msdus = self.expired_msdus.saturating_add(msdus.count_ones());
        match plan.admission {
            TxAdmission::Open(ticket) => {
                sink.ampdu_completed(plan.queue, ticket, PublicationEnd::Expired { msdus })
            }
            TxAdmission::Protected(key) => {
                let (index, ticket) = self.release_key(key);
                sink.ampdu_completed(plan.queue, ticket, PublicationEnd::Expired { msdus });
                self.finish_key_removal(hardware, index, sink);
            }
        }
        Ok(buffer)
    }

    /// Plan an aggregate whose values a caller's types guarantee
    /// ([`AmpduValues`]): a pure function of the core's state, the
    /// aggregate's subframes and its values, which [`ampdu::publish`]
    /// carries out. Every check the aggregate owner would make at commit is
    /// made here first, so a refused aggregate keeps its subframes.
    ///
    /// # Errors
    ///
    /// Why the aggregate cannot go as one publication.
    fn ampdu_settings<S: AmpduBacking, const SLOTS: usize>(
        &self,
        values: AmpduValues,
        admission: &TxAdmission<'core, Path>,
        buffer: &mut Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    ) -> Result<ampdu::AmpduSettings, AmpduRefusal> {
        let AmpduValues {
            role,
            access_category,
            rate,
            protection,
            power_ceiling_dbm,
            backoff_slots,
            min_mpdu_start_spacing,
        } = values;
        if !buffer.matches_admission(role, admission) {
            return Err(AmpduRefusal::KeyUseMismatch);
        }
        let common = self.plan_common(
            access_category,
            role,
            rate,
            protection,
            admission,
            power_ceiling_dbm,
            backoff_slots,
        );
        let capabilities = esp32s31_ampdu_capabilities(SLOTS);
        let count = buffer.subframes();
        if count == 0 {
            return Err(AmpduRefusal::Empty);
        }
        if count > usize::from(capabilities.max_subframes) {
            return Err(AmpduRefusal::TooManySubframes);
        }
        if min_mpdu_start_spacing > 7 {
            return Err(AmpduRefusal::StartSpacing);
        }
        if buffer.lengths().any(|len| len < MIN_FRAME_LENGTH) {
            return Err(AmpduRefusal::ShortSubframe);
        }
        // One recipient answers the aggregate with its BlockAck.
        if buffer.first_mpdu().is_none_or(|first| first[4] & 1 != 0) {
            return Err(AmpduRefusal::GroupRecipient);
        }

        let queue = common.queue;
        let ceiling = common.single.power_ceiling_dbm;
        let txop_limit = self
            .tx
            .policy()
            .access_policy(common.role, queue)
            .txop_limit_units_32_us();
        let density = HtAmpduDensity::from_ampdu_parameters((min_mpdu_start_spacing & 0x07) << 2);
        let mic_and_fcs = common.hardware_mic_length + TX_FCS_SIZE;
        let (power_code, format) = match common.rate {
            TxPhyRate::Ht(rate) => {
                // The access category's TXOP limit is the caller's to keep.
                // The owner's own length rule: the rate's ceiling and the
                // declared maximum it was configured with.
                let mut budget = buffer
                    .owner
                    .ht_length_budget(rate)
                    .map_err(|_| AmpduRefusal::Length)?;
                for psdu in buffer.lengths().map(|len| len + mic_and_fcs) {
                    let psdu = u32::try_from(psdu).map_err(|_| AmpduRefusal::Length)?;
                    budget.push(psdu, 0).map_err(|_| AmpduRefusal::Length)?;
                }
                (rate.power_lookup_code(), None)
            }
            TxPhyRate::He(rate) => {
                // The HE program carries the access category's advertised
                // TXOP limit; the core sets no ceiling of its own.
                let txop = he_txop_limit(txop_limit, HeEdcaTxopLimit::DEFAULT)
                    .map_err(|_| AmpduRefusal::Length)?;
                let policy = HeAmpduPolicy::new(rate, density, txop);
                let mut budget = buffer
                    .owner
                    .he_length_budget(policy)
                    .map_err(|_| AmpduRefusal::Length)?;
                for psdu in buffer.lengths().map(|len| len + mic_and_fcs) {
                    let psdu = u16::try_from(psdu).map_err(|_| AmpduRefusal::Length)?;
                    let delimiters = rate
                        .ampdu_empty_delimiters(psdu, density)
                        .ok_or(AmpduRefusal::Length)?;
                    budget
                        .push(u32::from(psdu), delimiters)
                        .map_err(|_| AmpduRefusal::Length)?;
                }
                (rate.power_lookup_code(), Some(policy))
            }
            TxPhyRate::Legacy(_) => return Err(AmpduRefusal::LegacyRate),
        };
        let data_power = self.tx.single_attempt_power_pair(power_code, ceiling);
        let publication = AmpduPublication {
            data_power_primary: data_power.primary as u8,
            data_power_alternate: data_power.alternate as u8,
            control: self.tx.single_attempt_control_frame(
                common.rate,
                common.single.protection,
                ceiling,
            ),
            aifsn: self
                .tx
                .policy()
                .contention_parameters(common.role, queue)
                .aifsn(),
            contention_window: common.single.backoff_slots,
            // `TxOperation::Data`, as for an MPDU.
            scheduler_priority: queue.vendor_data_scheduler_priority(),
            packet_priority: queue.vendor_data_packet_priority(),
        };
        let format = match (common.rate, format) {
            (TxPhyRate::Ht(rate), _) => AmpduFormat::Ht {
                rate,
                protection_spacing: ampdu::protection_spacing(min_mpdu_start_spacing),
            },
            (_, Some(policy)) => AmpduFormat::He {
                policy,
                bss_color: self.tx.policy().he_bss_color(),
            },
            (_, None) => return Err(AmpduRefusal::LegacyRate),
        };
        Ok(ampdu::AmpduSettings {
            lifetime: self
                .tx
                .policy()
                .access_policy(common.role, queue)
                .msdu_lifetime(),
            queue,
            format,
            role: AmpduTxRoleAdapter {
                interface: mac_interface(common.role),
                hardware_key_selector: common.hardware_key_selector,
            },
            hardware_mic_length: common.hardware_mic_length as u8,
            publication,
        })
    }

    /// Consume one interrupt or deadline edge of the published attempts.
    ///
    /// Every published queue is offered the edge and claims only its own
    /// queue's state. When a timeout abort ends, the CCA it forced is
    /// released, and every queue is offered a timeout edge once more: one
    /// whose timeout arrived during the settle aborts now.
    ///
    /// # Errors
    ///
    /// A fault of the transmit owner.
    pub fn service<
        H: HtAmpduHardware + CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        let mut wake = wake;
        loop {
            let mut abort_ended = false;
            for index in 0..LOWER_MAC_TX_QUEUES {
                // An idle queue is skipped without moving its slot.
                if self.queues[index].is_none() {
                    continue;
                }
                let Some(attempt) = self.queues[index].take() else {
                    continue;
                };
                let may_begin_timeout_abort = !self.abort_settling();
                let Attempt {
                    queue,
                    role,
                    admission,
                    rate,
                    admitted_at,
                    work,
                } = attempt;
                let queued = match work {
                    Work::Held { slot, .. }
                        if self.msdu_expired(role, queue.category(), admitted_at) =>
                    {
                        self.finish_attempt(
                            hardware,
                            role,
                            queue,
                            admission,
                            slot,
                            PublicationEnd::Expired { msdus: 1 },
                            rate,
                            sink,
                        );
                        continue;
                    }
                    held @ (Work::Held { .. } | Work::Quarantined { .. }) => {
                        self.queues[index] = Some(Attempt {
                            queue,
                            role,
                            admission,
                            rate,
                            admitted_at,
                            work: held,
                        });
                        continue;
                    }
                    Work::Published(queued) => queued,
                };
                let settling = queued.abort_settling();
                match self.tx.service_queued_single_attempt(
                    hardware,
                    queued,
                    wake,
                    may_begin_timeout_abort,
                ) {
                    Ok(QueuedSingleAttemptProgress::Pending(queued)) => {
                        self.queues[index] = Some(Attempt {
                            queue,
                            role,
                            admission,
                            rate,
                            admitted_at,
                            work: Work::Published(queued),
                        });
                    }
                    Ok(QueuedSingleAttemptProgress::Complete { outcome, slot }) => {
                        abort_ended |= settling;
                        self.finish_attempt(
                            hardware,
                            role,
                            queue,
                            admission,
                            slot,
                            publication_end(outcome),
                            rate,
                            sink,
                        );
                    }
                    Err(QueuedSingleAttemptFailed { error, quarantine }) => {
                        self.queues[index] = Some(Attempt {
                            queue,
                            role,
                            admission,
                            rate,
                            admitted_at,
                            work: Work::Quarantined { _owner: quarantine },
                        });
                        let fault = LowerMacFault::Tx(error);
                        self.fault = Some(fault);
                        return Err(fault);
                    }
                }
            }
            if !abort_ended {
                break;
            }
            wake = WifiTxWake::Interrupt {
                events: crate::mac::irq::EVENT_TX_TIMEOUT,
            };
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one terminal returns the complete admitted attempt"
    )]
    fn finish_attempt<H: CcmpKeyHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        hardware: &mut H,
        role: VifRole,
        queue: FreeQueue<'core, Path>,
        admission: TxAdmission<'core, Path>,
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        end: PublicationEnd,
        rate: PhyRate,
        sink: &mut K,
    ) {
        if let PublicationEnd::Expired { msdus } = end {
            self.expired_msdus = self.expired_msdus.saturating_add(msdus.count_ones());
        }
        if matches!(
            end,
            PublicationEnd::Collision
                | PublicationEnd::Completed {
                    status: TxStatus::AckTimeout | TxStatus::CtsTimeout | TxStatus::Collision,
                    ..
                }
        ) {
            self.tx
                .record_retry_failure(role, LegacyTxQueue::from_access_category(queue.category()));
        }
        match admission {
            TxAdmission::Open(ticket) => sink.tx_completed(queue, ticket, slot, end, rate),
            TxAdmission::Protected(key) => {
                let (index, ticket) = self.release_key(key);
                sink.tx_completed(queue, ticket, slot, end, rate);
                self.finish_key_removal(hardware, index, sink);
            }
        }
    }

    /// The owned view of one received MPDU, or `None` while the port does
    /// not receive or no receive rule admits it. The hardware policies pass
    /// a superset of the requested rules; this narrows it to them.
    ///
    /// The receive prefix carries no noise floor; the frame's is the PHY's
    /// noise-floor estimate when it is received, the value the S31 station
    /// reads for its rate control (`StaNoiseFloorHardware`).
    fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        let admitted = self.monitor || self.vifs.iter().flatten().any(|vif| vif.admits(frame.mpdu));
        (self.fault.is_none() && admitted).then(|| {
            let noise_floor_dbm = hardware.read_noise_floor_dbm();
            let mut common = portable::rx_meta(frame.metadata, self.channel());
            common.noise_floor_dbm = RxEvidence::HardwareObserved(noise_floor_dbm);
            (
                frame.mpdu,
                Esp32s31RxMeta {
                    common,
                    rssi_dbm: frame.rssi_dbm,
                    noise_floor_dbm,
                },
            )
        })
    }

    /// End a queue's held attempt as [`PublicationEnd::Withdrawn`]: it
    /// never reached the hardware, and its slot comes back with it.
    fn abort_held<H: CcmpKeyHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        hardware: &mut H,
        index: usize,
        sink: &mut K,
    ) {
        let Some(attempt) = self.queues[index].take() else {
            return;
        };
        match attempt {
            Attempt {
                role,
                queue,
                work: Work::Held { slot, .. },
                admission,
                rate,
                ..
            } => {
                self.finish_attempt(
                    hardware,
                    role,
                    queue,
                    admission,
                    slot,
                    PublicationEnd::Withdrawn,
                    rate,
                    sink,
                );
            }
            published => self.queues[index] = Some(published),
        }
    }

    /// Return every held attempt without publishing it.
    fn withdraw_held_attempts<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) {
        for index in 0..LOWER_MAC_TX_QUEUES {
            self.abort_held(hardware, index, sink);
        }
    }

    /// Retain the phase's six-bit color proof through the core boundary.
    pub fn set_checked_he_bss_color(&mut self, color: oer_ieee80211_contracts_port::BssColor) {
        self.tx.policy_mut().install_he_bss_color(color.get());
    }

    /// The beacon receive priority.
    // SOURCE(esp32s31): complete pinned `libpp[pm_coex.o]::pm_coex_update_rx_beacon_pti`
    // passes `coex_pti_get(0)`, or zero, as both arguments of
    // `hal_set_rx_beacon_pti`.
    pub fn set_rx_beacon_priority<H: RxBeaconPriorityHardware>(
        &mut self,
        hardware: &mut H,
        priority: RxBeaconPriority,
    ) {
        match priority {
            RxBeaconPriority::BeaconWindow => {
                hardware.set_rx_beacon_pti(self.coex.beacon_window_pti());
            }
            RxBeaconPriority::Zero => hardware.set_rx_beacon_pti(PRIORITY_ZERO),
            RxBeaconPriority::Cleared => hardware.clear_rx_beacon_pti(),
        }
    }

    /// The EDCA parameters the attempts of the interface of `role`
    /// contend with, whose records lie within the capabilities' limits: a
    /// set the policy refuses leaves the installed one. Every publication
    /// writes its queue's AIFSN and backoff from its own interface's set,
    /// so each interface contends with exactly the set it asked for, which
    /// this returns.
    ///
    /// # Errors
    ///
    /// The record the policy refused.
    pub fn set_checked_edca<K: oer_ieee80211_contracts_port::Ieee80211Capabilities>(
        &mut self,
        role: VifRole,
        parameters: oer_ieee80211_contracts_port::SupportedEdca<K>,
    ) -> Result<oer_ieee80211_contracts_port::SupportedEdca<K>, crate::mac::edca::EdcaParametersError>
    {
        self.tx.policy_mut().install_edca(role, parameters.get())?;
        Ok(parameters)
    }

    /// Tune the next `Enable` to `channel`, on a disabled port.
    fn set_checked_channel(&mut self, channel: WifiChannel) {
        if self.channel != channel {
            self.break_tsf_relation(VifRole::Station);
            self.break_tsf_relation(VifRole::AccessPoint);
        }
        self.channel = channel;
    }

    /// The station address the cold start published.
    pub const fn station_address(&self) -> [u8; 6] {
        self.config.station_address
    }

    /// Apply `config` and its receive `policy` to its role's interface.
    fn apply_vif<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        config: CoreVif,
        policy: ReceivePolicy,
    ) {
        match policy {
            ReceivePolicy::StationDisabled => hardware.disable_station_receive_registers(),
            ReceivePolicy::Station { bssid } => hardware.apply_sta_link_policy(bssid),
            ReceivePolicy::StationOtherBss { bssid } => hardware.apply_sta_esp_now_policy(bssid),
            ReceivePolicy::AccessPointDisabled => hardware.disable_ap_link_policy(),
            ReceivePolicy::AccessPoint { address } => hardware.apply_ap_link_policy(address),
        }
        let previous = self.vifs[config.role as usize].replace(config);
        // A new association, a reassociation or a roam: the interface
        // follows another TSF.
        if previous.is_none_or(|previous| previous.bss() != config.bss()) {
            self.break_tsf_relation(config.role);
        }
    }

    /// Install a key whose values and state a caller's types guarantee:
    /// the scope's slots have room (an access point's `association_id` is
    /// free), and a group key identifier is a valid one.
    ///
    /// # Errors
    ///
    /// The hardware refused the key.
    pub fn install_checked_key<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        installation: association::TransmitKeyInstallation<'_, 'core, Path>,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<KeyHandle<'core, Path>, association::KeyInstallRefused<'core, Path>> {
        let (spp, scope, slot) = installation.into_parts();
        self.install_key_parts(hardware, spp, scope, slot, temporal_key)
    }

    pub(crate) fn install_key_parts<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        spp: oer_ieee80211_wire::security::Spp,
        scope: TransmitKeyScope,
        slot: association::VacantKeySlot<'core, Path>,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<KeyHandle<'core, Path>, association::KeyInstallRefused<'core, Path>> {
        let (free, place) = match self.vacant_key(scope.into()) {
            Ok(vacant) => vacant,
            Err(fault) => return Err(association::KeyInstallRefused { fault, slot }),
        };
        hardware.configure_ccmp_association(
            match scope {
                TransmitKeyScope::StationPairwise { .. } => {
                    oer_esp32s31_hal::types::MacInterface::Station
                }
                _ => oer_esp32s31_hal::types::MacInterface::AccessPoint,
            },
            spp_mode(spp),
        );
        let token = match scope {
            TransmitKeyScope::StationPairwise { peer } => {
                install_sta_pairwise_ccmp(hardware, peer, temporal_key)
                    .map(TransmitKeyToken::StaPairwise)
            }
            TransmitKeyScope::AccessPointPairwise {
                peer,
                association_id,
            } => install_ap_pairwise_ccmp(hardware, peer, association_id, temporal_key)
                .map(TransmitKeyToken::ApPairwise),
            TransmitKeyScope::AccessPointGroup { key_id } => {
                install_ap_group_ccmp(hardware, key_id, temporal_key).map(TransmitKeyToken::ApGroup)
            }
        };
        let token = match token {
            Ok(token) => token,
            Err(error) => {
                return Err(association::KeyInstallRefused {
                    fault: LowerMacFault::Key(error),
                    slot,
                });
            }
        };
        *place = Some(scope.into());
        Ok(KeyHandle {
            slot,
            index: free,
            token,
            scope,
            brand: core::marker::PhantomData,
        })
    }

    /// Install a station GTK for reception. The returned handle cannot issue
    /// TX rights or allocate TX packet numbers; removal consumes it normally.
    pub fn install_station_group_key<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        association: &association::StationAssociation<'core, Path>,
        slot: association::StationGroupSlot<'core, Path>,
        key_id: oer_ieee80211_wire::ccmp::GroupKeyId,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<ReceiveKeyHandle<'core, Path>, association::KeyInstallRefused<'core, Path>> {
        self.install_station_group_parts(hardware, association.spp(), slot, key_id, temporal_key)
    }

    pub(crate) fn install_station_group_parts<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        spp: oer_ieee80211_wire::security::Spp,
        slot: association::StationGroupSlot<'core, Path>,
        key_id: oer_ieee80211_wire::ccmp::GroupKeyId,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<ReceiveKeyHandle<'core, Path>, association::KeyInstallRefused<'core, Path>> {
        let scope = CheckedKeyScope::StationGroup { key_id };
        let slot = association::VacantKeySlot::StationGroup(slot);
        let (index, place) = match self.vacant_key(scope) {
            Ok(vacant) => vacant,
            Err(fault) => return Err(association::KeyInstallRefused { fault, slot }),
        };
        hardware.configure_ccmp_association(
            oer_esp32s31_hal::types::MacInterface::Station,
            spp_mode(spp),
        );
        let token = match install_sta_group_ccmp(hardware, key_id, temporal_key) {
            Ok(token) => token,
            Err(error) => {
                return Err(association::KeyInstallRefused {
                    fault: LowerMacFault::Key(error),
                    slot,
                });
            }
        };
        *place = Some(scope);
        Ok(ReceiveKeyHandle {
            slot,
            index,
            token,
            material: crate::mac::crypto::StaGroupCcmpKeyMaterial::new(key_id, *temporal_key),
            key_id,
            brand: core::marker::PhantomData,
        })
    }

    /// Replace one occupied receive slot inside the port's finite RX cut.
    /// The handle retains zeroizing rollback material until removal/reset.
    /// Even a successful internal rollback poisons this core: it grants no
    /// reusable old owner and no successful portable terminal.
    pub(crate) fn replace_station_group_key<H: CcmpKeyHardware>(
        &mut self,
        hardware: &mut H,
        installed: &mut ReceiveKeyHandle<'core, Path>,
        key_id: oer_ieee80211_wire::ccmp::GroupKeyId,
        temporal_key: &[u8; CCMP_128_KEY_BYTES],
    ) -> Result<(), crate::mac::crypto::StaGroupCcmpReplaceError> {
        use crate::mac::crypto::{
            StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError, replace_sta_group_ccmp_with_rollback,
        };
        if self.fault.is_some() {
            return Err(StaGroupCcmpReplaceError::CurrentMaterialMismatch);
        }
        let Some(scope) = self.keys.installed.get_mut(installed.index) else {
            self.fault = Some(LowerMacFault::Key(CryptoKeyError::HardwareRejected));
            return Err(StaGroupCcmpReplaceError::CurrentMaterialMismatch);
        };
        let replacement = StaGroupCcmpKeyMaterial::new(key_id, *temporal_key);
        let result = replace_sta_group_ccmp_with_rollback(
            hardware,
            &mut installed.token,
            &installed.material,
            &replacement,
        );
        if result.is_ok() {
            installed.material = replacement;
            installed.key_id = key_id;
            *scope = Some(CheckedKeyScope::StationGroup { key_id });
        } else {
            self.fault = Some(LowerMacFault::Key(CryptoKeyError::HardwareRejected));
        }
        result
    }

    fn vacant_key(
        &mut self,
        scope: CheckedKeyScope,
    ) -> Result<(usize, &mut Option<CheckedKeyScope>), LowerMacFault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if self
            .keys
            .installed
            .iter()
            .flatten()
            .any(|installed| installed.same_slot(scope))
        {
            return Err(LowerMacFault::Key(CryptoKeyError::Occupied));
        }
        self.keys
            .installed
            .iter_mut()
            .enumerate()
            .find(|(_, place)| place.is_none())
            .ok_or(LowerMacFault::Key(CryptoKeyError::Occupied))
    }

    /// Return one admitted right to its key's count. A right naming no bank
    /// position poisons the core; its ticket still returns.
    fn release_key(&mut self, key: KeyUse<'core, Path>) -> (usize, TxTicket<'core, Path>) {
        let index = key.index;
        let ticket = match self.keys.uses.get_mut(index) {
            Some(uses) => uses.release(key),
            None => {
                self.fault.get_or_insert(LowerMacFault::KeyBank);
                key.ticket
            }
        };
        (index, ticket)
    }

    /// Admit one protected attempt from the live handle and one TX ticket.
    /// No reference into the core is held. The ticket bounds all admissions
    /// by the same `TICKETS` used by `CorePools<TICKETS>`; the counter cannot
    /// overflow and there is no capacity-refusal branch.
    pub fn admit_key(
        &mut self,
        key: &KeyHandle<'core, Path>,
        ticket: TxTicket<'core, Path>,
    ) -> Result<KeyUse<'core, Path>, KeyAdmissionRefused<'core, Path>> {
        if self.fault.is_some() {
            return Err(KeyAdmissionRefused {
                ticket,
                reason: KeyAdmissionError::Poisoned,
            });
        }
        let Some(uses) = self.keys.uses.get_mut(key.index) else {
            self.fault.get_or_insert(LowerMacFault::KeyBank);
            return Err(KeyAdmissionRefused {
                ticket,
                reason: KeyAdmissionError::Poisoned,
            });
        };
        Ok(uses.admit(key, ticket))
    }

    /// Consume the sole installed handle at removal's FIFO position. The
    /// programmed material stays until the last admitted use returns. No
    /// further right can be obtained from the moved handle.
    pub fn remove_checked_key<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        key: impl Into<RemovingKey<'core, Path>>,
        sink: &mut K,
    ) {
        let key = key.into();
        let index = key.index;
        let Some(removal) = self.keys.removals.get_mut(index) else {
            // No position can retain it; the poisoned core keeps the
            // programmed material until reset.
            self.fault.get_or_insert(LowerMacFault::KeyBank);
            return;
        };
        *removal = Some(key);
        self.finish_key_removal(hardware, index, sink);
    }

    fn finish_key_removal<H: CcmpKeyHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        hardware: &mut H,
        index: usize,
        sink: &mut K,
    ) {
        let (Some(uses), Some(removal), Some(installed)) = (
            self.keys.uses.get(index),
            self.keys.removals.get_mut(index),
            self.keys.installed.get_mut(index),
        ) else {
            return;
        };
        if self.fault.is_some() || !uses.is_empty() {
            return;
        }
        if let Some(key) = removal.take() {
            key.token.clear(hardware);
            *installed = None;
            sink.key_removed(KeyRemoved {
                slot: key.slot,
                scope: key.scope,
                brand: key.brand,
            });
        }
    }

    /// Return an admission which was never published (including submission
    /// refusal), or one carried by a detached aggregate's terminal. Published
    /// MPDUs keep this owner private and return it through their terminal.
    pub fn return_admission<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        admission: TxAdmission<'core, Path>,
        sink: &mut K,
    ) -> Result<TxTicket<'core, Path>, QuarantinedAdmission<'core, Path>> {
        if self.fault.is_some() {
            return Err(QuarantinedAdmission {
                _admission: admission,
            });
        }
        Ok(match admission {
            TxAdmission::Open(ticket) => ticket,
            TxAdmission::Protected(key) => {
                let (index, ticket) = self.release_key(key);
                self.finish_key_removal(hardware, index, sink);
                ticket
            }
        })
    }

    /// Start an agreement whose values a caller's types guarantee: its TID
    /// and window within the limits, its peer individual. It replaces the
    /// agreement of the same peer and TID in its bank, or takes a free one.
    ///
    /// # Errors
    ///
    /// No bank is free, or the hardware refused or misread the agreement.
    pub fn start_checked_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        agreement: RxBlockAckValues,
    ) -> Result<(), LowerMacFault> {
        let bank = self
            .rx_block_ack_bank(agreement.role, agreement.peer, agreement.tid)
            .or_else(|| self.rx_block_acks.iter().position(Option::is_none))
            .ok_or(LowerMacFault::NoRxBlockAckBank)?;
        hardware
            .program_rx_block_ack(S31RxBlockAckAgreement {
                hardware_index: bank as u8,
                interface: mac_interface(agreement.role),
                peer: agreement.peer,
                tid: agreement.tid,
                starting_sequence: agreement.start_sequence,
                window: u16::from(agreement.window),
            })
            .map_err(|error| match error {
                S31RxBlockAckAgreementError::HardwareReadbackMismatch => {
                    LowerMacFault::RxBlockAckReadback
                }
                error => LowerMacFault::RxBlockAck(error),
            })?;
        self.rx_block_acks[bank] = Some(RxBlockAckEntry {
            role: agreement.role,
            peer: agreement.peer,
            tid: agreement.tid,
        });
        Ok(())
    }

    /// End the agreement of `role` with `peer` on `tid`: `false` when none
    /// exists.
    ///
    /// # Errors
    ///
    /// The hardware refused to clear its bank.
    pub fn end_checked_rx_block_ack<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        role: VifRole,
        peer: [u8; 6],
        tid: u8,
    ) -> Result<bool, LowerMacFault> {
        let Some(bank) = self.rx_block_ack_bank(role, peer, tid) else {
            return Ok(false);
        };
        hardware
            .clear_rx_block_ack(bank as u8)
            .map_err(LowerMacFault::RxBlockAck)?;
        self.rx_block_acks[bank] = None;
        Ok(true)
    }

    fn rx_block_ack_bank(&self, role: VifRole, peer: [u8; 6], tid: u8) -> Option<usize> {
        self.rx_block_acks
            .iter()
            .position(|entry| *entry == Some(RxBlockAckEntry { role, peer, tid }))
    }

    /// Whether every queue is empty or retains only an unpublished attempt.
    pub fn published_idle(&self) -> bool {
        !self.attempts().any(|attempt| !attempt.work.held())
    }

    /// The station's TSF and the generation of its relation, for a
    /// [`TsfSample`](oer_ieee80211_contracts_port::TsfSample) the backend pairs
    /// with its radio clock: the access-point timer has no read seam.
    pub fn station_tsf_reading<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
    ) -> (VifTsf, TsfGeneration) {
        let at = self.station_tsf.read(hardware);
        (
            VifTsf {
                vif: VifRole::Station,
                at,
            },
            self.station_tsf.generation(),
        )
    }

    /// Start a new generation of the TSF relation of the interface with
    /// `role`.
    fn break_tsf_relation(&mut self, role: VifRole) {
        match role {
            VifRole::Station => self.station_tsf.break_relation(),
            VifRole::AccessPoint => self.access_point_tsf.break_relation(),
        }
    }

    /// Set the station's TSF. A set beyond the drift its relation allows
    /// starts a new generation of the relation.
    ///
    /// # Errors
    ///
    /// The set the relation cannot take.
    pub fn set_station_tsf<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        at: TsfInstant,
    ) -> Result<(), TsfTimingError> {
        self.station_tsf.set(hardware, at).map(|_| ())
    }

    /// Restart the access point's TSF from zero, a new generation of its
    /// relation.
    pub fn restart_access_point_tsf<H: LowerMacHardware>(&mut self, hardware: &mut H) {
        self.access_point_tsf.restart(hardware);
    }

    /// Program the station TBTT schedule. The event fires the schedule's
    /// lead before each TBTT; the wake lead published beside it keeps the
    /// vendor's window above the lead.
    ///
    /// # Errors
    ///
    /// A schedule the timer registers cannot hold: nothing changed.
    pub fn set_station_tbtt<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        next: TsfInstant,
        beacon_interval_micros: u64,
        lead_micros: u64,
    ) -> Result<(), UnsupportedTbtt> {
        let interval_micros = u32::try_from(beacon_interval_micros).map_err(|_| UnsupportedTbtt)?;
        let interval =
            core::num::NonZeroU64::new(u64::from(interval_micros)).ok_or(UnsupportedTbtt)?;
        let ahead_micros = u16::try_from(lead_micros).map_err(|_| UnsupportedTbtt)?;
        let wake_ahead_micros = ahead_micros
            .checked_add(STATION_TBTT_WAKE_WINDOW_MICROS)
            .ok_or(UnsupportedTbtt)?;
        let first = next.as_micros();
        hardware.start_station_tbtt(StaTbttSchedule {
            first_tbtt_tsf: first,
            interval_micros,
            ahead_micros,
            wake_ahead_micros,
        });
        self.tbtt = Some(StationTbtt { first, interval });
        Ok(())
    }

    /// Stop the station's TBTT schedule.
    pub fn stop_station_tbtt<H: LowerMacHardware>(&mut self, hardware: &mut H) {
        if self.tbtt.take().is_some() {
            hardware.stop_station_tbtt();
        }
    }

    /// Report the station TBTT whose event the power interrupt delivered
    /// (`MacPowerInterruptObservation::sta_tbtt`). Without a schedule the edge is stale
    /// and reports nothing.
    pub fn station_tbtt<H: StationTsfHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &self,
        hardware: &mut H,
        sink: &mut K,
    ) {
        if let Some(tbtt) = self.tbtt {
            let now = self.station_tsf.read(hardware).as_micros();
            let at = TsfInstant::from_micros(tbtt.announced(now));
            sink.tbtt(TbttEvent {
                tbtt: VifTsf {
                    vif: VifRole::Station,
                    at,
                },
            });
        }
    }

    /// Start or stop monitor reception through the open promiscuous
    /// policy, which runs only while no interface receives.
    ///
    /// # Errors
    ///
    /// An interface receives: nothing changed.
    pub fn set_monitor<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        enabled: bool,
    ) -> Result<(), UnsupportedVif> {
        if enabled {
            if self
                .vifs
                .iter()
                .flatten()
                .any(|vif| !vif.receive.is_empty())
            {
                return Err(UnsupportedVif);
            }
            hardware.configure_open_promiscuous_receive();
        } else if self.monitor {
            hardware.disable_open_promiscuous_receive();
        }
        self.monitor = enabled;
        Ok(())
    }
}

/// MAC priority zero.
const PRIORITY_ZERO: MacPti = match MacPti::new(0) {
    Some(priority) => priority,
    None => panic!("priority zero is a MAC priority"),
};

/// A publication the transmit owner refused: its fault, and the slot that
/// goes back.
#[must_use = "retain the returned transmit slot and handle the publication fault"]
pub struct SubmitFault<
    'core,
    'slot,
    const BUFFER_SIZE: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    pub queue: FreeQueue<'core, Path>,
    pub fault: LowerMacFault,
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    pub admission: TxAdmission<'core, Path>,
}

/// An interface configuration the receive policies cannot take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedVif;

/// A TBTT schedule the station timer cannot hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedTbtt;

/// What every attempt shares.
struct CommonAdmission {
    queue: LegacyTxQueue,
    role: VifRole,
    rate: TxPhyRate,
    hardware_key_selector: u8,
    hardware_mic_length: usize,
    /// Protection, backoff and power ceiling; `no_ack` is the MPDU's.
    single: SingleAttempt,
}

/// The scope of a checked key: its role, its kind and what places it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckedKeyScope {
    StationPairwise {
        peer: [u8; 6],
    },
    StationGroup {
        key_id: oer_ieee80211_wire::ccmp::GroupKeyId,
    },
    AccessPointPairwise {
        peer: [u8; 6],
        association_id: u16,
    },
    AccessPointGroup {
        key_id: crate::mac::crypto::ApGroupKeyId,
    },
}

impl CheckedKeyScope {
    const fn role(self) -> VifRole {
        match self {
            Self::StationPairwise { .. } | Self::StationGroup { .. } => VifRole::Station,
            Self::AccessPointPairwise { .. } | Self::AccessPointGroup { .. } => {
                VifRole::AccessPoint
            }
        }
    }
    fn same_slot(self, other: Self) -> bool {
        match (self, other) {
            (Self::StationPairwise { .. }, Self::StationPairwise { .. })
            | (Self::StationGroup { .. }, Self::StationGroup { .. }) => true,
            (
                Self::AccessPointPairwise {
                    association_id: a, ..
                },
                Self::AccessPointPairwise {
                    association_id: b, ..
                },
            ) => a == b,
            (Self::AccessPointGroup { key_id: a }, Self::AccessPointGroup { key_id: b }) => a == b,
            _ => false,
        }
    }
}

/// The values of an MPDU attempt a caller's types guarantee: the port
/// enabled, the access category's queue free, `role` configured, `key`
/// installed for `role`, the rate carried on the channel, the power,
/// backoff and coexistence level within the capabilities, the length at
/// least the header through Address 1 and at most the capabilities'
/// `max_mpdu_length`, and `no_ack` only of an individual frame at a rate
/// that sends one without an ACK ([`EnabledView::submit_checked`]).
pub struct MpduValues<'slot, const BUFFER_SIZE: usize> {
    role: VifRole,
    admitted_at: oer_time::Instant,
    /// The slot holding the MPDU after the TX metadata word.
    slot: crate::mac::tx::TxSlotFree<'slot, BUFFER_SIZE>,
    /// The MPDU's length, without its MIC and FCS.
    len: usize,
    /// An individual frame sent without soliciting its ACK.
    no_ack: bool,
    rate: TxPhyRate,
    /// The rate the completion reports.
    reported: PhyRate,
    protection: Protection,
    power_ceiling_dbm: Option<i8>,
    /// The backoff selected for this publication, in slots.
    backoff_slots: u16,
    operation: TxOperation,
    /// A transmission of an MPDU the plan sends again.
    retry: bool,
}

/// Untrusted parameters checked before constructing an MPDU publication.
#[derive(Clone, Copy, Debug)]
pub struct MpduParameters {
    pub role: VifRole,
    /// The backend admission sample, retained unchanged on retransmission.
    pub admitted_at: oer_time::Instant,
    pub len: usize,
    pub no_ack: bool,
    pub rate: PhyRate,
    pub protection: Protection,
    pub power_ceiling_dbm: Option<i8>,
    pub backoff_slots: u16,
    pub operation: TxOperation,
    pub retry: bool,
}

/// Why an MPDU cannot become checked submission values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduValueError {
    Length,
    Rate,
    Backoff,
    PowerCeiling,
    NoAck,
}

/// Checked construction refuses with the complete, unpublished slot.
#[must_use = "retain the refused slot"]
pub struct MpduValuesRefused<'slot, const BUFFER_SIZE: usize> {
    pub slot: crate::mac::tx::TxSlotFree<'slot, BUFFER_SIZE>,
    pub reason: MpduValueError,
}

impl<'slot, const BUFFER_SIZE: usize> MpduValues<'slot, BUFFER_SIZE> {
    /// Validate the public request once; callers cannot mutate the resulting
    /// length, rate or protection fields before submission. Encryption itself
    /// comes from the separately consumed `TxAdmission`, never these values.
    pub fn checked(
        slot: crate::mac::tx::TxSlotFree<'slot, BUFFER_SIZE>,
        values: MpduParameters,
    ) -> Result<Self, MpduValuesRefused<'slot, BUFFER_SIZE>> {
        let capabilities = esp32s31_lower_mac_capabilities(BUFFER_SIZE);
        let checked = (|| {
            if !(MIN_FRAME_LENGTH..=usize::from(capabilities.max_mpdu_length)).contains(&values.len)
            {
                return Err(MpduValueError::Length);
            }
            let rate = TxPhyRate::try_from(values.rate).map_err(|_| MpduValueError::Rate)?;
            if values.backoff_slots > MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS {
                return Err(MpduValueError::Backoff);
            }
            if values.power_ceiling_dbm.is_some_and(|ceiling| ceiling < 0) {
                return Err(MpduValueError::PowerCeiling);
            }
            if values.no_ack && !matches!(rate, TxPhyRate::Legacy(_)) {
                return Err(MpduValueError::NoAck);
            }
            Ok(rate)
        })();
        match checked {
            Ok(rate) => Ok(Self {
                role: values.role,
                admitted_at: values.admitted_at,
                slot,
                len: values.len,
                no_ack: values.no_ack,
                rate,
                reported: values.rate,
                protection: values.protection,
                power_ceiling_dbm: values.power_ceiling_dbm,
                backoff_slots: values.backoff_slots,
                operation: values.operation,
                retry: values.retry,
            }),
            Err(reason) => Err(MpduValuesRefused { slot, reason }),
        }
    }
}

/// The values of an aggregate a caller's types guarantee: `role`
/// configured, `key` installed for it, the power and backoff within the
/// capabilities ([`LowerMacCore::plan_ampdu`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduValues {
    pub role: VifRole,
    pub access_category: WmmAccessCategory,
    pub rate: TxPhyRate,
    pub protection: Protection,
    pub power_ceiling_dbm: Option<i8>,
    /// The backoff selected for this publication, in slots.
    pub backoff_slots: u16,
    /// The recipient's Minimum MPDU Start Spacing encoding, 0 to 7.
    pub min_mpdu_start_spacing: u8,
}

/// A refused aggregate plan returns its admission without publishing.
#[must_use = "retain both admission owners and handle the reason"]
pub struct AmpduPlanningRefused<'core, Path: PoolPath = Root<oer_radio_port::LocalId<'core>>> {
    pub queue: FreeQueue<'core, Path>,
    pub admission: TxAdmission<'core, Path>,
    pub reason: AmpduRefusal,
}

impl<Path: PoolPath> core::fmt::Debug for AmpduPlanningRefused<'_, Path> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AmpduPlanningRefused")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Why an aggregate cannot go as one publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRefusal {
    /// No subframe.
    Empty,
    /// More subframes than the aggregate owner holds.
    TooManySubframes,
    /// A start spacing encoding above 7.
    StartSpacing,
    /// A subframe shorter than its header through Address 1.
    ShortSubframe,
    /// The first subframe is group addressed: no one recipient answers.
    GroupRecipient,
    /// Longer than the rate's ceiling, the declared maximum or the HE TXOP
    /// limit allows, with every subframe's MIC, FCS and delimiters.
    Length,
    /// A non-HT rate: aggregates go in HT or HE PPDUs.
    LegacyRate,
    /// A subframe or interface does not match the owned encryption admission.
    KeyUseMismatch,
}

/// An MPDU submission planned.
struct Admission {
    queue: LegacyTxQueue,
    plan: OrdinaryTxPlan,
    single: SingleAttempt,
}

const fn mac_interface(role: VifRole) -> MacInterface {
    match role {
        VifRole::Station => MacInterface::Station,
        VifRole::AccessPoint => MacInterface::AccessPoint,
    }
}

/// The one receive-policy transaction a configuration maps onto.
#[derive(Clone, Copy)]
enum ReceivePolicy {
    StationDisabled,
    /// Station policy six: the BSS-member rules.
    Station {
        bssid: [u8; 6],
    },
    /// Station policy six, mode two: the BSS-member rules and management
    /// frames of every BSS.
    StationOtherBss {
        bssid: [u8; 6],
    },
    AccessPointDisabled,
    /// Access-point policy eight: the BSS-member rules and the Probe
    /// Requests the access point answers, wildcard ones included.
    AccessPoint {
        address: [u8; 6],
    },
}

/// Map a receive filter onto the smallest S31 receive policy that passes a
/// superset of it; [`EnabledView::received`] narrows the superset.
fn receive_policy(config: &LowerMacConfig, vif: &CoreVif) -> Result<ReceivePolicy, UnsupportedVif> {
    let limits = esp32s31_lower_mac_capabilities(0).receive_filters(vif.role);
    if !limits.contains(vif.receive) {
        return Err(UnsupportedVif);
    }
    match vif.role {
        VifRole::Station => {
            // The station address is published by the cold start only.
            if vif.address != config.station_address {
                return Err(UnsupportedVif);
            }
            let other_bss = vif.receive.contains(ReceiveFilter::OTHER_BSS_MANAGEMENT);
            match vif.bssid {
                _ if vif.receive.is_empty() => Ok(ReceivePolicy::StationDisabled),
                // Outside a BSS only other BSSs' management is defined; the
                // policy matches the broadcast BSSID, as standalone ESP-NOW
                // reception does.
                None if vif.receive == ReceiveFilter::OTHER_BSS_MANAGEMENT => {
                    Ok(ReceivePolicy::StationOtherBss {
                        bssid: BROADCAST_ADDRESS,
                    })
                }
                None => Err(UnsupportedVif),
                Some(bssid) if other_bss => Ok(ReceivePolicy::StationOtherBss { bssid }),
                Some(bssid) => Ok(ReceivePolicy::Station { bssid }),
            }
        }
        VifRole::AccessPoint => {
            if vif.address[0] & 1 != 0 || vif.bssid.is_some_and(|bssid| bssid != vif.address) {
                return Err(UnsupportedVif);
            }
            if vif.receive.is_empty() {
                Ok(ReceivePolicy::AccessPointDisabled)
            } else {
                Ok(ReceivePolicy::AccessPoint {
                    address: vif.address,
                })
            }
        }
    }
}

/// How one publication ended.
fn publication_end(outcome: OrdinaryTxOutcome) -> PublicationEnd {
    match outcome {
        OrdinaryTxOutcome::Success(completed) | OrdinaryTxOutcome::HardwareFailure(completed) => {
            PublicationEnd::Completed {
                status: completed.status(),
                ack_snr_db: completed.report().status.ack_snr_db,
                block_ack: block_ack(completed.report()),
            }
        }
        OrdinaryTxOutcome::HardwareTimeout(_) => PublicationEnd::TimedOut,
        OrdinaryTxOutcome::CollisionLimit(_) => PublicationEnd::Collision,
    }
}

/// The BlockAck a BlockAckReq's completion carried.
fn block_ack(report: crate::ordinary_tx::OrdinaryTxReport) -> Option<BlockAckReport> {
    report.block_ack.map(|observation| BlockAckReport {
        start_sequence: observation.block_ack.starting_sequence,
        bitmap: observation.block_ack.bitmap,
    })
}

#[cfg(test)]
mod tests;

fn spp_mode(spp: oer_ieee80211_wire::security::Spp) -> oer_esp32s31_hal::types::MacSppMode {
    match spp {
        oer_ieee80211_wire::security::Spp::Negotiated => {
            oer_esp32s31_hal::types::MacSppMode::Authenticated
        }
        oer_ieee80211_wire::security::Spp::NotNegotiated => {
            oer_esp32s31_hal::types::MacSppMode::Unauthenticated
        }
    }
}

// The private queued path received its slot through MpduValues' consumed free
// loan or a successful queue-detach completion. A slot whose DMA owner left
// its free phase is stale.
fn queued_free_buffer<const N: usize>(
    slot: Pin<&mut TxSlot<N>>,
) -> Result<&mut [u8; N], OrdinaryTxError> {
    match slot.access() {
        crate::mac::tx::TxSlotAccess::Free(free) => Ok(free.into_buffer()?),
        _ => Err(OrdinaryTxError::Tx(crate::mac::tx::TxError::Stale)),
    }
}
