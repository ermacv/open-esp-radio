//! Private SRAM images for the restricted legacy passive-scanning profile.

#![forbid(unsafe_code)]

use crate::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_power::LeTxPower,
    link_state_event::{LinkStateEventWord, item_with_le_1m_power},
    sram_link::ControllerSramLinkAddress,
};

pub(super) const BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS: usize = 0x84 / 4;
const RX_HEAD_MASK: u32 = 0x000f_ffff;
const LINK_STATE_18_BIT_20: u32 = 1 << 20;
const LINK_STATE_18_BIT_31: u32 = 1 << 31;
const LINK_STATE_18_ACCEPT_LIST_FILTER: u32 = 1 << 25;
const LINK_STATE_2C_FILTER_POLICY_SHIFT: u32 = 24;
const WORD_00: usize = 0;
const WORD_08: usize = 2;
const WORD_0C: usize = 3;
const WORD_14: usize = 5;
const WORD_18: usize = 6;
const WORD_24: usize = 9;
const WORD_2C: usize = 11;
const WORD_30: usize = 12;
const WORD_34: usize = 13;
const WORD_38: usize = 14;
const WORD_48: usize = 18;
const WORD_50: usize = 20;
const WORD_60: usize = 24;
const SCHEDULER_HARDWARE_CHAIN_ADJUSTED_START: u32 = 1 << 22;
const SCHEDULER_HARDWARE_CHAIN_EVENT_READY: u32 = 1 << 23;
const SCHEDULER_HARDWARE_CHAIN_TIMING_MASK: u32 =
    SCHEDULER_HARDWARE_CHAIN_ADJUSTED_START | SCHEDULER_HARDWARE_CHAIN_EVENT_READY;
const SCHEDULER_CONTEXT_EVENT_READY: u32 = 1 << 31;
const SCHEDULER_FREQUENCY_AND_KIND_MASK: u32 = 0x0000_7fff;
const SCHEDULER_SCANNER_EVENT_KIND: u32 = 1;

/// Primary advertising channel observed by one passive scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanPrimaryChannel {
    Channel37,
    Channel38,
    Channel39,
}

impl LegacyScanPrimaryChannel {
    const fn frequency_image(self) -> u8 {
        match self {
            Self::Channel37 => 0,
            Self::Channel38 => 24,
            Self::Channel39 => 78,
        }
    }
}

/// Non-empty raw Controller window produced by the scheduler timebase.
///
/// The integers remain opaque after construction; only the private memory
/// codec can place them in a positional scanner item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanSchedulerWindow {
    start: u32,
    end: u32,
}

impl LegacyScanSchedulerWindow {
    /// Bind one wrapping Controller-tick interval.
    pub const fn from_controller_ticks(start: u32, end: u32) -> Option<Self> {
        if start == end {
            None
        } else {
            Some(Self { start, end })
        }
    }

    pub(crate) const fn start(self) -> u32 {
        self.start
    }

    pub(crate) const fn end(self) -> u32 {
        self.end
    }
}

/// Result of applying the scanner's earliest-start constraint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanStartSelection {
    Requested,
    EarliestAvailable,
}

/// Dynamic inputs to the single supported passive-scanning reset profile.
///
/// Construction fixes LE 1M, public own address, disabled privacy and
/// disabled periodic synchronization. Callers cannot supply positional
/// descriptor words or vendor option images.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanResetConfig {
    default_tx_power: LeTxPower,
    scan_type: LegacyScanType,
    filter_policy: LegacyScanFilterPolicy,
}

/// Which advertisers the scanner reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanFilterPolicy {
    /// Every advertiser.
    AcceptAll,
    /// Only the devices of the filter accept list; hardware drops the others
    /// by the published device table.
    AcceptListOnly,
}

