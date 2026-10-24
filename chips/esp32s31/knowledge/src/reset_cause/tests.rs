use super::ResetCause;

#[test]
fn every_code_round_trips() {
    for code in 0..=u8::MAX {
        assert_eq!(ResetCause::from_code(code).code(), code, "code {code:#04x}");
    }
}

#[test]
fn unnamed_codes_are_other() {
    for code in [0x00, 0x02, 0x04, 0x0a, 0x0e, 0x11, 0x15, 0x19, 0x1b, 0x3f] {
        assert_eq!(ResetCause::from_code(code), ResetCause::Other(code));
    }
}

#[test]
fn named_codes_are_not_other() {
    let named = (0..=u8::MAX)
        .filter(|&code| !matches!(ResetCause::from_code(code), ResetCause::Other(_)))
        .count();
    assert_eq!(named, 19);
}

#[test]
fn power_on_software_and_usb_uart_codes() {
    assert_eq!(ResetCause::from_code(0x01), ResetCause::ChipPowerOn);
    assert_eq!(ResetCause::from_code(0x03), ResetCause::CoreSoftware);
    // The ESP32-C5 gives the USB Serial/JTAG UART reset 0x15; the S31 0x17.
    assert_eq!(ResetCause::from_code(0x15), ResetCause::Other(0x15));
    assert_eq!(ResetCause::from_code(0x17), ResetCause::CoreUsbUart);
}
