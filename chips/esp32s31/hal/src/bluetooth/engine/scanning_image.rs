//! Private SRAM images for the restricted legacy passive-scanning profile.

#![forbid(unsafe_code)]

#[cfg(test)]
use crate::bluetooth::engine::le_phy_packet::LeCrcInit;
use crate::bluetooth::engine::shared_layout::LinkStateImage;
use crate::layouts::*;

use crate::bluetooth::engine::{
    le_phy_packet::LeAccessAddress, le_tx_power::LeTxPower, sram_link::ControllerSramLink,
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
    pub(super) const fn from_bound(address: ControllerSramLink<BleBufferHeader>) -> Self {
        Self(
            BleLinkStateRxPath::ZERO
                .with_header(Some(address.reference()))
                .expect("a bound link is encodable"),
        )
    }
}

/// SOURCE: pinned `ble_6.o::r_sym_ble_KkAldzIlkQuEkNQp1g6q`
/// (`r_ble_lll_scan_reset_link_state`), current extent 0x478. RX-path codes
/// 4/c survive at +0x40e/+0x2fa; resolving-entry publication is +0x474.
/// This open restricted LE 1M profile retains its explicit policy values.
/// The current vendor option branches and +0x18/+0x48 transforms differ;
/// whole-profile equivalence is not established by those surviving facts.
const SCAN_RESET: LinkStateImage = LinkStateImage {
    tx_path: BleLinkStateTxPath::ZERO
        .with_max_tx_octets(0xff)
        .with_queue_ready(true),
    rx_path: BleLinkStateRxPath::ZERO
        .with_unconsumed_limit(0xff)
        .with_bits_28_31_opaque_constant::<0x4>(),
    control_policy: BleLinkStateControlPolicy::ZERO
        .with_bits_20_23_opaque_constant::<0x1>()
        .with_bit_29_opaque(true)
        .with_bit_31_opaque(true),
    packet_policy: BleLinkStatePacketPolicy::ZERO.with_bits_26_27_opaque_constant::<0x1>(),
    receive_context: BleLinkStateReceiveContext::ZERO.with_bit_30_opaque(true),
    own_address_policy: BleLinkStateOwnAddressPolicy::ZERO
        .with_bits_20_22_opaque_constant::<1>()
        .with_bits_24_26_opaque_constant::<0x1>(),
    crc_initialization: BleLinkStateCrcInitialization::ZERO.with_value_constant::<0x555555>(),
    sequence_policy: BleLinkStateSequencePolicy::ZERO.with_bits_9_12_opaque_constant::<0xf>(),
    access_address: BleLinkStateAccessAddress::ZERO
        .with_value(LeAccessAddress::PRIMARY_ADVERTISING.controller_image()),
    word_0048_opaque: BleLinkStateWord0048Opaque::ZERO.with_bit_9_opaque(true),
    direction_finding: BleLinkStateDirectionFinding::ZERO
        .with_common_radio_policy_constant::<0x3>(),
    ..LinkStateImage::ZERO
};

/// Complete generated scanner link-state value under CPU ownership.
#[derive(Clone, Copy)]
pub(super) struct LegacyScanLinkStateImage(pub(super) LinkStateImage);

impl LegacyScanLinkStateImage {
    pub(super) const fn restricted_le_1m(
        rx_head: LegacyScanRxHeadProjection,
        config: ScanResetImage,
    ) -> Self {
        let mut image = SCAN_RESET;
        image.rx_path = image.rx_path.with_header_from(rx_head.0);
        image.receive_context = image
            .receive_context
            .with_scan_accept_list_filter(config.accept_list);
        image.crc_initialization = image
            .crc_initialization
            .with_scan_filter_policy(config.accept_list as u8)
            .expect("a boolean fits the filter field");
        image.event = image.event.with_power_index(config.power.index());
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
        value: ScanItemImage,
        window: LegacyScanSchedulerWindow,
    ) -> Self {
        self.word_00 = self
            .word_00
            .with_flexible_start(value.flexible_start)
            .with_bit_23_opaque(true);
        self.word_04 = self.word_04.with_event_ready(true);
        self.word_14 = self
            .word_14
            .with_power_index(link_state.power_index())
            .with_phy_code_28_29_constant::<{ crate::layouts::BleSchedulerItemRateAndPower::PHY_CODE_28_29_LE_1M }>()
            .with_phy_code_30_31_constant::<{ crate::layouts::BleSchedulerItemRateAndPower::PHY_CODE_30_31_LE_1M }>();
        self.word_18 = self
            .word_18
            .with_arbitration_priority(value.priority.value())
            .expect("reviewed priority fits")
            .with_priority_copy(value.priority_copy.value())
            .expect("zero fits")
            .with_frequency(value.channel.frequency_image())
            .expect("primary channel fits");
        self.word_38 = BleSchedulerItemStatus::ZERO;
        self.raw_start_word_44 = self.raw_start_word_44.with_value(window.start());
        self.raw_end_word_48 = self.raw_end_word_48.with_value(window.end());
        self
    }
}

#[cfg(test)]
mod tests;

/// Hardware scanner item kinds admitted by this physical graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanItemKind {
    Listen,
    RequestResponse,
}
impl ScanItemKind {
    pub(crate) const fn item_kind(self) -> u8 {
        match self {
            Self::Listen => crate::layouts::BleSchedulerItemLinkState::KIND_SCAN_LISTEN,
            Self::RequestResponse => {
                crate::layouts::BleSchedulerItemLinkState::KIND_SCAN_REQUEST_RESPONSE
            }
        }
    }
}
/// Detached physical reset inputs, selected by the driver's role policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanResetImage {
    pub power: LeTxPower,
    pub kind: ScanItemKind,
    pub accept_list: bool,
}
impl ScanResetImage {
    pub const fn new(power: LeTxPower, kind: ScanItemKind, accept_list: bool) -> Self {
        Self {
            power,
            kind,
            accept_list,
        }
    }
}
/// Detached scanner item fields; no CPU layout view or pointer image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanItemImage {
    pub channel: LegacyScanPrimaryChannel,
    pub flexible_start: bool,
    pub priority: super::SchedulerEventPriority,
    pub priority_copy: super::SchedulerEventPriority,
}