impl LegacyScanFilterPolicy {
    /// The link-state writes of this policy: `+0x18` bit 25 and the Host's
    /// filter policy in `+0x2c` bits 25:24.
    ///
    /// SOURCE: the pinned vendor Controller's scanner link state with scan
    /// filter policy 1 on the stand (`HIL_VENDOR_ACCEPT_LIST_2026_09_29`):
    /// `+0x18` becomes `0xc200_0000` instead of `0xc000_0000` and `+0x2c`
    /// `0x0155_5555` instead of `0x0055_5555`.
    const fn apply(self, word_18: u32, word_2c: u32) -> (u32, u32) {
        match self {
            Self::AcceptAll => (word_18, word_2c),
            Self::AcceptListOnly => (
                word_18 | LINK_STATE_18_ACCEPT_LIST_FILTER,
                word_2c | 1 << LINK_STATE_2C_FILTER_POLICY_SHIFT,
            ),
        }
    }
}

/// Whether the scanner requests scan responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanType {
    /// Only listen.
    Passive,
    /// Answer scannable advertising with `SCAN_REQ`.
    Active,
}

impl LegacyScanType {
    /// Scheduler item `+0x08` bits 23:20 the vendor scanner allocator
    /// (`r_ble_lll_scan_alloc_memory`) writes for this scan type.
    pub(crate) const fn item_kind(self) -> u32 {
        match self {
            Self::Passive => 0x00c0_0000,
            Self::Active => 0x0040_0000,
        }
    }
}

impl LegacyScanResetConfig {
    /// Construct the restricted LE 1M profile of `scan_type` and
    /// `filter_policy`.
    pub const fn le_1m_public(
        default_tx_power: LeTxPower,
        scan_type: LegacyScanType,
        filter_policy: LegacyScanFilterPolicy,
    ) -> Self {
        Self {
            default_tx_power,
            scan_type,
            filter_policy,
        }
    }

    pub(super) const fn default_tx_power(self) -> LeTxPower {
        self.default_tx_power
    }

    pub(super) const fn scan_type(self) -> LegacyScanType {
        self.scan_type
    }
}

/// Raw controller-tick length of one scan window from its anchor to its end.
///
/// SOURCE: pinned `libble_app.a[ble_6.o]::r_sym_ble_M0sTWGzdUqAUyXoK849F`
/// (`r_ble_lll_scan_restart`) stores
/// `r_sched_timer_convertDiffToTicks(window end - start)` at link state
/// `+0x34` for a finite window. The open scanner always schedules finite
/// windows, so its events take that branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanWindowTicks(u32);

impl LegacyScanWindowTicks {
    /// Bind one raw window length.
    pub const fn new(ticks: u32) -> Option<Self> {
        if ticks == 0 || ticks > i32::MAX as u32 {
            None
        } else {
            Some(Self(ticks))
        }
    }
}

/// Opaque projection of the bound scanner RX head into a link-state word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LegacyScanRxHeadProjection(u32);

impl LegacyScanRxHeadProjection {
    pub(super) const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn apply(self, word: u32) -> u32 {
        (word & !RX_HEAD_MASK) | self.0
    }
}

/// Complete private link-state allocation for the first scanner profile.
///
/// Raw words never leave the memory crate. The storage layer may only install
/// this image after binding the real RX head of the same pinned graph.
#[derive(Clone, Copy)]
pub(super) struct LegacyScanLinkStateImage {
    words: [u32; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS],
}

impl LegacyScanLinkStateImage {
    /// Build the exact reset result over a zero-based open-driver allocation.
    pub(super) const fn restricted_le_1m(
        rx_head: LegacyScanRxHeadProjection,
        config: LegacyScanResetConfig,
    ) -> Self {
        let mut words = [0; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS];

        // All masks and positional images remain private to this SRAM codec.
        words[WORD_00] = 0x1ff0_0000;
        words[WORD_08] = rx_head.apply(0x4ff0_0000);
        words[WORD_0C] = 0xa010_0000;
        words[WORD_14] = 0x0400_0000;
        let (word_18, word_2c) = config.filter_policy.apply(
            0x4000_0000,
            LeCrcInit::LE_PRESET.apply_to_controller_word(0),
        );
        words[WORD_18] = word_18;
        words[WORD_24] = 0x0110_0000;
        words[WORD_2C] = word_2c;
        words[WORD_30] = 0x0000_1e00;
        // SOURCE: pinned `r_sym_ble_KkAldzIlkQuEkNQp1g6q`
        // (`r_ble_lll_scan_reset_link_state`) stores the zero tick difference
        // at `+0x34` and the power index in byte `+0x61`, not in `+0x04`.
        words[WORD_60] = LinkStateEventWord::from_word(0)
            .with_power(config.default_tx_power())
            .word();
        words[WORD_38] = LeAccessAddress::PRIMARY_ADVERTISING.controller_image();
        words[WORD_48] = 0x0000_0200;
        words[WORD_50] = 0x0300_0000;

        Self { words }
    }

