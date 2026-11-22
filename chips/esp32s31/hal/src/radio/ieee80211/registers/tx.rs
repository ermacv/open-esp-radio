//! The ordinary EDCA TX queue transactions of the Wi-Fi MAC.
//!
//! Each transaction is the vendor's sequence over the generated parts' field
//! accessors; the descriptor head reaches `CONTROL%s` only through the typed
//! pointer setter, from a [`DmaRef`] the prepared DMA authority holds.

use crate::layouts::{DmaRef, WifiTxDmaDescriptor};

use core::marker::PhantomData;

use crate::radio::ieee80211::dma::tx_storage::CommitTxDma;
use crate::types::{MacPlcpRate as PlcpRate, MacRtsRate as RtsRate};
use oer_chips_esp32s31_pac::wifi_mac::{
    control_n::PlcpFormat, length_control::TxopListSelect, plcp1::VectorFormat,
};
use oer_portable_memory::{HardwareOwnedTxDma, PreparedTxDma};
use oer_portable_mmio::Bits;

use crate::radio::ieee80211::registers::{
    MacInterface, MacPlacement, MacPti, MacTxPtiCount, MacTxQueueIndex, WifiDescriptorAddressLow,
    WifiRadioRegisters, byte, halfword,
};

const DESCRIPTOR_ADDRESS_LOW_MASK: u32 = WifiDescriptorAddressLow::MAX;

/// Read-only state of one ordinary hardware TX queue.
///
/// This projection deliberately keeps the queue's descriptor identity and
/// publication/result state together. It is used at low-frequency role
/// boundaries to distinguish a real publication from a stale completion;
/// it grants no register mutation authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacOrdinaryTxQueueSnapshot {
    pub descriptor_address_low: u32,
    pub plcp_format: u8,
    pub legacy_signal: u16,
    pub rate: u8,
    pub key_entry_index: u8,
    pub bssid_select: u8,
    pub vector_format: u8,
    pub interface: MacInterface,
    pub aifsn: u8,
    pub contention_window: u16,
    pub scheduler_priority: u8,
    /// Signed calibrated data primary/alternate and RTS primary/alternate codes.
    pub power_codes: [i8; 4],
    pub cca_force: u8,
    pub cca_aux_force: u8,
    /// Shared MAC activity encoding sampled after the queue configuration.
    /// Vendor channel-stop code identifies TX/RX activity within this field;
    /// it does not define a transmit-start timestamp or a CCA/NAV state.
    /// The snapshot is a sequence of reads, not an atomic hardware capture.
    pub mac_active_state: u8,
    /// Cumulative hang counters sampled independently of queue/activity state.
    pub hang: crate::radio::ieee80211::registers::rx::statistics::MacRxHangStatistics,
    /// Global wrapping 16-bit RX progress counters, sampled independently.
    pub rx_mpdu_count: u16,
    pub rx_signal_count: u16,
    pub rx_end_count: u16,
    pub rx_fcs_error_count: u16,
    pub rx_abort_count: u16,
    pub ownership: MacTxQueueOwnership,
    pub pending: MacTxQueuePending,
}

/// The queue's `ENABLE` and `VALID` bits as observed: who holds the queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxQueueOwnership {
    /// Neither bit: software holds the queue.
    Idle,
    /// `ENABLE` alone.
    Enabled,
    /// `VALID` alone.
    Valid,
    /// Both bits: the queue is published to the hardware.
    Published,
}

impl MacTxQueueOwnership {
    const fn observed(enabled: bool, valid: bool) -> Self {
        match (enabled, valid) {
            (false, false) => Self::Idle,
            (true, false) => Self::Enabled,
            (false, true) => Self::Valid,
            (true, true) => Self::Published,
        }
    }
}

/// The queue's raised result events, each one independent of the others.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacTxQueuePending(u8);

impl MacTxQueuePending {
    const COMPLETION: u8 = 1;
    const TIMEOUT: u8 = 1 << 1;
    const COLLISION: u8 = 1 << 2;

    /// The set of the raised events among the three.
    const fn observed(completion: bool, timeout: bool, collision: bool) -> Self {
        Self(
            (if completion { Self::COMPLETION } else { 0 })
                | (if timeout { Self::TIMEOUT } else { 0 })
                | (if collision { Self::COLLISION } else { 0 }),
        )
    }

    /// Whether the completion event is raised.
    pub const fn completion(self) -> bool {
        self.0 & Self::COMPLETION != 0
    }

    /// Whether the timeout event is raised.
    pub const fn timeout(self) -> bool {
        self.0 & Self::TIMEOUT != 0
    }

    /// Whether the collision event is raised.
    pub const fn collision(self) -> bool {
        self.0 & Self::COLLISION != 0
    }
}

/// Closed argument projection of complete `hal_set_tx_pti`.
///
/// The four PTI fields are intentionally distinct: the vendor leaf accepts
/// independent values and writes them through five separate fresh-read RMW
/// edges after the scheduler priority edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPtiProgram {
    pub scheduler_priority: MacPti,
    pub pti_2: MacPti,
    pub pti_1: MacPti,
    pub pti_0: MacPti,
    pub pti_3: MacPti,
    pub count: MacTxPtiCount,
}

/// Why a queue was not prepared for a chain. Whatever the reason, its chain
/// stays with its owner: nothing was handed to the hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxPrepareRefused {
    /// The queue still runs a chain; nothing was written. Its completion
    /// frees it.
    QueueActive,
    /// A retry does not change the answer.
    Fault(MacTxPrepareFault),
}

/// A refusal of a chain that no later attempt changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxPrepareFault {
    /// The program was bound to another chain; nothing was written.
    ForeignProgram,
    /// The queue names another chain, at this address: it was prepared for
    /// that one since, or the hardware reads back something else. Nothing
    /// was started.
    ChainNotNamed { named: u32 },
    /// This hardware backend programs no such PPDU; nothing was written.
    Unsupported,
}

/// Whether a program's descriptor address names the authority's chain.
fn tx_descriptor_head_matches(
    authority_head: DmaRef<WifiTxDmaDescriptor>,
    descriptor_address_low: u32,
) -> bool {
    descriptor_address_low == authority_head.bits() & DESCRIPTOR_ADDRESS_LOW_MASK
}

/// The preamble of a DSSS or CCK PPDU, as `oer_protocols_ieee80211_wire`'s
/// `DsssPreamble` names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacDsssPreamble {
    Long,
    Short,
}

/// One non-HT transmit rate, with the variants of
/// `oer_protocols_ieee80211_wire`'s `LegacyRate`.
///
/// It feeds two fields: `PLCP1%s.RATE` (`plcp1::Rate`, which also holds the
/// HT and HE rates) and `LENGTH_CONTROL%s.RTS_RATE` (`length_control::RtsRate`,
/// the non-HT rates alone), with the same code in each.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MacLegacyRate {
    #[default]
    Dsss1M,
    Dsss2M(MacDsssPreamble),
    Cck5M5(MacDsssPreamble),
    Cck11M(MacDsssPreamble),
    Ofdm6M,
    Ofdm9M,
    Ofdm12M,
    Ofdm18M,
    Ofdm24M,
    Ofdm36M,
    Ofdm48M,
    Ofdm54M,
}

impl MacLegacyRate {
    /// The rate's `PLCP1%s.RATE` value.
    pub const fn plcp_rate(self) -> PlcpRate {
        use MacDsssPreamble::{Long, Short};
        match self {
            Self::Dsss1M => PlcpRate::Dsss1mLong,
            Self::Dsss2M(Long) => PlcpRate::Dsss2mLong,
            Self::Dsss2M(Short) => PlcpRate::Dsss2mShort,
            Self::Cck5M5(Long) => PlcpRate::Cck5m5Long,
            Self::Cck5M5(Short) => PlcpRate::Cck5m5Short,
            Self::Cck11M(Long) => PlcpRate::Cck11mLong,
            Self::Cck11M(Short) => PlcpRate::Cck11mShort,
            Self::Ofdm6M => PlcpRate::Ofdm6m,
            Self::Ofdm9M => PlcpRate::Ofdm9m,
            Self::Ofdm12M => PlcpRate::Ofdm12m,
            Self::Ofdm18M => PlcpRate::Ofdm18m,
            Self::Ofdm24M => PlcpRate::Ofdm24m,
            Self::Ofdm36M => PlcpRate::Ofdm36m,
            Self::Ofdm48M => PlcpRate::Ofdm48m,
            Self::Ofdm54M => PlcpRate::Ofdm54m,
        }
    }

