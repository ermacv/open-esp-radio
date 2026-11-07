//! Concrete physical image fixtures for graph tests, independent of driver policy.
use super::*;
pub(super) struct ChannelFixture([Option<LegacyAdvertisingPrimaryChannel>; 3]);
impl ChannelFixture {
    pub(super) fn new(a: bool, b: bool, c: bool) -> Option<Self> {
        if !a && !b && !c {
            return None;
        }
        Some(Self([
            a.then_some(LegacyAdvertisingPrimaryChannel::Channel37),
            b.then_some(LegacyAdvertisingPrimaryChannel::Channel38),
            c.then_some(LegacyAdvertisingPrimaryChannel::Channel39),
        ]))
    }
}
pub(super) fn priority() -> SchedulerEventPriority {
    SchedulerEventPriority::new(1).expect("fixture priority fits")
}
pub(super) fn advertising_item(
    channel: LegacyAdvertisingPrimaryChannel,
    flexible_start: bool,
    start: u32,
    end: u32,
) -> AdvertisingItemImage {
    AdvertisingItemImage {
        channel,
        flexible_start,
        start,
        end,
        sequence_lead: 0,
        priority: priority(),
        priority_copy: priority(),
    }
}
pub(super) fn image(
    channels: ChannelFixture,
    start: u32,
    duration: u32,
    sequence_lead: u32,
) -> AdvertisingEventImage {
    let mut values = std::vec::Vec::new();
    for channel in channels.0.into_iter().flatten() {
        let index = values.len();
        let raw_start = start.wrapping_add(duration.wrapping_mul(index as u32));
        let mut item = advertising_item(
            channel,
            index != 0,
            raw_start,
            raw_start.wrapping_add(duration),
        );
        item.sequence_lead = sequence_lead;
        values.push(item);
    }
    AdvertisingEventImage::new(&values).expect("fixture has one to three items")
}
pub(super) fn scan_item(channel: LegacyScanPrimaryChannel, flexible_start: bool) -> ScanItemImage {
    ScanItemImage {
        channel,
        flexible_start,
        priority: priority(),
        priority_copy: SchedulerEventPriority::new(0).expect("fixture priority fits"),
    }
}
