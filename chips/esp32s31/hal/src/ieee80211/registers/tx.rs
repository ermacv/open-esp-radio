//! The ordinary EDCA TX queue transactions of the Wi-Fi MAC.
//!
//! Each transaction is the vendor's sequence over the generated parts' field
//! accessors; the descriptor head reaches `CONTROL%s` only through the typed
//! pointer setter, from a [`DmaRef`] the prepared DMA authority holds.

use crate::layouts::{DmaRef, WifiTxDmaDescriptor};

use core::marker::PhantomData;

use oer_memory::{HardwareOwnedTxDma, PreparedTxDma};

use crate::ieee80211::registers::{
    MacInterface, MacPlacement, MacPti, MacTxPtiCount, MacTxQueueIndex, ORDINARY_QUEUE_COUNT,
    WifiDescriptorAddressLow, WifiRadioRegisters, bits, byte, halfword,
};

const DESCRIPTOR_ADDRESS_LOW_MASK: u32 = WifiDescriptorAddressLow::MAX;

/// Read-only state of one ordinary hardware TX queue.
///
/// This projection deliberately keeps the queue's descriptor identity and
/// publication/result state together. It is used at low-frequency role
/// boundaries to distinguish a real publication from a stale completion;
/// it grants no register mutation authority.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacOrdinaryTxQueueSnapshot {
    pub descriptor_address_low: u32,
    pub plcp_format: u8,
    pub legacy_signal: u16,
    pub rate: u8,
    pub key_entry_index: u8,
    pub bssid_select: u8,
    pub vector_format: u8,
    pub interface: u8,
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
    pub hang: crate::ieee80211::registers::rx::statistics::MacRxHangStatistics,
    /// Global wrapping 16-bit RX progress counters, sampled independently.
    pub rx_mpdu_count: u16,
    pub rx_signal_count: u16,
    pub rx_end_count: u16,
    pub rx_fcs_error_count: u16,
    pub rx_abort_count: u16,
    pub valid: bool,
    pub enabled: bool,
    pub completion_pending: bool,
    pub timeout_pending: bool,
    pub collision_pending: bool,
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

fn assert_tx_descriptor_head(
    authority_head: DmaRef<WifiTxDmaDescriptor>,
    descriptor_address_low: u32,
) {
    assert!(
        descriptor_address_low == authority_head.bits() & DESCRIPTOR_ADDRESS_LOW_MASK,
        "TX control register does not reference the retained DMA chain"
    );
}

/// Reviewed ESP32-S31 selector for one non-HT transmit rate.
///
/// Hardware codes are selected from the generated PAC domain. Callers can
/// decode observations without maintaining another hardware dictionary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MacLegacyRate {
    #[default]
    Dsss1MLong,
    Dsss2MLong,
    Cck5M5Long,
    Cck11MLong,
    Dsss2MShort,
    Cck5M5Short,
    Cck11MShort,
    Ofdm48M,
    Ofdm24M,
    Ofdm12M,
    Ofdm6M,
    Ofdm54M,
    Ofdm36M,
    Ofdm18M,
    Ofdm9M,
}

impl MacLegacyRate {
    /// The generated hardware code of this non-HT rate.
    pub const fn code(self) -> u8 {
        match self {
            Self::Dsss1MLong => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Dsss1mLong.bits() as u8,
            Self::Dsss2MLong => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Dsss2mLong.bits() as u8,
            Self::Cck5M5Long => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Cck5m5Long.bits() as u8,
            Self::Cck11MLong => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Cck11mLong.bits() as u8,
            Self::Dsss2MShort => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Dsss2mShort.bits() as u8,
            Self::Cck5M5Short => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Cck5m5Short.bits() as u8,
            Self::Cck11MShort => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Cck11mShort.bits() as u8,
            Self::Ofdm48M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm48m.bits() as u8,
            Self::Ofdm24M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm24m.bits() as u8,
            Self::Ofdm12M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm12m.bits() as u8,
            Self::Ofdm6M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm6m.bits() as u8,
            Self::Ofdm54M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm54m.bits() as u8,
            Self::Ofdm36M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm36m.bits() as u8,
            Self::Ofdm18M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm18m.bits() as u8,
            Self::Ofdm9M => oer_esp32s31_pac::wifi_mac::plcp1::Rate::Ofdm9m.bits() as u8,
        }
    }

