//! Private SRAM codec for the restricted legacy-advertising reset profile.
//!
//! The transform is limited to one legacy `ADV_NONCONN_IND`, LE 1M, no RX
//! chain, no CTE, no resolving list and the reviewed standalone controller
//! options. It does not create scheduler timing or publication authority.

#![forbid(unsafe_code)]

use crate::layouts::*;

use crate::bluetooth::engine::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_power::LeTxPower,
    sram_link::ControllerSramLinkAddress,
};

/// Semantic primary channel selected by one legacy advertising event.
///
/// The private descriptor codec, rather than the portable Link Layer or chip
/// orchestration layer, owns the ESP32-S31 frequency image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingPrimaryChannel {
    Channel37,
    Channel38,
    Channel39,
}

impl LegacyAdvertisingPrimaryChannel {
    const fn frequency_image(self) -> u8 {
        match self {
            Self::Channel37 => 0,
            Self::Channel38 => 24,
            Self::Channel39 => 78,
        }
    }
}

/// Address behavior selected by the TxAdd bit of the prepared PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingOwnAddress {
    Public,
    Random([u8; 6]),
}

/// Why an encoded packet cannot select the restricted reset profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingPduError {
    MissingAdvertiserAddress,
    UnsupportedPduType,
    UnsupportedHeaderFlags,
}

/// Complete observed word subset changed by the restricted reset body.
#[derive(Clone, Copy)]
pub(super) struct LegacyAdvertisingLinkStateWords {
    pub(super) word_00: BleLinkStateTxPath,
    pub(super) word_04: BleLinkStateScanResponse,
    pub(super) word_08: BleLinkStateRxPath,
    pub(super) word_0c: BleLinkStateControlPolicy,
    pub(super) word_14: BleLinkStatePacketPolicy,
    pub(super) word_18: BleLinkStateReceiveContext,
    pub(super) word_24: BleLinkStateOwnAddressPolicy,
    pub(super) crc_init_word_2c: BleLinkStateCrcInitialization,
    pub(super) word_30: BleLinkStateSequencePolicy,
    pub(super) word_34: BleLinkStateEventSpan,
    pub(super) access_address_word_38: BleLinkStateAccessAddress,
    pub(super) word_3c: BleLinkStateOwnAddressLow,
    pub(super) word_40: BleLinkStateOwnAddressHigh,
    pub(super) word_50: BleLinkStateDirectionFinding,
    pub(super) word_60: BleLinkStateEvent,
}