    /// The rate's `LENGTH_CONTROL%s.RTS_RATE` value.
    pub const fn rts_rate(self) -> RtsRate {
        use MacDsssPreamble::{Long, Short};
        match self {
            Self::Dsss1M => RtsRate::Dsss1mLong,
            Self::Dsss2M(Long) => RtsRate::Dsss2mLong,
            Self::Dsss2M(Short) => RtsRate::Dsss2mShort,
            Self::Cck5M5(Long) => RtsRate::Cck5m5Long,
            Self::Cck5M5(Short) => RtsRate::Cck5m5Short,
            Self::Cck11M(Long) => RtsRate::Cck11mLong,
            Self::Cck11M(Short) => RtsRate::Cck11mShort,
            Self::Ofdm6M => RtsRate::Ofdm6m,
            Self::Ofdm9M => RtsRate::Ofdm9m,
            Self::Ofdm12M => RtsRate::Ofdm12m,
            Self::Ofdm18M => RtsRate::Ofdm18m,
            Self::Ofdm24M => RtsRate::Ofdm24m,
            Self::Ofdm36M => RtsRate::Ofdm36m,
            Self::Ofdm48M => RtsRate::Ofdm48m,
            Self::Ofdm54M => RtsRate::Ofdm54m,
        }
    }

    /// The non-HT rate of a `PLCP1%s.RATE` value; an HT or HE rate has none.
    pub const fn from_plcp_rate(rate: PlcpRate) -> Option<Self> {
        use MacDsssPreamble::{Long, Short};
        match rate {
            PlcpRate::Dsss1mLong => Some(Self::Dsss1M),
            PlcpRate::Dsss2mLong => Some(Self::Dsss2M(Long)),
            PlcpRate::Dsss2mShort => Some(Self::Dsss2M(Short)),
            PlcpRate::Cck5m5Long => Some(Self::Cck5M5(Long)),
            PlcpRate::Cck5m5Short => Some(Self::Cck5M5(Short)),
            PlcpRate::Cck11mLong => Some(Self::Cck11M(Long)),
            PlcpRate::Cck11mShort => Some(Self::Cck11M(Short)),
            PlcpRate::Ofdm6m => Some(Self::Ofdm6M),
            PlcpRate::Ofdm9m => Some(Self::Ofdm9M),
            PlcpRate::Ofdm12m => Some(Self::Ofdm12M),
            PlcpRate::Ofdm18m => Some(Self::Ofdm18M),
            PlcpRate::Ofdm24m => Some(Self::Ofdm24M),
            PlcpRate::Ofdm36m => Some(Self::Ofdm36M),
            PlcpRate::Ofdm48m => Some(Self::Ofdm48M),
            PlcpRate::Ofdm54m => Some(Self::Ofdm54M),
            _ => None,
        }
    }

    /// The rate's rate-control byte (`wifi_phy_rate_t`), which is its
    /// `PLCP1%s.RATE` code: the number the backend's rate control and the
    /// receive descriptors carry.
    pub const fn code(self) -> u8 {
        self.plcp_rate().bits() as u8
    }

    /// The non-HT rate of a rate-control byte; an unknown or HT code has none.
    pub const fn from_code(code: u8) -> Option<Self> {
        match PlcpRate::from_bits(code as u32) {
            Some(rate) => Self::from_plcp_rate(rate),
            None => None,
        }
    }
}

/// Software-requested control exchange for one ordinary queue publication.
///
/// The queue PROTECTION word carries one request flag per mechanism: bit 30
/// requests a hardware-generated CTS addressed to this transmitter, bit 31 a
/// hardware-generated RTS addressed to the PPDU receiver.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MacTxProtection {
    #[default]
    None,
    CtsToSelf,
    RtsCts,
}

impl MacTxProtection {
    /// The request's `PROTECTION%s.SOFTWARE_CTS` value.
    pub(crate) const fn software_cts(
        self,
    ) -> oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareCts {
        match self {
            Self::CtsToSelf => oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareCts::Requested,
            Self::None | Self::RtsCts => {
                oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareCts::None
            }
        }
    }

    /// The request's `PROTECTION%s.SOFTWARE_RTS` value.
    pub(crate) const fn software_rts(
        self,
    ) -> oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareRts {
        match self {
            Self::RtsCts => oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareRts::Requested,
            Self::None | Self::CtsToSelf => {
                oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareRts::None
            }
        }
    }
}

/// Control frame associated with one queue publication.
///
/// `rate` and the power pair are always published in the queue's
/// LENGTH_CONTROL and power vectors, including when `protection` is `None`:
/// the recovered `mac_tx_set_len` writes the rate for every PPDU. The rate is
/// the transmit rate of the hardware-generated RTS or CTS-to-self when a
/// protection request is present. Duration/NAV values are generated by the
/// MAC from the programmed PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxControlFrame {
    pub protection: MacTxProtection,
    pub rate: MacLegacyRate,
    pub power_primary: u8,
    pub power_alternate: u8,
}

/// Response the MAC awaits after one legacy PPDU, published as the PLCP0
/// format selector.
///
/// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_set_plcp0` derives
/// the selector from descriptor flags: a group-addressed frame (flag
/// `0x2`) keeps format zero; an individually addressed frame selects
/// `1 + bit19`, bit 19 marking the BlockAckReq built by
/// `libpp[pp.o]::ppFillAMPDUBar` (vendor-library BAR answer). HT A-MPDUs,
/// which are answered by a BlockAck as well, use format two.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MacLegacyTxResponse {
    /// A group-addressed frame: nothing answers it.
    None,
    /// An individually addressed frame answered by an ACK.
    #[default]
    Ack,
    /// A BlockAckReq answered by a BlockAck, which the MAC captures in the
    /// queue's BlockAck registers.
    BlockAck,
}

impl MacLegacyTxResponse {
    const fn plcp_format(self) -> PlcpFormat {
        match self {
            Self::None => oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::NoResponse,
            Self::Ack => oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::AckResponse,
            Self::BlockAck => {
                oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::BlockAckResponse
            }
        }
    }
}

/// The largest `timeout` a queue program accepts: the queue configuration's
/// `TIMEOUT` field (`WIFI_MAC.CONFIG.TIMEOUT`, generated `TIMEOUT_MAX`).
pub const MAC_TX_TIMEOUT_MAX: u16 = {
    const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::config::TIMEOUT_MAX;
    if MAX > u16::MAX as u32 {
        panic!("the queue timeout field fits a u16");
    }
    MAX as u16
};

/// Semantic inputs for one bounded legacy queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxParameters {
    pub control: MacTxControlFrame,
    pub rate: MacLegacyRate,
    pub signal: u16,
    pub data_power: u8,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
    pub response: MacLegacyTxResponse,
    pub hardware_key_selector: u8,
}

/// The queue's numbers of one program, each checked against its field's
/// width once, when the program is made: the writers take them as they are.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QueueFields {
    timeout: Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::TIMEOUT_WIDTH }>,
    scheduler_priority:
        Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_WIDTH }>,
    packet_priority: Bits<{ oer_chips_esp32s31_pac::wifi_mac::pti::PTI_0_WIDTH }>,
    priority_count: Bits<{ oer_chips_esp32s31_pac::wifi_mac::pti::COUNT_WIDTH }>,
    aifsn: Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::AIFSN_WIDTH }>,
    contention_window: Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::CONTENTION_WINDOW_WIDTH }>,
    key_entry_index: Bits<{ oer_chips_esp32s31_pac::wifi_mac::plcp1::KEY_ENTRY_INDEX_WIDTH }>,
}

impl QueueFields {
    /// The numbers as their fields; `None` if one is wider than its field.
    const fn new(
        timeout: u16,
        scheduler_priority: u8,
        packet_priority: u8,
        priority_count: u16,
        aifsn: u8,
        contention_window: u16,
        key_entry_index: u8,
    ) -> Option<Self> {
        let (
            Ok(timeout),
            Ok(scheduler_priority),
            Ok(packet_priority),
            Ok(priority_count),
            Ok(aifsn),
            Ok(contention_window),
            Ok(key_entry_index),
        ) = (
            Bits::new(timeout as u32),
            Bits::new(scheduler_priority as u32),
            Bits::new(packet_priority as u32),
            Bits::new(priority_count as u32),
            Bits::new(aifsn as u32),
            Bits::new(contention_window as u32),
            Bits::new(key_entry_index as u32),
        )
        else {
            return None;
        };
        Some(Self {
            timeout,
            scheduler_priority,
            packet_priority,
            priority_count,
            aifsn,
            contention_window,
            key_entry_index,
        })
    }
}

/// HAL-owned register program for one bounded legacy PPDU.
///
/// All register images are private. Higher layers can retain and inspect the
/// semantic values through accessors, but cannot publish or test raw PLCP
/// geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxProgram {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    parameters: MacLegacyTxParameters,
    fields: QueueFields,
    signal: Bits<{ oer_chips_esp32s31_pac::wifi_mac::plcp1::LEGACY_SIGNAL_WIDTH }>,
}

impl MacLegacyTxProgram {
    /// Legacy rate of the data PPDU bound to this publication.
    pub const fn rate(self) -> MacLegacyRate {
        self.parameters.rate
    }