    /// Decode a non-HT observation; unknown and HT codes are refused.
    pub const fn from_code(code: u8) -> Option<Self> {
        use oer_esp32s31_pac::wifi_mac::plcp1::Rate;
        match Rate::from_bits(code as u32) {
            Some(Rate::Dsss1mLong) => Some(Self::Dsss1MLong),
            Some(Rate::Dsss2mLong) => Some(Self::Dsss2MLong),
            Some(Rate::Cck5m5Long) => Some(Self::Cck5M5Long),
            Some(Rate::Cck11mLong) => Some(Self::Cck11MLong),
            Some(Rate::Dsss2mShort) => Some(Self::Dsss2MShort),
            Some(Rate::Cck5m5Short) => Some(Self::Cck5M5Short),
            Some(Rate::Cck11mShort) => Some(Self::Cck11MShort),
            Some(Rate::Ofdm48m) => Some(Self::Ofdm48M),
            Some(Rate::Ofdm24m) => Some(Self::Ofdm24M),
            Some(Rate::Ofdm12m) => Some(Self::Ofdm12M),
            Some(Rate::Ofdm6m) => Some(Self::Ofdm6M),
            Some(Rate::Ofdm54m) => Some(Self::Ofdm54M),
            Some(Rate::Ofdm36m) => Some(Self::Ofdm36M),
            Some(Rate::Ofdm18m) => Some(Self::Ofdm18M),
            Some(Rate::Ofdm9m) => Some(Self::Ofdm9M),
            _ => None,
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
    const fn plcp_format(self) -> u8 {
        match self {
            Self::None => {
                oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::NoResponse.bits() as u8
            }
            Self::Ack => {
                oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::AckResponse.bits() as u8
            }
            Self::BlockAck => {
                oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::BlockAckResponse.bits() as u8
            }
        }
    }
}

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

/// HAL-owned register program for one bounded legacy PPDU.
///
/// All register images are private. Higher layers can retain and inspect the
/// semantic values through accessors, but cannot publish or test raw PLCP
/// geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxProgram {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    parameters: MacLegacyTxParameters,
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
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacLegacyTxParameters,
    ) -> Option<Self> {
        if parameters.signal > 0x0fff
            || parameters.timeout > 0x0fff
            || parameters.scheduler_priority > 0x0f
            || parameters.packet_priority > 0x0f
            || parameters.priority_count > 0x0fff
            || parameters.aifsn > 0x0f
            || parameters.contention_window > 0x03ff
            || parameters.hardware_key_selector > 0x3f
        {
            return None;
        }

        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
        })
    }

    pub const fn interface(self) -> MacInterface {
        self.parameters.interface
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
    const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs0.bits() as u8,
            Self::Mcs1 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs1.bits() as u8,
            Self::Mcs2 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs2.bits() as u8,
            Self::Mcs3 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs3.bits() as u8,
            Self::Mcs4 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs4.bits() as u8,
            Self::Mcs5 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs5.bits() as u8,
            Self::Mcs6 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs6.bits() as u8,
            Self::Mcs7 => oer_esp32s31_pac::wifi_mac::ht_signal::Mcs::Mcs7.bits() as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtGuardInterval {
    Long800Ns,
    Short400Ns,
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
    const fn register_value(self) -> u16 {
        match self {
            Self::Density0To4 => 20,
            Self::Density5 => 40,
            Self::Density6 => 76,
            Self::Density7 => 148,
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
    const fn register_value(self) -> u8 {
        match (self.guard_interval, self.mcs) {
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs0) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs0.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs1) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs1.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs2) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs2.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs3) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs3.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs4) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs4.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs5) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs5.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs6) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs6.bits() as u8
            }
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs7) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtLongMcs7.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs0) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs0.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs1) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs1.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs2) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs2.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs3) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs3.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs4) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs4.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs5) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs5.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs6) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs6.bits() as u8
            }
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs7) => {
                oer_esp32s31_pac::wifi_mac::plcp1::Rate::HtShortMcs7.bits() as u8
            }
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
    pub txop: bool,
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
}

impl MacHtTxProgram {
    /// Control frame retained with this descriptor-bound program.
    pub const fn control(self) -> MacTxControlFrame {
        self.parameters.control
    }

