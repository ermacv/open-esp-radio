//! Host requests a backend's capabilities exclude, and timed requests the
//! core cannot place within their horizon.

use bt_hci::cmd::{Opcode, OpcodeGroup};
use oer_bluetooth_radio::{
    LeCapabilities, LePhys, LeRadioCapabilities, LegacyAdvertisingTypes, RadioDuration,
};

use super::{
    Bench, OUTPUT, Request, SET_ADV_ENABLE, SET_ADV_PARAMS, SET_SCAN_ENABLE, SET_SCAN_PARAMS,
    SUCCESS, TIMING, TRANSMITTER_TEST, UNSUPPORTED,
};

const ADD_TO_ACCEPT_LIST: Opcode = Opcode::new(OpcodeGroup::LE, 0x0011);
const ENHANCED_TRANSMITTER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x0034);

/// Nonconnectable advertising, passive scanning without the accept list and
/// LE 1M tests: no connection, no responsive advertising.
#[derive(Debug, PartialEq)]
enum Narrow {}

impl LeCapabilities for Narrow {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: true,
            responsive: false,
        },
        passive_scanning: true,
        direct_test_mode: true,
        phys: LePhys::LE_1M,
        test_phys: LePhys::LE_1M,
        min_tx_power_dbm: -24,
        max_ahead: RadioDuration::from_micros(1 << 30),
        timing: TIMING,
        ..LeRadioCapabilities::NONE
    };
}

/// [`Narrow`] with a horizon of one millisecond: no advertising event of a
/// 100 ms interval is placed within it.
#[derive(Debug, PartialEq)]
enum Myopic {}

impl LeCapabilities for Myopic {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        max_ahead: RadioDuration::from_micros(1_000),
        ..Narrow::CAPS
    };
}

fn scan_parameters(active: bool, accept_list: bool) -> [u8; 7] {
    let mut parameters = [0; 7];
    parameters[0] = u8::from(active);
    parameters[1..3].copy_from_slice(&48u16.to_le_bytes());
    parameters[3..5].copy_from_slice(&48u16.to_le_bytes());
    parameters[6] = u8::from(accept_list);
    parameters
}

#[test]
fn a_scanner_the_backend_does_not_serve_is_unsupported_before_any_request() {
    let mut bench = Bench::<1, OUTPUT, Narrow>::configured();
    for (active, accept_list) in [(true, false), (false, true)] {
        assert_eq!(
            bench.command(SET_SCAN_PARAMS, &scan_parameters(active, accept_list)),
            Some(SUCCESS)
        );
        assert_eq!(bench.command(SET_SCAN_ENABLE, &[1, 0]), Some(UNSUPPORTED));
        assert_eq!(bench.step().unwrap(), None);
    }
    // The passive scanner it serves is configured.
    assert_eq!(
        bench.command(SET_SCAN_PARAMS, &scan_parameters(false, false)),
        Some(SUCCESS)
    );
    assert_eq!(bench.command(SET_SCAN_ENABLE, &[1, 0]), None);
    assert!(matches!(
        bench.step().unwrap(),
        Some(Request::ConfigureScanner(..))
    ));
}

#[test]
fn advertising_that_needs_a_connection_or_a_response_is_unsupported() {
    let mut bench = Bench::<1, OUTPUT, Narrow>::configured();
    let mut parameters = super::nonconnectable_parameters();
    // ADV_IND: connectable and scannable.
    parameters[4] = 0x00;
    assert_eq!(bench.command(SET_ADV_PARAMS, &parameters), Some(SUCCESS));
    assert_eq!(bench.command(SET_ADV_ENABLE, &[1]), Some(UNSUPPORTED));
    // ADV_SCAN_IND: scannable only, a response the backend does not send.
    parameters[4] = 0x02;
    assert_eq!(bench.command(SET_ADV_PARAMS, &parameters), Some(SUCCESS));
    assert_eq!(bench.command(SET_ADV_ENABLE, &[1]), Some(UNSUPPORTED));
    assert_eq!(bench.step().unwrap(), None);
}

#[test]
fn an_accept_list_the_backend_does_not_keep_is_unsupported() {
    let mut bench = Bench::<1, OUTPUT, Narrow>::configured();
    assert_eq!(
        bench.command(
            ADD_TO_ACCEPT_LIST,
            &[0x01, 0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1]
        ),
        Some(UNSUPPORTED)
    );
    assert_eq!(bench.step().unwrap(), None);
}

#[test]
fn a_test_on_a_phy_the_backend_does_not_serve_is_unsupported() {
    let mut bench = Bench::<1, OUTPUT, Narrow>::configured();
    // LE Coded S=8.
    assert_eq!(
        bench.command(ENHANCED_TRANSMITTER_TEST, &[0, 37, 0, 3]),
        Some(UNSUPPORTED)
    );
    assert_eq!(bench.command(TRANSMITTER_TEST, &[0, 37, 0]), Some(SUCCESS));
}

#[test]
fn an_event_past_the_horizon_is_skipped_as_a_refused_one() {
    let mut bench = Bench::<1, OUTPUT, Myopic>::configured();
    assert_eq!(
        bench.command(SET_ADV_PARAMS, &super::nonconnectable_parameters()),
        Some(SUCCESS)
    );
    bench.send(SET_ADV_ENABLE, &[1]);
    assert!(matches!(
        bench.step().unwrap(),
        Some(Request::ConfigureAdvertising(_))
    ));
    assert_eq!(bench.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    // The first event lies beyond a millisecond: nothing is submitted, and
    // the core plans again later instead of holding the radio.
    assert_eq!(bench.step().unwrap(), None);
    assert!(bench.core.wants_radio());
}

#[test]
fn the_core_needs_its_values_served_when_the_image_is_built() {
    use oer_bluetooth_radio::{LeConnectionCapabilities, LinkAcknowledgement};
    let connection = |max_data_payload| LeRadioCapabilities {
        peripheral_connection: Some(LeConnectionCapabilities {
            max_data_payload,
            link_acknowledgement: LinkAcknowledgement::Hardware,
            max_first_receive_wait: RadioDuration::from_micros(65_473),
            max_receive_wait: RadioDuration::from_micros(131_070),
        }),
        ..Narrow::CAPS
    };
    assert!(crate::serves_the_core(&Narrow::CAPS));
    assert!(crate::serves_the_core(&LeRadioCapabilities::NONE));
    // A connection carries the Data Length Extension maximum and its MIC.
    assert!(crate::serves_the_core(&connection(255)));
    assert!(!crate::serves_the_core(&connection(254)));
    // A served role needs LE 1M at 0 dBm.
    assert!(!crate::serves_the_core(&LeRadioCapabilities {
        min_tx_power_dbm: 1,
        ..Narrow::CAPS
    }));
    assert!(!crate::serves_the_core(&LeRadioCapabilities {
        phys: LePhys::NONE,
        ..Narrow::CAPS
    }));
}
