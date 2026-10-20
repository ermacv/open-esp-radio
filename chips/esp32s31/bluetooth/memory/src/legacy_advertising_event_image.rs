//! Private SRAM codec for the restricted legacy-advertising reset profile.
//!
//! The transform is limited to one legacy `ADV_NONCONN_IND`, LE 1M, no RX
//! chain, no CTE, no resolving list and the reviewed standalone controller
//! options. It does not create scheduler timing or publication authority.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::layouts::*;

use crate::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_power::LeTxPower,
    sram_link::ControllerSramLinkAddress,
};

const ADV_NONCONN_IND_TYPE: u8 = 0x02;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 5) | (1 << 7);
const DEVICE_ADDRESS_BYTES: usize = 6;

/// Scheduling priority `r_sym_ble_Ok6PLzc6qsIOuEzma5oM` returns for the
/// first event at the default low advertising level.
const DEFAULT_ADVERTISING_PRIORITY: u8 = 1;

/// Item `+0x00` bit 22, which the scanner's earliest-available start also
/// selects: the item may start as soon as its predecessor ends.

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

/// Canonical non-empty primary-channel plan for one hardware event.
///
/// The plan is semantic: it contains no frequency, whitening or SRAM image.
/// Selected channels are always ordered 37, 38, 39.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingPrimaryChannelPlan {
    channels: [LegacyAdvertisingPrimaryChannel; 3],
    len: u8,
}

impl LegacyAdvertisingPrimaryChannelPlan {
    pub const fn new(channel_37: bool, channel_38: bool, channel_39: bool) -> Option<Self> {
        let mut channels = [LegacyAdvertisingPrimaryChannel::Channel37; 3];
        let mut len = 0;
        if channel_37 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel37;
            len += 1;
        }
        if channel_38 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel38;
            len += 1;
        }
        if channel_39 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel39;
            len += 1;
        }
        if len == 0 {
            None
        } else {
            Some(Self {
                channels,
                len: len as u8,
            })
        }
    }

    pub const fn channel_count(self) -> usize {
        self.len as usize
    }

    pub const fn channel(self, position: usize) -> Option<LegacyAdvertisingPrimaryChannel> {
        if position < self.channel_count() {
            Some(self.channels[position])
        } else {
            None
        }
    }
}

/// Address behavior selected by the TxAdd bit of the prepared PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LegacyAdvertisingOwnAddress {
    Public,
    Random([u8; DEVICE_ADDRESS_BYTES]),
}

impl LegacyAdvertisingOwnAddress {
    pub(super) fn from_pdu(pdu: &[u8]) -> Result<Self, LegacyAdvertisingPduError> {
        if pdu.len() < 2 + DEVICE_ADDRESS_BYTES {
            return Err(LegacyAdvertisingPduError::MissingAdvertiserAddress);
        }
        let header = pdu[0];
        if header & 0x0f != ADV_NONCONN_IND_TYPE {
            return Err(LegacyAdvertisingPduError::UnsupportedPduType);
        }
        if header & RESERVED_HEADER_BITS != 0 {
            return Err(LegacyAdvertisingPduError::UnsupportedHeaderFlags);
        }

        if header & TX_ADD_RANDOM == 0 {
            Ok(Self::Public)
        } else {
            let mut address = [0; DEVICE_ADDRESS_BYTES];
            address.copy_from_slice(&pdu[2..2 + DEVICE_ADDRESS_BYTES]);
            Ok(Self::Random(address))
        }
    }
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
    ) -> Self {
        self.word_00 = BleLinkStateTxPath::from_bits(tx_header.compressed_image())
            .with_max_tx_octets(255)
            .with_queue_ready(true)
            .with_bits_29_30_opaque(self.word_00.bits_29_30_opaque())
            .expect("retained prefix fits")
            .with_valid(true);
        self.word_04 = self
            .word_04
            .with_header(None)
            .expect("empty link fits")
            .with_bits_28_31_opaque(0)
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
                    .with_bits_16_23_opaque(self.word_40.bits_16_23_opaque() | 3);
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
            .with_bits_16_31_opaque(0)
            .with_priority(DEFAULT_ADVERTISING_PRIORITY)
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LegacyAdvertisingItemPosition {
    /// The first selected channel, which starts at its programmed anchor.
    First,
    /// A later selected channel, which may start once its predecessor ends.
    Follower,
}

impl LegacyAdvertisingItemPosition {
    pub(super) const fn of(index: usize) -> Self {
        if index == 0 {
            Self::First
        } else {
            Self::Follower
        }
    }
}

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
        channel: LegacyAdvertisingPrimaryChannel,
        position: LegacyAdvertisingItemPosition,
        raw_start: u32,
        raw_end: u32,
    ) -> Self {
        self.word_00 = self
            .word_00
            .with_address(None)
            .expect("empty link fits")
            .with_flexible_start(matches!(position, LegacyAdvertisingItemPosition::Follower));
        self.word_04 = self.word_04.with_event_ready(true);
        self.word_14 = self
            .word_14
            .with_bits_28_29_opaque(0)
            .expect("zero fits")
            .with_bits_30_31_opaque(0)
            .expect("zero fits")
            .with_power_index(link_state.power_index());
        self.word_18 = self
            .word_18
            .with_frequency(channel.frequency_image())
            .expect("primary channel fits")
            .with_arbitration_priority(1)
            .expect("priority fits")
            .with_bits_4_7_opaque(1)
            .expect("reviewed image fits");
        self.word_38 = ADVERTISING_ITEM_EVENT.word_38;
        self.raw_start_word_44 = self.raw_start_word_44.with_value(raw_start);
        self.raw_end_word_48 = self.raw_end_word_48.with_value(raw_end);
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
        .with_bits_16_27_opaque(0x710)
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
