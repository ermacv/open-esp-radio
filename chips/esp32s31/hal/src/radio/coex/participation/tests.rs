use super::*;
use crate::power::radio::WifiClient;

/// A client takes part with its one state, and leaves with it.
#[test]
fn a_client_takes_part_with_its_one_state() {
    let coexisting = CoexAbsent::<WifiClient>::issued().enter();
    assert_eq!(coexisting.client(), RadioClient::Wifi);
    let _absent: CoexAbsent<WifiClient> = coexisting.leave();
    ExternalCoexistence::started().stop();
}