    /// Bind one semantic HT publication to its prepared DMA authority.
    pub fn new(
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacHtTxParameters,
    ) -> Option<Self> {
        if parameters.length == 0
            || parameters.descriptor_count == 0
            || parameters.descriptor_count > 0x7f
            || (matches!(parameters.format, MacHtTxFormat::SingleMpdu)
                && parameters.descriptor_count != 1)
            || parameters.timeout > 0x0fff
            || parameters.scheduler_priority > 0x0f
            || parameters.packet_priority > 0x0f
            || parameters.priority_count > 0x0fff
            || parameters.aifsn > 0x0f
            || parameters.contention_window > 0x03ff
            || parameters.hardware_key_selector > 0x3f
        {
            return None;
        }

        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
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
    const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs0.bits() as u8,
            Self::Mcs1 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs1.bits() as u8,
            Self::Mcs2 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs2.bits() as u8,
            Self::Mcs3 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs3.bits() as u8,
            Self::Mcs4 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs4.bits() as u8,
            Self::Mcs5 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs5.bits() as u8,
            Self::Mcs6 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs6.bits() as u8,
            Self::Mcs7 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs7.bits() as u8,
            Self::Mcs8 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs8.bits() as u8,
            Self::Mcs9 => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Mcs::Mcs9.bits() as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeGuardIntervalAndLtf {
    OneLtf800Ns,
    TwoLtf800Ns,
    TwoLtf1600Ns,
    FourLtf3200Ns,
}

impl MacHeGuardIntervalAndLtf {
    const fn register_value(self) -> oer_esp32s31_pac::wifi_mac::he_su_signal_a1::GiLtf {
        match self {
            Self::OneLtf800Ns => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::GiLtf::OneLtf800ns,
            Self::TwoLtf800Ns => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::GiLtf::TwoLtf800ns,
            Self::TwoLtf1600Ns => oer_esp32s31_pac::wifi_mac::he_su_signal_a1::GiLtf::TwoLtf1600ns,
            Self::FourLtf3200Ns => {
                oer_esp32s31_pac::wifi_mac::he_su_signal_a1::GiLtf::FourLtf3200ns
            }
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
    pub guard_interval_and_ltf: MacHeGuardIntervalAndLtf,
    pub fec_coding: MacHeFecCoding,
    pub dcm: bool,
}

impl MacHeRate {
    const fn plcp_rate_field(self) -> u8 {
        match self.mcs {
            MacHeMcs::Mcs0 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs0.bits() as u8,
            MacHeMcs::Mcs1 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs1.bits() as u8,
            MacHeMcs::Mcs2 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs2.bits() as u8,
            MacHeMcs::Mcs3 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs3.bits() as u8,
            MacHeMcs::Mcs4 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs4.bits() as u8,
            MacHeMcs::Mcs5 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs5.bits() as u8,
            MacHeMcs::Mcs6 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs6.bits() as u8,
            MacHeMcs::Mcs7 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs7.bits() as u8,
            MacHeMcs::Mcs8 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs8.bits() as u8,
            MacHeMcs::Mcs9 => oer_esp32s31_pac::wifi_mac::plcp1::Rate::HeMcs9.bits() as u8,
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
}

impl MacHeTxProgram {
    /// Control frame retained with this descriptor-bound program.
    pub const fn control(self) -> MacTxControlFrame {
        self.parameters.control
    }

    pub fn new(
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        parameters: MacHeTxParameters,
    ) -> Option<Self> {
        if parameters.apep_length == 0
            || parameters.descriptor_count == 0
            || parameters.descriptor_count > 32
            || (matches!(parameters.format, MacHeTxFormat::Smpdu)
                && parameters.descriptor_count != 1)
            || parameters.bss_color > 0x3f
            || parameters.spatial_reuse > 0x0f
            || parameters.protection_spacing > 0x03ff
            || parameters.timeout > 0x0fff
            || parameters.scheduler_priority > 0x0f
            || parameters.packet_priority > 0x0f
            || parameters.priority_count > 0x0fff
            || parameters.aifsn > 0x0f
            || parameters.contention_window > 0x03ff
            || parameters.hardware_key_selector > 0x3f
        {
            return None;
        }
        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
        })
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
    trigger_flow: bool,
    trigger_based_packet_count: u8,
    last_tx_was_trigger_based: bool,
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

    pub const fn trigger_flow(self) -> bool {
        self.trigger_flow
    }

    pub const fn used_alternate(self) -> bool {
        self.last_tx_was_trigger_based
    }

    pub const fn trigger_based_packet_count(self) -> u8 {
        self.trigger_based_packet_count
    }

    pub const fn last_tx_was_trigger_based(self) -> bool {
        self.last_tx_was_trigger_based
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
            trigger_flow: false,
            trigger_based_packet_count: 0,
            last_tx_was_trigger_based: false,
            secondary_trigger_based_packet_count: 0,
            ack_snr_encoded: 0,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_trigger_flow_model(mut self, trigger_flow: bool) -> Self {
        self.trigger_flow = trigger_flow;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_trigger_packet_counts_model(
        mut self,
        primary: u8,
        last_tx_was_trigger_based: bool,
        secondary: u8,
    ) -> Self {
        self.trigger_based_packet_count = primary;
        self.last_tx_was_trigger_based = last_tx_was_trigger_based;
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
    Timeout,
    Completed,
}

/// Result of one finite queue-detach transaction.
#[derive(Debug, Eq, PartialEq)]
pub enum MacTxDetachOutcome<T> {
    /// The requested collision/timeout edge was not pending.
    NoEvent,
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
    pub fn ordinary_tx_queue_snapshot(&self, queue: u8) -> MacOrdinaryTxQueueSnapshot {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
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
            interface: byte(config.interface()),
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
            valid: control.valid(),
            enabled: control.enable(),
            completion_pending: queue::completion_pending(common, queue),
            timeout_pending: queue::timeout_pending(common, queue),
            collision_pending: queue::collision_pending(common, queue),
        }
    }

    /// Execute complete `hal_set_tx_pti` over one bounded logical queue.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        let queue = u8::try_from(queue.get()).expect("a TX queue index fits a byte");
        let bank = queue::bank(queue);
        self.mac.wifi_mac_tx_queue_control.modify_config(bank, |w| {
            w.set_scheduler_priority(bits(program.scheduler_priority.get()));
        });
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_pti(bank, |w| {
            w.set_pti_2(bits(program.pti_2.get()));
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_1(bits(program.pti_1.get()));
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_0(bits(program.pti_0.get()));
        });
        vectors.modify_pti(bank, |w| {
            w.set_pti_3(bits(program.pti_3.get()));
        });
        vectors.modify_pti(bank, |w| {
            w.set_count(bits(program.count.get()));
        });
    }

    /// Prepare one legacy queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_legacy_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        assert!(
            dma.descriptor_head() == program.descriptor_head,
            "the program is bound to this DMA chain"
        );
        if !self.prepare_legacy_mac_tx(queue, program) {
            return false;
        }
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        true
    }

    /// Prepare one HT queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_ht_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        assert!(
            dma.descriptor_head() == program.descriptor_head,
            "the program is bound to this DMA chain"
        );
        if !self.prepare_ht_mac_tx(queue, program) {
            return false;
        }
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        true
    }

    /// Prepare one HE queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_he_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        assert!(
            dma.descriptor_head() == program.descriptor_head,
            "the program is bound to this DMA chain"
        );
        if !self.prepare_he_mac_tx(queue, program) {
            return false;
        }
        queue::configure_protection(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            program.control().protection,
        );
        true
    }

