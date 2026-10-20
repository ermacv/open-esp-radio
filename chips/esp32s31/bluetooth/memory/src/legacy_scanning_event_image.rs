//! Private SRAM images for the restricted legacy passive-scanning profile.

#![forbid(unsafe_code)]

#[cfg(test)]
use crate::le_phy_packet::LeCrcInit;
use crate::shared_layout::LinkStateImage;
use oer_esp32s31_hal::layouts::*;

use crate::{
    le_phy_packet::LeAccessAddress, le_tx_power::LeTxPower, sram_link::ControllerSramLinkAddress,
};

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
    const fn apply(
        self,
        context: BleLinkStateReceiveContext,
        crc: BleLinkStateCrcInitialization,
    ) -> (BleLinkStateReceiveContext, BleLinkStateCrcInitialization) {
        match self {
            Self::AcceptAll => (context, crc),
            Self::AcceptListOnly => (
                context.with_scan_accept_list_filter(true),
                crc.with_scan_filter_policy(1)
                    .expect("reviewed filter fits"),
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
    pub(crate) const fn item_kind(self) -> u8 {
        match self {
            Self::Passive => 12,
            Self::Active => 4,
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
pub(super) struct LegacyScanRxHeadProjection(BleLinkStateRxPath);

impl LegacyScanRxHeadProjection {
    pub(super) const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(BleLinkStateRxPath::from_bits(address.compressed_image()))
    }
}

/// SOURCE: pinned `ble_6.o::r_sym_ble_KkAldzIlkQuEkNQp1g6q`
/// (`r_ble_lll_scan_reset_link_state`), the restricted LE 1M reset image.
const SCAN_RESET: LinkStateImage = LinkStateImage {
    tx_path: BleLinkStateTxPath::ZERO
        .with_max_tx_octets(0xff)
        .with_queue_ready(true),
    rx_path: BleLinkStateRxPath::ZERO
        .with_unconsumed_limit(0xff)
        .with_bits_28_31_opaque(0x4)
        .expect("reviewed field image fits"),
    control_policy: BleLinkStateControlPolicy::ZERO
        .with_bits_20_23_opaque(0x1)
        .expect("reviewed field image fits")
        .with_bit_29_opaque(true)
        .with_bit_31_opaque(true),
    packet_policy: BleLinkStatePacketPolicy::ZERO
        .with_bits_26_27_opaque(0x1)
        .expect("reviewed field image fits"),
    receive_context: BleLinkStateReceiveContext::ZERO.with_bit_30_opaque(true),
    own_address_policy: BleLinkStateOwnAddressPolicy::ZERO
        .with_bits_16_27_opaque(0x110)
        .expect("reviewed field image fits"),
    crc_initialization: BleLinkStateCrcInitialization::ZERO
        .with_value(0x555555)
        .expect("reviewed field image fits"),
    sequence_policy: BleLinkStateSequencePolicy::ZERO
        .with_bits_9_12_opaque(0xf)
        .expect("reviewed field image fits"),
    access_address: BleLinkStateAccessAddress::ZERO
        .with_value(LeAccessAddress::PRIMARY_ADVERTISING.controller_image()),
    word_0048_opaque: BleLinkStateWord0048Opaque::ZERO.with_bit_9_opaque(true),
    direction_finding: BleLinkStateDirectionFinding::ZERO
        .with_common_radio_policy(0x3)
        .expect("reviewed field image fits"),
    ..LinkStateImage::ZERO
};

/// Complete generated scanner link-state value under CPU ownership.
#[derive(Clone, Copy)]
pub(super) struct LegacyScanLinkStateImage(pub(super) LinkStateImage);

impl LegacyScanLinkStateImage {
    pub(super) const fn restricted_le_1m(
        rx_head: LegacyScanRxHeadProjection,
        config: LegacyScanResetConfig,
    ) -> Self {
        let mut image = SCAN_RESET;
        image.rx_path = rx_head
            .0
            .with_unconsumed_limit(image.rx_path.unconsumed_limit())
            .with_bits_28_31_opaque(image.rx_path.bits_28_31_opaque())
            .expect("retained image fits");
        let (context, crc) = config
            .filter_policy
            .apply(image.receive_context, image.crc_initialization);
        image.receive_context = context;
        image.crc_initialization = crc;
        image.event = image
            .event
            .with_power_index(config.default_tx_power().index());
        Self(image)
    }

    pub(super) const fn started(mut self) -> Self {
        self.0.receive_context = self.0.receive_context.with_scan_started(true);
        self
    }

    pub(super) const fn with_window(mut self, window: LegacyScanWindowTicks) -> Self {
        self.0.event_span = self.0.event_span.with_value(window.0);
        self.0.receive_context = self.0.receive_context.with_bit_20_opaque(false);
        self
    }

    const fn power_index(self) -> u8 {
        self.0.event.power_index()
    }
    #[cfg(test)]
    pub(super) const fn retains_rx_head(self, head: LegacyScanRxHeadProjection) -> bool {
        self.0.rx_path.header() == head.0.header()
    }
    #[cfg(test)]
    pub(super) const fn crc_init(self) -> LeCrcInit {
        LeCrcInit::from_controller_word(self.0.crc_initialization.value())
    }
    #[cfg(test)]
    pub(super) const fn access_address(self) -> LeAccessAddress {
        LeAccessAddress::from_controller_image(self.0.access_address.value())
    }
    #[cfg(test)]
    pub(super) const fn started_flag(self) -> bool {
        self.0.receive_context.scan_started()
    }
    #[cfg(test)]
    pub(super) const fn window_ticks(self) -> u32 {
        self.0.event_span.value()
    }
}

/// Complete scanner-item subset changed before common scheduler admission.
#[derive(Clone, Copy)]
pub(super) struct LegacyScanSchedulerItemWords {
    pub(super) word_00: BleSchedulerItemHardwareNext,
    pub(super) word_04: BleSchedulerItemContext,
    pub(super) word_14: BleSchedulerItemRateAndPower,
    pub(super) word_18: BleSchedulerItemChannelAndPriority,
    pub(super) word_38: BleSchedulerItemStatus,
    pub(super) raw_start_word_44: BleSchedulerItemRawStart,
    pub(super) raw_end_word_48: BleSchedulerItemRawEnd,
}

impl LegacyScanSchedulerItemWords {
    pub(super) const fn prepare_first_event(
        mut self,
        link_state: LegacyScanLinkStateImage,
        channel: LegacyScanPrimaryChannel,
        window: LegacyScanSchedulerWindow,
        start_selection: LegacyScanStartSelection,
    ) -> Self {
        self.word_00 = self
            .word_00
            .with_flexible_start(matches!(
                start_selection,
                LegacyScanStartSelection::EarliestAvailable
            ))
            .with_bit_23_opaque(true);
        self.word_04 = self.word_04.with_event_ready(true);
        self.word_14 = self
            .word_14
            .with_power_index(link_state.power_index())
            .with_bits_28_29_opaque(0)
            .expect("zero fits")
            .with_bits_30_31_opaque(0)
            .expect("zero fits");
        self.word_18 = self
            .word_18
            .with_arbitration_priority(1)
            .expect("reviewed priority fits")
            .with_bits_4_7_opaque(0)
            .expect("zero fits")
            .with_frequency(channel.frequency_image())
            .expect("primary channel fits");
        self.word_38 = BleSchedulerItemStatus::ZERO;
        self.raw_start_word_44 = self.raw_start_word_44.with_value(window.start());
        self.raw_end_word_48 = self.raw_end_word_48.with_value(window.end());
        self
    }
}

#[cfg(test)]
mod tests;
