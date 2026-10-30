use std::{os::unix::fs::symlink, path::Path};

use super::*;

/// `uhubctl` on a made-up stand: a USB 2 hub `3-8.3` behind `3-8`, a hub
/// `3-8.3.4` on its port 4, and their USB 3 companions on bus 4.
const STATUS: &str = "\
Current status for hub 4-1.3.4 [0bda:0411 Generic USB3.2 Hub, USB 3.20, 4 ports, ppps]
  Port 4: 02a0 power 5gbps Rx.Detect
Current status for hub 3-8.3.4 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0000 off
  Port 3: 0100 power
  Port 4: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:01:D0]
Current status for hub 3-8.3 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 2: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:02:64]
  Port 4: 0503 power highspeed enable connect [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
Current status for hub 3-9 [1a40:0101 Generic USB 2.0 Hub, USB 2.00, 4 ports, nops]
  Port 1: 0103 power enable connect [303a:1001 Espressif USB JTAG/serial debug unit 02:00:00:00:03:00]
";

/// A sysfs USB device directory with `hub`'s ports 1..=4 (interface
/// `<hub>:1.0`) and `device` linking a port to what it carries.
struct Tree(tempfile::TempDir);

impl Tree {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn device(&self, location: &str, class: &str) -> &Self {
        let device = self.root().join(location);
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join("bDeviceClass"), format!("{class}\n")).unwrap();
        self
    }

    fn hub(&self, location: &str) -> &Self {
        self.device(location, HUB_CLASS);
        for port in 1..=4 {
            std::fs::create_dir_all(self.port(location, port)).unwrap();
        }
        self
    }

    fn port(&self, hub: &str, port: u32) -> PathBuf {
        self.root()
            .join(hub)
            .join(format!("{hub}:1.0"))
            .join(format!("{hub}-port{port}"))
    }

    /// `hub`'s `port` carries `location`.
    fn plug(&self, hub: &str, port: u32, location: &str) -> &Self {
        symlink(
            format!("../../../{location}"),
            self.port(hub, port).join("device"),
        )
        .unwrap();
        self
    }

    /// `hub`'s `port` and `companion`'s are USB 2 and USB 3 peers.
    fn peer(&self, hub: &str, companion: &str, port: u32) -> &Self {
        symlink(
            format!("../../../{companion}/{companion}:1.0/{companion}-port{port}"),
            self.port(hub, port).join("peer"),
        )
        .unwrap();
        self
    }
}

/// The made-up stand's tree: the board `3-8.3.4.4` alone on its port.
fn stand_tree() -> Tree {
    let tree = Tree::new();
    tree.hub("3-8.3").hub("3-8.3.4").hub("4-1.3.4");
    tree.device("3-8.3.4.4", "ef")
        .plug("3-8.3.4", 4, "3-8.3.4.4");
    tree.plug("3-8.3", 4, "3-8.3.4");
    tree.peer("3-8.3.4", "4-1.3.4", 4);
    tree
}

fn stand(hub: &str) -> Stand {
    Stand::parse(&format!(
        "schema = 2\n[[hub]]\nid = \"bottom\"\nusb2 = \"3-8.3.4\"\n{hub}"
    ))
    .unwrap()
}

#[test]
fn the_status_names_each_hub_its_power_switching_and_ports() {
    let hubs = parse_status(STATUS);
    assert_eq!(
        hubs.iter()
            .map(|hub| hub.location.as_str())
            .collect::<Vec<_>>(),
        ["4-1.3.4", "3-8.3.4", "3-8.3", "3-9"]
    );
    assert!(hubs[1].per_port_power && !hubs[3].per_port_power);
    assert_eq!(
        hubs[1].ports[1],
        PortStatus {
            port: 2,
            powered: false,
            connected: false
        }
    );
    assert_eq!(
        hubs[1].ports[3],
        PortStatus {
            port: 4,
            powered: true,
            connected: true
        }
    );
}

#[test]
fn the_board_alone_on_a_switching_port_is_found_there() {
    let tree = stand_tree();
    let report = parse_status(STATUS);
    let port = power_port(tree.root(), &report, None, "3-8.3.4.4").unwrap();
    assert_eq!(
        port,
        PowerPort {
            hub: "3-8.3.4".to_owned(),
            port: 4,
            hub_id: None
        }
    );
    // The stand file names the hub; nothing about the board's port.
    let port = power_port(tree.root(), &report, Some(&stand("")), "3-8.3.4.4").unwrap();
    assert_eq!(port.to_string(), "bottom (3-8.3.4) port 4");
}

#[test]
fn a_port_carrying_more_than_the_board_is_refused() {
    let report = parse_status(STATUS);
    // A hub: the port of hub `3-8.3` that carries `3-8.3.4`.
    let tree = stand_tree();
    let error = power_port(tree.root(), &report, None, "3-8.3.4").unwrap_err();
    assert!(error.contains("is a hub"), "{error}");
    // Something behind the board.
    let tree = stand_tree();
    tree.device("3-8.3.4.4.1", "00");
    let error = power_port(tree.root(), &report, None, "3-8.3.4.4").unwrap_err();
    assert!(error.contains("3-8.3.4.4.1 hangs behind"), "{error}");
    // A device on the USB 3 companion of the port.
    let tree = stand_tree();
    tree.device("4-1.3.4.4", "00")
        .plug("4-1.3.4", 4, "4-1.3.4.4");
    let error = power_port(tree.root(), &report, None, "3-8.3.4.4").unwrap_err();
    assert!(error.contains("companion"), "{error}");
    // The port links another device than the board.
    let tree = stand_tree();
    std::fs::remove_file(tree.port("3-8.3.4", 4).join("device")).unwrap();
    tree.device("3-8.3.4.9", "00")
        .plug("3-8.3.4", 4, "3-8.3.4.9");
    let error = power_port(tree.root(), &report, None, "3-8.3.4.4").unwrap_err();
    assert!(error.contains("not the board"), "{error}");
}

#[test]
fn a_hub_without_per_port_power_or_the_port_is_refused() {
    let tree = stand_tree();
    let report = parse_status(STATUS);
    let error = power_port(tree.root(), &report, None, "3-9.1").unwrap_err();
    assert!(error.contains("does not switch power per port"), "{error}");
    let error = power_port(tree.root(), &report, None, "3-7.1").unwrap_err();
    assert!(error.contains("reports no hub 3-7"), "{error}");
    let error = power_port(tree.root(), &report, None, "3-8.3.4.5").unwrap_err();
    assert!(error.contains("has no port 5"), "{error}");
}

#[test]
fn the_stand_file_s_hub_policy_is_kept() {
    let tree = stand_tree();
    let report = parse_status(STATUS);
    for (hub, reason) in [
        ("protected = [4]\n", "is protected"),
        ("switchable = [1, 2]\n", "not in its switchable list"),
        ("keeps-power = [4]\n", "keeps its power"),
    ] {
        let error = power_port(tree.root(), &report, Some(&stand(hub)), "3-8.3.4.4").unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
    power_port(
        tree.root(),
        &report,
        Some(&stand("switchable = [4]\n")),
        "3-8.3.4.4",
    )
    .unwrap();
}