impl LegacyAdvertisingLinkStateWords {
    /// Apply the exact no-RX/no-CTE/no-privacy LE 1M reset projection.
    ///
    /// SOURCE: pinned `libble_app.a[ble_2.o]::r_sym_ble_7lsXnox2LxrGG0FmY7qR`
    /// (`r_ble_lll_adv_reset_link_state`). Its `r_sym_ble_VTHfF9d12DSYglVnHRqM`
    /// prefix stores the scheduling priority of `r_sym_ble_Ok6PLzc6qsIOuEzma5oM`
    /// in byte `+0x60`, one for the default low advertising level. The body
    /// stores the transmit-power index in byte `+0x61` and no longer in
    /// `+0x04`, and sets bits 31:29 of `+0x08`. The channel-39 power copy of
    /// the link state is selected by `ble_adv_tx_options` bit 2, which the
    /// supported profile leaves clear.
    pub(super) const fn reset(
        mut self,
        tx_header: ControllerSramLinkAddress,
        own_address: LegacyAdvertisingOwnAddress,
        default_tx_power: LeTxPower,
        priority: SchedulerEventPriority,
    ) -> Self {
        // SOURCE: the reset's high-halfword transform retains the previous
        // pointer index's upper byte and ORs the bound header into it.
        // Operate on the decoded field, independently of its location in SRAM.
        let previous_index = self.word_00.header().to_le_bytes();
        let next_index = tx_header.compressed_image().to_le_bytes();
        let header_index = u32::from_le_bytes([
            next_index[0],
            next_index[1],
            previous_index[2] | next_index[2],
            next_index[3],
        ]);
        self.word_00 = self
            .word_00
            .with_header_from(BleLinkStateTxPath::from_bits(header_index))
            .with_max_tx_octets(255)
            .with_queue_ready(true)
            .with_valid(true);
        self.word_04 = self
            .word_04
            .with_header(None)
            .expect("empty link fits")
            .with_phy_code_28_29(0)
            .expect("zero fits")
            .with_phy_code_30_31(0)
            .expect("zero fits");
        self.word_08 = ADVERTISING_RESET.word_08;
        self.word_0c = self
            .word_0c
            .with_bit_29_opaque(true)
            .with_bit_31_opaque(true);
        self.word_14 = self
            .word_14
            .with_bits_26_27_opaque(1)
            .expect("reviewed profile fits");
        self.word_18 = self
            .word_18
            .with_bits_26_29_opaque(self.word_18.bits_26_29_opaque() & 7)
            .expect("retained prefix fits")
            .with_bit_30_opaque(false)
            .with_scan_started(false);
        self.word_24 = ADVERTISING_RESET.word_24;
        match own_address {
            LegacyAdvertisingOwnAddress::Public => {}
            LegacyAdvertisingOwnAddress::Random(address) => {
                self.word_24 = self
                    .word_24
                    .with_source(1)
                    .expect("random address source fits");
                self.word_3c = self.word_3c.with_value(u32::from_le_bytes([
                    address[0], address[1], address[2], address[3],
                ]));
                self.word_40 = self
                    .word_40
                    .with_value(u16::from_le_bytes([address[4], address[5]]))
                    .with_bits_16_17_opaque(3)
                    .expect("two set bits fit");
            }
        }
        self.crc_init_word_2c = self
            .crc_init_word_2c
            .with_scan_filter_policy(0)
            .expect("zero fits")
            .with_value(LeCrcInit::LE_PRESET.controller_value())
            .expect("CRC fits");
        self.word_30 = self
            .word_30
            .with_bits_0_7_opaque(0)
            .with_bits_9_12_opaque(15)
            .expect("reviewed sequence fits")
            .with_bit_13_opaque(false);
        self.word_34 = ADVERTISING_RESET.word_34;
        self.access_address_word_38 = ADVERTISING_RESET.access_address_word_38;
        self.word_50 = self
            .word_50
            .with_common_radio_policy(3)
            .expect("reviewed radio policy fits");
        self.word_60 = self
            .word_60
            .with_priority(priority.value())
            .with_power_index(default_tx_power.index());
        self
    }

    /// The power index the common insertion copies into the item.
    const fn power_index(self) -> u8 {
        self.word_60.power_index()
    }
}

/// Complete scheduler-item subset changed before first-event admission.
#[derive(Clone, Copy)]
pub(super) struct LegacyAdvertisingSchedulerItemWords {
    pub(super) word_00: BleSchedulerItemHardwareNext,
    pub(super) word_04: BleSchedulerItemContext,
    pub(super) word_14: BleSchedulerItemRateAndPower,
    pub(super) word_18: BleSchedulerItemChannelAndPriority,
    pub(super) word_38: BleSchedulerItemStatus,
    pub(super) raw_start_word_44: BleSchedulerItemRawStart,
    pub(super) raw_end_word_48: BleSchedulerItemRawEnd,
    pub(super) word_4c: BleSchedulerItemControl,
}

/// Position of one channel item within its advertising event.
impl LegacyAdvertisingSchedulerItemWords {
    /// Lower one accepted LE 1M channel item into the private layout.
    ///
    /// SOURCE: pinned `libble_app.a[ble_2.o]::r_sym_ble_GlcyfUkkhUzGUt8un0d8`
    /// (`r_ble_lll_adv_sched_first_pri_event`) clears item `+0x00` bit 22 on
    /// the first item; `r_sym_ble_eNifqLwR78cnxeKb1y6t`, which chains the
    /// remaining primary channels, sets it on every legacy follower.
    pub(super) const fn prepare_event_item(
        mut self,
        link_state: LegacyAdvertisingLinkStateWords,
        image: AdvertisingItemImage,
    ) -> Self {
        self.word_00 = self
            .word_00
            .with_address(None)
            .expect("empty link fits")
            .with_flexible_start(image.flexible_start);
        self.word_04 = self.word_04.with_event_ready(true);
        self.word_14 = self
            .word_14
            .with_phy_code_28_29(0)
            .expect("zero fits")
            .with_phy_code_30_31(0)
            .expect("zero fits")
            .with_power_index(link_state.power_index());
        self.word_18 = self
            .word_18
            .with_frequency(image.channel.frequency_image())
            .expect("primary channel fits")
            .with_arbitration_priority(image.priority.value())
            .expect("priority fits")
            .with_priority_copy(image.priority_copy.value())
            .expect("reviewed image fits");
        self.word_38 = ADVERTISING_ITEM_EVENT.word_38;
        self.raw_start_word_44 = self.raw_start_word_44.with_value(image.start);
        self.raw_end_word_48 = self.raw_end_word_48.with_value(image.end);
        self.word_4c = self.word_4c.with_event(0);
        self
    }
}

