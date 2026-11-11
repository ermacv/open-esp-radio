//! The access point's runner over the real S31 port, device, receive
//! session and DMA owners, on the backend's model of the hardware atoms and
//! virtual time. The S31 declares no beacon Timestamp rule, so the runner
//! refuses the port before its radio starts; the runner's positive start,
//! beacon and stop paths run over the model only.
//!
//! The bench polls the runner only once woken, runs the interrupt handler's
//! signals before each poll, ends each PPDU the air takes and moves time to
//! the next deadline when nothing is woken.

use super::*;
use oer_chips_family_radio_ieee80211_policy::access_point::ADVERTISEMENT;
use oer_protocols_ieee80211_contracts_port::GroupKeyId;
use oer_protocols_ieee80211_core_roles_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    beacon::ApBeacon, engine::ApEngine, security_policy::ApSecurityPolicy,
};
use oer_protocols_ieee80211_core_security_rsn::{PmkDerivation, frames::RsnGtk};
use oer_protocols_ieee80211_runtime::access_point::{
    AP_BUFFERS, AP_KEYS, ApEnd, ApNonces, ApPolicy, ApResources, ApRunner, ApShared, ApUnsupported,
    GROUP_KEY_ID, StopWatch, network::ApNetworkStorage, serves_beacons,
};
use oer_protocols_ieee80211_wire::{
    beacon::AP_BEACON_CAPACITY, phy::LegacyRate, security::spp::Interface,
    sequence::SequenceNumber, ssid::Ssid,
};

struct Nonces;

impl ApNonces for Nonces {
    fn nonce(&mut self) -> Option<[u8; 32]> {
        Some([4; 32])
    }
}

/// What the bench saw of the run.
struct Seen {
    running: bool,
    ended: Option<std::string::String>,
}

/// Run the bench until `done` holds, given the shared status and the run's
/// end so far.
type Until<'u> = &'u mut dyn FnMut(&str, &mut dyn FnMut(&ApShared, Option<&str>) -> bool);

