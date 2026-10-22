use super::{
    LegacyAdvertisingLinkStateWords, LegacyAdvertisingOwnAddress, LegacyAdvertisingPrimaryChannel,
    LegacyAdvertisingSchedulerItemWords,
};
use crate::bluetooth::engine::test_images::{advertising_item, priority};
use crate::bluetooth::engine::{ControllerSramLinkAddress, LeTxPower};
use crate::layouts::*;

fn zero_link_state() -> LegacyAdvertisingLinkStateWords {
    LegacyAdvertisingLinkStateWords {
        word_00: BleLinkStateTxPath::from_bits(0),
        word_04: BleLinkStateScanResponse::from_bits(0),
        word_08: BleLinkStateRxPath::from_bits(0),
        word_0c: BleLinkStateControlPolicy::from_bits(0),
        word_14: BleLinkStatePacketPolicy::from_bits(0),
        word_18: BleLinkStateReceiveContext::from_bits(0),
        word_24: BleLinkStateOwnAddressPolicy::from_bits(0),
        crc_init_word_2c: BleLinkStateCrcInitialization::from_bits(0),
        word_30: BleLinkStateSequencePolicy::from_bits(0),
        word_34: BleLinkStateEventSpan::from_bits(0),
        access_address_word_38: BleLinkStateAccessAddress::from_bits(0),
        word_3c: BleLinkStateOwnAddressLow::from_bits(0),
        word_40: BleLinkStateOwnAddressHigh::from_bits(0),
        word_50: BleLinkStateDirectionFinding::from_bits(0),
        word_60: BleLinkStateEvent::from_bits(0),
    }
}

fn zero_item() -> LegacyAdvertisingSchedulerItemWords {
    LegacyAdvertisingSchedulerItemWords {
        word_00: BleSchedulerItemHardwareNext::from_bits(0),
        word_04: BleSchedulerItemContext::from_bits(0),
        word_14: BleSchedulerItemRateAndPower::from_bits(0),
        word_18: BleSchedulerItemChannelAndPriority::from_bits(0),
        word_38: BleSchedulerItemStatus::from_bits(0),
        raw_start_word_44: BleSchedulerItemRawStart::from_bits(0),
        raw_end_word_48: BleSchedulerItemRawEnd::from_bits(0),
        word_4c: BleSchedulerItemControl::from_bits(0),
    }
}

fn reset(dbm: i8) -> LegacyAdvertisingLinkStateWords {
    zero_link_state().reset(
        ControllerSramLinkAddress::new(0x2f00_1000).expect("controller SRAM"),
        LegacyAdvertisingOwnAddress::Public,
        LeTxPower::from_dbm(dbm).expect("provider level"),
        priority(),
    )
}

#[test]
fn an_item_carries_the_power_its_link_state_was_reset_with() {
    for dbm in [-24, 0, 9, 21] {
        let link_state = reset(dbm);
        assert_eq!(
            link_state.power_index(),
            LeTxPower::from_dbm(dbm).unwrap().index()
        );
        let item = zero_item().prepare_event_item(
            link_state,
            advertising_item(LegacyAdvertisingPrimaryChannel::Channel37, false, 10, 20),
        );
        assert_eq!(
            u32::from(item.word_14.power_index()),
            u32::from(link_state.power_index())
        );
    }
}

#[test]
fn the_power_leaves_the_rate_word_untouched() {
    let low = reset(-24);
    let high = reset(21);
    assert_eq!(low.word_04, high.word_04);
    // Only the power byte of the priority word differs.
    assert_eq!(low.word_60.priority(), high.word_60.priority());
    assert_ne!(low.word_60, high.word_60);
}

#[test]
fn reset_retains_the_previous_upper_pointer_lane_and_opaque_event_lanes() {
    let mut initial = zero_link_state();
    initial.word_00 = BleLinkStateTxPath::from_bits(
        ControllerSramLinkAddress::new(0x2f04_2000)
            .expect("previous bound cursor")
            .compressed_image()
            | 0x6000_0000,
    );
    initial.word_60 = BleLinkStateEvent::from_bits(0xa5a5_0000);
    let result = initial.reset(
        ControllerSramLinkAddress::new(0x2f00_1000).expect("reset TX header"),
        LegacyAdvertisingOwnAddress::Public,
        LeTxPower::from_dbm(9).expect("provider power"),
        priority(),
    );
    assert_eq!(result.word_00.header_address(), Some(0x2f04_1000));
    assert_eq!(result.word_00.bits() & 0x6000_0000, 0x6000_0000);
    assert_eq!(result.word_60.bits() >> 16, initial.word_60.bits() >> 16);
    assert_eq!(
        result.word_60.power_index(),
        LeTxPower::from_dbm(9).expect("provider power").index()
    );
}

#[test]
fn a_detached_event_image_checks_its_complete_item_capacity() {
    let item = advertising_item(LegacyAdvertisingPrimaryChannel::Channel37, false, 10, 20);
    assert_eq!(super::AdvertisingEventImage::new(&[]), None);
    assert_eq!(super::AdvertisingEventImage::new(&[item; 4]), None);
    let accepted = super::AdvertisingEventImage::new(&[item; 3]).expect("three physical items fit");
    assert_eq!(accepted.items(), &[item; 3]);
}
