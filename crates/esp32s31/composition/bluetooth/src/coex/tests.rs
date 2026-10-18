use oer_bluetooth_radio::RadioActivity;

use super::{
    BLE_COEX_STATUS_ADVERTISING, BLE_COEX_STATUS_CONNECTED, BLE_COEX_STATUS_SCANNING,
    BleCoexStatusChange,
};

const ADVERTISING: RadioActivity = RadioActivity {
    advertising: true,
    ..RadioActivity::IDLE
};
const CONNECTED: RadioActivity = RadioActivity {
    connected: true,
    ..RadioActivity::IDLE
};

#[test]
fn each_role_publishes_its_own_status_bit() {
    for (activity, bit) in [
        (ADVERTISING, BLE_COEX_STATUS_ADVERTISING),
        (
            RadioActivity {
                scanning: true,
                ..RadioActivity::IDLE
            },
            BLE_COEX_STATUS_SCANNING,
        ),
        (CONNECTED, BLE_COEX_STATUS_CONNECTED),
    ] {
        assert_eq!(
            BleCoexStatusChange::between(RadioActivity::IDLE, activity),
            BleCoexStatusChange { clear: 0, set: bit }
        );
        assert_eq!(
            BleCoexStatusChange::between(activity, RadioActivity::IDLE),
            BleCoexStatusChange { clear: bit, set: 0 }
        );
    }
}

#[test]
fn a_connection_from_advertising_moves_only_the_changed_bits() {
    let both = RadioActivity {
        advertising: true,
        connected: true,
        ..RadioActivity::IDLE
    };
    // Advertising ends in a connection: the connection bit appears before
    // the advertising bit is withdrawn when both summaries are seen.
    assert_eq!(
        BleCoexStatusChange::between(ADVERTISING, both),
        BleCoexStatusChange {
            clear: 0,
            set: BLE_COEX_STATUS_CONNECTED
        }
    );
    assert_eq!(
        BleCoexStatusChange::between(both, CONNECTED),
        BleCoexStatusChange {
            clear: BLE_COEX_STATUS_ADVERTISING,
            set: 0
        }
    );
    // An unchanged summary changes nothing.
    assert_eq!(
        BleCoexStatusChange::between(both, both),
        BleCoexStatusChange { clear: 0, set: 0 }
    );
}