/// Start the access point over the real S31 port and run `script` on it,
/// with the bench's register model and clock.
fn on_the_s31_port(script: impl FnOnce(Until<'_>, &ApShared, &Atoms, &Clocked)) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Clocked::new();
        let reader = clock.clock.clone();
        let atoms = Atoms::new(Rc::new(move || reader.now().as_micros()));
        let arena: &'static mut WifiRxStatic<N, B, S, Q, Deferred, HostDma> =
            leak(bytemuck::Zeroable::zeroed());
        let (rx, _stop, _frames) = WifiRxArena::bind(
            Placed::host(arena),
            WifiIrqSignals {
                mac: deferred_signal().0,
                receive: deferred_signal().0,
                power: deferred_signal().0,
            },
        )
        .ok()
        .expect("a zeroed static binds");
        let requests = rx.requests();
        let walker: &'static RefCell<RxWalkerModel> = leak(RefCell::new(RxWalkerModel::new()));
        let cell: &'static HandlerCell<WifiIrqLine<Deferred>> = leak(HandlerCell::new());
        let line = Rc::new(ModelLine::new(cell, requests, atoms.clone()));
        let device: &'static mut Device = leak(RadioDevice::new(RadioParts {
            wifi: ModelFrontier::new(ModelRegisters::new(atoms.clone(), walker), channel()),
            rx,
            interrupt_cell: cell,
            bound: HostActivation,
            actor: (),
            timer: clock.clone(),
            doorbell: ModelDoorbell(line.clone()),
            domain: domain(),
            power_limits: LIMITS,
            #[cfg(oer_trace)]
            stall: leak(
                oer_chips_esp32s31_radio_ieee80211_backend::rx::counters::stall::RxStall::new(),
            ),
        }));
        let pool: &'static TxPool<TX> = model_pool().expect("model pool");
        let ceiling = domain()
            .permit(RegulatedChannel::from(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            ))
            .unwrap()
            .tx_ceiling(LIMITS);
        let transmitter = OrdinaryTxOwner::new(WifiTxResources {
            slot: Pin::static_mut(leak(
                DescriptorSlot::new_model_descriptor().expect("model descriptor"),
            )),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: CappedTxPowerProfile::new(ModelPower, ceiling),
            entropy: entropy as fn() -> u32,
            timer: clock.clone(),
        })
        .into_idle()
        .ok()
        .expect("idle transmitter");
        let parts = PortParts {
            device,
            shared: leak(PortSharedStorage::new()),
            transmitter,
            pool,
            bank: AP_BUFFERS as u8,
            descriptors: descriptors(),
            config: LowerMacConfig {
                station_address: STATION,
                channel: channel(),
                publication_timeout: PUBLICATION_BOUND,
                tsf_epoch: 1,
            },
            access_point_address: IndividualAddress::new(ACCESS_POINT).unwrap(),
            coex: leak(Coex),
            observations: (),
        };
        let mut installations = Installations::<_, port::InstallationPools>::claim(claim);
        let minted: Minted<_, port::InstallationPools> = installations.first().unwrap();
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let port::PortInstallation {
            port,
            commands,
            resources,
        } = Esp32s31Port::install(parts, minted, driver)
            .ok()
            .expect("install");

        // The access point's resources, as the composition gives them.
        let mut tickets = heapless::Vec::new();
        for ticket in resources.tickets {
            tickets
                .push(ticket)
                .ok()
                .expect("the runner holds every ticket");
        }
        let mut buffers = heapless::Vec::new();
        for buffer in resources.buffers.into_iter().take(AP_BUFFERS) {
            buffers.push(buffer).ok().expect("the runner's buffers");
        }
        let (start, bank) = resources.access_point_association.into_parts();
        let (pairwise_slots, group_slots) = bank.into_parts();
        let mut pairwise = heapless::Vec::new();
        for slot in pairwise_slots.into_iter().take(AP_KEYS) {
            pairwise.push(slot).ok().expect("the runner's slots");
        }
        let [group, ..] = group_slots;
        let ap_resources = ApResources {
            tickets,
            access: resources.terminal_access,
            buffers,
            configure: resources.access_point_commands.configure.with_context(()),
            association: start,
            pairwise,
            group,
            group_permission: resources.access_point_group_permission,
            beacon: resources.access_point_beacon,
        };
        let pmk = {
            let mut derivation = PmkDerivation::new(b"password", b"s31-ap").unwrap();
            loop {
                if let Some(pmk) = derivation.step() {
                    break pmk;
                }
            }
        };
        let key_id = GroupKeyId::new(GROUP_KEY_ID).unwrap();
        let service = AccessPointService::<Interface>::new(
            ACCESS_POINT,
            pmk,
            RsnGtk::new(key_id, true, &[0x55; 16]),
            1,
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            leak(AccessPointPeerStorage::new()).lend(),
        )
        .unwrap();
        let beacon = ApBeacon::new(
            leak([0; AP_BEACON_CAPACITY]),
            &ADVERTISEMENT,
            ACCESS_POINT,
            &Ssid::new(b"s31-ap").unwrap(),
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            100,
            1,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        )
        .unwrap();
        let engine = ApEngine::new(
            service,
            beacon,
            ADVERTISEMENT,
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            key_id,
        );
        let (_device, _transmit, end) =
            leak(ApNetworkStorage::new()).split::<core::convert::Infallible>();
        let mut runner = ApRunner::new(
            port,
            commands,
            ap_resources,
            engine,
            ApPolicy {
                management_rate: oer_protocols_ieee80211_contracts_port::PhyRate::Legacy(
                    LegacyRate::Dsss1M,
                ),
                management_transmissions: core::num::NonZeroU8::new(4).unwrap(),
                data_transmissions: core::num::NonZeroU8::new(4).unwrap(),
                packet_numbers:
                    oer_chips_family_radio_ieee80211_policy::ccmp::TX_PACKET_NUMBER_STEP,
                group_generation: 1,
            },
            supported(),
            [0x55; 16],
            end,
            (),
            Nonces,
        );
        let shared = ApShared::new();
        // Observations, not limits: what the runner, its end and its run
        // occupy over the S31 port.
        std::println!(
            "sizes: runner {} B, end {} B",
            core::mem::size_of_val(&runner),
            core::mem::size_of::<ApEnd<oer_chips_esp32s31_radio_ieee80211_backend::port::Fault>>(),
        );
        let seen = RefCell::new(Seen {
            running: false,
            ended: None,
        });
        let airy = Airy {
            clock: clock.clone(),
        };
        let mut run = pin!(async {
            let end = runner.run(&shared, &airy).await;
            seen.borrow_mut().ended = Some(match end {
                ApEnd::Stopped => "stopped".into(),
                ApEnd::StartRefused(refused) => std::format!("start refused: {refused:?}"),
                ApEnd::StopUnconfirmed(unconfirmed) => std::format!("{unconfirmed:?}"),
                ApEnd::Faulted(fault) => std::format!("faulted: {fault:?}"),
                ApEnd::Contract(contract) => std::format!("contract: {contract:?}"),
                ApEnd::Refused(refusal) => std::format!("refused: {refusal:?}"),
                ApEnd::Unrecycled(fault) => std::format!("unrecycled: {fault:?}"),
                ApEnd::Unkept => "unkept".into(),
                ApEnd::Unaccepted => "unaccepted".into(),
                ApEnd::Unsupported(unsupported) => {
                    // Every owner stays in the runner, none apart.
                    let whole = runner.take_kept().is_none()
                        && runner.take_returned().is_none()
                        && runner.take_unrecycled().is_none();
                    std::format!(
                        "unsupported: {unsupported:?}, {}",
                        if whole {
                            "owners in the runner"
                        } else {
                            "an owner apart"
                        }
                    )
                }
            });
            if !matches!(end, ApEnd::Stopped | ApEnd::Unsupported(_)) {
                // The run ended before its stop: the stop goes on until the
                // port confirms it.
                let finished = runner.finish_stop(&airy, |_| {}).await;
                if let Some(ended) = seen.borrow_mut().ended.as_mut() {
                    ended.push_str(match finished {
                        Ok(StopWatch::Confirmed) => ", stop confirmed",
                        Ok(StopWatch::Unkept) => ", an item unkept",
                        Ok(StopWatch::Faulted { .. }) => ", stop unconfirmed, a fault kept",
                        Ok(StopWatch::DeviceFailed { .. }) => {
                            ", stop unconfirmed, the device failed it"
                        }
                        Err(_) => ", stop confirmed, a unit unrecycled",
                    });
                }
            }
        });
        std::println!(
            "sizes: run with its bench {} B",
            core::mem::size_of_val(&*run)
        );
        let mut task = Task::new(run.as_mut());

        let mut until = |what: &str, done: &mut dyn FnMut(&ApShared, Option<&str>) -> bool| {
            for _ in 0..PASSES {
                for _ in 0..PASSES {
                    oer_chips_family_platform_interrupt_table::signal::service::<Deferred>();
                    let ran = task.run();
                    if ran {
                        clock.pass(POLL_MICROS);
                    }
                    let frames = atoms.take_air();
                    let air = !frames.is_empty();
                    for frame in frames {
                        line.latch(atoms.end_ppdu(frame.queue, 0, 0));
                    }
                    seen.borrow_mut().running = shared.status().running;
                    // Each step is looked at: a state one turn passes
                    // through is seen.
                    let ended = seen.borrow().ended.clone();
                    if done(&shared, ended.as_deref()) {
                        return;
                    }
                    if !ran && !air {
                        break;
                    }
                }
                if !clock.pass_to_next() {
                    panic!("{what}: nothing is woken and no time is due");
                }
            }
            panic!("{what}: {PASSES} passes without an end");
        };
        script(&mut until, &shared, &atoms, &clock);
        let _held = &seen;
    }));
}

