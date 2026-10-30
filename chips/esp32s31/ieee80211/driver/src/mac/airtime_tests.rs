use core::num::NonZeroU32;
use oer_ieee80211_upper_mac::airtime::{AirtimeCandidate, AirtimeError, AirtimeStorage};

use crate::mac::tx_work::MacTxWork;

fn us(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap()
}

fn candidate(key: u8, minimum: u32) -> AirtimeCandidate<u8> {
    AirtimeCandidate {
        key,
        minimum_micros: us(minimum),
    }
}

mod queues;

#[test]
fn unknown_receipt_keeps_the_reservation_until_an_explicit_charge_is_supplied() {
    let mut storage = AirtimeStorage::<_, 1, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let reservation = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap();
    let mut receipt = MacTxWork::new();
    receipt.record(1000, 1, None);
    let completion = reservation.published().completed(receipt);
    assert_eq!(completion.key(), 1);
    assert_eq!(completion.reserved_micros(), us(1000));
    assert!(completion.work().estimated_exchange_micros(0).is_none());
    assert!(matches!(
        scheduler.reserve([], &mut ()),
        Err(AirtimeError::ReservationCapacity)
    ));
    assert_eq!(scheduler.balance_micros(1), Some(0));
    // Synthetic caller policy for this ownership test, not a PHY estimate.
    let retained = scheduler.settle(completion, us(1700), &mut ()).unwrap();
    assert_eq!(retained, receipt);
    assert_eq!(scheduler.balance_micros(1), Some(-700));
}
