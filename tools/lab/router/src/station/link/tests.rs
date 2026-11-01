use super::*;

#[test]
fn an_ht40_link_reads_every_counter_both_rates_the_width_and_the_survey() {
    let link = parse_link(include_str!("ht40.txt")).unwrap();
    assert_eq!(
        (
            link.rx_bytes,
            link.rx_packets,
            link.tx_bytes,
            link.tx_packets
        ),
        (9_214_632, 6603, 41_023_987, 27_931)
    );
    assert_eq!((link.tx_retries, link.tx_failed), (1204, 3));
    assert_eq!(link.rx_drop_misc, Some(17));
    assert_eq!(
        (link.rx_duration_us, link.tx_duration_us),
        (Some(801_233), Some(2_011_483))
    );
    assert_eq!(link.signal_avg_dbm, Some(-39));
    let tx = link.tx_bitrate.as_ref().unwrap();
    assert_eq!(
        (tx.kbit_per_s, tx.format, tx.mcs, tx.width_mhz, tx.short_gi),
        (Some(150_000), RateFormat::Ht, Some(7), 40, true)
    );
    assert_eq!(tx.text, "150.0 MBit/s MCS 7 40MHz short GI");
    let rx = link.rx_bitrate.as_ref().unwrap();
    assert_eq!(
        (rx.kbit_per_s, rx.mcs, rx.width_mhz, rx.short_gi),
        (Some(65_000), Some(7), 20, false)
    );
    assert_eq!(
        (link.channel, link.frequency_mhz, link.width_mhz),
        (Some(13), Some(2472), Some(40))
    );
    assert_eq!(
        link.survey,
        Some(Survey {
            frequency_mhz: 2472,
            noise_dbm: Some(-91),
            active_ms: Some(1_931_882),
            busy_ms: Some(95_312),
            receive_ms: Some(40_121),
            bss_receive_ms: Some(1311),
            transmit_ms: Some(21_843),
        })
    );
    assert_eq!(link.tid0_aqm_drops, Some(4));
    let json = serde_json::to_value(&link).unwrap();
    assert_eq!(serde_json::from_value::<StationLink>(json).unwrap(), link);
}

#[test]
fn what_the_driver_does_not_report_stays_absent() {
    let text = "[station]\n\trx bytes:\t1\n\trx packets:\t2\n\ttx bytes:\t3\n\
                \ttx packets:\t4\n\ttx retries:\t0\n\ttx failed:\t0\n\
                \ttx bitrate:\t(unknown)\n[channel]\n\
                \tchannel 6 (2437 MHz), width: 20 MHz (no HT), center1: 2437 MHz\n\
                [survey]\n[aqm]\n";
    let link = parse_link(text).unwrap();
    assert_eq!(link.tx_bitrate.unwrap().kbit_per_s, None);
    assert_eq!(link.rx_bitrate, None);
    assert_eq!((link.channel, link.width_mhz), (Some(6), Some(20)));
    assert_eq!(
        (link.survey, link.tid0_aqm_drops, link.rx_duration_us),
        (None, None, None)
    );
}

#[test]
fn a_missing_counter_or_an_unreadable_known_line_refuses_the_observation() {
    let complete = include_str!("ht40.txt");
    for refused in [
        complete.replace("\ttx retries:\t1204\n", ""),
        complete.replace("rx bytes:\t9214632", "rx bytes:\tmany"),
        complete.replace("rx duration:\t801233 us", "rx duration:\t801233 ms"),
        complete.replace("150.0 MBit/s", "150 MBit/s"),
        complete.replace("MCS 7 40MHz", "MCS seven 40MHz"),
        complete.replace("2472 MHz [in use]", "2472 MHz"),
        complete.replace("channel 13 (2472 MHz)", "channel x (2472 MHz)"),
        complete.replace("[station]\n", ""),
        complete.replace("tid0 drops: 4", "tid0 drops: x"),
    ] {
        assert!(parse_link(&refused).is_err(), "{refused}");
    }
}

#[test]
fn every_format_s_rate_names_its_mcs_streams_width_and_guard_interval() {
    let rate = |text| {
        let rate = parse_bitrate(text).unwrap();
        (
            rate.kbit_per_s,
            rate.format,
            rate.mcs,
            rate.streams,
            rate.width_mhz,
            rate.short_gi,
        )
    };
    assert_eq!(
        rate("54.0 MBit/s"),
        (Some(54_000), RateFormat::Legacy, None, None, 20, false)
    );
    assert_eq!(
        rate("5.5 MBit/s"),
        (Some(5500), RateFormat::Legacy, None, None, 20, false)
    );
    assert_eq!(
        rate("72.2 MBit/s MCS 7 short GI"),
        (Some(72_200), RateFormat::Ht, Some(7), None, 20, true)
    );
    assert_eq!(
        rate("135.0 MBit/s MCS 7 40MHz"),
        (Some(135_000), RateFormat::Ht, Some(7), None, 40, false)
    );
    assert_eq!(
        rate("433.3 MBit/s VHT-MCS 9 80MHz short GI VHT-NSS 1"),
        (Some(433_300), RateFormat::Vht, Some(9), Some(1), 80, true)
    );
    assert_eq!(
        rate("286.7 MBit/s 40MHz HE-MCS 11 HE-NSS 2 HE-GI 0 HE-DCM 0"),
        (Some(286_700), RateFormat::He, Some(11), Some(2), 40, false)
    );
    for refused in [
        "",
        "54.0",
        "54.0 Mbit/s",
        "54.00 MBit/s",
        "x.0 MBit/s",
        "6.5 MBit/s MCS",
    ] {
        assert!(parse_bitrate(refused).is_err(), "{refused}");
    }
}

/// A router that answers one script.
struct Answer(Option<String>, &'static str);

impl Remote for Answer {
    fn run(&mut self, script: &str) -> Result<String> {
        self.0 = Some(script.to_owned());
        Ok(self.1.to_owned())
    }
}

#[test]
fn the_router_reads_only_the_target_station_and_filters_its_identity() {
    let mut router = Answer(None, include_str!("ht40.txt"));
    let target = Mac::parse("02:00:00:00:00:01").unwrap();
    let link = link(&mut router, "phy0-ap0", target).unwrap();
    assert_eq!(link.tx_bitrate.unwrap().mcs, Some(7));
    let script = router.0.unwrap();
    assert!(script.contains("mac=02:00:00:00:00:01\n"), "{script}");
    assert!(script.contains("iw dev \"$iface\" station get \"$mac\""));
    assert!(script.contains("grep -v '^Station '"));
    assert!(!script.contains("station dump"), "never another peer");
}
