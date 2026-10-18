//! The HCI codecs and a real `bt-hci`/Trouble Host over the in-process
//! transport.

use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{Opcode, OpcodeGroup, SyncCmd, controller_baseband::Reset},
    controller::{Controller, ExternalController},
    event::{CommandComplete, CommandCompleteWithStatus},
    param::Error as HciError,
    transport::Transport,
};
use embassy_futures::{
    block_on,
    join::{join, join3},
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, BootstrapCommandCompleteEvent, BootstrapHostBuffers,
    BootstrapPhase, HciCommandPacket, HciControllerResponse, LeAcceptListCommandCompleteEvent,
    LeControllerBootstrap, LeControllerBootstrapConfig, LeControllerCommandClassification,
    LeRandCommandCompleteEvent, LeRandomSource, LeRandomUnavailable,
    classify_le_controller_command,
};
use trouble_host::{BleHostError, Error as TroubleError, HostResources, Packet, PacketPool};

use oer_bluetooth_hci::HostToControllerFrame;

use crate::{
    InProcessHciControllerTransport, InProcessHciHostTransport, LeControllerHciEndpoints,
    LeControllerHciResources, in_process::InProcessHciChannel,
};

type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 80>;

fn requires_trouble_controller<C: trouble_host::Controller>() {}

#[test]
fn bt_hci_and_trouble_share_one_controller_contract() {
    type ContractTransport = InProcessHciHostTransport<'static, NoopRawMutex, 1, 1, 16>;
    requires_trouble_controller::<ExternalController<ContractTransport, 1>>();
}

#[test]
fn unknown_command_response_roundtrips_through_the_real_hci_boundary() {
    let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
    let LeControllerCommandClassification::Unsupported(response) =
        classify_le_controller_command(HciCommandPacket::new(opcode, &[]))
    else {
        panic!("a vendor opcode outside the inventory must be unsupported");
    };
    let mut channel = InProcessHciChannel::<NoopRawMutex, 1, 1, 16>::new();
    let (host, controller) = channel.split();

    controller
        .try_publish(response.kind(), response.as_bytes())
        .expect("the owned completion fits the empty Controller queue");

    let mut packet = [0; 16];
    let ControllerToHostPacket::Event(event) =
        block_on(host.read(&mut packet)).expect("the Host receives the retained completion")
    else {
        panic!("Unknown Command completion changed packet kind");
    };
    let complete = CommandComplete::from_hci_bytes_complete(event.data)
        .expect("the response is a complete Command Complete event");
    let complete: CommandCompleteWithStatus<'_> = complete
        .try_into()
        .expect("the response retains its status return parameter");

    assert_eq!(complete.cmd_opcode, opcode);
    assert_eq!(complete.status, HciError::UNKNOWN_CMD.to_status());
    assert!(complete.return_param_bytes.is_empty());
}

struct TestPacket([u8; 64]);

impl AsRef<[u8]> for TestPacket {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for TestPacket {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Packet for TestPacket {}

struct TestPacketPool;

impl PacketPool for TestPacketPool {
    type Packet = TestPacket;

    const MTU: usize = 64;

    fn allocate() -> Option<Self::Packet> {
        Some(TestPacket([0; 64]))
    }

    fn capacity() -> usize {
        2
    }
}

#[test]
fn external_controller_exec_completes_from_bootstrap_dispatch() {
    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
        27,
        1,
    )
    .unwrap();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let external = ExternalController::<_, 1>::new(host);

