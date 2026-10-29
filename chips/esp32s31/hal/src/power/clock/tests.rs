//! The ESP32-S31 table through the chip-neutral planner: each module emits
//! its vendor dependency edges, lowest first, at refcount boundaries.

use std::vec::Vec;

use oer_espressif_modem_clock::{
    DependencySet, execute_acquire, execute_release, table_is_consistent,
};

use super::*;

/// The IEEE 802.15.4 module set, low bit first.
const IEEE802154_EDGES: [Dependency; 7] = [
    Dependency(crate::power::clock::ChipDependency::Pll160AndModemSource),
    Dependency(crate::power::clock::ChipDependency::Coexistence),
    Dependency(crate::power::clock::ChipDependency::WifiBb80x1),
    Dependency(crate::power::clock::ChipDependency::Etm),
    Dependency(crate::power::clock::ChipDependency::BtApbAndSecurity),
    Dependency(crate::power::clock::ChipDependency::BtIeee802154CommonBaseband),
    Dependency(crate::power::clock::ChipDependency::Ieee802154ApbAndMac),
];

fn acquire(
    planner: ModemClockPlanner,
    dependencies: DependencySet<Dependency>,
) -> (ModemClockPlanner, ModemClockLease, Vec<Dependency>) {
    let mut edges = Vec::new();
    let prepared = planner
        .prepare_acquire(dependencies)
        .unwrap_or_else(|_| panic!("managed baseline"));
    let (planner, lease) = execute_acquire(prepared, |dependency| {
        edges.push(dependency);
        Ok::<(), ()>(())
    })
    .unwrap_or_else(|_| panic!("every edge performed"));
    (planner, lease, edges)
}

fn release(
    planner: ModemClockPlanner,
    lease: ModemClockLease,
) -> (ModemClockPlanner, Vec<Dependency>) {
    let mut edges = Vec::new();
    let prepared = planner
        .prepare_release(lease)
        .unwrap_or_else(|_| panic!("exact lease"));
    let planner = execute_release(prepared, |dependency| {
        edges.push(dependency);
        Ok::<(), ()>(())
    })
    .unwrap_or_else(|_| panic!("every edge performed"));
    (planner, edges)
}

#[test]
fn the_table_is_consistent() {
    assert!(table_is_consistent::<Dependency>());
}

#[test]
fn the_ieee802154_set_is_acquired_and_released_low_bit_first() {
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let planner = ModemClockPlanner::managed(identity);
    let (planner, lease, edges) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Ieee802154),
    );
    assert_eq!(edges, IEEE802154_EDGES);
    let (_planner, edges) = release(planner, lease);
    assert_eq!(edges, IEEE802154_EDGES);
}

#[test]
fn a_second_module_enables_only_its_missing_dependencies() {
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let planner = ModemClockPlanner::managed(identity);
    let (planner, wifi, _) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Wifi),
    );
    // Coexistence, the PLL source and the 80x1 clock are already on for Wi-Fi.
    let (planner, bluetooth, edges) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Bluetooth),
    );
    assert_eq!(
        edges,
        [
            Dependency(crate::power::clock::ChipDependency::Etm),
            Dependency(crate::power::clock::ChipDependency::BtMac),
            Dependency(crate::power::clock::ChipDependency::BtPeripheral),
            Dependency(crate::power::clock::ChipDependency::BtApbAndSecurity),
            Dependency(crate::power::clock::ChipDependency::BtIeee802154CommonBaseband),
        ]
    );
    // Leaving Wi-Fi keeps every dependency Bluetooth still uses.
    let (planner, edges) = release(planner, wifi);
    assert_eq!(
        edges,
        [
            Dependency(crate::power::clock::ChipDependency::WifiApb),
            Dependency(crate::power::clock::ChipDependency::WifiBb44m),
            Dependency(crate::power::clock::ChipDependency::WifiMac),
            Dependency(crate::power::clock::ChipDependency::WifiBb),
        ]
    );
    let (_planner, edges) = release(planner, bluetooth);
    assert_eq!(
        edges,
        [
            Dependency(crate::power::clock::ChipDependency::Pll160AndModemSource),
            Dependency(crate::power::clock::ChipDependency::Coexistence),
            Dependency(crate::power::clock::ChipDependency::WifiBb80x1),
            Dependency(crate::power::clock::ChipDependency::Etm),
            Dependency(crate::power::clock::ChipDependency::BtMac),
            Dependency(crate::power::clock::ChipDependency::BtPeripheral),
            Dependency(crate::power::clock::ChipDependency::BtApbAndSecurity),
            Dependency(crate::power::clock::ChipDependency::BtIeee802154CommonBaseband),
        ]
    );
}

#[test]
fn the_analog_i2c_master_edge_follows_every_module_request() {
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let planner = ModemClockPlanner::managed(identity);
    let (planner, first, first_edges) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Phy),
    );
    assert!(first_edges.contains(&Dependency(
        crate::power::clock::ChipDependency::AnalogI2cMaster
    )));
    let (planner, second, second_edges) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::AnalogI2cMaster),
    );
    // The platform owner counts analog-I2C references, so each request
    // reaches it even while the dependency is already enabled.
    assert_eq!(
        second_edges,
        [Dependency(
            crate::power::clock::ChipDependency::AnalogI2cMaster
        )]
    );
    let (planner, released) = release(planner, second);
    assert_eq!(
        released,
        [Dependency(
            crate::power::clock::ChipDependency::AnalogI2cMaster
        )]
    );
    let (_planner, released) = release(planner, first);
    assert!(released.contains(&Dependency(
        crate::power::clock::ChipDependency::AnalogI2cMaster
    )));
}

#[test]
fn initialized_wifi_keeps_its_clocks_and_re_enables_them_on_the_next_request() {
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let mut planner = ModemClockPlanner::managed(identity);
    planner.set_wifi_initialized(true);
    let (planner, wifi, _) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Wifi),
    );
    let (planner, released) = release(planner, wifi);
    // Only the dependencies outside the Wi-Fi clock group are switched off.
    assert_eq!(
        released,
        [
            Dependency(crate::power::clock::ChipDependency::Pll160AndModemSource),
            Dependency(crate::power::clock::ChipDependency::Coexistence)
        ]
    );
    // The next zero-to-one request enables them again, including the
    // baseband reset carried by the WifiBb edge.
    let (_planner, _lease, edges) = acquire(
        planner,
        crate::power::clock::module_dependencies(ModemClockModule::Wifi),
    );
    assert!(edges.contains(&Dependency(crate::power::clock::ChipDependency::WifiBb)));
}