    /// Publish the final ENABLE|VALID edge for a hardware-owned TX chain.
    pub fn start_bound_mac_tx(
        &mut self,
        dma: &dyn HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: u8,
    ) {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        let control = self
            .mac
            .wifi_mac_tx_queue_control
            .read_control_n(queue::bank(queue));
        assert_tx_descriptor_head(
            dma.descriptor_head(),
            control.descriptor_address_low().get(),
        );
        self.start_prepared_mac_tx(queue);
    }

    /// The idle check and `hal_mac_tx_config_timeout` of one ordinary queue,
    /// before any vector write.
    ///
    /// SOURCE: complete hal_mac_tx_config_timeout. This precedes every
    /// vector write in the recovered libpp[lmac.o]::lmacSetTxFrame parent.
    fn begin_mac_tx(&mut self, queue: u8, timeout: u16) -> bool {
        let bank = queue::bank(queue);
        let control_bank = &mut self.mac.wifi_mac_tx_queue_control;
        let control_state = control_bank.read_control_n(bank);
        if control_state.enable() || control_state.valid() {
            return false;
        }
        control_bank.modify_config(bank, |w| {
            w.set_timeout(bits(u32::from(timeout)));
        });
        true
    }

    /// Program one ordinary queue up to, but excluding, its ENABLE|VALID edge.
    ///
    /// Keeping the final edge separate lets the MAC publish its software
    /// ownership state before hardware can complete the queue.
    fn prepare_legacy_mac_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        let parameters = program.parameters;
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        assert!(
            u32::from(parameters.timeout) <= oer_esp32s31_pac::wifi_mac::config::TIMEOUT_MAX,
            "the program bounds the timeout"
        );
        assert!(
            u32::from(parameters.scheduler_priority)
                <= oer_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.packet_priority) <= oer_esp32s31_pac::wifi_mac::pti::PTI_0_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.priority_count) <= oer_esp32s31_pac::wifi_mac::pti::COUNT_MAX,
            "the program bounds the count"
        );
        assert!(
            u32::from(parameters.aifsn) <= oer_esp32s31_pac::wifi_mac::config::AIFSN_MAX,
            "the program bounds AIFSN"
        );
        assert!(
            u32::from(parameters.contention_window)
                <= oer_esp32s31_pac::wifi_mac::config::CONTENTION_WINDOW_MAX,
            "the program bounds the window"
        );

        if !self.begin_mac_tx(queue, parameters.timeout) {
            return false;
        }

        self.program_legacy_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            parameters.aifsn,
            parameters.contention_window,
            parameters.interface,
        );
        true
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
        queue: u8,
        descriptor_head: DmaRef<WifiTxDmaDescriptor>,
        plcp_format: u8,
    ) {
        self.mac
            .wifi_mac_tx_queue_control
            .modify_control_n(queue::bank(queue), |w| {
                w.set_descriptor_address_low(descriptor_head)
                    .set_bits_20_21_opaque(bits(2))
                    .set_bit_22_opaque(true)
                    .set_bit_23_opaque(false)
                    .set_plcp_format(bits(u32::from(plcp_format)))
                    .set_bits_27_29_opaque(bits(0))
                    .set_valid(false)
                    .set_enable(false);
            });
        queue::configure_rts(
            &mut self.mac.wifi_mac_tx_queue_control,
            &mut self.mac.wifi_mac_tx_queue_vector,
            queue,
            false,
            None,
        );
    }

    /// The PLCP1%s image, stored from zero (`publish_mac_tx_plcp1_fields`).
    fn publish_plcp1(&mut self, queue: u8, plcp1: Plcp1) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_plcp1(queue::bank(queue), |w| {
                w.set_legacy_signal(bits(u32::from(plcp1.legacy_signal)))
                    .set_rate(bits(u32::from(plcp1.rate)))
                    .set_key_entry_index(bits(u32::from(plcp1.key_entry_index)))
                    .set_bssid_select(plcp1.interface.field())
                    .set_vector_format(bits(u32::from(plcp1.vector_format)))
                    .set_high_control_28(false)
                    .set_cbw40(plcp1.cbw40)
                    .set_high_control_31_30(bits(0));
            });
    }

    /// Clear trigger-based transmission on the queue's HE queue control.
    fn clear_trigger_based_enable(&mut self, queue: u8) {
        self.mac.wifi_mac_he_init_suffix.modify_queue_control(
            queue::he_queue_control(queue),
            |w| {
                w.set_trigger_based_enable(false);
            },
        );
    }

    /// The parent's independent PTI-low edge.
    fn set_txop(&mut self, queue: u8, txop: bool) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_pti(queue::bank(queue), |w| {
                w.set_txop(txop);
            });
    }

    /// LENGTH_CONTROL%s stored from zero
    /// (`publish_mac_tx_length_control_fields`): the TX/RX queue antenna
    /// enable, the control-frame rate and the HT20 EOF count.
    fn publish_length_control(&mut self, queue: u8, rts_rate: u8, ht20_eof_count: u8) {
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_length_control(queue::bank(queue), |w| {
                w.set_txrx_queue_antenna_last(false)
                    .set_txrx_queue_antenna_value(false)
                    .set_txrx_queue_antenna_enable(true)
                    .set_tx_queue_antenna_last(false)
                    .set_tx_queue_antenna_value(false)
                    .set_tx_queue_antenna_enable(false)
                    .set_rts_rate(rts_rate)
                    .set_bits_14_21_opaque(0)
                    .set_ht20_eof_count(bits(u32::from(ht20_eof_count)))
                    .set_ht20_txop_count(bits(0))
                    .set_bits_28_31_opaque(bits(0));
            });
    }

    /// POWER%s stored from zero (`publish_mac_tx_power_fields`).
    fn publish_power(
        &mut self,
        queue: u8,
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
    fn publish_packet_pti(
        &mut self,
        queue: u8,
        scheduler_priority: u8,
        packet_priority: u8,
        priority_count: u16,
    ) {
        let bank = queue::bank(queue);
        self.mac.wifi_mac_tx_queue_control.modify_config(bank, |w| {
            w.set_scheduler_priority(bits(u32::from(scheduler_priority)));
        });
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        let packet = bits::<4>(u32::from(packet_priority));
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
            w.set_count(bits(u32::from(priority_count)));
        });
    }

    /// Publish the complete legacy responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction, as for
    /// [`Self::program_ht_mac_tx_ppdu`].
    pub(crate) fn program_legacy_mac_tx_ppdu(&mut self, queue: u8, program: MacLegacyTxProgram) {
        let parameters = program.parameters;
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        assert!(
            u32::from(parameters.scheduler_priority)
                <= oer_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.packet_priority) <= oer_esp32s31_pac::wifi_mac::pti::PTI_0_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.priority_count) <= oer_esp32s31_pac::wifi_mac::pti::COUNT_MAX,
            "the program bounds the count"
        );
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            parameters.response.plcp_format(),
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: parameters.signal,
                rate: parameters.rate.code(),
                key_entry_index: parameters.hardware_key_selector,
                interface: parameters.interface,
                vector_format: oer_esp32s31_pac::wifi_mac::plcp1::VectorFormat::Legacy.bits() as u8,
                cbw40: false,
            },
        );
        self.clear_trigger_based_enable(queue);
        // The parent's independent PTI-low edge; a legacy PPDU holds no TXOP.
        self.set_txop(queue, false);
        self.publish_length_control(queue, parameters.control.rate.code(), 1);
        // `mac_tx_set_txop_q` of a queue without a granted TXOP slot: the
        // slot `lmacInitAc` leaves (and `lmacReleaseTxopQueue` restores)
        // clears the TXOP count through one fresh-read update.
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_length_control(queue::bank(queue), |w| {
                w.set_ht20_txop_count(bits(0));
            });
        self.publish_power(queue, parameters.data_power, 0, parameters.control);
        self.publish_packet_pti(
            queue,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
        );
    }

    /// Program one non-aggregate HT queue up to its final ENABLE|VALID edge.
    ///
    /// This is deliberately separate from the legacy routine: an HT PPDU has
    /// two additional vector words and three descriptor-count RMW edges which
    /// must not be silently omitted by a shared "mostly legacy" formatter.
    pub(crate) fn prepare_ht_mac_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool {
        let parameters = program.parameters;
        assert!(
            u32::from(parameters.timeout) <= oer_esp32s31_pac::wifi_mac::config::TIMEOUT_MAX,
            "the program bounds the timeout"
        );
        assert!(
            u32::from(parameters.aifsn) <= oer_esp32s31_pac::wifi_mac::config::AIFSN_MAX,
            "the program bounds AIFSN"
        );
        assert!(
            u32::from(parameters.contention_window)
                <= oer_esp32s31_pac::wifi_mac::config::CONTENTION_WINDOW_MAX,
            "the program bounds the window"
        );
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );

        // SOURCE: complete hal_mac_tx_config_timeout, followed by the
        // libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu non-HE HT branch.
        if !self.begin_mac_tx(queue, parameters.timeout) {
            return false;
        }

        self.program_ht_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            parameters.aifsn,
            parameters.contention_window,
            parameters.interface,
        );
        true
    }

    /// Publish the complete non-HE HT responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction. Keeping this
    /// slice explicit lets the compiled production implementation be compared
    /// to the same vendor responsibility without a shadow register model.
    pub(crate) fn program_ht_mac_tx_ppdu(&mut self, queue: u8, program: MacHtTxProgram) {
        let parameters = program.parameters;
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        assert!(
            u32::from(parameters.descriptor_count)
                <= oer_esp32s31_pac::wifi_mac::ht_descriptor_counts::DESCRIPTOR_COUNT_A_MAX,
            "the program bounds the count"
        );
        assert!(
            parameters.protection_spacing.register_value() <= 0x03ff,
            "every spacing fits its field"
        );
        assert!(
            u32::from(parameters.scheduler_priority)
                <= oer_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.packet_priority) <= oer_esp32s31_pac::wifi_mac::pti::PTI_0_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.priority_count) <= oer_esp32s31_pac::wifi_mac::pti::COUNT_MAX,
            "the program bounds the count"
        );

        let bank = queue::bank(queue);
        let cbw40 = matches!(parameters.rate.channel_width, MacHtChannelWidth::Mhz40);
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            match parameters.format {
                MacHtTxFormat::SingleMpdu => {
                    oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HtSingleMpdu.bits() as u8
                }
                MacHtTxFormat::Ampdu => {
                    oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HtAmpdu.bits() as u8
                }
            },
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: 0,
                rate: parameters.rate.register_value(),
                key_entry_index: parameters.hardware_key_selector,
                interface: parameters.interface,
                vector_format: oer_esp32s31_pac::wifi_mac::plcp1::VectorFormat::Ht.bits() as u8,
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
            w.set_mcs(bits(u32::from(parameters.rate.mcs.index())))
                .set_cbw40(cbw40)
                .set_length(parameters.length)
                .set_smoothing(true)
                .set_not_sounding(true)
                .set_required_one(true)
                .set_aggregation(matches!(parameters.format, MacHtTxFormat::Ampdu))
                .set_stbc(bits(
                    oer_esp32s31_pac::wifi_mac::ht_signal::Stbc::None.bits(),
                ))
                .set_fec_coding(false)
                .set_short_gi(matches!(
                    parameters.rate.guard_interval,
                    MacHtGuardInterval::Short400Ns
                ));
        });
        let count = bits::<7>(u32::from(parameters.descriptor_count));
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a(count);
        });
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_b(count);
        });
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a_copy(count);
        });

        let spacing = bits::<10>(u32::from(parameters.protection_spacing.register_value()));
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
            MacHtTxFormat::SingleMpdu => 0,
            MacHtTxFormat::Ampdu => 1,
        };
        self.publish_length_control(queue, parameters.control.rate.code(), entry_class);
        // DATA_LENGTH%s stored from zero (`publish_mac_tx_data_length_fields`).
        self.mac
            .wifi_mac_tx_queue_vector
            .modify_data_length(bank, |w| {
                w.set_ht40_length(bits(u32::from(parameters.length)))
                    .set_ht40_eof_count(bits(u32::from(entry_class)))
                    .set_ht40_txop_count(bits(0))
                    .set_ht40_mcs(bits(u32::from(parameters.rate.mcs.index())));
            });
        self.publish_power(
            queue,
            parameters.data_power_primary,
            parameters.data_power_alternate,
            parameters.control,
        );
        self.publish_packet_pti(
            queue,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
        );
    }

    /// Program one HE SU A-MPDU up to its final ENABLE|VALID edge.
    ///
    /// This is separate from HT because HE publishes two different vector
    /// words and deliberately does not write the non-HE DATA_LENGTH word.
    fn prepare_he_mac_tx(&mut self, queue: u8, program: MacHeTxProgram) -> bool {
        let parameters = program.parameters;
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        assert!(
            parameters.descriptor_count <= 32,
            "the program bounds the count"
        );
        assert!(
            parameters.protection_spacing <= 0x03ff,
            "the program bounds the spacing"
        );
        assert!(
            u32::from(parameters.timeout) <= oer_esp32s31_pac::wifi_mac::config::TIMEOUT_MAX,
            "the program bounds the timeout"
        );
        assert!(
            u32::from(parameters.scheduler_priority)
                <= oer_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.packet_priority) <= oer_esp32s31_pac::wifi_mac::pti::PTI_0_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.priority_count) <= oer_esp32s31_pac::wifi_mac::pti::COUNT_MAX,
            "the program bounds the count"
        );
        assert!(
            u32::from(parameters.aifsn) <= oer_esp32s31_pac::wifi_mac::config::AIFSN_MAX,
            "the program bounds AIFSN"
        );
        assert!(
            u32::from(parameters.contention_window)
                <= oer_esp32s31_pac::wifi_mac::config::CONTENTION_WINDOW_MAX,
            "the program bounds the window"
        );

        if !self.begin_mac_tx(queue, parameters.timeout) {
            return false;
        }
        self.program_he_mac_tx_ppdu(queue, program);

        queue::configure_edca(
            &mut self.mac.wifi_mac_tx_queue_control,
            queue,
            parameters.aifsn,
            parameters.contention_window,
            parameters.interface,
        );
        true
    }

    /// Publish the complete HE SU responsibility of vendor
    /// `libpp[hal_mac_tx.o]::hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction, as for
    /// [`Self::program_ht_mac_tx_ppdu`].
    pub(crate) fn program_he_mac_tx_ppdu(&mut self, queue: u8, program: MacHeTxProgram) {
        let parameters = program.parameters;
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        assert!(
            parameters.descriptor_count <= 32,
            "the program bounds the count"
        );
        assert!(
            parameters.protection_spacing <= 0x03ff,
            "the program bounds the spacing"
        );
        assert!(
            u32::from(parameters.scheduler_priority)
                <= oer_esp32s31_pac::wifi_mac::config::SCHEDULER_PRIORITY_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.packet_priority) <= oer_esp32s31_pac::wifi_mac::pti::PTI_0_MAX,
            "the program bounds the priority"
        );
        assert!(
            u32::from(parameters.priority_count) <= oer_esp32s31_pac::wifi_mac::pti::COUNT_MAX,
            "the program bounds the count"
        );
        let bank = queue::bank(queue);
        self.publish_prepared_control(
            queue,
            program.descriptor_head,
            match parameters.format {
                MacHeTxFormat::Smpdu => {
                    oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HeSmpdu.bits() as u8
                }
                MacHeTxFormat::Ampdu => {
                    oer_esp32s31_pac::wifi_mac::control_n::PlcpFormat::HeAmpdu.bits() as u8
                }
            },
        );
        self.publish_plcp1(
            queue,
            Plcp1 {
                legacy_signal: 0,
                rate: parameters.rate.plcp_rate_field(),
                key_entry_index: parameters.hardware_key_selector,
                interface: parameters.interface,
                vector_format: oer_esp32s31_pac::wifi_mac::plcp1::VectorFormat::He.bits() as u8,
                cbw40: false,
            },
        );
        self.clear_trigger_based_enable(queue);
        // The parent's independent PTI-low edge; this SU profile holds no
        // TXOP.
        self.set_txop(queue, false);

        // SOURCE: complete libpp[hal_mac_tx.o]::mac_tx_set_hesig stores A1
        // then A2/length (each from zero, `publish_mac_tx_he_signal_a1_fields`
        // and `publish_mac_tx_he_signal_a2_fields`) before publishing the
        // descriptor counts.
        let vectors = &mut self.mac.wifi_mac_tx_queue_vector;
        vectors.modify_he_su_signal_a1(bank, |w| {
            w.set_format(true)
                .set_beam_change(true)
                .set_uplink(true)
                .set_mcs(bits(u32::from(parameters.rate.mcs.index())))
                .set_dcm(parameters.rate.dcm)
                .set_bss_color(bits(u32::from(parameters.bss_color)))
                .set_vector_required_14(true)
                .set_spatial_reuse(bits(u32::from(parameters.spatial_reuse)))
                .set_bandwidth(bits(oer_esp32s31_pac::wifi_mac::he_su_signal_a1::Bandwidth::Mhz20.bits()))
                .set_gi_ltf(parameters.rate.guard_interval_and_ltf.register_value())
                .set_nsts_and_midamble_periodicity(bits(oer_esp32s31_pac::wifi_mac::he_su_signal_a1::NstsAndMidamblePeriodicity::OneStreamNoMidamble.bits()))
                .set_vector_required_high(bits(0x3f));
        });
        vectors.modify_he_su_signal_a2_length(bank, |w| {
            w.set_control_required_0(true)
                .set_fec_coding(matches!(parameters.rate.fec_coding, MacHeFecCoding::Ldpc))
                .set_control_required_2(true)
                .set_bits_5_7_opaque(bits(0))
                .set_control_required_8(true)
                .set_descriptor_class_match(false)
                .set_apep_length(parameters.apep_length)
                .set_ampdu_length_valid(matches!(parameters.format, MacHeTxFormat::Ampdu))
                .set_bit_27_opaque(false);
        });
        // Unlike HT's three fresh-read edges, the HE leaf writes all three
        // descriptor counts as one register image, from zero
        // (`publish_mac_tx_he_descriptor_counts`).
        let count = bits::<7>(u32::from(parameters.descriptor_count));
        vectors.modify_ht_descriptor_counts(bank, |w| {
            w.set_descriptor_count_a(count)
                .set_descriptor_count_b(count)
                .set_descriptor_count_a_copy(count)
                .set_bit_21_opaque(false);
        });
        // SOURCE: complete libpp[hal_mac_tx.o]::mac_tx_set_hesig then replaces the three finite
        // channel-width minimum-MPDU lanes and clears software CTS in one
        // fresh-read update that keeps only software RTS.
        let spacing = bits::<10>(u32::from(parameters.protection_spacing));
        self.mac
            .wifi_mac_tx_queue_control
            .modify_protection(bank, |w| {
                w.set_minimum_mpdu_length_cbw20(spacing)
                    .set_minimum_mpdu_length_cbw40(spacing)
                    .set_minimum_mpdu_length_cbw80(spacing)
                    .set_software_cts(false);
            });

        // HE reaches mac_tx_set_len for LENGTH_CONTROL, but its flag-bit-31
        // branch intentionally skips the non-HE DATA_LENGTH register.
        self.publish_length_control(queue, parameters.control.rate.code(), 1);
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
        self.publish_packet_pti(
            queue,
            parameters.scheduler_priority,
            parameters.packet_priority,
            parameters.priority_count,
        );
    }

    pub(crate) fn start_prepared_mac_tx(&mut self, queue: u8) {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        oer_riscv::fence();
        // SOURCE: publication prefix of `libpp[hal_mac_tx.o]::hal_mac_txq_enable` offsets 0x00..0x1e, before libpp[lmac.o]::GetAccess. It reads the
        // already prepared CONTROL word and ORs only ENABLE|VALID before
        // writing it back. In particular, it does not reconstruct PLCP0 from
        // the caller's earlier software image: formatter leaves may have
        // changed control fields after the initial PLCP0 publication.
        // The remaining vendor body updates access/HE bookkeeping and
        // statistics; equivalence of this prefix does not cover that body.
        queue::publish_queue(&mut self.mac.wifi_mac_tx_queue_control, queue);
        oer_riscv::fence();
    }

    pub fn take_mac_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        if !queue::completion_pending(&self.mac.wifi_mac_tx_common, queue) {
            return None;
        }

        let bank = queue::bank(queue);
        let tx_results = &self.mac.wifi_mac_block_ack;
        let (trigger_based_packet_count, last_tx_was_trigger_based) = match queue {
            0 => {
                let information = tx_results.read_tx_queue_information_q0();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            1 => {
                let information = tx_results.read_tx_queue_information_q1();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            2 => {
                let information = tx_results.read_tx_queue_information_q2();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            3 => {
                let information = tx_results.read_tx_queue_information_q3();
                (
                    byte(information.trigger_based_packet_count()),
                    information.last_tx_was_trigger_based(),
                )
            }
            _ => unreachable!("the queue was checked against the ordinary count"),
        };
        let completion = &self.mac.wifi_mac_tx_completion;
        let secondary_trigger_based_packet_count = byte(
            completion
                .read_aux_c(bank)
                .secondary_trigger_based_packet_count(),
        );
        let primary = completion.read_primary(bank);
        let alternate = completion.read_alternate(bank);
        let (status, detail) = if last_tx_was_trigger_based {
            (byte(alternate.status()), alternate.detail())
        } else {
            (byte(primary.status()), primary.detail())
        };
        let common = &mut self.mac.wifi_mac_tx_common;
        let trigger_flow = queue::queue_in_trigger_flow(common, queue);
        queue::acknowledge_completion(common, queue);
        oer_riscv::fence();
        Some(MacTxCompletionObservation {
            status,
            detail,
            trigger_flow,
            trigger_based_packet_count,
            last_tx_was_trigger_based,
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
        queue: u8,
    ) -> Option<MacHtAmpduCompletionObservation> {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        if !queue::completion_pending(&self.mac.wifi_mac_tx_common, queue) {
            return None;
        }
        let block_ack = self.read_tx_block_ack_observation(queue)?;
        let tx = self.take_mac_tx_completion(queue)?;
        Some(MacHtAmpduCompletionObservation {
            tx,
            block_ack_control: block_ack.control,
            block_ack_starting_sequence: block_ack.starting_sequence,
            block_ack_bitmap: block_ack.bitmap,
            block_ack_received: block_ack.block_ack_received,
        })
    }

    pub fn begin_mac_tx_timeout_abort(&mut self, queue: u8) -> bool {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
        let common = &mut self.mac.wifi_mac_tx_common;
        if !queue::timeout_pending(common, queue) {
            return false;
        }
        let _ = queue::set_cca_force(common, 3);
        oer_riscv::fence();
        true
    }

    /// Execute one detach transaction and consume its proof before allowing
    /// this register owner to be borrowed for another queue operation.
    pub fn with_detached_mac_tx<'registers, R>(
        &'registers mut self,
        queue: u8,
        reason: MacTxDetachReason,
        detached: impl FnOnce(MacTxQueueDetached<'registers>) -> R,
    ) -> MacTxDetachOutcome<R> {
        assert!(
            queue < ORDINARY_QUEUE_COUNT,
            "ordinary TX queue is out of range"
        );
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
                oer_riscv::fence();
                queue::acknowledge_collision(common, queue);
            }
            MacTxDetachReason::Timeout => {
                if !queue::timeout_pending(common, queue) {
                    return MacTxDetachOutcome::NoEvent;
                }
                let was_valid = queue::queue_valid(queue_control, queue);
                queue::invalidate_queue(queue_control, queue);
                let _ = queue::set_cca_force(common, 0);
                if was_valid {
                    queue::disable_queue(queue_control, queue);
                }
                queue::acknowledge_timeout(common, queue);
            }
            MacTxDetachReason::Completed => {
                // A completion edge was consumed separately before this
                // transaction. Disable is still required even if hardware
                // already cleared ENABLE|VALID while completing the PPDU.
                queue::disable_queue(queue_control, queue);
            }
        }
        oer_riscv::fence();
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
    legacy_signal: u16,
    rate: u8,
    key_entry_index: u8,
    interface: MacInterface,
    vector_format: u8,
    cbw40: bool,
}

pub mod power_init;
pub mod queue;
pub mod statistics;

#[cfg(test)]
mod tests;