    /// Control frame retained with this descriptor-bound program.
    pub const fn control(self) -> MacTxControlFrame {
        self.parameters.control
    }

    /// Bind one semantic legacy publication to its prepared DMA authority.
    pub fn new(
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacLegacyTxParameters,
    ) -> Option<Self> {
        let Some(fields) = QueueFields::new(
            parameters.timeout,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
            parameters.aifsn,
            parameters.contention_window,
            parameters.hardware_key_selector,
        ) else {
            return None;
        };
        let Ok(signal) = Bits::new(u32::from(parameters.signal)) else {
            return None;
        };

        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
            fields,
            signal,
        })
    }

    pub const fn interface(self) -> MacInterface {
        self.parameters.interface
    }

    /// Hardware key slot the MAC encrypts the MPDU under; zero for none.
    pub const fn hardware_key_selector(self) -> u8 {
        self.parameters.hardware_key_selector
    }

    pub const fn scheduler_priority(self) -> u8 {
        self.parameters.scheduler_priority
    }

    pub const fn packet_priority(self) -> u8 {
        self.parameters.packet_priority
    }

    pub const fn priority_count(self) -> u16 {
        self.parameters.priority_count
    }

    pub const fn signal(self) -> u16 {
        self.parameters.signal
    }

    /// Power code of the data PPDU.
    pub const fn data_power(self) -> u8 {
        self.parameters.data_power
    }

    /// Backoff slots the queue counts down.
    pub const fn contention_window(self) -> u16 {
        self.parameters.contention_window
    }

    /// Arbitration interframe space number the queue waits before counting.
    pub const fn aifsn(self) -> u8 {
        self.parameters.aifsn
    }

    /// Response the MAC awaits after the PPDU.
    pub const fn response(self) -> MacLegacyTxResponse {
        self.parameters.response
    }
}

/// Reviewed MCS selector for one non-HE HT PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtMcs {
    Mcs0,
    Mcs1,
    Mcs2,
    Mcs3,
    Mcs4,
    Mcs5,
    Mcs6,
    Mcs7,
}

impl MacHtMcs {
    const fn signal_mcs(self) -> oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs {
        match self {
            Self::Mcs0 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs0,
            Self::Mcs1 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs1,
            Self::Mcs2 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs2,
            Self::Mcs3 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs3,
            Self::Mcs4 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs4,
            Self::Mcs5 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs5,
            Self::Mcs6 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs6,
            Self::Mcs7 => oer_chips_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs7,
        }
    }

    /// The MCS's `DATA_LENGTH%s.HT40_MCS` value: the same index.
    const fn ht40_mcs(self) -> oer_chips_esp32s31_pac::wifi_mac::data_length::Ht40Mcs {
        use oer_chips_esp32s31_pac::wifi_mac::data_length::Ht40Mcs;
        match self {
            Self::Mcs0 => Ht40Mcs::Mcs0,
            Self::Mcs1 => Ht40Mcs::Mcs1,
            Self::Mcs2 => Ht40Mcs::Mcs2,
            Self::Mcs3 => Ht40Mcs::Mcs3,
            Self::Mcs4 => Ht40Mcs::Mcs4,
            Self::Mcs5 => Ht40Mcs::Mcs5,
            Self::Mcs6 => Ht40Mcs::Mcs6,
            Self::Mcs7 => Ht40Mcs::Mcs7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtGuardInterval {
    /// The 800 ns guard interval.
    Normal,
    /// The short, 400 ns, guard interval.
    Short,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtChannelWidth {
    Mhz20,
    Mhz40,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtProtectionSpacing {
    Density0To4,
    Density5,
    Density6,
    Density7,
}

impl MacHtProtectionSpacing {
    /// The spacing as the minimum-MPDU-length fields hold it.
    const fn register_value(
        self,
    ) -> Bits<{ oer_chips_esp32s31_pac::wifi_mac::protection::MINIMUM_MPDU_LENGTH_CBW20_WIDTH }>
    {
        match self {
            Self::Density0To4 => Bits::constant::<20>(),
            Self::Density5 => Bits::constant::<40>(),
            Self::Density6 => Bits::constant::<76>(),
            Self::Density7 => Bits::constant::<148>(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtTxFormat {
    SingleMpdu,
    Ampdu,
}

/// Protocol-semantic rate selected for one non-HE HT PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtRate {
    pub mcs: MacHtMcs,
    pub guard_interval: MacHtGuardInterval,
    pub channel_width: MacHtChannelWidth,
}

impl MacHtRate {
    const fn plcp_rate(self) -> PlcpRate {
        match (self.guard_interval, self.mcs) {
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs0) => crate::types::MacPlcpRate::HtLongMcs0,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs1) => crate::types::MacPlcpRate::HtLongMcs1,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs2) => crate::types::MacPlcpRate::HtLongMcs2,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs3) => crate::types::MacPlcpRate::HtLongMcs3,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs4) => crate::types::MacPlcpRate::HtLongMcs4,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs5) => crate::types::MacPlcpRate::HtLongMcs5,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs6) => crate::types::MacPlcpRate::HtLongMcs6,
            (MacHtGuardInterval::Normal, MacHtMcs::Mcs7) => crate::types::MacPlcpRate::HtLongMcs7,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs0) => crate::types::MacPlcpRate::HtShortMcs0,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs1) => crate::types::MacPlcpRate::HtShortMcs1,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs2) => crate::types::MacPlcpRate::HtShortMcs2,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs3) => crate::types::MacPlcpRate::HtShortMcs3,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs4) => crate::types::MacPlcpRate::HtShortMcs4,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs5) => crate::types::MacPlcpRate::HtShortMcs5,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs6) => crate::types::MacPlcpRate::HtShortMcs6,
            (MacHtGuardInterval::Short, MacHtMcs::Mcs7) => crate::types::MacPlcpRate::HtShortMcs7,
        }
    }
}

/// Semantic inputs for one bounded HT queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtTxParameters {
    pub control: MacTxControlFrame,
    pub rate: MacHtRate,
    pub format: MacHtTxFormat,
    pub length: u16,
    pub descriptor_count: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub protection_spacing: MacHtProtectionSpacing,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
    pub txop: crate::types::MacTxop,
}

/// HAL-owned register program for one bounded HT PPDU.
///
/// Whole queue-vector words and their field geometry remain private. Higher
/// layers select only protocol-semantic values and bind them to a prepared DMA
/// authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtTxProgram {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    parameters: MacHtTxParameters,
    fields: QueueFields,
    length: Bits<{ oer_chips_esp32s31_pac::wifi_mac::data_length::HT40_LENGTH_WIDTH }>,
    descriptor_count:
        Bits<{ oer_chips_esp32s31_pac::wifi_mac::ht_descriptor_counts::DESCRIPTOR_COUNT_A_WIDTH }>,
}

impl MacHtTxProgram {
    /// Control frame retained with this descriptor-bound program.
    pub const fn control(self) -> MacTxControlFrame {
        self.parameters.control
    }

    /// Backoff slots the queue counts down.
    pub const fn contention_window(self) -> u16 {
        self.parameters.contention_window
    }

    /// Arbitration interframe space number the queue waits before counting.
    pub const fn aifsn(self) -> u8 {
        self.parameters.aifsn
    }

