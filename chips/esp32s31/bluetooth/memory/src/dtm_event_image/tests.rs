use super::{
    DtmLinkStateReviewedWords, DtmReceiverEventPhase, DtmRole, DtmSchedulerItemEventType,
    DtmSchedulerItemReviewedWords, DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy, LeTxPower,
};
use crate::le_phy_packet::{LeAccessAddress, LeCrcInit};

fn link_state() -> DtmLinkStateReviewedWords {
    DtmLinkStateReviewedWords {
        word_00: 0,
        word_08: 0,
        profile_word_14: super::DtmLinkStateProfileWord::from_storage(0),
        crc_init: LeCrcInit::LE_PRESET,
        word_30: 0,
        word_34: 0x1234_5678,
        access_address: LeAccessAddress::DIRECT_TEST_MODE,
        word_50: 0,
        word_60: 0,
    }
}

fn item() -> DtmSchedulerItemReviewedWords {
    DtmSchedulerItemReviewedWords {
        word_00: 0,
        word_04: 0,
        word_08: 0,
        word_0c: 0,
        word_14: 0,
        word_18: 0,
        word_2c: 0,
        word_44: 0,
        word_48: 0,
        word_4c: 0,
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
        2,
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
    assert_eq!(receiver.word_34, 0);
    let transmitter = link_state().apply_reset(None, None, power(0), 0, DtmRole::Transmitter);
    assert_eq!(transmitter.word_34, link_state().word_34);
}

#[test]
fn the_sequence_start_is_stored_as_given() {
    let event = item()
        .apply_event(
            4,
            DtmSchedulerItemEventType::Receiver {
                phase: DtmReceiverEventPhase::Initial,
                phy: DtmSchedulerReceiverPhy::Le1M,
            },
            100,
            300,
        )
        .apply_sequence_start(101);
    assert_eq!(event.word_0c, 101);
    assert_eq!(event.word_44, 100);
}