#[cfg(test)]
mod tests;

/// SOURCE: pinned `ble_2.o::r_sym_ble_7lsXnox2LxrGG0FmY7qR`, fixed reset lanes.
const ADVERTISING_RESET: LegacyAdvertisingLinkStateWords = LegacyAdvertisingLinkStateWords {
    word_00: BleLinkStateTxPath::ZERO,
    word_04: BleLinkStateScanResponse::ZERO,
    word_08: BleLinkStateRxPath::ZERO
        .with_unconsumed_limit(0xff)
        .with_bits_28_31_opaque(0xe)
        .expect("reviewed field image fits"),
    word_0c: BleLinkStateControlPolicy::ZERO,
    word_14: BleLinkStatePacketPolicy::ZERO,
    word_18: BleLinkStateReceiveContext::ZERO,
    word_24: BleLinkStateOwnAddressPolicy::ZERO
        .with_bits_20_22_opaque(1)
        .expect("reviewed field image fits")
        .with_bits_24_26_opaque(0x7)
        .expect("reviewed field image fits"),
    crc_init_word_2c: BleLinkStateCrcInitialization::ZERO,
    word_30: BleLinkStateSequencePolicy::ZERO,
    word_34: BleLinkStateEventSpan::ZERO,
    access_address_word_38: BleLinkStateAccessAddress::ZERO
        .with_value(LeAccessAddress::PRIMARY_ADVERTISING.controller_image()),
    word_3c: BleLinkStateOwnAddressLow::ZERO,
    word_40: BleLinkStateOwnAddressHigh::ZERO,
    word_50: BleLinkStateDirectionFinding::ZERO,
    word_60: BleLinkStateEvent::ZERO,
};

/// SOURCE: pinned `ble_2.o::r_sym_ble_7lsXnox2LxrGG0FmY7qR`, fixed event lanes.
const ADVERTISING_ITEM_EVENT: LegacyAdvertisingSchedulerItemWords =
    LegacyAdvertisingSchedulerItemWords {
        word_00: BleSchedulerItemHardwareNext::ZERO,
        word_04: BleSchedulerItemContext::ZERO,
        word_14: BleSchedulerItemRateAndPower::ZERO,
        word_18: BleSchedulerItemChannelAndPriority::ZERO,
        word_38: BleSchedulerItemStatus::ZERO,
        raw_start_word_44: BleSchedulerItemRawStart::ZERO,
        raw_end_word_48: BleSchedulerItemRawEnd::ZERO,
        word_4c: BleSchedulerItemControl::ZERO,
    };

/// Four-bit scheduler arbitration priority; not a coexistence lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerEventPriority(u8);
impl SchedulerEventPriority {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 15 { Some(Self(value)) } else { None }
    }
    pub const fn value(self) -> u8 {
        self.0
    }
}
/// Detached input to one closed physical item projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingItemImage {
    pub channel: LegacyAdvertisingPrimaryChannel,
    pub flexible_start: bool,
    pub priority: SchedulerEventPriority,
    pub priority_copy: SchedulerEventPriority,
    pub start: u32,
    pub end: u32,
    pub sequence_lead: u32,
}
/// One to three item values. This contains no SRAM view or publication proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingEventImage {
    items: [AdvertisingItemImage; 3],
    len: u8,
}
impl AdvertisingEventImage {
    pub fn new(items: &[AdvertisingItemImage]) -> Option<Self> {
        if items.is_empty() || items.len() > 3 {
            return None;
        }
        let mut image = Self {
            items: [items[0]; 3],
            len: items.len() as u8,
        };
        image.items[..items.len()].copy_from_slice(items);
        Some(image)
    }
    pub fn items(&self) -> &[AdvertisingItemImage] {
        &self.items[..usize::from(self.len)]
    }
}