    block_on(async {
        let reset = Reset::new();
        let mut event_buffer = external.alloc_buf().unwrap();
        let worker = async {
            let mut command_buffer = [0; 80];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut command_buffer).await.unwrap()
            else {
                panic!("Reset changed packet kind");
            };
            let response = dispatch_test_packet(&mut bootstrap, command);
            controller
                .publish(bt_hci::PacketKind::Event, response.as_bytes())
                .await
                .unwrap();
            controller
                .publish(bt_hci::PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
        };

        let (completed, observed, ()) = join3(
            reset.exec(&external),
            external.read(&mut event_buffer),
            worker,
        )
        .await;
        completed.unwrap();
        assert!(matches!(
            observed.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    });
}

#[test]
fn real_trouble_runner_reaches_initialized_over_the_source_owned_hci_boundary() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap();
    // Trouble's security feature requests entropy during Runner startup;
    // workspace feature unification can enable it for this test too.
    // Deterministic entropy is a host-test input, never a production RNG.
    let entropy = BootstrapTestEntropy(core::sync::atomic::AtomicUsize::new(0));
    let mut hci = LeControllerHciResources::<NoopRawMutex, 4, 1, 255>::new(config).unwrap();
    let LeControllerHciEndpoints { host, controller } = hci.split();
    let mut bootstrap = LeControllerBootstrap::new(config);
    let external = ExternalController::<_, 2>::new(host);
    let mut resources = HostResources::<TestPacketPool, 1, 1>::new();
    let stack = trouble_host::new(external, &mut resources).build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let stop = Signal::<NoopRawMutex, ()>::new();

    block_on(async {
        let initialized_probe = async {
            let result = peripheral.set_filter_accept_list(&[]).await;
            stop.signal(());
            result
        };

        // This public Trouble operation cannot emit its command until the
        // Runner has completed its initial ACL/mask bootstrap and published
        // the internal initialized state. This bootstrap-only Controller then
        // rejects the operational command because it owns no filter list.
        let controller_and_probe = join(
            drive_bootstrap_until(&controller, &mut bootstrap, &entropy, &stop),
            initialized_probe,
        );
        match select(runner.run(), controller_and_probe).await {
            Either::First(result) => {
                panic!("Trouble Runner stopped during bootstrap: {result:?}")
            }
            Either::Second(((), probe_result)) => {
                assert!(matches!(
                    probe_result,
                    Err(BleHostError::BleHost(TroubleError::Hci(
                        HciError::CMD_DISALLOWED
                    )))
                ));
            }
        }
    });

    assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    assert_eq!(
        bootstrap.host_buffers(),
        Some(BootstrapHostBuffers {
            acl_data_packet_length: 255,
            total_acl_data_packets: 1,
        })
    );
}

struct BootstrapTestEntropy(core::sync::atomic::AtomicUsize);

impl LeRandomSource for BootstrapTestEntropy {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        let sequence = self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
        Ok((sequence as u64).to_le_bytes())
    }
}

/// A minimal bootstrap-only Controller over the raw transport.
async fn drive_bootstrap_until(
    controller: &InProcessHciControllerTransport<'_, NoopRawMutex, 4, 1, 255>,
    bootstrap: &mut LeControllerBootstrap,
    entropy: &BootstrapTestEntropy,
    stop: &Signal<NoopRawMutex, ()>,
) {
    let mut buffer = [0; 255];
    loop {
        match select(stop.wait(), controller.wait_receive_ready()).await {
            Either::First(()) => return,
            Either::Second(()) => {}
        }
        let Ok(HostToControllerFrame::Command(command)) = controller.try_receive(&mut buffer)
        else {
            panic!("Trouble bootstrap must submit an HCI command");
        };
        let response = match classify_le_controller_command(command) {
            LeControllerCommandClassification::Bootstrap(command) => {
                let response = bootstrap.dispatch(command, true);
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            LeControllerCommandClassification::Random(_) => {
                let response = LeRandCommandCompleteEvent::success(entropy.random_bytes().unwrap());
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            LeControllerCommandClassification::Unsupported(response) => {
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            LeControllerCommandClassification::AcceptList(command) => {
                let response = LeAcceptListCommandCompleteEvent::new(
                    command.opcode(),
                    HciError::CMD_DISALLOWED.to_status(),
                );
                controller
                    .publish(bt_hci::PacketKind::Event, response.as_bytes())
                    .await
            }
            _ => panic!("bootstrap must not start a radio role"),
        };
        response.unwrap();
    }
}

/// Dispatch a bootstrap command; the transport tests send no other command.
fn dispatch_test_packet(
    bootstrap: &mut LeControllerBootstrap,
    command: HciCommandPacket<'_>,
) -> BootstrapCommandCompleteEvent {
    match classify_le_controller_command(command) {
        LeControllerCommandClassification::Bootstrap(command) => bootstrap.dispatch(command, false),
        LeControllerCommandClassification::MalformedBootstrap(response) => response,
        _ => panic!("the transport tests send only bootstrap commands"),
    }
}
