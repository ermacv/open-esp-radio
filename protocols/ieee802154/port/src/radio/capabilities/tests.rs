use super::*;
use crate::Ieee802154Instant;

#[test]
fn unknown_bits_fail_closed_and_known_sets_round_trip() {
    // The fourteen low bits are published capabilities; the wider image
    // fails closed above them.
    for bit in 0..14 {
        let image = 1_u32 << bit;
        assert!(RadioCapabilities::from_bits(image).is_ok());
    }
    assert_eq!(
        RadioCapabilities::from_bits(1 << 14),
        Err(CapabilityBitsError {
            bits: 1 << 14,
            unknown: 1 << 14
        })
    );
    assert_eq!(
        RadioCapabilities::from_bits(RadioCapabilities::TIME_SYNC.bits()),
        Ok(RadioCapabilities::TIME_SYNC)
    );
    let set = RadioCapabilities::CSMA_CA | RadioCapabilities::ENERGY_SCAN;
    assert_eq!(RadioCapabilities::from_bits(set.bits()), Ok(set));
    assert!(set.supports_tx_mode(TxMode::CsmaCa { max_backoffs: 4 }));
    assert!(!set.supports_tx_mode(TxMode::Scheduled {
        at: Ieee802154Instant::from_micros(10),
        cca: false,
    }));
}

/// A scheduled transmission with a CCA needs both capabilities.
#[test]
fn a_scheduled_cca_transmission_needs_the_cca_capability() {
    let at = Ieee802154Instant::from_micros(10);
    let scheduled = RadioCapabilities::SCHEDULED_TRANSMIT;
    assert!(scheduled.supports_tx_mode(TxMode::Scheduled { at, cca: false }));
    assert!(!scheduled.supports_tx_mode(TxMode::Scheduled { at, cca: true }));
    let with_cca = scheduled | RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT;
    assert!(with_cca.supports_tx_mode(TxMode::Scheduled { at, cca: true }));
}