    /// Bind one semantic HT publication to its prepared DMA authority.
    pub fn new(
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacHtTxParameters,
    ) -> Option<Self> {
        let Some(fields) = QueueFields::new(
            parameters.timeout,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
            parameters.aifsn,
            parameters.contention_window,
            parameters.hardware_key_selector,
        ) else {
            return None;
        };
        if parameters.length == 0
            || parameters.descriptor_count == 0
            || u32::from(parameters.descriptor_count)
                > oer_chips_esp32s31_pac::wifi_mac::ht_descriptor_counts::DESCRIPTOR_COUNT_A_MAX
            || (matches!(parameters.format, MacHtTxFormat::SingleMpdu)
                && parameters.descriptor_count != 1)
        {
            return None;
        }

        let (Ok(length), Ok(descriptor_count)) = (
            Bits::new(parameters.length as u32),
            Bits::new(parameters.descriptor_count as u32),
        ) else {
            return None;
        };
        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
            fields,
            length,
            descriptor_count,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeMcs {
    Mcs0,
    Mcs1,
    Mcs2,
    Mcs3,
    Mcs4,
    Mcs5,
    Mcs6,
    Mcs7,
    Mcs8,
    Mcs9,
}

impl MacHeMcs {
    const fn signal_mcs(self) -> oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs {
        match self {
            Self::Mcs0 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs0,
            Self::Mcs1 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs1,
            Self::Mcs2 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs2,
            Self::Mcs3 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs3,
            Self::Mcs4 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs4,
            Self::Mcs5 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs5,
            Self::Mcs6 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs6,
            Self::Mcs7 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs7,
            Self::Mcs8 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs8,
            Self::Mcs9 => oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeFecCoding {
    Bcc,
    Ldpc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeRate {
    pub mcs: MacHeMcs,
    pub guard_interval_and_ltf: crate::types::MacHeGiLtf,
    pub fec_coding: MacHeFecCoding,
    pub dcm: bool,
}

impl MacHeRate {
    const fn plcp_rate(self) -> PlcpRate {
        match self.mcs {
            MacHeMcs::Mcs0 => crate::types::MacPlcpRate::HeMcs0,
            MacHeMcs::Mcs1 => crate::types::MacPlcpRate::HeMcs1,
            MacHeMcs::Mcs2 => crate::types::MacPlcpRate::HeMcs2,
            MacHeMcs::Mcs3 => crate::types::MacPlcpRate::HeMcs3,
            MacHeMcs::Mcs4 => crate::types::MacPlcpRate::HeMcs4,
            MacHeMcs::Mcs5 => crate::types::MacPlcpRate::HeMcs5,
            MacHeMcs::Mcs6 => crate::types::MacPlcpRate::HeMcs6,
            MacHeMcs::Mcs7 => crate::types::MacPlcpRate::HeMcs7,
            MacHeMcs::Mcs8 => crate::types::MacPlcpRate::HeMcs8,
            MacHeMcs::Mcs9 => crate::types::MacPlcpRate::HeMcs9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeTxFormat {
    Smpdu,
    Ampdu,
}

/// Semantic inputs for one bounded HE SU queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxParameters {
    pub control: MacTxControlFrame,
    pub rate: MacHeRate,
    pub format: MacHeTxFormat,
    pub apep_length: u16,
    pub descriptor_count: u8,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    pub software_he_control: Option<u32>,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub protection_spacing: u16,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
}

/// HAL-owned transaction for one bounded HE SU PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxProgram {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    parameters: MacHeTxParameters,
    fields: QueueFields,
    descriptor_count:
        Bits<{ oer_chips_esp32s31_pac::wifi_mac::ht_descriptor_counts::DESCRIPTOR_COUNT_A_WIDTH }>,
    bss_color: Bits<{ oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::BSS_COLOR_WIDTH }>,
    spatial_reuse: Bits<{ oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::SPATIAL_REUSE_WIDTH }>,
    protection_spacing:
        Bits<{ oer_chips_esp32s31_pac::wifi_mac::protection::MINIMUM_MPDU_LENGTH_CBW20_WIDTH }>,
}

impl MacHeTxProgram {
    /// Control frame retained with this descriptor-bound program.
    pub const fn control(self) -> MacTxControlFrame {
        self.parameters.control
    }

    pub fn new(
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacHeTxParameters,
    ) -> Option<Self> {
        let Some(fields) = QueueFields::new(
            parameters.timeout,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
            parameters.aifsn,
            parameters.contention_window,
            parameters.hardware_key_selector,
        ) else {
            return None;
        };
        if parameters.apep_length == 0
            || parameters.descriptor_count == 0
            || parameters.descriptor_count > 32
            || (matches!(parameters.format, MacHeTxFormat::Smpdu)
                && parameters.descriptor_count != 1)
        {
            return None;
        }
        let (Ok(descriptor_count), Ok(bss_color), Ok(spatial_reuse), Ok(protection_spacing)) = (
            Bits::new(parameters.descriptor_count as u32),
            Bits::new(parameters.bss_color as u32),
            Bits::new(parameters.spatial_reuse as u32),
            Bits::new(parameters.protection_spacing as u32),
        ) else {
            return None;
        };
        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
            fields,
            descriptor_count,
            bss_color,
            spatial_reuse,
            protection_spacing,
        })
    }
}

/// Whether a transmission was ordinary or part of a hardware Trigger-based
/// flow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxFlow {
    Ordinary,
    TriggerBased,
}

impl MacTxFlow {
    /// The flow an observed bit names.
    const fn observed(trigger_based: bool) -> Self {
        if trigger_based {
            Self::TriggerBased
        } else {
            Self::Ordinary
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxCompletionObservation {
    status: u8,
    detail: u8,
    /// This queue completed as part of a hardware Trigger-based transmit flow.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_get_txq_in_trig_flow_state` and
    /// `libpp[lmac.o]::lmacProcessTxComplete`. The HAL returns
    /// `QUEUE_STATE[31:24]` as one bitmap; the completion dispatcher selects
    /// the completed queue's bit.
    flow: MacTxFlow,
    trigger_based_packet_count: u8,
    last_transmission: MacTxFlow,
    secondary_trigger_based_packet_count: u8,
    ack_snr_encoded: u8,
}

impl MacTxCompletionObservation {
    pub const fn status(self) -> u8 {
        self.status
    }

    pub const fn detail(self) -> u8 {
        self.detail
    }

    /// Whether the queue completed within a hardware Trigger-based flow.
    pub const fn flow(self) -> MacTxFlow {
        self.flow
    }

    pub const fn trigger_based_packet_count(self) -> u8 {
        self.trigger_based_packet_count
    }

    /// Whether the last transmission was trigger-based: its result is then
    /// the alternate result word's.
    pub const fn last_transmission(self) -> MacTxFlow {
        self.last_transmission
    }

    pub const fn secondary_trigger_based_packet_count(self) -> u8 {
        self.secondary_trigger_based_packet_count
    }

    pub const fn ack_snr_encoded(self) -> u8 {
        self.ack_snr_encoded
    }

    /// Construct a semantic completion supplied by a native hardware model.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new_model(status: u8, detail: u8) -> Self {
        Self {
            status,
            detail,
            flow: MacTxFlow::Ordinary,
            trigger_based_packet_count: 0,
            last_transmission: MacTxFlow::Ordinary,
            secondary_trigger_based_packet_count: 0,
            ack_snr_encoded: 0,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_flow_model(mut self, flow: MacTxFlow) -> Self {
        self.flow = flow;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_trigger_packet_counts_model(
        mut self,
        primary: u8,
        last_transmission: MacTxFlow,
        secondary: u8,
    ) -> Self {
        self.trigger_based_packet_count = primary;
        self.last_transmission = last_transmission;
        self.secondary_trigger_based_packet_count = secondary;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_ack_snr_encoded_model(mut self, encoded: u8) -> Self {
        self.ack_snr_encoded = encoded;
        self
    }
}

/// TX completion and BlockAck sampled before acknowledging the completion
/// edge for one A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtAmpduCompletionObservation {
    tx: MacTxCompletionObservation,
    block_ack_control: u8,
    block_ack_starting_sequence: u16,
    block_ack_bitmap: u64,
    /// Hardware says the completed PPDU received a BlockAck response.
    ///
    /// This is independent of the ordinary TX status. The bitmap registers
    /// are not cleared at every completion and therefore must not be trusted
    /// when this result bit is clear.
    block_ack_received: bool,
}

impl MacHtAmpduCompletionObservation {
    pub const fn tx(self) -> MacTxCompletionObservation {
        self.tx
    }

    pub const fn block_ack_control(self) -> u8 {
        self.block_ack_control
    }

    pub const fn block_ack_starting_sequence(self) -> u16 {
        self.block_ack_starting_sequence
    }

    pub const fn block_ack_bitmap(self) -> u64 {
        self.block_ack_bitmap
    }

    pub const fn block_ack_received(self) -> bool {
        self.block_ack_received
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn new_model(
        tx: MacTxCompletionObservation,
        block_ack_control: u8,
        block_ack_starting_sequence: u16,
        block_ack_bitmap: u64,
        block_ack_received: bool,
    ) -> Self {
        Self {
            tx,
            block_ack_control,
            block_ack_starting_sequence,
            block_ack_bitmap,
            block_ack_received,
        }
    }
}

/// Hardware edge which must precede reuse of one TX descriptor chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxDetachReason {
    Collision,
    Completed,
}

/// The fixed settle of a queue's abort, between forcing CCA and the
/// queue's detach.
///
/// SOURCE: complete `libpp[lmac.o]::lmacStopTransmit` (`PROMOTED_LMAC_TX`)
/// forces CCA to three and delays 16 us before it samples VALID.
pub const MAC_TX_ABORT_SETTLE: oer_portable_time::settle::SettleLimit =
    match oer_portable_time::settle::SettleLimit::new(oer_portable_time::Duration::from_micros(16))
    {
        Some(limit) => limit,
        None => panic!("the abort's settle is within a settle's limit"),
    };

/// How one queue's abort ([`WifiRadioRegisters::abort_mac_tx`]) ended.
#[derive(Debug, Eq, PartialEq)]
pub enum MacTxAbortOutcome<T> {
    /// A [`MacTxAbortCause::TimeoutEdge`] abort found no edge: nothing was
    /// written.
    NoEdge,
    /// VALID was already clear: the MAC took the PPDU before CCA was forced.
    /// The queue keeps its descriptors; only its own completion ends them.
    OnAir,
    /// The queue was valid and is disabled; the callback consumed the proof.
    Aborted(T),
}

/// A queue's proof that it let its chain go, which a storage's release
/// takes: a detach names the descriptor the queue held, which the storage
/// confirms is its own; an abort ([`MacTxQueueAborted`]) disabled the queue
/// that held the storage's published chain and reads no address back.
pub trait MacTxQueueReleased: sealed::Released {
    /// The release of the chain whose head is `descriptor_head`, or the
    /// detached queue's other chain.
    fn releases(&self, descriptor_head: u32) -> Result<(), ForeignChain>;
}

/// A detached queue held a chain whose head is not the storage's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignChain;

mod sealed {
    pub trait Released {}
}

impl sealed::Released for MacTxQueueDetached<'_> {}
impl MacTxQueueReleased for MacTxQueueDetached<'_> {
    fn releases(&self, descriptor_head: u32) -> Result<(), ForeignChain> {
        if self.confirms_descriptor_head(descriptor_head) {
            Ok(())
        } else {
            Err(ForeignChain)
        }
    }
}

impl sealed::Released for MacTxQueueAborted<'_> {}
impl MacTxQueueReleased for MacTxQueueAborted<'_> {
    /// The abort holds the queue the storage published to: no address is
    /// read back, and nothing is left to confirm.
    fn releases(&self, _descriptor_head: u32) -> Result<(), ForeignChain> {
        Ok(())
    }
}

/// Proof that an abort disabled one queue while it was still valid, so the
/// queue no longer owns its TX chain.
///
/// It borrows the register owner, as [`MacTxQueueDetached`] does, and is
/// made only by [`WifiRadioRegisters::abort_mac_tx`]. It carries no
/// descriptor address: the abort reads none back to decide.
pub struct MacTxQueueAborted<'registers> {
    _registers: PhantomData<&'registers mut ()>,
}

impl MacTxQueueAborted<'_> {
    const fn new() -> Self {
        Self {
            _registers: PhantomData,
        }
    }

    /// Construct an abort's proof in a native model with no asynchronous DMA.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new_model() -> Self {
        Self::new()
    }
}

/// What begins the abort of a published queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxAbortCause {
    /// The queue's own timeout edge; without it the abort does not begin.
    TimeoutEdge,
    /// The caller gave up on the queue: the air can hold it past its
    /// programmed timeout without that edge, so the abort needs none.
    Forced,
}

/// Result of one finite queue-detach transaction.
#[derive(Debug, Eq, PartialEq)]
pub enum MacTxDetachOutcome<T> {
    /// The requested collision edge was not pending.
    NoEvent,
    /// The abort found VALID already clear and no proof the queue let go:
    /// it keeps its descriptors until its completion ends them.
    OnAir,
    /// The edge existed, but queue disable/invalid readback did not converge.
    Failed,
    /// Queue ownership was returned and the callback consumed its proof.
    Detached(T),
}

/// Non-forgeable proof that one hardware queue no longer owns its TX chain.
///
/// The value borrows the register owner, so it cannot be retained while that
/// same owner starts another queue transaction. Its fields and target
/// constructor remain private to this crate; safe production code can obtain
/// it only from [`WifiRadioRegisters::with_detached_mac_tx`]. Validation builds
/// additionally expose an isolated model constructor for compiled probes.
pub struct MacTxQueueDetached<'registers> {
    descriptor_address_low: u32,
    _registers: PhantomData<&'registers mut ()>,
}

impl MacTxQueueDetached<'_> {
    fn from_descriptor_address_low(descriptor_address_low: u32) -> Self {
        Self {
            descriptor_address_low,
            _registers: PhantomData,
        }
    }

    /// Check that the detached queue referenced this descriptor chain.
    pub const fn confirms_descriptor_head(&self, descriptor_head: u32) -> bool {
        self.descriptor_address_low == descriptor_head & DESCRIPTOR_ADDRESS_LOW_MASK
    }

    /// Construct a detach edge in a native model with no asynchronous DMA.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new_model(descriptor_head: u32) -> Self {
        Self {
            descriptor_address_low: descriptor_head & DESCRIPTOR_ADDRESS_LOW_MASK,
            _registers: PhantomData,
        }
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Sample one ordinary queue without acknowledging or changing it.
    pub fn ordinary_tx_queue_snapshot(&self, queue: MacTxQueueIndex) -> MacOrdinaryTxQueueSnapshot {
        let bank = queue::bank(queue);
        let control_bank = &self.mac.wifi_mac_tx_queue_control;
        let control = control_bank.read_control_n(bank);
        let config = control_bank.read_config(bank);
        let vectors = &self.mac.wifi_mac_tx_queue_vector;
        let plcp1 = vectors.read_plcp1(bank);
        let power = vectors.read_power(bank);
        let common = &self.mac.wifi_mac_tx_common;
        let cca = common.read_cca_control();
        let rx = &self.mac.wifi_mac_rx_statistics;
        MacOrdinaryTxQueueSnapshot {
            descriptor_address_low: control.descriptor_address_low().get(),
            plcp_format: byte(control.plcp_format()),
            legacy_signal: halfword(plcp1.legacy_signal()),
            rate: byte(plcp1.rate()),
            key_entry_index: byte(plcp1.key_entry_index()),
            bssid_select: byte(plcp1.bssid_select()),
            vector_format: byte(plcp1.vector_format()),
            interface: MacInterface::from(config.interface()),
            aifsn: byte(config.aifsn()),
            contention_window: halfword(config.contention_window()),
            scheduler_priority: byte(config.scheduler_priority()),
            power_codes: [
                power.data_primary() as i8,
                power.data_alternate() as i8,
                power.rts_primary() as i8,
                power.rts_alternate() as i8,
            ],
            cca_force: byte(cca.force()),
            cca_aux_force: byte(cca.phy_aux_force()),
            mac_active_state: self.mac_channel_active_state(),
            hang: self.rx_hang_statistics_snapshot(),
            rx_mpdu_count: rx.read_mpdu_and_cfo().mpdu_count(),
            rx_signal_count: rx.read_signal_field().count(),
            rx_end_count: rx.read_end().count(),
            rx_fcs_error_count: rx.read_fcs_error().count(),
            rx_abort_count: rx.read_abort().count(),
            ownership: MacTxQueueOwnership::observed(control.enable(), control.valid()),
            pending: MacTxQueuePending::observed(
                queue::completion_pending(common, queue),
                queue::timeout_pending(common, queue),
                queue::collision_pending(common, queue),
            ),
        }
    }

    /// Execute complete `hal_set_tx_pti` over one bounded logical queue.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        let bank = queue::bank(queue);
        self.mac.wifi_mac_tx_queue_control.modify_config(bank, |w| {
            w.set_scheduler_priority(program.scheduler_priority.field());
        });
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_pti(bank, |w| {
            w.set_pti_2(program.pti_2.field());
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_1(program.pti_1.field());
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_0(program.pti_0.field());
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_3(program.pti_3.field());
        });
        vectors.modify_pti(bank, |w| {
            w.set_count(program.count.field());
        });
    }

