use super::*;

#[test]
fn a_closed_report_yields_its_objects_among_boot_logs() {
    let console = "I (278) phy_init: phy_version\n\
                   oer-vendor-calibration phy_param 0a00ff\n\
                   oer-vendor-calibration-end\n";
    let objects = parse(console).unwrap();
    assert_eq!(objects["phy_param"], [0x0a, 0x00, 0xff]);
    assert!(complete(console));
}

#[test]
fn register_replies_are_read_by_address() {
    let console = "r 20100434\noer-vendor-calibration-register 20100434 0000abcd\n";
    assert_eq!(registers(console).unwrap()[&0x2010_0434], 0xabcd);
    assert_eq!(register_request(0x2010_0434), "r 20100434\n");
}

#[test]
fn analog_replies_are_read_by_block_and_register() {
    let console = format!(
        "{}oer-vendor-calibration-analog 61 0a",
        analog_line(0x61, 0x09, 0x5c)
    );
    let values = analog(&console).unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[&(0x61, 0x09)], 0x5c);
    assert_eq!(analog_request(0x6b, 0x02), "a 6b 02\n");
    assert!(analog("oer-vendor-calibration-analog 61 09\n").is_err());
}

#[test]
fn unreadable_registers_and_reports_are_counted() {
    let console = format!(
        "oer-vendor-calibration-end\n{}oer-vendor-calibration-end\n",
        unreadable_line(0x2010_2800)
    );
    assert_eq!(reports(&console), 2);
    assert!(unreadable(&console).unwrap().contains(&0x2010_2800));
}

#[test]
fn restart_replies_report_the_stopped_modem_flags() {
    let console = "oer-vendor-calibration-restarted 00000000\n\
                   oer-vendor-calibration-restarted 000";
    assert_eq!(restarts(console).unwrap(), [0]);
}

#[test]
fn a_reply_still_arriving_is_not_read() {
    let console = "oer-vendor-calibration-register 20100434 0000abcd\n\
                   oer-vendor-calibration-register 2010";
    let values = registers(console).unwrap();
    assert_eq!(values.len(), 1);
}

#[test]
fn open_repeated_or_malformed_reports_fail() {
    assert!(parse("oer-vendor-calibration phy_param 00\n").is_err());
    assert!(
        parse(
            "oer-vendor-calibration a 00\noer-vendor-calibration a 01\n\
             oer-vendor-calibration-end\n"
        )
        .is_err()
    );
    assert!(parse("oer-vendor-calibration a 0\noer-vendor-calibration-end\n").is_err());
}
