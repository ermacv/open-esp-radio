use super::{
    DtmLinkStateReviewedWords, DtmReceiverEventPhase, DtmRole, DtmSchedulerFrequency,
    DtmSchedulerItemEventType, DtmSchedulerItemReviewedWords, DtmSchedulerReceiverPhy,
    DtmSchedulerTransmitterPhy, LeTxPower,
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
fn only_a_recurring_event_copies_the_reset_power_into_the_item() {
    let link = link_state().apply_reset(None, None, power(9), 0, DtmRole::Transmitter);
    assert_eq!(link.power_index(), power(9).index());

    let initial = item().apply_event(
        DtmSchedulerFrequency::from_controller_image(2).unwrap(),
        DtmSchedulerItemEventType::Transmitter(DtmSchedulerTransmitterPhy::Le1M),
        10,
        20,
    );
    assert_eq!(initial.power_index(), 0);
    assert_eq!(
        initial.apply_recurring_power(link).power_index(),
        power(9).index()
    );
}

#[test]
fn a_receiver_reset_stores_the_zero_tick_difference() {
    let receiver = link_state().apply_reset(None, None, power(0), 0, DtmRole::Receiver);
    assert_eq!(receiver.word_34.value(), 0);
    let transmitter = link_state().apply_reset(None, None, power(0), 0, DtmRole::Transmitter);
    assert_eq!(transmitter.word_34, link_state().word_34);
}

#[test]
fn the_sequence_start_is_stored_as_given() {
    let event = item()
        .apply_event(
            DtmSchedulerFrequency::from_controller_image(4).unwrap(),
            DtmSchedulerItemEventType::Receiver {
                phase: DtmReceiverEventPhase::Initial,
                phy: DtmSchedulerReceiverPhy::Le1M,
            },
            100,
            300,
        )
        .apply_sequence_start(101);
    assert_eq!(event.word_0c.value(), 101);
    assert_eq!(event.word_44.value(), 100);
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
            for event_type in [
                DtmSchedulerItemEventType::Transmitter(DtmSchedulerTransmitterPhy::Le1M),
                DtmSchedulerItemEventType::Receiver {
                    phase: DtmReceiverEventPhase::Initial,
                    phy: DtmSchedulerReceiverPhy::Le1M,
                },
                DtmSchedulerItemEventType::Receiver {
                    phase: DtmReceiverEventPhase::Recurring,
                    phy: DtmSchedulerReceiverPhy::LeCoded,
                },
            ] {
                let mut input = item();
                input.word_18 = BleSchedulerItemChannelAndPriority::from_bits(seed);
                let actual = input
                    .apply_event(frequency, event_type, 10, 20)
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
