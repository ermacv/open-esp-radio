use super::{
    DtmItemConfiguration, DtmLinkStateReviewedWords, DtmReceiveWait, DtmSchedulerFrequency,
    DtmSchedulerItemReviewedWords, LeTxPower,
};
use crate::bluetooth::engine::le_phy_packet::{LeAccessAddress, LeCrcInit};
use crate::layouts::*;

fn link_state() -> DtmLinkStateReviewedWords {
    DtmLinkStateReviewedWords {
        word_00: BleLinkStateTxPath::from_bits(0),
        word_08: BleLinkStateRxPath::from_bits(0),
        profile_word_14: super::DtmLinkStateProfileWord::from_storage(0),
        crc_init: LeCrcInit::LE_PRESET,
        word_30: BleLinkStateSequencePolicy::from_bits(0),
        word_34: BleLinkStateEventSpan::from_bits(0x1234_5678),
        access_address: LeAccessAddress::DIRECT_TEST_MODE,
        word_50: BleLinkStateDirectionFinding::from_bits(0),
        word_60: BleLinkStateEvent::from_bits(0),
    }
}

fn item() -> DtmSchedulerItemReviewedWords {
    DtmSchedulerItemReviewedWords {
        word_00: BleSchedulerItemHardwareNext::from_bits(0),
        word_04: BleSchedulerItemContext::from_bits(0),
        word_08: BleSchedulerItemLinkState::from_bits(0),
        word_0c: BleSchedulerItemSequenceStart::from_bits(0),
        word_14: BleSchedulerItemRateAndPower::from_bits(0),
        word_18: BleSchedulerItemChannelAndPriority::from_bits(0),
        word_2c: BleSchedulerItemReceiveWait::from_bits(0),
        word_44: BleSchedulerItemRawStart::from_bits(0),
        word_48: BleSchedulerItemRawEnd::from_bits(0),
        word_4c: BleSchedulerItemControl::from_bits(0),
    }
}

fn power(dbm: i8) -> LeTxPower {
    LeTxPower::from_dbm(dbm).expect("provider level")
}

#[test]
fn frequency_domain_accepts_127_and_rejects_128() {
    assert!(DtmSchedulerFrequency::from_controller_image(127).is_some());
    assert_eq!(DtmSchedulerFrequency::from_controller_image(128), None);
    assert_eq!(DtmSchedulerFrequency::from_controller_image(255), None);
}

#[test]
fn frequency_updates_match_the_vendor_mask_and_preserve_all_high_17_bits() {
    // SOURCE: dtm_4.o sym_dtm_2zeOUjc7g55zkDNQuZkg +0x6e..+0x80
    // and sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1 +0x74..+0x8a:
    // fresh load, AND 0xffff80ff, OR the frequency shifted by eight.
    for seed in [0, u32::MAX, 0xaaaa_aaf5, 0x5555_55fa] {
        for image in 0..=127 {
            let frequency = DtmSchedulerFrequency::from_controller_image(image).unwrap();
            for configuration in [
                DtmItemConfiguration::new(
                    true,
                    0,
                    None,
                    crate::bluetooth::engine::SchedulerEventPriority::new(3)
                        .expect("fixture priority fits"),
                )
                .expect("test PHY fits"),
                DtmItemConfiguration::new(
                    false,
                    0,
                    Some(DtmReceiveWait::new(1, 15).expect("test timing fits")),
                    crate::bluetooth::engine::SchedulerEventPriority::new(3)
                        .expect("fixture priority fits"),
                )
                .expect("test PHY fits"),
                DtmItemConfiguration::new(
                    false,
                    2,
                    Some(DtmReceiveWait::new(1, 0).expect("test timing fits")),
                    crate::bluetooth::engine::SchedulerEventPriority::new(3)
                        .expect("fixture priority fits"),
                )
                .expect("test PHY fits"),
            ] {
                let mut input = item();
                input.word_18 = BleSchedulerItemChannelAndPriority::from_bits(seed);
                let actual = input
                    .apply_event(frequency, configuration, 10, 20)
                    .word_18
                    .bits();
                let priority_image = (seed & !0xf) | 3;
                let vendor = (priority_image & 0xffff_80ff) | (u32::from(image) << 8);
                assert_eq!(actual, vendor, "seed={seed:#010x}, frequency={image}");
                assert_eq!(actual & 0xffff_8000, seed & 0xffff_8000);
            }
        }
    }
}

#[test]
fn clearing_the_span_is_an_explicit_physical_operation() {
    let original = link_state();
    let cleared = original.apply_reset(None, None, power(0), 0, true);
    let retained = original.apply_reset(None, None, power(0), 0, false);
    assert_eq!(cleared.event_span(), 0);
    assert_eq!(retained.event_span(), original.event_span());
}

#[test]
fn physical_inputs_reject_out_of_range_phy_timing_and_priority() {
    let priority =
        crate::bluetooth::engine::SchedulerEventPriority::new(3).expect("fixture priority");
    assert!(DtmItemConfiguration::new(true, 3, None, priority).is_some());
    assert_eq!(DtmItemConfiguration::new(true, 4, None, priority), None);
    assert_eq!(DtmReceiveWait::new(1, 32), None);
    assert_eq!(
        crate::bluetooth::engine::SchedulerEventPriority::new(16),
        None
    );
}
