use super::*;

#[test]
fn epoch_never_overwrites_or_duplicates_the_phase_owner() {
    let config = ControlTxConfig {
        completion_timeout: oer_time::Duration::from_micros(250_000),
        poll_interval: oer_time::Duration::from_micros(1),
    };
    let mut epoch = StaTxEpoch::from_control(7_u8, config);
    assert_eq!(epoch.control(), Ok(&7));
    assert_eq!(epoch.take_control(), Ok(7));
    assert_eq!(epoch.take_control(), Err(StaTxEpochError::OwnerUnavailable));
    assert_eq!(epoch.restore_control(9), Ok(()));
    assert_eq!(
        epoch.restore_control(11),
        Err((StaTxEpochError::OwnerAlreadyPresent, 11))
    );
    assert_eq!(epoch.config(), config);
}
