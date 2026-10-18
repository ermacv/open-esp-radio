use super::*;
use crate::rx::HeGuardIntervalAndLtf;

fn completion(status: u8, detail: u8) -> TxCompletion {
    TxCompletion::new_model(TxCookie(1), status, detail)
}

#[test]
fn decoded_completion_uses_vendor_status_and_detail_dispatch() {
    assert_eq!(
        completion(0, 0).disposition(),
        TxCompletionDisposition::Success
    );
    assert_eq!(
        completion(1, 3).disposition(),
        TxCompletionDisposition::Collision
    );
    assert_eq!(
        completion(1, 2).disposition(),
        TxCompletionDisposition::Terminal(TxCompletionFailure::RtsError { detail: 2 })
    );
    assert_eq!(
        completion(2, 0xff).disposition(),
        TxCompletionDisposition::CtsTimeout
    );
    assert_eq!(
        completion(4, 0).disposition(),
        TxCompletionDisposition::CtsTimeout
    );
    assert_eq!(
        completion(4, 4).disposition(),
        TxCompletionDisposition::Collision
    );
    assert_eq!(
        completion(4, 2).disposition(),
        TxCompletionDisposition::AckTimeout
    );
    assert_eq!(
        completion(4, 0xc0).disposition(),
        TxCompletionDisposition::Terminal(TxCompletionFailure::SecurityKeyError)
    );
    assert_eq!(
        completion(5, 0).disposition(),
        TxCompletionDisposition::AckTimeout
    );
    assert_eq!(
        completion(6, 0).disposition(),
        TxCompletionDisposition::Terminal(TxCompletionFailure::InvalidStatus { status: 6 })
    );
}

#[test]
fn private_he_apep_oracle_preserves_the_complete_vendor_wrap_domain() {
    let profiles = [
        (HeGuardIntervalAndLtf::TwoLtf800Ns, 31.2_f32, 13.6_f32),
        (HeGuardIntervalAndLtf::TwoLtf1600Ns, 32.0_f32, 14.4_f32),
        (HeGuardIntervalAndLtf::FourLtf3200Ns, 40.0_f32, 16.0_f32),
    ];
    let data_bits_per_symbol = [117_i32, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
    let estimated_block_ack_us = [68_i32, 44, 44, 32, 32, 32, 32, 32, 32, 32];

    let mut wrapped = 0_u16;
    for units_32_us in 1_u16..=u16::from(u8::MAX) {
        let txop = HeEdcaTxopLimit::from_units_32_us(units_32_us).unwrap();
        for (guard_interval_and_ltf, preamble_us, symbol_us) in profiles {
            for mcs_index in 0..10 {
                let data_symbols = (((i32::from(units_32_us) * 32 - 36)
                    - estimated_block_ack_us[mcs_index])
                    as f32
                    - preamble_us)
                    / symbol_us;
                let expected = ((data_bits_per_symbol[mcs_index] as f32)
                    .mul_add(data_symbols, -22.0_f32) as i32
                    / 8) as u32;
                let rate = HeRate::new(
                    HeMcs::from_index(mcs_index as u8).unwrap(),
                    guard_interval_and_ltf,
                );
                assert_eq!(rate.vendor_unchecked_maximum_apep_bytes(txop), expected);
                if expected > i32::MAX as u32 {
                    wrapped = wrapped.saturating_add(1);
                }
            }
        }
    }
    assert_ne!(
        wrapped, 0,
        "the private oracle retains wrapped blob outputs"
    );
}
