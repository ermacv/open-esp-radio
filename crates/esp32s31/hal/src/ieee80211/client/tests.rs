use super::WifiCold;
use crate::root::RadioHardware;

/// The partition round-trips through the cold owner without MMIO.
#[test]
fn a_cold_client_returns_its_partition() {
    let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let cold = WifiCold::from_partition(partitions.wifi);
    let _partition = cold.into_partition();
}

/// A running epoch reunites into the same clocked client without MMIO,
/// keeping its initialized status.
#[test]
fn a_running_epoch_reunites_with_its_clocked_client() {
    let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let clocked = super::WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi));
    let (runtime, interrupts, clocks) = clocked.into_running();
    let clocked = super::WifiClocked::from_running(runtime, interrupts, clocks);
    assert!(!clocked.initialized());
}

/// The runtime channel capability borrows the shared PHY from the arbiter
/// lease and ends with it.
#[test]
fn the_runtime_channel_capability_borrows_the_lease() {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let clocked = super::WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi));
    let (mut runtime, _interrupts, _clocks) = clocked.into_running();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
    let mut platform = ();
    let channel = runtime.channel_hal(&mut platform, &mut lease);
    drop(channel);
    let _mac = runtime.wifi_mac_hal();
}

#[test]
fn clock_release_consumes_the_obligation_only_on_success() {
    let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let clocked = super::WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi));
    let released = match clocked.finish_clock_release(Ok(())) {
        Ok(released) => released,
        Err(_) => panic!("successful release"),
    };
    let _powered = released.into_powered();
}

#[test]
fn every_clock_release_failure_retains_stopped_custody_and_exact_obligation() {
    for reason in [
        super::ModemClockError::AlreadyEnabled,
        super::ModemClockError::NotEnabled,
        super::ModemClockError::Rejected,
        super::ModemClockError::Poisoned,
    ] {
        let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
        let clocked = super::WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi));
        let (stopped, poison) = match clocked.finish_clock_release(Err(reason)) {
            Err(owners) => owners,
            Ok(_) => panic!("failure must not discharge the obligation"),
        };
        let _registers = stopped._registers;
        let _obligation = poison._obligation;
    }
}
