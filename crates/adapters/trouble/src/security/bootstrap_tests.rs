//! The actual security-enabled Trouble Host boots through the production Controller core.

use bt_hci::{controller::ExternalController, param::Error as HciError};
use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicU8, Ordering::Relaxed},
    task::{Context, Poll, Waker},
};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_bluetooth_controller::{LeController, LeControllerConfig};
use oer_bluetooth_hci::*;
use oer_bluetooth_hci_transport::*;
use oer_bluetooth_radio::NoRadio;
use oer_bluetooth_runtime::{ServeExit, Served, serve};
use trouble_host::{BleHostError, Error as HostError, prelude::*};

struct Entropy {
    calls: AtomicU8,
    fail: bool,
}
impl LeRandomSource for Entropy {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        let sequence = self.calls.fetch_add(1, Relaxed) + 1;
        // Deterministic test fixture, never firmware entropy.
        if self.fail {
            Err(LeRandomUnavailable)
        } else {
            Ok([sequence; 8])
        }
    }
}

#[test]
fn security_host_initializes_using_four_standard_le_rand_completions() {
    run_bootstrap(false, false);
}

#[test]
fn security_host_rejects_entropy_failure_instead_of_initializing() {
    run_bootstrap(true, false);
}

#[test]
fn bond_working_set_changes_do_not_require_controller_privacy() {
    run_bootstrap(false, true);
}

fn run_bootstrap(fail: bool, bonds: bool) {
    let entropy = Entropy {
        calls: AtomicU8::new(0),
        fail,
    };
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap();
    let mut hci = LeControllerHciResources::<NoopRawMutex, 4, 4, 258>::new(config).unwrap();
    let LeControllerHciEndpoints { host, controller } = hci.split();
    let mut core = LeController::<'_, oer_bluetooth_radio::NoCapabilities, 12, 1>::new(
        LeControllerConfig {
            bootstrap: config,
            version: None,
        },
        Some(&entropy),
    );
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(ExternalController::<_, 4>::new(host), &mut resources).build();
    stack.set_io_capabilities(IoCapabilities::DisplayYesNo);
    stack
        .set_pairing_policy(PairingPolicy::NumericComparisonOnly)
        .unwrap();
    let mut runner = stack.runner();
    // Bootstrap never needs the radio. The service runs the port's body
    // until it is asked to stop, then disables and uninstalls the port.
    let stop = Signal::<NoopRawMutex, ()>::new();
    let mut controller_task = pin!(serve(
        NoRadio,
        (),
        &controller,
        &mut core,
        &oer_time_embassy::EmbassyClock,
        stop.wait(),
    ));
    // Public commands wait for Host initialization. Keep polling afterwards too:
    // the Host still executes post-initialization commands after opening that
    // gate. A single completion would miss a late startup failure.
    let probe = async {
        stack
            .command(bt_hci::cmd::info::ReadBdAddr::new())
            .await
            .unwrap();
        for _ in 0..8 {
            embassy_futures::yield_now().await;
        }
        stack
            .command(bt_hci::cmd::info::ReadBdAddr::new())
            .await
            .unwrap();
        if bonds {
            assert!(!stack.is_privacy_enabled());
            let record = trouble_host::BondInformation::new(
                trouble_host::Identity::from(trouble_host::Address::random([1, 2, 3, 4, 5, 0xc6])),
                trouble_host::LongTermKey::new(1),
                SecurityLevel::EncryptedAuthenticated,
                true,
            );
            stack.add_bond_information(record.clone()).unwrap();
            for _ in 0..4 {
                embassy_futures::yield_now().await;
            }
            stack
                .command(bt_hci::cmd::info::ReadBdAddr::new())
                .await
                .unwrap();
            stack.remove_bond_information(record.identity).unwrap();
            for _ in 0..4 {
                embassy_futures::yield_now().await;
            }
            stack
                .command(bt_hci::cmd::info::ReadBdAddr::new())
                .await
                .unwrap();
        }
    };
    let mut test = pin!(async {
        match select(probe, select(runner.run(), controller_task.as_mut())).await {
            Either::First(()) => {
                assert!(!fail);
            }
            Either::Second(Either::First(result)) => {
                assert!(fail, "Host stopped unexpectedly: {result:?}");
                assert!(matches!(
                    result,
                    Err(BleHostError::BleHost(HostError::Hci(
                        HciError::HARDWARE_FAILURE
                    )))
                ));
            }
            Either::Second(Either::Second(_)) => panic!("Controller unexpectedly returned"),
        }
        stop.signal(());
        assert_eq!(
            controller_task.as_mut().await,
            (Served::Stopped(ServeExit::Stopped), Some(()))
        );
    });
    // All peers live in this deterministic future: bound polling rather than
    // hanging the suite if bootstrap stops making progress. No wall-clock sleep.
    let mut completed = false;
    for _ in 0..1024 {
        if let Poll::Ready(()) = test.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            completed = true;
            break;
        }
    }
    assert!(completed, "bootstrap exceeded deterministic poll budget");
    assert_eq!(entropy.calls.load(Relaxed), if fail { 1 } else { 4 });
}
