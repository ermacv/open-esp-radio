//! The access point's runner over the real S31 port, device, receive
//! session and DMA owners, on the backend's model of the hardware atoms and
//! virtual time: its start (receive rules, group key, TSF) and its stop.
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
    AP_BUFFERS, AP_KEYS, ApEnd, ApNonces, ApPolicy, ApResources, ApRunner, ApShared, GROUP_KEY_ID,
    network::ApNetworkStorage,
};
use oer_protocols_ieee80211_wire::{
    beacon::AP_BEACON_CAPACITY, phy::LegacyRate, security::spp::Interface,
    sequence::SequenceNumber, ssid::WifiSsid,
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

/// Start the access point over the real S31 port and run `script` on it.
fn on_the_s31_port(
    script: impl FnOnce(&mut dyn FnMut(&str, &mut dyn FnMut(&ApShared) -> bool), &ApShared),
) {
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
            &WifiSsid::new(b"s31-ap").unwrap(),
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
        let runner = ApRunner::new(
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
                ApEnd::Stopped(_) => "stopped".into(),
                ApEnd::StartRefused(_, refused) => std::format!("start refused: {refused:?}"),
                ApEnd::StopUnconfirmed(_, unconfirmed) => std::format!("{unconfirmed:?}"),
                ApEnd::Faulted(_, fault) => std::format!("faulted: {fault:?}"),
                ApEnd::Contract(_, contract) => std::format!("contract: {contract:?}"),
                ApEnd::Refused(_, refusal) => std::format!("refused: {refusal:?}"),
            });
        });
        let mut task = Task::new(run.as_mut());

        let mut until = |what: &str, done: &mut dyn FnMut(&ApShared) -> bool| {
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
                    if !ran && !air {
                        break;
                    }
                }
                if done(&shared) {
                    return;
                }
                if let Some(ended) = seen.borrow().ended.clone() {
                    panic!("{what}: the runner ended: {ended}");
                }
                if !clock.pass_to_next() {
                    panic!("{what}: nothing is woken and no time is due");
                }
            }
            panic!("{what}: {PASSES} passes without an end");
        };
        script(&mut until, &shared);
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

#[test]
fn the_access_point_starts_and_stops_on_the_s31_port() {
    on_the_s31_port(|until, shared| {
        // The receive rules, the group key through its receive cut, then
        // the TSF: the access point runs.
        until("the access point runs", &mut |shared| {
            shared.status().running
        });
        // Its beacons go at their TBTTs.
        until("a beacon ends", &mut |shared| shared.status().beacons >= 2);
        shared.request_stop();
        until("the access point stops", &mut |shared| {
            !shared.status().running
        });
    });
}
