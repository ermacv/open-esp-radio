#[test]
fn home_channel_requires_one_valid_explicit_observation() {
    assert_eq!(
        super::home_channel("oer-vendor-calibration-channel 6\n").unwrap(),
        6
    );
    for bad in [
        "",
        "oer-vendor-calibration-channel 0\n",
        "oer-vendor-calibration-channel 15\n",
        "oer-vendor-calibration-channel x\n",
        "oer-vendor-calibration-channel 1\noer-vendor-calibration-channel 6\n",
    ] {
        assert!(super::home_channel(bad).is_err());
    }
}
