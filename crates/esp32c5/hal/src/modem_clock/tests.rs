use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Device(ModemClockDevice, bool),
    MacReset,
}

#[derive(Default)]
struct Recorder(Vec<Operation>);

impl ModemClockPort for Recorder {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.0.push(Operation::Device(device, enable));
    }

    fn pulse_ieee802154_mac_reset(&mut self) {
        self.0.push(Operation::MacReset);
    }
}

fn take(clocks: &mut Core<'_, Recorder>) -> Vec<Operation> {
    core::mem::take(&mut clocks.port.0)
}

/// The IEEE 802.15.4 devices in the vendor device order.
const IEEE802154_DEVICES: [ModemClockDevice; 5] = [
    ModemClockDevice::Coexistence,
    ModemClockDevice::Etm,
    ModemClockDevice::BluetoothApb,
    ModemClockDevice::BluetoothIeee802154CommonBaseband,
    ModemClockDevice::Ieee802154Mac,
];

fn devices(enable: bool, list: &[ModemClockDevice]) -> Vec<Operation> {
    list.iter()
        .map(|device| Operation::Device(*device, enable))
        .collect()
}

#[test]
fn the_table_is_consistent() {
    assert!(oer_radio_clock::table_is_consistent::<ModemClockDependency>());
}

#[test]
fn every_offered_module_has_published_device_actions() {
    for module in [
        ModemClockModule::Ieee802154,
        ModemClockModule::Coexistence,
        ModemClockModule::ModemEtm,
        ModemClockModule::BluetoothApb,
    ] {
        let set = module.dependencies();
        for dependency in
            <ModemClockDependency as oer_radio_clock::ModemClockDependency>::LOW_BIT_FIRST
        {
            assert!(
                !set.contains(*dependency) || device(*dependency).is_some(),
                "{module:?} needs {dependency:?}"
            );
        }
    }
}

#[test]
fn ieee802154_enables_and_disables_its_devices_in_vendor_order() {
    let identity = ModemClockPlannerIdentity::new();
    let mut clocks = Core::with_port(Recorder::default(), &identity);
    let grant = clocks
        .enable(ModemClockModule::Ieee802154)
        .expect("managed planner");
    assert_eq!(take(&mut clocks), devices(true, &IEEE802154_DEVICES));
    clocks.disable(grant).expect("exact grant");
    assert_eq!(take(&mut clocks), devices(false, &IEEE802154_DEVICES));
}

#[test]
fn a_shared_device_switches_only_at_its_count_boundaries() {
    let only_ieee802154 = [
        ModemClockDevice::Coexistence,
        ModemClockDevice::BluetoothIeee802154CommonBaseband,
        ModemClockDevice::Ieee802154Mac,
    ];
    let bluetooth_apb = [ModemClockDevice::Etm, ModemClockDevice::BluetoothApb];
    let identity = ModemClockPlannerIdentity::new();
    let mut clocks = Core::with_port(Recorder::default(), &identity);
    let apb = clocks
        .enable(ModemClockModule::BluetoothApb)
        .expect("Bluetooth APB");
    assert_eq!(take(&mut clocks), devices(true, &bluetooth_apb));
    let ieee802154 = clocks
        .enable(ModemClockModule::Ieee802154)
        .expect("IEEE 802.15.4");
    assert_eq!(take(&mut clocks), devices(true, &only_ieee802154));
    clocks.disable(ieee802154).expect("IEEE 802.15.4 grant");
    assert_eq!(take(&mut clocks), devices(false, &only_ieee802154));
    clocks.disable(apb).expect("Bluetooth APB grant");
    assert_eq!(take(&mut clocks), devices(false, &bluetooth_apb));
}

#[test]
fn only_an_ieee802154_grant_resets_the_mac() {
    let identity = ModemClockPlannerIdentity::new();
    let mut clocks = Core::with_port(Recorder::default(), &identity);
    let etm = clocks.enable(ModemClockModule::ModemEtm).expect("ETM");
    let ieee802154 = clocks
        .enable(ModemClockModule::Ieee802154)
        .expect("IEEE 802.15.4");
    take(&mut clocks);
    assert_eq!(
        clocks.reset_ieee802154_mac(&etm),
        Err(ModemClockError::WrongModule)
    );
    assert!(take(&mut clocks).is_empty());
    clocks
        .reset_ieee802154_mac(&ieee802154)
        .expect("IEEE 802.15.4 grant");
    assert_eq!(take(&mut clocks), [Operation::MacReset]);
}

#[test]
fn the_port_returns_only_after_every_grant() {
    let identity = ModemClockPlannerIdentity::new();
    let mut clocks = Core::with_port(Recorder::default(), &identity);
    let grant = clocks
        .enable(ModemClockModule::Coexistence)
        .expect("coexistence");
    let Err(mut clocks) = clocks.into_port() else {
        panic!("an outstanding grant keeps the owner");
    };
    clocks.disable(grant).expect("exact grant");
    assert!(clocks.into_port().is_ok());
}

#[test]
fn a_grant_of_another_owner_is_rejected_and_returned() {
    let first_identity = ModemClockPlannerIdentity::new();
    let second_identity = ModemClockPlannerIdentity::new();
    let mut first = Core::with_port(Recorder::default(), &first_identity);
    let mut second = Core::with_port(Recorder::default(), &second_identity);
    let grant = first.enable(ModemClockModule::ModemEtm).expect("ETM");
    take(&mut first);
    match second.disable(grant) {
        Err(ModemClockDisableError::Rejected(grant, error)) => {
            assert_eq!(error, ModemClockReleasePreparationError::CrossManagerLease);
            assert!(take(&mut second).is_empty());
            first.disable(grant).expect("original owner");
            assert_eq!(take(&mut first), devices(false, &[ModemClockDevice::Etm]));
        }
        other => panic!("a foreign grant must be rejected: {other:?}"),
    }
}
