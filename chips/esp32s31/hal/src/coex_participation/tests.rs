use super::*;
use crate::clock::{BluetoothClient, WifiClient};

/// The second participant starts coexistence, whatever it is, and a client
/// takes part with its one state.
#[test]
fn participants_follow_the_client_states() {
    let mut participants = CoexParticipants::default();
    let (wifi, count) = participants.enter_client(CoexAbsent::<WifiClient>::issued());
    assert_eq!(count, 1);
    let (external, count) = participants.enter_external();
    assert_eq!(count, 2);
    let (bluetooth, count) = participants.enter_client(CoexAbsent::<BluetoothClient>::issued());
    assert_eq!(count, 3);
    assert_eq!(wifi.client(), RadioClient::Wifi);

    let (_wifi, count) = participants.leave_client(wifi);
    assert_eq!(count, 2);
    assert_eq!(participants.leave_external(external), 1);
    let (_bluetooth, count) = participants.leave_client(bluetooth);
    assert_eq!(count, 0);
}
