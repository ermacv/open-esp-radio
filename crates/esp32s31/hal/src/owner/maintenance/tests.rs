use super::{Error, Observation, check_stopped};

#[derive(Default)]
struct Hardware {
    active: u8,
    walker: bool,
    mac_reads: usize,
    walker_reads: usize,
}

impl Observation for Hardware {
    fn mac_active(&mut self) -> u8 {
        self.mac_reads += 1;
        self.active
    }
    fn rx_walker_enabled(&mut self) -> bool {
        self.walker_reads += 1;
        self.walker
    }
}

#[test]
fn inactive_mac_does_not_authorize_a_live_rx_walker() {
    let mut hardware = Hardware {
        walker: true,
        ..Hardware::default()
    };
    assert_eq!(check_stopped(&mut hardware), Err(Error::RxWalkerEnabled));
    assert!(hardware.walker, "admission must not silently stop DMA");
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (1, 1));
}

#[test]
fn active_mac_is_rejected_without_polling_or_changing_hardware() {
    let mut hardware = Hardware {
        active: 3,
        walker: true,
        ..Hardware::default()
    };
    assert_eq!(
        check_stopped(&mut hardware),
        Err(Error::MacActive { state: 3 })
    );
    assert_eq!(hardware.active, 3);
    assert!(hardware.walker);
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (1, 0));
}

#[test]
fn release_rechecks_hardware_instead_of_reusing_admission_observation() {
    let mut hardware = Hardware::default();
    assert_eq!(check_stopped(&mut hardware), Ok(()));

    // PHY restoration can change baseband state. A successful entry check
    // cannot certify the caller's postcondition after those writes.
    hardware.active = 1;
    assert_eq!(
        check_stopped(&mut hardware),
        Err(Error::MacActive { state: 1 })
    );
    hardware.active = 0;
    hardware.walker = true;
    assert_eq!(check_stopped(&mut hardware), Err(Error::RxWalkerEnabled));
    hardware.walker = false;
    assert_eq!(check_stopped(&mut hardware), Ok(()));
    assert_eq!((hardware.mac_reads, hardware.walker_reads), (4, 3));
}

fn setup() -> super::MacInterruptSetup {
    super::MacInterruptSetup {
        inner: oer_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

fn registers() -> super::RadioRuntimeOwner {
    super::RadioRuntimeOwner::claim_for_validation()
}

#[test]
fn shutdown_confirmation_returns_the_same_frontier_only_after_a_stopped_check() {
    let rejected =
        super::confirm_stopped(registers(), setup(), |_| Err(Error::MacActive { state: 2 }))
            .err()
            .expect("active MAC must reject final-client shutdown");
    assert_eq!(rejected.error, Error::MacActive { state: 2 });

    let (_registers, _setup) =
        super::confirm_stopped(rejected.registers, rejected.interrupts, |_| Ok(()))
            .ok()
            .expect("stopped hardware must return the unchanged ownership frontier");
}
