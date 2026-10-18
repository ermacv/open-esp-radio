use oer_esp32s31_hal::types::MacKeyInstallOutcome;
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211_mac::crypto::{install_sta_group_ccmp, install_sta_pairwise_ccmp};

use super::*;

#[derive(Default)]
struct Hardware {
    cleared: std::vec::Vec<u8>,
}

impl CcmpKeyHardware for Hardware {
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

struct Control(bool);

impl ConnectedStaControlTeardown<Hardware, Tx> for Control {
    type Report = u8;
    type Error = u8;

    fn shutdown(
        &mut self,
        _hardware: &mut Hardware,
        _tx: &mut Tx,
    ) -> Result<Self::Report, Self::Error> {
        if self.0 { Err(1) } else { Ok(2) }
    }

    type Released = ();

    fn release(self) -> Self::Released {}
}

struct Rx(bool);

impl ConnectedStaRxPark<Hardware> for Rx {
    type Parked = u8;
    type Error = u8;

    fn try_park(self, _hardware: &mut Hardware) -> Result<Self::Parked, (Self, Self::Error)> {
        if self.0 { Err((self, 3)) } else { Ok(4) }
    }
}

struct Tx {
    active: bool,
    key: Option<oer_esp32s31_ieee80211_mac::crypto::StaPairwiseCcmpSlot>,
}

impl ConnectedStaTxTeardown for Tx {
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
            security: ConnectedTxSecurity::Wpa2Personal(
                self.key.take().expect("test TX owns pairwise key"),
            ),
            sequences: StaTxSequenceCounters::new(SequenceNumber::new(6).unwrap()),
            aggregate: 7,
        })
    }
}

fn services(
    hardware: &mut Hardware,
    control_failure: bool,
    rx_failure: bool,
    tx_active: bool,
) -> (
    SingleRoleServices<Hardware, Rx, Tx, Control>,
    ConnectedStaGroupSecurity,
) {
    let pairwise = install_sta_pairwise_ccmp(hardware, [1, 2, 3, 4, 5, 6], &[0x11; 16]).unwrap();
    let group = install_sta_group_ccmp(hardware, 1, &[0x22; 16]).unwrap();
    (
        SingleRoleServices::with_control(
            core::mem::take(hardware),
            Rx(rx_failure),
            Tx {
                active: tx_active,
                key: Some(pairwise),
            },
            Control(control_failure),
        ),
        ConnectedStaGroupSecurity::Wpa2Personal(group),
    )
}

#[test]
fn teardown_orders_control_rx_tx_and_both_key_clears() {
    let mut hardware = Hardware::default();
    let (services, group) = services(&mut hardware, false, false, false);
    let stopped = ConnectedStaTeardownPort::try_teardown(services, group)
        .unwrap_or_else(|_| panic!("idle mock owners must stop"));
    assert_eq!(stopped.parked_rx, 4);
    assert_eq!(stopped.tx_resources, 5);
    assert_eq!(
        stopped.sequences.peek_non_qos(),
        SequenceNumber::new(6).unwrap()
    );
    assert_eq!(stopped.aggregate, 7);
    assert_eq!(stopped.control, 2);
    assert_eq!(stopped.hardware.cleared, [1, 4]);
}

#[test]
fn already_parked_rx_crosses_teardown_without_touching_hardware() {
    let parked = AlreadyParkedRx::new(9_u8);
    let returned = <AlreadyParkedRx<u8> as ConnectedStaRxPark<Hardware>>::try_park(
        parked,
        &mut Hardware::default(),
    )
    .unwrap_or_else(|_| unreachable!("already-parked RX is infallible"));

    assert_eq!(returned, 9);
}

#[test]
fn each_failed_stage_returns_its_exact_frontier() {
    for (control, rx, tx, expected) in [
        (true, false, false, 1),
        (false, true, false, 2),
        (false, false, true, 3),
    ] {
        let mut hardware = Hardware::default();
        let (services, group) = services(&mut hardware, control, rx, tx);
        let failure = ConnectedStaTeardownPort::try_teardown(services, group)
            .err()
            .expect("selected stage must fail");
        let observed = match failure {
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
