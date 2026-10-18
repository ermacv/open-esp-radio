use super::*;
use oer_ieee80211_mac::ftm::{FtmBurstDuration, FtmFormatAndBandwidth, FtmRequestParameters};
use oer_ieee80211_sta::ftm::{FtmRequester, FtmRequesterConfig, FtmRequesterService};

fn transmission() -> FtmRequestTransmission {
    let parameters = FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        4,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap();
    let config = FtmRequesterConfig::new(
        parameters,
        oer_time::Duration::from_micros(1_000),
        oer_time::Duration::from_micros(100),
        oer_time::Duration::from_micros(10_000),
        1,
    )
    .unwrap();
    let mut requester = FtmRequester::<3>::new(config);
    requester
        .start([1; 6], oer_time::Instant::from_micros(0))
        .unwrap();
    let FtmRequesterService::Transmit(transmission) = requester
        .service(oer_time::Instant::from_micros(0))
        .unwrap()
    else {
        panic!("new FTM request must be ready")
    };
    transmission
}

#[test]
fn production_admission_stops_before_phy_or_publication() {
    assert_eq!(
        station_ftm_request_frontier(&transmission()),
        StationFtmHardwareError::Unsupported {
            reached: StationFtmHardwareStage::PortableInitialRequestValidated,
            missing: StationFtmUnsupportedStage::RuntimePhyOwnerBinding,
        }
    );
    let frontier = station_ftm_hardware_frontier();
    assert_eq!(
        frontier.phy_enable_leaf,
        StationFtmFrontierStatus::ReversibleSourceTransaction
    );
    assert_eq!(
        frontier.advertised_initiator_capability,
        StationFtmFrontierStatus::IntentionallyDisabled
    );
    assert_eq!(
        frontier.distance_estimate,
        StationFtmFrontierStatus::IntentionallyDisabled
    );
}