/// The time between two reads of the runner's clock.
const STEP_MICROS: u64 = 50;

/// The runner's clock, on which time runs as on the chip: every read is
/// [`STEP_MICROS`] after the one before, so no two turns share an instant.
struct Airy {
    clock: Clocked,
}

impl Clock for Airy {
    fn now(&self) -> Instant {
        self.clock.pass(STEP_MICROS);
        self.clock.now()
    }
}

impl Timer for Airy {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.clock.wait_until(deadline)
    }
}

/// The S31 declares no beacon Timestamp rule the runner writes: a run over
/// the real S31 port ends unsupported before it asks the radio to start.
/// The MAC runtime never runs and no beacon is counted; the runner
/// keeps every owner (none kept apart, returned or unrecycled), and the
/// composition's start check refuses with the same cause first.
#[test]
fn the_access_point_over_the_s31_port_is_refused_unsupported_before_the_radio_starts() {
    assert_eq!(
        serves_beacons::<Caps>(),
        Err(ApUnsupported::BeaconTimestamp)
    );
    on_the_s31_port(|until, shared, atoms, _| {
        until("the run ends unsupported", &mut |shared, ended| {
            assert!(!atoms.running(), "the MAC runtime never runs");
            assert!(!shared.status().running, "the access point never runs");
            match ended {
                None => false,
                Some(ended) => {
                    assert_eq!(ended, "unsupported: BeaconTimestamp, owners in the runner");
                    true
                }
            }
        });
        assert_eq!(shared.status().beacons, 0);
    });
}
