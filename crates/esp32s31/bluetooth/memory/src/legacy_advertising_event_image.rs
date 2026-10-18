//! Private SRAM codec for the restricted legacy-advertising reset profile.
//!
//! The transform is limited to one legacy `ADV_NONCONN_IND`, LE 1M, no RX
//! chain, no CTE, no resolving list and the reviewed standalone controller
//! options. It does not create scheduler timing or publication authority.

#![forbid(unsafe_code)]

use crate::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_power::LeTxPower,
    link_state_event::{LinkStateEventWord, item_with_le_1m_power},
    sram_link::ControllerSramLinkAddress,
};

const ADV_NONCONN_IND_TYPE: u8 = 0x02;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 5) | (1 << 7);
const DEVICE_ADDRESS_BYTES: usize = 6;

const LOW_TWENTY_MASK: u32 = 0x000f_ffff;
/// Scheduling priority `r_sym_ble_Ok6PLzc6qsIOuEzma5oM` returns for the
/// first event at the default low advertising level.
const DEFAULT_ADVERTISING_PRIORITY: u8 = 1;
const RATE_LANES_MASK: u32 = 0xf000_0000;
const OPTIONS_IMAGE_MASK: u32 = 0x3f00_0000;
const REVIEWED_STANDALONE_OPTIONS: u32 = 3 << 24;

const SCHEDULER_ITEM_HARDWARE_NEXT_MASK: u32 = 0x000f_ffff;
/// Item `+0x00` bit 22, which the scanner's earliest-available start also
/// selects: the item may start as soon as its predecessor ends.
const SCHEDULER_ITEM_CHAINED_START: u32 = 1 << 22;
const SCHEDULER_ITEM_FREQUENCY_MASK: u32 = 0x0000_7f00;

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
    pub(super) word_00: u32,
    pub(super) word_04: u32,
    pub(super) word_08: u32,
    pub(super) word_0c: u32,
    pub(super) word_14: u32,
    pub(super) word_18: u32,
    pub(super) word_24: u32,
    pub(super) crc_init_word_2c: u32,
    pub(super) word_30: u32,
    pub(super) word_34: u32,
    pub(super) access_address_word_38: u32,
    pub(super) word_3c: u32,
    pub(super) word_40: u32,
    pub(super) word_50: u32,
    pub(super) word_60: u32,
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
        let transformed_high_half =
            ((((self.word_00 | 0x8000_0000) >> 16) as u16 & 0xe00f) | 0x1ff0) as u32;
        self.word_00 = (transformed_high_half << 16) | tx_header.compressed_image();

        self.word_04 &= !(LOW_TWENTY_MASK | RATE_LANES_MASK);
        self.word_08 = 0xeff0_0000;
        self.word_0c |= 0xa000_0000;
        self.word_14 = (self.word_14 | 0x0400_0000) & !0x0800_0000;
        self.word_18 &= 0x1fff_ffff;

        self.word_24 = 0x0710_0000;
        match own_address {
            LegacyAdvertisingOwnAddress::Public => {
                self.word_24 &= !0x3000_0000;
            }
            LegacyAdvertisingOwnAddress::Random(address) => {
                self.word_24 = (self.word_24 & !0x3000_0000) | 0x1000_0000;
                self.word_3c = u32::from_le_bytes([address[0], address[1], address[2], address[3]]);
                self.word_40 = (self.word_40 & 0xff00_0000)
                    | address[4] as u32
                    | ((address[5] as u32) << 8)
                    | ((self.word_40 | 0x0003_0000) & 0x00ff_0000);
            }
        }

        self.crc_init_word_2c =
            LeCrcInit::LE_PRESET.apply_to_controller_word(self.crc_init_word_2c & !0x0300_0000);
        self.word_30 = (self.word_30 & 0xffff_c100) | 0x0000_1e00;
        self.word_34 = 0;
        self.access_address_word_38 = LeAccessAddress::PRIMARY_ADVERTISING.controller_image();
        self.word_50 = (self.word_50 & !OPTIONS_IMAGE_MASK) | REVIEWED_STANDALONE_OPTIONS;
        self.word_60 = LinkStateEventWord::from_word(self.word_60 & 0xffff_0000)
            .with_power(default_tx_power)
            .with_priority(DEFAULT_ADVERTISING_PRIORITY)
            .word();
        self
    }

    /// The power index the common insertion copies into the item.
    const fn power_index(self) -> u8 {
        LinkStateEventWord::from_word(self.word_60).power_index()
    }
}

/// Complete scheduler-item subset changed before first-event admission.
#[derive(Clone, Copy)]
pub(super) struct LegacyAdvertisingSchedulerItemWords {
    pub(super) word_00: u32,
    pub(super) word_04: u32,
    pub(super) word_14: u32,
    pub(super) word_18: u32,
    pub(super) word_38: u32,
    pub(super) raw_start_word_44: u32,
    pub(super) raw_end_word_48: u32,
    pub(super) word_4c: u32,
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
        // The executor links the items; each carries only its own window.
        self.word_00 &= !(SCHEDULER_ITEM_HARDWARE_NEXT_MASK | SCHEDULER_ITEM_CHAINED_START);
        if matches!(position, LegacyAdvertisingItemPosition::Follower) {
            self.word_00 |= SCHEDULER_ITEM_CHAINED_START;
        }
        self.word_04 |= 0x8000_0000;
        self.word_14 = item_with_le_1m_power(self.word_14, link_state.power_index());
        self.word_18 = (self.word_18 & !(SCHEDULER_ITEM_FREQUENCY_MASK | 0xff))
            | ((channel.frequency_image() as u32) << 8)
            | 0x11;
        self.word_38 = 0;
        self.raw_start_word_44 = raw_start;
        self.raw_end_word_48 = raw_end;
        self.word_4c &= !0xff;
        self
    }
}

#[cfg(test)]
mod tests;
