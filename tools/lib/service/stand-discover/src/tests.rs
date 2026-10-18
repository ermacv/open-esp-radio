use super::*;
use oer_tool_service_stand_power::PortStatus;

/// `uhubctl` on the stand, 2026-10-04: the peer on the middle hub's port 2,
/// the DUT on its port 3, the bottom hub cascaded on its port 4.
const REPORT: &str = "\
Current status for hub 4-1.3 [0bda:0411 Generic USB3.2 Hub, USB 3.20, 4 ports, ppps]
  Port 1: 02a0 power 5gbps Rx.Detect
  Port 4: 0203 power 5gbps U0 enable connect [0bda:0411 Generic USB3.2 Hub, USB 3.20, 4 ports, ppps]
Current status for hub 3-8.3.4 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0000 off
  Port 3: 0100 power
  Port 4: 0100 power
Current status for hub 3-8.3 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:25:64]
  Port 3: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:F6:D0]
  Port 4: 0503 power highspeed enable connect [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
Current status for hub 3-8 [0bda:5423 TerraMaster 4-Port USB 2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0100 power
  Port 3: 0503 power highspeed enable connect [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 4: 0100 power
";

const STAND: &str = r#"
schema = 1
[stand]
id = "test"
air = "exclusive"
[[hub]]
id = "rsh-top"
usb2 = "3-8"
protected = [3]
switchable = [1, 4]
buttons = { 1 = 1, 2 = 2, 4 = 3 }
[[hub]]
id = "rsh-mid"
usb2 = "3-8.3"
protected = [4]
switchable = [1, 2, 3]
buttons = { 1 = 4, 2 = 5, 3 = 6 }
[[hub]]
id = "rsh-bottom"
usb2 = "3-8.3.4"
switchable = [1, 2, 3, 4]
buttons = { 1 = 7, 2 = 8, 3 = 9, 4 = 10 }
[[board]]
id = "dut-a"
usb-serial = "02:00:00:00:F6:D0"
chip = "chip-a"
radios = ["wifi-2g4"]
roles = ["dut"]
port = { hub = "rsh-mid", port = 3 }
reset = ["power"]
[[board]]
id = "peer-a"
usb-serial = "02:00:00:00:25:64"
chip = "chip-b"
radios = ["ieee802154"]
roles = ["peer"]
port = { hub = "rsh-mid", port = 2 }
reset = ["power"]
"#;

fn stand(text: &str) -> Stand {
    let stand = Stand::parse(text).unwrap();
    stand.validate().unwrap();
    stand
}

fn place(hub: &str, port: u8, button: Option<u8>) -> Place {
    Place {
        hub: hub.into(),
        port,
        button,
    }
}

#[test]
fn the_report_gives_each_port_its_power_and_device() {
    let hubs = power::parse(REPORT);
    let mid = hubs.iter().find(|hub| hub.location == "3-8.3").unwrap();
    assert_eq!(
        mid.ports[1],
        PortStatus {
            port: 2,
            powered: true,
            connected: true,
            device: Some((
                "303a:1001".into(),
                Some(oer_tool_data_stand_file::DeviceId::parse("02:00:00:00:25:64").unwrap())
            )),
        }
    );
    // A cascaded hub has no serial number; an off port neither power nor device.
    assert_eq!(mid.ports[3].device, Some(("0bda:5411".into(), None)));
    let bottom = hubs.iter().find(|hub| hub.location == "3-8.3.4").unwrap();
    assert!(!bottom.ports[1].powered && bottom.ports[1].device.is_none());
}

#[test]
fn discovery_finds_both_boards_on_their_ports_and_buttons() {
    let stand = stand(STAND);
    let findings = discover(&stand, &power::parse(REPORT));
    assert_eq!(
        findings,
        [
            Finding::InPlace {
                board: "dut-a".into(),
                place: place("rsh-mid", 3, Some(6)),
            },
            Finding::InPlace {
                board: "peer-a".into(),
                place: place("rsh-mid", 2, Some(5)),
            },
        ]
    );
    let text = describe(&stand, &findings);
    assert!(
        text.contains("ok      dut-a on rsh-mid:3 (button 6)"),
        "{text}"
    );
}

#[test]
fn discovery_names_moved_missing_and_new_boards() {
    // The DUT moved to the bottom hub's port 1; a new board sits on the
    // middle hub's port 3; the peer's port is off.
    let report = REPORT
        .replace(
            "  Port 2: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:25:64]",
            "  Port 2: 0000 off",
        )
        .replace(
            "  Port 3: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:F6:D0]",
            "  Port 3: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 40:4C:CA:00:00:01]",
        )
        .replace(
            "Current status for hub 3-8.3.4 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]\n  Port 1: 0100 power",
            "Current status for hub 3-8.3.4 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]\n  Port 1: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:F6:D0]",
        );
    let stand = stand(STAND);
    let findings = discover(&stand, &power::parse(&report));
    assert_eq!(
        findings,
        [
            Finding::Moved {
                board: "dut-a".into(),
                expected: place("rsh-mid", 3, Some(6)),
                found: place("rsh-bottom", 1, Some(7)),
            },
            Finding::Missing {
                board: "peer-a".into(),
                expected: place("rsh-mid", 2, Some(5)),
                why: Absence::PortOff,
            },
            Finding::New {
                serial: "40:4C:CA:00:00:01".into(),
                place: place("rsh-mid", 3, Some(6)),
                switchable: true,
            },
        ]
    );
    let text = describe(&stand, &findings);
    assert!(
        text.contains("usb-serial = \"40:4C:CA:00:00:01\""),
        "{text}"
    );
    assert!(
        text.contains("port = { hub = \"rsh-mid\", port = 3 }"),
        "{text}"
    );
}

#[test]
fn an_empty_powered_port_points_at_its_button() {
    let report = REPORT.replace(
        "  Port 2: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:25:64]",
        "  Port 2: 0100 power",
    );
    let stand = stand(STAND);
    let text = describe(&stand, &discover(&stand, &power::parse(&report)));
    assert!(
        text.contains(
            "missing peer-a on rsh-mid:2 (button 5): button 5 is off or the board is not plugged in"
        ),
        "{text}"
    );
}

#[test]
fn blink_refuses_cascade_and_unswitchable_ports() {
    let stand = stand(STAND);
    let refused = |target: &str| blink_target(&stand, target).unwrap_err().to_string();
    assert!(refused("rsh-top:3").contains("protected"), "a cascade");
    assert!(refused("rsh-mid:4").contains("protected"), "a cascade");
    assert!(refused("rsh-top:2").contains("not switchable"));
    assert!(refused("rsh-side:1").contains("not in the stand file"));
    assert!(refused("rsh-mid").contains("not HUB:PORT"));
    let (hub, port) = blink_target(&stand, "rsh-bottom:2").unwrap();
    assert_eq!((hub.usb2.as_str(), port), ("3-8.3.4", 2));
}
