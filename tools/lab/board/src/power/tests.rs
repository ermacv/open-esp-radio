use super::*;

const REPORT: &str =
    "Current status for hub 4-8.3 [0bda:0411 Generic USB3.2 Hub, USB 3.20, 4 ports, ppps]
  Port 2: 02a0 power 5gbps Rx.Detect
Current status for hub 3-8.3 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0000 off
  Port 3: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:F6:D0]
  Port 4: 0103 power enable connect [046d:c52b Logitech USB Receiver]
";

#[test]
fn a_port_is_read_from_its_own_hub_not_its_companion() {
    let report = parse(REPORT);
    assert_eq!(powered(&report, "3-8.3", 2), Some(false));
    assert_eq!(powered(&report, "3-8.3", 1), Some(true));
    assert_eq!(powered(&report, "4-8.3", 2), Some(true));
    assert_eq!(powered(&report, "3-8.3", 9), None);
}

#[test]
fn an_espressif_port_reports_its_mac_and_another_device_none() {
    let report = parse(REPORT);
    let hub = report.iter().find(|hub| hub.location == "3-8.3").unwrap();
    let board = hub.ports.iter().find(|port| port.port == 3).unwrap();
    assert!(board.powered && board.connected);
    assert_eq!(
        board.device,
        Some((
            String::from("303a:1001"),
            Some(oer_lab_model::device_id::DeviceId::parse("02:00:00:00:F6:D0").unwrap())
        ))
    );
    let other = hub.ports.iter().find(|port| port.port == 4).unwrap();
    assert_eq!(other.device, Some((String::from("046d:c52b"), None)));
}

fn cause(code: u32) -> ResetCause {
    ResetCauseField {
        address: 0x2070_1030,
        bit_offset: 1,
        bit_width: 6,
        power_on: 1,
    }
    .cause(code << 1 | 1)
}

#[test]
fn a_power_cycle_needs_the_board_to_leave_and_return() {
    let after = Duration::from_millis(3100);
    let cycle = |left, returned| PowerCycle { left, returned };
    assert_eq!(cycle(true, Some(after)).verdict(), Ok(after));
    assert!(
        cycle(false, Some(after))
            .verdict()
            .unwrap_err()
            .contains("does not cut its power")
    );
    assert!(
        cycle(true, None)
            .verdict()
            .unwrap_err()
            .contains("did not return")
    );
}

#[test]
fn a_power_loss_also_needs_a_power_on_reset() {
    let after = Duration::from_millis(3100);
    let loss = |left, returned, reset| PowerLoss {
        cycle: PowerCycle { left, returned },
        reset,
    };
    assert_eq!(
        loss(true, Some(after), Some(Ok(cause(1)))).verdict(),
        Ok(after)
    );
    assert!(
        loss(false, Some(after), Some(Ok(cause(1))))
            .verdict()
            .is_err()
    );
    // Dropped from the bus and back without losing power: the board keeps
    // the cause of its last reset, a JTAG reset here.
    let kept_power = loss(true, Some(after), Some(Ok(cause(0x18))))
        .verdict()
        .unwrap_err();
    assert!(
        kept_power.contains("0x18, not a power-on reset"),
        "{kept_power}"
    );
    let unread = loss(true, Some(after), Some(Err(String::from("no JTAG"))))
        .verdict()
        .unwrap_err();
    assert!(unread.contains("could not be read: no JTAG"), "{unread}");
    assert!(loss(true, Some(after), None).verdict().is_err());
}

