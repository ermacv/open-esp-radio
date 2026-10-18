use crate::{
    datapath::irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
    roles::station::tx::ConnectedTxTeardownParts,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_esp32s31_hal::types::MacKeyInstallOutcome;

use oer_esp32s31_ieee80211_mac::{
    crypto::{install_sta_group_ccmp, install_sta_pairwise_ccmp},
    irq::MacInterruptRoute,
};

use super::*;

struct TestRoute {
    active: bool,
}

impl MacInterruptRoute for TestRoute {
    type Platform = ();
    type Setup = u32;
    type Error = u8;

    fn activate(
        &mut self,
        _platform: &Self::Platform,
        setup: Self::Setup,
        _event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        self.active = true;
        let _ = setup;
        Ok(())
    }

    fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        self.active = false;
        Ok(41)
    }
}

struct TestRunner(u32, u16);

impl ConnectedEpochRunnerOwner for TestRunner {
    type Network = u32;
    type Services = u16;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
        (self.0, self.1)
    }
}

fn active_epoch() -> InterruptEpoch<'static, TestRoute, CriticalSectionRawMutex> {
    static MAC: EmbassyMacIrqRuntime<CriticalSectionRawMutex> = EmbassyMacIrqRuntime::new();
    static POWER: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> = EmbassyPowerIrqRuntime::new();
    let mut epoch = InterruptEpoch::new(TestRoute { active: false }, 40, &MAC, &POWER);
    epoch
        .activate(&(), oer_esp32s31_hal::types::MacInterruptMask::COLD_RX)
        .expect("test route activates");
    epoch
}

#[test]
fn reusable_parts_retain_the_installed_irq_epoch() {
    let epoch = active_epoch();
    let stopped = quiesce_esp32s31_connected_epoch(epoch, &(), TestRunner(7, 8))
        .unwrap_or_else(|_| panic!("ready shutdown must succeed"));
    assert_eq!(stopped.network, 7);
    assert_eq!(stopped.services, 8);
    assert!(stopped.interrupt.is_active());
}

#[derive(Default)]
struct TeardownHardware {
    cleared: std::vec::Vec<u8>,
}

impl CcmpKeyHardware for TeardownHardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _index: u8,
        _identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.cleared.push(index);
    }
}

struct TeardownControl {
    fail: bool,
}

impl ConnectedStaControlTeardown<TeardownHardware, TeardownTx> for TeardownControl {
    type Report = u8;
    type Error = u8;

    fn shutdown(
        &mut self,
        _hardware: &mut TeardownHardware,
        _tx: &mut TeardownTx,
    ) -> Result<Self::Report, Self::Error> {
        if self.fail { Err(1) } else { Ok(2) }
    }

    type Released = ();

    fn release(self) -> Self::Released {}
}

struct TeardownRx {
    fail: bool,
}

impl ConnectedStaRxPark<TeardownHardware> for TeardownRx {
    type Parked = u8;
    type Error = u8;

    fn try_park(
        self,
        _hardware: &mut TeardownHardware,
    ) -> Result<Self::Parked, (Self, Self::Error)> {
        if self.fail { Err((self, 3)) } else { Ok(4) }
    }
}

struct TeardownTx {
    active: bool,
    pairwise: Option<oer_esp32s31_ieee80211_mac::crypto::StaPairwiseCcmpSlot>,
}

impl ConnectedStaTxTeardown for TeardownTx {
    type Resources = u8;
    type Aggregate = u8;

    fn try_return(
        mut self,
    ) -> Result<ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self> {
        if self.active {
            return Err(self);
        }
        Ok(ConnectedTxTeardownParts {
            resources: 5,
            security: oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxSecurity::Wpa2Personal(
                self.pairwise.take().expect("test TX owns its pairwise key"),
            ),
            sequences: oer_ieee80211_mac::station::StaTxSequenceCounters::new(
                SequenceNumber::new(6).unwrap(),
            ),
            aggregate: 7,
        })
    }
}

type TeardownServices =
    SingleRoleServices<TeardownHardware, TeardownRx, TeardownTx, TeardownControl>;

fn teardown_frontier(
    control_failure: bool,
    rx_failure: bool,
    tx_active: bool,
) -> (
    ConnectedEpochQuiesced<u8, u32, TeardownServices>,
    ConnectedStaGroupSecurity,
) {
    let mut hardware = TeardownHardware::default();
    let pairwise = install_sta_pairwise_ccmp(&mut hardware, [1, 2, 3, 4, 5, 6], &[0x11; 16])
        .expect("test hardware installs its pairwise key");
    let group = install_sta_group_ccmp(&mut hardware, 1, &[0x22; 16])
        .expect("test hardware installs its group key");
    let services = SingleRoleServices::with_control(
        hardware,
        TeardownRx { fail: rx_failure },
        TeardownTx {
            active: tx_active,
            pairwise: Some(pairwise),
        },
        TeardownControl {
            fail: control_failure,
        },
    );
    (
        ConnectedEpochQuiesced {
            interrupt: 16,
            interrupt_drain: MacInterruptEpochDrain::default(),
            network: 17,
            services,
        },
        ConnectedStaGroupSecurity::Wpa2Personal(group),
    )
}

#[test]
fn complete_teardown_returns_network_and_driver_frontier_together() {
    let (frontier, group) = teardown_frontier(false, false, false);
    let stopped = frontier
        .try_teardown(group)
        .unwrap_or_else(|_| panic!("idle connected frontier must stop"));
    assert_eq!(stopped.network, 17);
    assert_eq!(stopped.driver.parked_rx, 4);
    assert_eq!(stopped.driver.tx_resources, 5);
    assert_eq!(stopped.driver.aggregate, 7);
    assert_eq!(stopped.driver.control, 2);
    assert_eq!(stopped.driver.hardware.cleared, [1, 4]);
}

#[test]
fn every_driver_teardown_failure_retains_network_and_driver_owners() {
    for (control, rx, tx, expected) in [
        (true, false, false, 1),
        (false, true, false, 2),
        (false, false, true, 3),
    ] {
        let (frontier, group) = teardown_frontier(control, rx, tx);
        let failure = frontier
            .try_teardown(group)
            .err()
            .expect("selected teardown stage must fail");
        assert_eq!(failure.network, 17);
        let observed = match failure.error {
            ConnectedStaTeardownFailure::Control { error, .. } => {
                assert_eq!(error, 1);
                1
            }
            ConnectedStaTeardownFailure::Rx { error, .. } => {
                assert_eq!(error, 3);
                2
            }
            ConnectedStaTeardownFailure::TxActive { .. } => 3,
        };
        assert_eq!(observed, expected);
    }
}
