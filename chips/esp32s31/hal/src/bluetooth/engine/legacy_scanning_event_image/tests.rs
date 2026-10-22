use crate::bluetooth::engine::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    sram_link::ControllerSramLinkAddress,
};
use crate::layouts::*;

use super::{
    LegacyScanLinkStateImage, LegacyScanPrimaryChannel, LegacyScanResetConfig,
    LegacyScanRxHeadProjection, LegacyScanSchedulerItemWords, LegacyScanSchedulerWindow,
    LegacyScanStartSelection, LegacyScanWindowTicks,
};

#[test]
fn restricted_profile_retains_only_semantic_dynamic_inputs() {
    let head = LegacyScanRxHeadProjection::from_bound(
        ControllerSramLinkAddress::new(0x2f00_0100)
            .expect("the model header is a nonzero controller link"),
    );
    let config = LegacyScanResetConfig::le_1m_public(
        crate::bluetooth::engine::LeTxPower::from_dbm(0).expect("provider level"),
        crate::bluetooth::engine::LegacyScanType::Passive,
        crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll,
    );

    let image = LegacyScanLinkStateImage::restricted_le_1m(head, config);

    assert!(image.retains_rx_head(head));
    assert_eq!(image.crc_init(), LeCrcInit::LE_PRESET);
    assert_eq!(image.access_address(), LeAccessAddress::PRIMARY_ADVERTISING);
    // The reset leaves the window length at the zero tick difference.
    assert_eq!(image.window_ticks(), 0);
}

fn head() -> LegacyScanRxHeadProjection {
    LegacyScanRxHeadProjection::from_bound(
        ControllerSramLinkAddress::new(0x2f00_0100)
            .expect("the model header is a nonzero controller link"),
    )
}

#[test]
fn an_event_records_the_window_and_copies_the_reset_power() {
    for dbm in [-24, 9, 21] {
        let power = crate::bluetooth::engine::LeTxPower::from_dbm(dbm).expect("provider level");
        let image = LegacyScanLinkStateImage::restricted_le_1m(
            head(),
            LegacyScanResetConfig::le_1m_public(
                power,
                crate::bluetooth::engine::LegacyScanType::Passive,
                crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll,
            ),
        )
        .with_window(LegacyScanWindowTicks::new(20_000).unwrap());
        assert_eq!(image.window_ticks(), 20_000);
        let item = LegacyScanSchedulerItemWords {
            word_00: BleSchedulerItemHardwareNext::ZERO,
            word_04: BleSchedulerItemContext::ZERO,
            word_14: BleSchedulerItemRateAndPower::ZERO,
            word_18: BleSchedulerItemChannelAndPriority::ZERO,
            word_38: BleSchedulerItemStatus::ZERO,
            raw_start_word_44: BleSchedulerItemRawStart::ZERO,
            raw_end_word_48: BleSchedulerItemRawEnd::ZERO,
        }
        .prepare_first_event(
            image,
            LegacyScanPrimaryChannel::Channel37,
            LegacyScanSchedulerWindow::from_controller_ticks(100, 200).unwrap(),
            LegacyScanStartSelection::Requested,
        );
        assert_eq!(item.word_14.power_index(), power.index());
    }
}

#[test]
fn scan_window_ticks_are_nonempty_forward_half_range_durations() {
    assert!(LegacyScanWindowTicks::new(i32::MAX as u32).is_some());
    assert_eq!(LegacyScanWindowTicks::new(i32::MAX as u32 + 1), None);
    assert_eq!(LegacyScanWindowTicks::new(0), None);
}
