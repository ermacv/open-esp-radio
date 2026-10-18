use super::*;

fn series(power_dbm: i8, frame_rssi_dbm: &[i8], live_rssi_dbm: i8) -> Series {
    Series {
        power_dbm,
        frame_rssi_dbm: frame_rssi_dbm.to_vec(),
        live_rssi_dbm,
    }
}

fn following() -> Measurements {
    Measurements {
        high: series(20, &[-30, -31, -29], -29),
        low: series(-15, &[-62, -63, -61], -60),
    }
}

#[test]
fn a_live_read_that_follows_the_last_frame_passes() {
    let summary = evaluate(&following()).unwrap();
    assert_eq!(summary.high_last_frame_dbm, -29);
    assert_eq!(summary.low_live_dbm, -60);
}

#[test]
fn each_miss_of_the_live_read_fails_the_cell() {
    type Change = fn(&mut Measurements);
    let cases: [(&str, Change); 5] = [
        ("stale strong read", |m| m.high.live_rssi_dbm = -60),
        ("stale weak read", |m| m.low.live_rssi_dbm = -29),
        ("constant read", |m| {
            m.high.live_rssi_dbm = -45;
            m.low.live_rssi_dbm = -45;
            m.high.frame_rssi_dbm = vec![-45];
            m.low.frame_rssi_dbm = vec![-45];
        }),
        ("no strong frame", |m| m.high.frame_rssi_dbm.clear()),
        ("no weak frame", |m| m.low.frame_rssi_dbm.clear()),
    ];
    for (case, change) in cases {
        let mut measurements = following();
        change(&mut measurements);
        assert!(evaluate(&measurements).is_err(), "{case} passed");
    }
}
