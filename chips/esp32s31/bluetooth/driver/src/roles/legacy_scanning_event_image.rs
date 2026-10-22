//! Pure scanner start, filter and active/passive policy.
use oer_esp32s31_hal::bluetooth::engine::{
    LeTxPower, LegacyScanPrimaryChannel, ScanItemImage, ScanItemKind, ScanResetImage,
    SchedulerEventPriority,
};
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

/// Whether the scanner requests scan responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanType {
    /// Only listen.
    Passive,
    /// Answer scannable advertising with `SCAN_REQ`.
    Active,
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

    pub const fn image(self) -> ScanResetImage {
        ScanResetImage::new(
            self.default_tx_power,
            match self.scan_type {
                LegacyScanType::Passive => ScanItemKind::Listen,
                LegacyScanType::Active => ScanItemKind::RequestResponse,
            },
            matches!(self.filter_policy, LegacyScanFilterPolicy::AcceptListOnly),
        )
    }
}
impl LegacyScanStartSelection {
    pub const fn image(self, channel: LegacyScanPrimaryChannel) -> ScanItemImage {
        ScanItemImage {
            channel,
            flexible_start: matches!(self, Self::EarliestAvailable),
            // SOURCE: pinned scanner first-event arbitration and priority-copy fields.
            priority: SchedulerEventPriority::new(1).expect("reviewed priority fits"),
            priority_copy: SchedulerEventPriority::new(0).expect("reviewed priority fits"),
        }
    }
}