    /// Prepare one legacy queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_legacy_mac_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        program: MacLegacyTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        if dma.descriptor_head() != program.descriptor_head {
            return Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ForeignProgram,
            ));
        }
        self.prepare_legacy_mac_tx(queue, program)?;
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        Ok(())
    }

    /// Prepare one HT queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_ht_mac_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        program: MacHtTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        if dma.descriptor_head() != program.descriptor_head {
            return Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ForeignProgram,
            ));
        }
        self.prepare_ht_mac_tx(queue, program)?;
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        Ok(())
    }

    /// Prepare one HE queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_he_mac_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        program: MacHeTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        if dma.descriptor_head() != program.descriptor_head {
            return Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ForeignProgram,
            ));
        }
        self.prepare_he_mac_tx(queue, program)?;
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        Ok(())
    }

    /// Hand `publication`'s chain to the hardware and start `queue`.
    ///
    /// The chain stays with its owner until the queue is seen to name it, in
    /// this one exclusive call: a queue prepared for another chain since
    /// refuses, and nothing is started. `handed_over` runs after that check
    /// and before the hardware can complete the queue, for the owner to
    /// publish its software state.
    pub fn start_bound_mac_tx<D: CommitTxDma>(
        &mut self,
        publication: D,
        queue: MacTxQueueIndex,
        handed_over: impl FnOnce(),
    ) -> Result<(), MacTxPrepareRefused> {
        self.prepared_chain(&publication, queue)?;
        handed_over();
        publication.commit(|started| self.start_prepared_mac_tx(started, queue));
        Ok(())
    }

    /// Whether the queue names `dma`'s chain.
    fn prepared_chain(
        &self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
    ) -> Result<(), MacTxPrepareRefused> {
        let named = self
            .mac
            .wifi_mac_tx_queue_control
            .read_control_n(queue::bank(queue))
            .descriptor_address_low()
            .get();
        if tx_descriptor_head_matches(dma.descriptor_head(), named) {
            Ok(())
        } else {
            Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ChainNotNamed { named },
            ))
        }
    }

    /// The idle check and `hal_mac_tx_config_timeout` of one ordinary queue,
    /// before any vector write.
    ///
    /// SOURCE: complete hal_mac_tx_config_timeout. This precedes every
    /// vector write in the recovered libpp[lmac.o]::lmacSetTxFrame parent.
    fn begin_mac_tx(
        &mut self,
        queue: MacTxQueueIndex,
        timeout: Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::TIMEOUT_WIDTH }>,
    ) -> Result<(), MacTxPrepareRefused> {
        let bank = queue::bank(queue);
        let control_bank = &mut self.mac.wifi_mac_tx_queue_control;
        let control_state = control_bank.read_control_n(bank);
        if control_state.enable() || control_state.valid() {
            return Err(MacTxPrepareRefused::QueueActive);
        }
        control_bank.modify_config(bank, |w| {
            w.set_timeout(timeout);
        });
        Ok(())
    }

    /// Program one ordinary queue up to, but excluding, its ENABLE|VALID edge.
    ///
    /// Keeping the final edge separate lets the MAC publish its software
    /// ownership state before hardware can complete the queue.
    fn prepare_legacy_mac_tx(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacLegacyTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let parameters = program.parameters;

        self.begin_mac_tx(queue, program.fields.timeout)?;

        self.program_legacy_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.fields.aifsn,
            program.fields.contention_window,
            parameters.interface,
        );
        Ok(())
    }

    /// The prepared CONTROL%s image of `mac_tx_set_plcp0`, stored from zero:
    /// the descriptor head's low address bits, the vendor's constant two
    /// and set bit 22, and the PLCP format; ENABLE, VALID and the other
    /// opaque lanes zero. The software RTS request is then cleared through
    /// one fresh-read protection update.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_set_plcp0`.
    fn publish_prepared_control(
        &mut self,
        queue: MacTxQueueIndex,
        descriptor_head: DmaRef<WifiTxDmaDescriptor>,
        plcp_format: PlcpFormat,
    ) {
        self.mac
            .wifi_mac_tx_queue_control
            .modify_control_n(queue::bank(queue), |w| {
                w.set_descriptor_address_low(descriptor_head)
                    .set_bits_20_21_opaque(Bits::constant::<2>())
                    .set_bit_22_opaque(true)
                    .set_bit_23_opaque(false)
                    .set_plcp_format(plcp_format.into())
                    .set_bits_27_29_opaque(Bits::constant::<0>())
                    .set_valid(false)
                    .set_enable(false);
            });
        queue::clear_software_rts(&mut self.mac.wifi_mac_tx_queue_control, queue);
    }

    /// The PLCP1%s image, stored from zero (`publish_mac_tx_plcp1_fields`).
    fn publish_plcp1(&mut self, queue: MacTxQueueIndex, plcp1: Plcp1) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_plcp1(queue::bank(queue), |w| {
                w.set_legacy_signal(plcp1.legacy_signal)
                    .set_rate(plcp1.rate.into())
                    .set_key_entry_index(plcp1.key_entry_index)
                    .set_bssid_select(super::values::interface_bssid_select(plcp1.interface))
                    .set_vector_format(plcp1.vector_format.into())
                    .set_high_control_28(false)
                    .set_cbw40(plcp1.cbw40)
                    .set_high_control_31_30(Bits::constant::<0>());
            });
    }

    /// Clear trigger-based transmission on the queue's HE queue control.
    fn clear_trigger_based_enable(&mut self, queue: MacTxQueueIndex) {
        self.mac.wifi_mac_he_init_suffix.modify_queue_control(
            queue::he_queue_control(queue),
            |w| {
                w.set_trigger_based_enable(false);
            },
        );
    }

    /// The parent's independent PTI-low edge.
    fn set_txop(&mut self, queue: MacTxQueueIndex, txop: crate::types::MacTxop) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_pti(queue::bank(queue), |w| {
                w.set_txop(txop);
            });
    }

    /// LENGTH_CONTROL%s stored from zero
    /// (`publish_mac_tx_length_control_fields`): the TX/RX queue antenna
    /// enable, the control-frame rate and the HT20 EOF count.
    fn publish_length_control(
        &mut self,
        queue: MacTxQueueIndex,
        rts_rate: RtsRate,
        ht20_eof_count: Bits<
            { oer_chips_esp32s31_pac::wifi_mac::length_control::HT20_EOF_COUNT_WIDTH },
        >,
    ) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_length_control(queue::bank(queue), |w| {
                w.set_txrx_queue_antenna_last(false)
                    .set_txrx_queue_antenna_value(false)
                    .set_txrx_queue_antenna_enable(true)
                    .set_tx_queue_antenna_last(false)
                    .set_tx_queue_antenna_value(false)
                    .set_tx_queue_antenna_enable(false)
                    .set_rts_rate(rts_rate.into())
                    .set_bits_14_21_opaque(0)
                    .set_ht20_eof_count(ht20_eof_count)
                    .set_ht20_txop_count(Bits::constant::<0>())
                    .set_txop_list_select(TxopListSelect::List0)
                    .set_bits_30_31_opaque(Bits::constant::<0>());
            });
    }

    /// POWER%s stored from zero (`publish_mac_tx_power_fields`).
    fn publish_power(
        &mut self,
        queue: MacTxQueueIndex,
        data_primary: u8,
        data_alternate: u8,
        control: MacTxControlFrame,
    ) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_power(queue::bank(queue), |w| {
                w.set_data_primary(data_primary)
                    .set_data_alternate(data_alternate)
                    .set_rts_primary(control.power_primary)
                    .set_rts_alternate(control.power_alternate);
            });
    }

    /// SOURCE: complete
    /// `libpp[hal_mac.o]::mac_tx_set_pti` and
    /// `libpp[hal_coex.o]::hal_set_tx_pti`. The scheduler
    /// priority in CONFIG is the unsigned minimum of the packet PTI and
    /// coexistence event-one PTI. The four packet lanes retain the original
    /// packet PTI, so these values must not be collapsed into one field.
    /// Each field is intentionally a separate fresh-read RMW and must not
    /// be coalesced.
    fn publish_packet_pti(&mut self, queue: MacTxQueueIndex, fields: QueueFields) {
        let bank = queue::bank(queue);
        self.mac.wifi_mac_tx_queue_control.modify_config(bank, |w| {
            w.set_scheduler_priority(fields.scheduler_priority);
        });
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        let packet = fields.packet_priority;
        vectors.modify_pti(bank, |w| {
            w.set_pti_2(packet);
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_1(packet);
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_0(packet);
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_3(packet);
        });
        vectors.modify_pti(bank, |w| {
            w.set_count(fields.priority_count);
        });
    }

    /// Publish the complete legacy responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction, as for
    /// [`Self::program_ht_mac_tx_ppdu`].
    pub(crate) fn program_legacy_mac_tx_ppdu(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacLegacyTxProgram,
    ) {
        let parameters = program.parameters;
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            parameters.response.plcp_format(),
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: program.signal,
                rate: parameters.rate.plcp_rate(),
                key_entry_index: program.fields.key_entry_index,
                interface: parameters.interface,
                vector_format: oer_chips_esp32s31_pac::wifi_mac::plcp1::VectorFormat::Legacy,
                cbw40: false,
            },
        );
        self.clear_trigger_based_enable(queue);
        // The parent's independent PTI-low edge; a legacy PPDU holds no TXOP.
        self.set_txop(queue, crate::types::MacTxop::None);
        self.publish_length_control(
            queue,
            parameters.control.rate.rts_rate(),
            Bits::constant::<1>(),
        );
        // `mac_tx_set_txop_q` of a queue without a granted TXOP slot: the
        // slot `lmacInitAc` leaves (and `lmacReleaseTxopQueue` restores)
        // clears the TXOP count through one fresh-read update.
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_length_control(queue::bank(queue), |w| {
                w.set_ht20_txop_count(Bits::constant::<0>());
            });
        self.publish_power(queue, parameters.data_power, 0, parameters.control);
        self.publish_packet_pti(queue, program.fields);
    }

    /// Program one non-aggregate HT queue up to its final ENABLE|VALID edge.
    ///
    /// This is deliberately separate from the legacy routine: an HT PPDU has
    /// two additional vector words and three descriptor-count RMW edges which
    /// must not be silently omitted by a shared "mostly legacy" formatter.
    pub(crate) fn prepare_ht_mac_tx(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacHtTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let parameters = program.parameters;

        // SOURCE: complete hal_mac_tx_config_timeout, followed by the
        // libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu non-HE HT branch.
        self.begin_mac_tx(queue, program.fields.timeout)?;

        self.program_ht_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.fields.aifsn,
            program.fields.contention_window,
            parameters.interface,
        );
        Ok(())
    }

    /// Publish the complete non-HE HT responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction. Keeping this
    /// slice explicit lets the compiled production implementation be compared
    /// to the same vendor responsibility without a shadow register model.
    pub(crate) fn program_ht_mac_tx_ppdu(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacHtTxProgram,
    ) {
        let parameters = program.parameters;

        let bank = queue::bank(queue);
        let cbw40 = matches!(parameters.rate.channel_width, MacHtChannelWidth::Mhz40);
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            match parameters.format {
                MacHtTxFormat::SingleMpdu => {
                    oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HtSingleMpdu
                }
                MacHtTxFormat::Ampdu => {
                    oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HtAmpdu
                }
            },
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: Bits::constant::<0>(),
                rate: parameters.rate.plcp_rate(),
                key_entry_index: program.fields.key_entry_index,
                interface: parameters.interface,
                vector_format: oer_chips_esp32s31_pac::wifi_mac::plcp1::VectorFormat::Ht,
                cbw40,
            },
        );
        self.clear_trigger_based_enable(queue);

        // The parent owns this independent PTI-low edge. `mac_tx_set_pti`
        // follows later and intentionally preserves it while updating the
        // four PTI lanes and the count.
        self.set_txop(queue, parameters.txop);

        // The bounded HT branch enters `mac_tx_set_htsig` here. It publishes
        // HT-SIG (stored from zero, `publish_mac_tx_ht_signal_fields`), the
        // three descriptor-count edges, the three negotiated minimum-MPDU
        // spacing edges and the two length words.
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_ht_signal(bank, |w| {
            w.set_mcs(parameters.rate.mcs.signal_mcs().into())
                .set_cbw40(cbw40)
                .set_length(parameters.length)
                .set_smoothing(true)
                .set_not_sounding(true)
                .set_required_one(true)
                .set_aggregation(matches!(parameters.format, MacHtTxFormat::Ampdu))
                .set_stbc(oer_chips_esp32s31_pac::wifi_mac::ht_signal::Stbc::None.into())
                .set_fec_coding(false)
                .set_short_gi(matches!(
                    parameters.rate.guard_interval,
                    MacHtGuardInterval::Short
                ));
        });
        let count = program.descriptor_count;
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a(count);
        });
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_b(count);
        });
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a_copy(count);
        });

        let spacing = parameters.protection_spacing.register_value();
        let control_bank = &mut self.mac.wifi_mac_tx_queue_control;
        control_bank.modify_protection(bank, |w| {
            w.set_minimum_mpdu_length_cbw20(spacing);
        });
        control_bank.modify_protection(bank, |w| {
            w.set_minimum_mpdu_length_cbw40(spacing);
        });
        control_bank.modify_protection(bank, |w| {
            w.set_minimum_mpdu_length_cbw80(spacing);
        });

        let entry_class = match parameters.format {
            MacHtTxFormat::SingleMpdu => Bits::constant::<0>(),
            MacHtTxFormat::Ampdu => Bits::constant::<1>(),
        };
        self.publish_length_control(queue, parameters.control.rate.rts_rate(), entry_class);
        // DATA_LENGTH%s stored from zero (`publish_mac_tx_data_length_fields`).
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_data_length(bank, |w| {
                w.set_ht40_length(program.length)
                    .set_ht40_eof_count(entry_class)
                    .set_ht40_txop_count(Bits::constant::<0>())
                    .set_ht40_mcs(parameters.rate.mcs.ht40_mcs().into());
            });
        self.publish_power(
            queue,
            parameters.data_power_primary,
            parameters.data_power_alternate,
            parameters.control,
        );
        self.publish_packet_pti(queue, program.fields);
    }

    /// Program one HE SU A-MPDU up to its final ENABLE|VALID edge.
    ///
    /// This is separate from HT because HE publishes two different vector
    /// words and deliberately does not write the non-HE DATA_LENGTH word.
    fn prepare_he_mac_tx(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacHeTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let parameters = program.parameters;

        self.begin_mac_tx(queue, program.fields.timeout)?;
        self.program_he_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.fields.aifsn,
            program.fields.contention_window,
            parameters.interface,
        );
        Ok(())
    }

    /// Publish the complete HE SU responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction, as for
    /// [`Self::program_ht_mac_tx_ppdu`].
    pub(crate) fn program_he_mac_tx_ppdu(
        &mut self,
        queue: MacTxQueueIndex,
        program: MacHeTxProgram,
    ) {
        let parameters = program.parameters;
        let bank = queue::bank(queue);
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            match parameters.format {
                MacHeTxFormat::Smpdu => {
                    oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HeSmpdu
                }
                MacHeTxFormat::Ampdu => {
                    oer_chips_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HeAmpdu
                }
            },
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: Bits::constant::<0>(),
                rate: parameters.rate.plcp_rate(),
                key_entry_index: program.fields.key_entry_index,
                interface: parameters.interface,
                vector_format: oer_chips_esp32s31_pac::wifi_mac::plcp1::VectorFormat::He,
                cbw40: false,
            },
        );
        self.clear_trigger_based_enable(queue);
        // The parent's independent PTI-low edge; this SU profile holds no
        // TXOP.
        self.set_txop(queue, crate::types::MacTxop::None);

        // SOURCE: complete libpp[hal_mac_tx.o]::mac_tx_set_hesig stores A1
        // then A2/length (each from zero, `publish_mac_tx_he_signal_a1_fields`
        // and `publish_mac_tx_he_signal_a2_fields`) before publishing the
        // descriptor counts.
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_he_su_signal_a1(bank, |w| {
            w.set_format(true)
                .set_beam_change(true)
                .set_uplink(true)
                .set_mcs(parameters.rate.mcs.signal_mcs().into())
                .set_dcm(parameters.rate.dcm)
                .set_bss_color(program.bss_color)
                .set_vector_required_14(true)
                .set_spatial_reuse(program.spatial_reuse)
                .set_bandwidth(oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::Bandwidth::Mhz20.into())
                .set_gi_ltf(parameters.rate.guard_interval_and_ltf)
                .set_nsts_and_midamble_periodicity(oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::NstsAndMidamblePeriodicity::OneStreamNoMidamble.into())
                .set_vector_required_high(Bits::constant::<0x3f>());
        });
        vectors.modify_he_su_signal_a2_length(bank, |w| {
            w.set_control_required_0(true)
                .set_fec_coding(matches!(parameters.rate.fec_coding, MacHeFecCoding::Ldpc))
                .set_control_required_2(true)
                // SOURCE: BLOB_LIBPP_HE_SU_A2_METADATA_COPY_20261108. The
                // vendor formatter copies TX metadata head bits 26:25 and 28
                // here and keeps bit 8 set unless head bits 27:25 are 0b100;
                // every ordinary vendor TX path clears those head bits, so an
                // ordinary frame carries zero and bit 8. Their meaning is not
                // established.
                .set_bits_5_7_opaque(Bits::constant::<0>())
                .set_control_required_8(true)
                .set_descriptor_class_match(false)
                .set_apep_length(parameters.apep_length)
                .set_ampdu_length_valid(matches!(parameters.format, MacHeTxFormat::Ampdu))
                .set_bit_27_opaque(false);
        });
        // Unlike HT's three fresh-read edges, the HE leaf writes all three
        // descriptor counts as one register image, from zero
        // (`publish_mac_tx_he_descriptor_counts`).
        let count = program.descriptor_count;
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a(count)
                .set_descriptor_count_b(count)
                .set_descriptor_count_a_copy(count)
                .set_bit_21_opaque(false);
        });
        // SOURCE: complete libpp[hal_mac_tx.o]::mac_tx_set_hesig then replaces the three finite
        // channel-width minimum-MPDU lanes and clears software CTS in one
        // fresh-read update that keeps only software RTS.
        let spacing = program.protection_spacing;
        self.mac
            .wifi_mac_tx_queue_control
            .modify_protection(bank, |w| {
                w.set_minimum_mpdu_length_cbw20(spacing)
                    .set_minimum_mpdu_length_cbw40(spacing)
                    .set_minimum_mpdu_length_cbw80(spacing)
                    .set_software_cts(
                        oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareCts::None,
                    );
            });

        // HE reaches mac_tx_set_len for LENGTH_CONTROL, but its flag-bit-31
        // branch intentionally skips the non-HE DATA_LENGTH register.
        self.publish_length_control(
            queue,
            parameters.control.rate.rts_rate(),
            Bits::constant::<1>(),
        );
        // SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` calls complete
        // `libpp[hal_mac_ctl.o]::hal_he_set_htc` immediately after
        // `libpp[hal_mac_tx.o]::mac_tx_set_hesig`. That leaf always publishes the complete word,
        // then sets or clears bit 28 through a fresh-read RMW. A missing
        // software image is not "no HE-Control": it deliberately leaves the
        // hardware-generated BSR path selected.
        let (he_control, software_he_control_enabled) = parameters
            .software_he_control
            .map_or((0, false), |image| (image, true));
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_he_control(bank, |w| {
            w.set_value(he_control);
        });
        vectors.modify_he_control_config(bank, |w| {
            w.set_software_he_control_enable(software_he_control_enabled);
        });
        // The complete parent selects and publishes the data/RTS power pair
        // only after the HE-Control leaf returns.
        self.publish_power(
            queue,
            parameters.data_power_primary,
            parameters.data_power_alternate,
            parameters.control,
        );
        self.publish_packet_pti(queue, program.fields);
    }

    pub(crate) fn start_prepared_mac_tx(
        &mut self,
        _hardware_owned: &impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
    ) {
        oer_portable_riscv::fence();
        // SOURCE: publication prefix of `libpp[hal_mac_tx.o]::hal_mac_txq_enable` offsets 0x00..0x1e, before libpp[lmac.o]::GetAccess. It reads the
        // already prepared CONTROL word and ORs only ENABLE|VALID before
        // writing it back. In particular, it does not reconstruct PLCP0 from
        // the caller's earlier software image: formatter leaves may have
        // changed control fields after the initial PLCP0 publication.
        // The remaining vendor body updates access/HE bookkeeping and
        // statistics; equivalence of this prefix does not cover that body.
        queue::publish_queue(&mut self.mac.wifi_mac_tx_queue_control, queue);
        oer_portable_riscv::fence();
    }

    pub fn take_mac_tx_completion(
        &mut self,
        queue: MacTxQueueIndex,
    ) -> Option<MacTxCompletionObservation> {
        if !queue::completion_pending(&self.mac.wifi_mac_tx_common, queue) {
            return None;
        }

        let bank = queue::bank(queue);
        let tx_results = &self.mac.wifi_mac_block_ack;
        let (trigger_based_packet_count, last_tx_was_trigger_based) = match queue {
            MacTxQueueIndex::Zero => {
                let information = tx_results.read_tx_queue_information_q0();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            MacTxQueueIndex::One => {
                let information = tx_results.read_tx_queue_information_q1();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            MacTxQueueIndex::Two => {
                let information = tx_results.read_tx_queue_information_q2();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            MacTxQueueIndex::Three => {
                let information = tx_results.read_tx_queue_information_q3();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
        };
        let completion = &self.mac.wifi_mac_tx_completion;
        let secondary_trigger_based_packet_count = byte(
            completion
                .read_aux_c(bank)
                .secondary_trigger_based_packet_count(),
        );
        let primary = completion.read_primary(bank);
        let alternate = completion.read_alternate(bank);
        let last_transmission = MacTxFlow::observed(last_tx_was_trigger_based);
        let (status, detail) = if matches!(last_transmission, MacTxFlow::TriggerBased) {
            (byte(alternate.status()), alternate.detail())
        } else {
            (byte(primary.status()), primary.detail())
        };
        let common = &mut self.mac.wifi_mac_tx_common;
        let flow = MacTxFlow::observed(queue::queue_in_trigger_flow(common, queue));
        queue::acknowledge_completion(common, queue);
        oer_portable_riscv::fence();
        Some(MacTxCompletionObservation {
            status,
            detail,
            flow,
            trigger_based_packet_count,
            last_transmission,
            secondary_trigger_based_packet_count,
            ack_snr_encoded: primary.ack_snr_encoded(),
        })
    }

    /// Sample the BlockAck solicited by an A-MPDU or a BlockAckReq before
    /// acknowledging its TX-complete edge.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_tx_get_blockack` and the recovered event-23 completion order.
    /// The three BlockAck words belong to the completed queue and must be read
    /// while that completion edge still owns the result registers.
    pub fn take_mac_block_ack_completion(
        &mut self,
        queue: MacTxQueueIndex,
    ) -> Option<MacHtAmpduCompletionObservation> {
        if !queue::completion_pending(&self.mac.wifi_mac_tx_common, queue) {
            return None;
        }
        let block_ack = self.read_tx_block_ack_observation(queue.get() as u8)?;
        let tx = self.take_mac_tx_completion(queue)?;
        Some(MacHtAmpduCompletionObservation {
            tx,
            block_ack_control: block_ack.control,
            block_ack_starting_sequence: block_ack.starting_sequence,
            block_ack_bitmap: block_ack.bitmap,
            block_ack_received: block_ack.block_ack_received,
        })
    }

    /// Abort `queue` by the vendor's sequence, in one step: force CCA to
    /// three, settle [`MAC_TX_ABORT_SETTLE`], sample VALID and clear it,
    /// release CCA, disable the queue if it was still valid, then
    /// acknowledge its timeout edge if one is pending. A
    /// [`MacTxAbortCause::TimeoutEdge`] abort with no edge writes nothing.
    ///
    /// Nothing is read back to decide: a queue that was valid is disabled and
    /// its chain returns through `aborted`; one that was not is on the air.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacStopTransmit`
    /// (`PROMOTED_LMAC_TX`): CCA forced, the 16 us delay, VALID sampled and
    /// cleared, CCA released, the queue disabled only if it was valid.
    pub fn abort_mac_tx<'registers, K: oer_portable_time::settle::SettleCounter + ?Sized, R>(
        &'registers mut self,
        queue: MacTxQueueIndex,
        cause: MacTxAbortCause,
        counter: &K,
        aborted: impl FnOnce(MacTxQueueAborted<'registers>) -> R,
    ) -> MacTxAbortOutcome<R> {
        let common = &mut self.mac.wifi_mac_tx_common;
        let queue_control = &mut self.mac.wifi_mac_tx_queue_control;
        if cause == MacTxAbortCause::TimeoutEdge && !queue::timeout_pending(common, queue) {
            return MacTxAbortOutcome::NoEdge;
        }
        let _ = queue::set_cca_force(common, Bits::constant::<3>());
        oer_portable_riscv::fence();
        // The vendor's fixed delay.
        oer_portable_time::settle::hold(oer_portable_time::settle::Settle::new(
            MAC_TX_ABORT_SETTLE,
            counter,
        ));
        let was_valid = queue::queue_valid(queue_control, queue);
        queue::invalidate_queue(queue_control, queue);
        let _ = queue::set_cca_force(common, Bits::constant::<0>());
        if was_valid {
            queue::disable_queue(queue_control, queue);
        }
        // Sampled after the disable: an edge rising until then would
        // otherwise stay for the queue's next publication.
        if queue::timeout_pending(common, queue) {
            queue::acknowledge_timeout(common, queue);
        }
        oer_portable_riscv::fence();
        if was_valid {
            MacTxAbortOutcome::Aborted(aborted(MacTxQueueAborted::new()))
        } else {
            MacTxAbortOutcome::OnAir
        }
    }

    /// Execute one detach transaction and consume its proof before allowing
    /// this register owner to be borrowed for another queue operation.
    pub fn with_detached_mac_tx<'registers, R>(
        &'registers mut self,
        queue: MacTxQueueIndex,
        reason: MacTxDetachReason,
        detached: impl FnOnce(MacTxQueueDetached<'registers>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let common = &mut self.mac.wifi_mac_tx_common;
        let queue_control = &mut self.mac.wifi_mac_tx_queue_control;
        let descriptor_address_low = queue_control
            .read_control_n(queue::bank(queue))
            .descriptor_address_low()
            .get();
        match reason {
            MacTxDetachReason::Collision => {
                if !queue::collision_pending(common, queue) {
                    return MacTxDetachOutcome::NoEvent;
                }
                // SOURCE: complete `libpp[lmac.o]::lmacProcessCollisions`
                // reaches disable before clearing the collision edge.
                queue::disable_queue(queue_control, queue);
                oer_portable_riscv::fence();
                queue::acknowledge_collision(common, queue);
            }
            MacTxDetachReason::Completed => {
                // A completion edge was consumed separately before this
                // transaction. Disable is still required even if hardware
                // already cleared ENABLE|VALID while completing the PPDU.
                queue::disable_queue(queue_control, queue);
            }
        }
        oer_portable_riscv::fence();
        if queue::queue_enabled(queue_control, queue) || queue::queue_valid(queue_control, queue) {
            return MacTxDetachOutcome::Failed;
        }
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::from_descriptor_address_low(
            descriptor_address_low,
        )))
    }
}

/// The fields of one PLCP1%s publication.
#[derive(Clone, Copy)]
struct Plcp1 {
    legacy_signal: Bits<{ oer_chips_esp32s31_pac::wifi_mac::plcp1::LEGACY_SIGNAL_WIDTH }>,
    rate: PlcpRate,
    key_entry_index: Bits<{ oer_chips_esp32s31_pac::wifi_mac::plcp1::KEY_ENTRY_INDEX_WIDTH }>,
    interface: MacInterface,
    vector_format: VectorFormat,
    cbw40: bool,
}

pub mod power_init;
pub mod queue;
pub mod statistics;

#[cfg(test)]
mod tests;
