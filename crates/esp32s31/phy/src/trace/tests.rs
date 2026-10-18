use super::*;

use oer_phy_trace::{Client, Refusal};

use crate::state::client::RadioClient;

#[test]
fn every_client_keeps_its_identity() {
    assert_eq!(client(RadioClient::Wifi), Client::Wifi);
    assert_eq!(client(RadioClient::Bluetooth), Client::Bluetooth);
    assert_eq!(client(RadioClient::Ieee802154), Client::Ieee802154);
}

#[test]
fn refusals_name_the_client_they_concern() {
    assert_eq!(
        refusal(ConcurrentPhyError::ClientAbsent(RadioClient::Ieee802154)),
        Refusal::ClientAbsent(Client::Ieee802154)
    );
    assert_eq!(refusal(ConcurrentPhyError::RfClosed), Refusal::RfClosed);
}

#[test]
fn calibration_paths_keep_the_cache_outcome() {
    assert_eq!(
        calibration_path(PhyCalibrationPath::FullAfterRejectedCache),
        CalibrationPath::FullAfterRejectedCache
    );
    assert_eq!(
        calibration_path(PhyCalibrationPath::PartialFromCache),
        CalibrationPath::PartialFromCache
    );
}