    pub(super) const fn words(self) -> [u32; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS] {
        self.words
    }

    pub(super) const fn from_words(words: [u32; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS]) -> Self {
        Self { words }
    }

    /// Apply the start's write after the reset: pinned `r_ble_lll_scan_start`
    /// (`r_sym_ble_aQLjUHo25zeoHGKf9dby`) sets bit 31 of `+0x18` before it
    /// publishes the scan backoff and restarts the first window.
    pub(super) const fn started(mut self) -> Self {
        self.words[WORD_18] |= LINK_STATE_18_BIT_31;
        self
    }

    /// Apply the restart's link-state writes for one finite window: the
    /// window length at `+0x34` and a clear bit 20 of `+0x18`.
    pub(super) const fn with_window(mut self, window: LegacyScanWindowTicks) -> Self {
        self.words[WORD_34] = window.0;
        self.words[WORD_18] &= !LINK_STATE_18_BIT_20;
        self
    }

    /// The power index the event copies into the item.
    const fn power_index(self) -> u8 {
        LinkStateEventWord::from_word(self.words[WORD_60]).power_index()
    }

    #[cfg(test)]
    pub(super) const fn retains_rx_head(self, head: LegacyScanRxHeadProjection) -> bool {
        self.words[WORD_08] & RX_HEAD_MASK == head.0
    }

    #[cfg(test)]
    pub(super) const fn crc_init(self) -> LeCrcInit {
        LeCrcInit::from_controller_word(self.words[WORD_2C])
    }

    #[cfg(test)]
    pub(super) const fn access_address(self) -> LeAccessAddress {
        LeAccessAddress::from_controller_image(self.words[WORD_38])
    }

    #[cfg(test)]
    pub(super) const fn started_flag(self) -> bool {
        self.words[WORD_18] & LINK_STATE_18_BIT_31 != 0
    }

    #[cfg(test)]
    pub(super) const fn window_ticks(self) -> u32 {
        self.words[WORD_34]
    }
}

/// Complete scanner-item subset changed before common scheduler admission.
#[derive(Clone, Copy)]
pub(super) struct LegacyScanSchedulerItemWords {
    pub(super) word_00: u32,
    pub(super) word_04: u32,
    pub(super) word_14: u32,
    pub(super) word_18: u32,
    pub(super) word_38: u32,
    pub(super) raw_start_word_44: u32,
    pub(super) raw_end_word_48: u32,
}

impl LegacyScanSchedulerItemWords {
    pub(super) const fn prepare_first_event(
        mut self,
        link_state: LegacyScanLinkStateImage,
        channel: LegacyScanPrimaryChannel,
        window: LegacyScanSchedulerWindow,
        start_selection: LegacyScanStartSelection,
    ) -> Self {
        self.word_00 &= !SCHEDULER_HARDWARE_CHAIN_TIMING_MASK;
        if matches!(start_selection, LegacyScanStartSelection::EarliestAvailable) {
            self.word_00 |= SCHEDULER_HARDWARE_CHAIN_ADJUSTED_START;
        }
        self.word_00 |= SCHEDULER_HARDWARE_CHAIN_EVENT_READY;
        self.word_04 |= SCHEDULER_CONTEXT_EVENT_READY;
        self.word_14 = item_with_le_1m_power(self.word_14, link_state.power_index());
        self.word_18 = (self.word_18 & !SCHEDULER_FREQUENCY_AND_KIND_MASK)
            | ((channel.frequency_image() as u32) << 8)
            | SCHEDULER_SCANNER_EVENT_KIND;
        self.word_38 = 0;
        self.raw_start_word_44 = window.start();
        self.raw_end_word_48 = window.end();
        self
    }
}

#[cfg(test)]
mod tests;
