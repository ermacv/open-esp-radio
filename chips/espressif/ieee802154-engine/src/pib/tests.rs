//! PIB behavior read from the pinned `esp_ieee802154_pib.c`.
use crate::{channel::*, tx_power::*, types::*};
use std::vec::Vec;

use super::{AutoPendingMode, Ieee802154MultipanIndex, Ieee802154Pib, Ieee802154PibDefaults};

use crate::engine::tests::{Call, Hw};

const LEVELS: [i8; 4] = [-9, -3, 4, 10];

fn levels() -> Ieee802154TxPowerLevels<'static> {
    Ieee802154TxPowerLevels::new(&LEVELS).unwrap()
}

fn channel(number: u8) -> Ieee802154Channel {
    Ieee802154Channel::new(number).unwrap()
}

fn published(pib: &mut Ieee802154Pib) -> Vec<Call> {
    let mut ll = Hw::default();
    pib.update(&mut ll, levels());
    ll.calls
}

/// `ieee802154_pib_init` then `ieee802154_pib_update`: the defaults are
/// published in the vendor order with the highest provider power
/// (esp_ieee802154_pib.c `ieee802154_pib_init`, `ieee802154_pib_update`).
#[test]
fn initial_pib_publishes_the_vendor_defaults_in_order() {
    let mut pib = Ieee802154Pib::new(
        Ieee802154PibDefaults::default(),
        levels(),
        super::test_permit(11, levels()),
    );
    assert!(pib.is_pending());
    assert!(!pib.rx_when_idle());
    assert_eq!(pib.power_table(), [10; 16]);
    assert_eq!(
        published(&mut pib),
        [
            Call::SetChannel(11),
            Call::SetTxPower(3),
            Call::SetCcaMode(Ieee802154CcaMode::EnergyDetection),
            Call::SetCcaThreshold(-75),
            Call::SetTxAutoAck(true),
            Call::SetRxAutoAck(true),
            Call::SetTxEnhancedAck(true),
            Call::SetCoordinator(false),
            Call::SetPromiscuous(true),
            Call::SetPendingMode(false),
        ]
    );
    assert!(!pib.is_pending());
    assert_eq!(published(&mut pib), []);
}

/// Setters mark the PIB pending only when the value changes; receive-when-idle
/// never does.
#[test]
fn only_a_changed_hardware_value_marks_the_pib_pending() {
    let mut pib = Ieee802154Pib::new(
        Ieee802154PibDefaults::default(),
        levels(),
        super::test_permit(11, levels()),
    );
    let _ = published(&mut pib);

    pib.set_channel(super::test_permit(11, levels()));
    pib.set_promiscuous(true);
    pib.set_cca_threshold(-75);
    pib.set_power(10);
    pib.set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, AutoPendingMode::Disable);
    pib.set_rx_when_idle(true);
    assert!(!pib.is_pending());
    assert!(pib.rx_when_idle());

    pib.set_coordinator(true);
    assert!(pib.is_pending());
    assert!(published(&mut pib).contains(&Call::SetCoordinator(true)));
}

/// The published power is the requested power of the current channel, resolved
/// by the provider floor scan.
#[test]
fn power_follows_the_current_channel() {
    let mut pib = Ieee802154Pib::new(
        Ieee802154PibDefaults::default(),
        levels(),
        super::test_permit(11, levels()),
    );
    pib.set_power_for_channel(channel(20), 0);
    pib.set_channel(super::test_permit(20, levels()));
    assert_eq!(pib.power(), 0);
    let calls = published(&mut pib);
    assert!(calls.contains(&Call::SetChannel(20)));
    assert!(calls.contains(&Call::SetTxPower(1)));
}

/// The one-bit hardware selector is set when any interface uses the enhanced
/// or Zigbee pending mode.
#[test]
fn enhanced_or_zigbee_pending_on_any_interface_selects_the_enhanced_lookup() {
    for (mode, enhanced) in [
        (AutoPendingMode::Disable, false),
        (AutoPendingMode::Enable, false),
        (AutoPendingMode::Enhanced, true),
        (AutoPendingMode::Zigbee, true),
    ] {
        let mut pib = Ieee802154Pib::new(
            Ieee802154PibDefaults::default(),
            levels(),
            super::test_permit(11, levels()),
        );
        pib.set_pending_mode(Ieee802154MultipanIndex::CONTEXT2, mode);
        assert_eq!(pib.pending_mode(Ieee802154MultipanIndex::CONTEXT2), mode);
        assert_eq!(
            published(&mut pib).last(),
            Some(&Call::SetPendingMode(enhanced))
        );
    }
}

/// The defaults accept a CCA threshold of the Kconfig range, its ends
/// included, and refuse one outside it.
#[test]
fn defaults_hold_a_threshold_of_the_kconfig_range() {
    for threshold in [-120, -75, 0] {
        let defaults = Ieee802154PibDefaults::new(Ieee802154CcaMode::Carrier, threshold).unwrap();
        assert_eq!(defaults.cca_threshold_dbm(), threshold);
        assert_eq!(defaults.cca_mode(), Ieee802154CcaMode::Carrier);
    }
    for threshold in [-121, 1] {
        assert_eq!(
            Ieee802154PibDefaults::new(Ieee802154CcaMode::Carrier, threshold),
            Err(oer_ieee802154_mac::CcaThresholdError::OutOfRange {
                requested_dbm: threshold,
                lowest_dbm: -120,
                highest_dbm: 0,
            })
        );
    }
    assert_eq!(Ieee802154PibDefaults::default().cca_threshold_dbm(), -75);
}

/// The port publishes the provider levels it resolves power through.
#[test]
fn provider_levels_are_published_as_the_port_levels() {
    assert_eq!(levels().portable().levels_dbm(), &LEVELS);
}

fn permit_with_ceiling(
    number: u8,
    ceiling_mbm: i32,
) -> Result<super::Ieee802154Permit, super::CeilingBelowLowestLevel> {
    let permitted = oer_radio_regulatory::TEST_DOMAIN
        .permit(channel(number))
        .unwrap();
    let ceiling = permitted.tx_ceiling(oer_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: Some(ceiling_mbm),
    });
    super::Ieee802154Permit::new(permitted, ceiling, levels())
}

/// The ceiling bounds the requested power: a request above it radiates the
/// ceiling's level, and a new channel brings its own ceiling.
#[test]
fn the_ceiling_bounds_the_published_power() {
    let mut pib = Ieee802154Pib::new(
        Ieee802154PibDefaults::default(),
        levels(),
        permit_with_ceiling(11, 500).unwrap(),
    );
    assert_eq!(pib.power(), 10);
    assert_eq!(pib.ceiling_dbm(), 5);
    // The provider level at or below 5 dBm is 4 (field code 2).
    assert!(published(&mut pib).contains(&Call::SetTxPower(2)));
    pib.set_channel(permit_with_ceiling(12, 1000).unwrap());
    assert!(pib.is_pending());
    assert!(published(&mut pib).contains(&Call::SetTxPower(3)));
}

/// A ceiling under the provider's lowest level cannot be honoured: the
/// permit is refused instead of radiating the lowest level.
#[test]
fn a_ceiling_below_the_lowest_level_is_refused() {
    assert_eq!(
        permit_with_ceiling(11, -1000).unwrap_err(),
        super::CeilingBelowLowestLevel {
            ceiling_dbm: -10,
            lowest_dbm: -9,
        }
    );
}
