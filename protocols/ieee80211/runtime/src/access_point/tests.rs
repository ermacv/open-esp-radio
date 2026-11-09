//! The runner over the model's port: its start order, its beacons at the
//! TBTTs, a probe's answer, and a stop that returns every owner.

use super::*;
use core::cell::Cell;
use core::convert::Infallible;
use std::boxed::Box;
use std::rc::Rc;
use std::vec::Vec;

use embassy_futures::select::{Either, select};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth, RegulatedChannel};
use oer_protocols_ieee80211_core_roles_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    beacon::ApBeacon, security_policy::ApSecurityPolicy,
};
use oer_protocols_ieee80211_core_security_rsn::frames::RsnGtk;
use oer_protocols_ieee80211_testing_model::{
    ModelAccessPointTsf, ModelCaps24, ModelContext, ModelInstall, ModelLarge, ModelPort,
    ModelRxMeta,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress,
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    beacon::AP_BEACON_CAPACITY,
    ccmp::GroupKeyId,
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    phy::LegacyRate,
    security::spp::Association,
    sequence::SequenceNumber,
    ssid::WifiSsid,
};
use oer_protocols_radio_port::{Installations, LocalId, SlotIdentity, with_local_slot};

use super::network::ApNetworkStorage;

const STATION: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 9];
const SSID: &[u8] = b"runner";
const PASSPHRASE: &[u8] = b"password";

struct Contexts;

impl ModelContext for Contexts {
    type Body = Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

type Port<'s> = ModelPort<'s, LocalId<'s>, ModelCaps24, Contexts>;

const ADVERTISEMENT: Advertisement = match Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    match WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: oer_portable_wire::U8::constant::<3>(),
            ecw_min: oer_portable_wire::U8::constant::<4>(),
            ecw_max: oer_portable_wire::U8::constant::<10>(),
            txop_limit_units_32_us: 0,
        }; 4],
    ) {
        Some(value) => value,
        None => panic!("the WMM parameters fit their fields"),
    },
    0x0421,
) {
    Some(value) => value,
    None => panic!("the advertisement leaves Privacy to the security mode"),
};

struct Nonces;

impl ApNonces for Nonces {
    fn nonce(&mut self) -> Option<[u8; 32]> {
        Some([4; 32])
    }
}

fn channel() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn supported() -> SupportedChannel<ModelCaps24> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(channel()))
            .unwrap(),
    )
    .unwrap()
}

fn policy() -> ApPolicy {
    ApPolicy {
        management_rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        management_transmissions: NonZeroU8::new(2).unwrap(),
        data_transmissions: NonZeroU8::new(4).unwrap(),
        packet_numbers: CcmpPacketNumberStep::ONE,
        group_generation: 1,
    }
}

fn probe_request() -> Vec<u8> {
    let mut frame = std::vec![0x40, 0, 0, 0];
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&[0, SSID.len() as u8]);
    frame.extend_from_slice(SSID);
    frame.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    frame
}

/// The frame controls of the access point's frames on the air.
fn sent_controls(air: &oer_protocols_ieee80211_testing_model::ModelAir) -> Vec<u8> {
    air.sent()
        .iter()
        .filter(|frame| frame.role == oer_protocols_ieee80211_contracts_port::VifRole::AccessPoint)
        .filter_map(|frame| frame.frame.first().copied())
        .collect()
}

/// Build a runner over the model's port of `$claim` on `$clock`: binds
/// `$runner`, its `$air`, its network `$device` and the shared `$shared`.
macro_rules! fixture {
    ($claim:ident, $clock:expr => $runner:ident, $air:ident, $device:ident, $shared:ident) => {
        let mut installations = Installations::claim($claim);
        let (install, $air) = Port::install(
            &mut installations,
            channel(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            $clock,
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let shared_resources = resources.shared;
        let ap = resources.access_point;
        let mut tickets = heapless::Vec::new();
        for ticket in shared_resources.tickets {
            tickets.push(ticket).ok().unwrap();
        }
        let mut buffers = heapless::Vec::new();
        for buffer in shared_resources.large {
            buffers.push(buffer).ok().unwrap();
        }
        let [pairwise_slot, spare] = ap.pairwise;
        let mut pairwise = heapless::Vec::new();
        pairwise.push(pairwise_slot).ok().unwrap();
        pairwise.push(spare).ok().unwrap();
        let [group, _spare_group] = ap.group;
        let resources = ApResources::<'_, Port<'_>, ModelLarge> {
            tickets,
            access: shared_resources.terminal_access,
            buffers,
            configure: ap.commands.configure,
            association: ap.association,
            pairwise,
            group,
            group_permission: ap.group_permission,
            beacon: ap.beacon,
        };
        let storage = Box::leak(Box::new(AccessPointPeerStorage::new()));
        let service = AccessPointService::<Association>::new(
            ACCESS_POINT,
            pmk(),
            RsnGtk::new(GroupKeyId::new(GROUP_KEY_ID).unwrap(), true, &[0x55; 16]),
            1,
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            storage.lend(),
        )
        .unwrap();
        let template = Box::leak(Box::new([0; AP_BEACON_CAPACITY]));
        let beacon = ApBeacon::new(
            template,
            &ADVERTISEMENT,
            ACCESS_POINT,
            &WifiSsid::new(SSID).unwrap(),
            channel(),
            100,
            2,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        )
        .unwrap();
        let engine = ApEngine::new(
            service,
            beacon,
            ADVERTISEMENT,
            channel(),
            GroupKeyId::new(GROUP_KEY_ID).unwrap(),
        );
        let network = Box::leak(Box::new(ApNetworkStorage::new()));
        let ($device, _transmit, end) = network.split::<Infallible>();
        let mut $runner = ApRunner::new(
            port,
            commands,
            resources,
            engine,
            policy(),
            supported(),
            [0x55; 16],
            end,
            (),
            Nonces,
        );
        let $shared = ApShared::new();
    };
}

fn pmk() -> oer_protocols_ieee80211_core_security_rsn::Pmk {
    let mut derivation =
        oer_protocols_ieee80211_core_security_rsn::PmkDerivation::new(PASSPHRASE, SSID).unwrap();
    loop {
        if let Some(pmk) = derivation.step() {
            break pmk;
        }
    }
}

/// The runner's clock: each turn reads it once. More reads than
/// [`SPIN_LIMIT`] between two polls of the test's script are a runner that
/// turns without yielding.
struct Counted {
    clock: Rc<VirtualClock<8>>,
    since_script: Cell<u32>,
    reads: Cell<u32>,
}

const SPIN_LIMIT: u32 = 64;

impl Counted {
    fn new() -> Self {
        Self {
            clock: Rc::new(VirtualClock::new()),
            since_script: Cell::new(0),
            reads: Cell::new(0),
        }
    }

    /// Let the runner run `polls` times while time stands still; the reads
    /// of the clock meanwhile.
    async fn idle(&self, polls: usize) -> u32 {
        let before = self.reads.get();
        for _ in 0..polls {
            self.since_script.set(0);
            embassy_futures::yield_now().await;
        }
        self.reads.get().saturating_sub(before)
    }

    /// Advance time by `step` and let the runner see it.
    async fn advance(&self, air: &oer_protocols_ieee80211_testing_model::ModelAir, step: Duration) {
        self.clock.advance(step).unwrap();
        air.clock_advanced();
        self.idle(64).await;
    }
}

impl oer_portable_time::Clock for Counted {
    fn now(&self) -> oer_portable_time::Instant {
        let reads = self.since_script.get().saturating_add(1);
        self.since_script.set(reads);
        self.reads.set(self.reads.get().saturating_add(1));
        assert!(
            reads <= SPIN_LIMIT,
            "the runner turned {reads} times without yielding"
        );
        oer_portable_time::Clock::now(&*self.clock)
    }
}

impl oer_portable_time::Timer for Counted {
    fn wait_until(
        &self,
        deadline: oer_portable_time::Instant,
    ) -> impl core::future::Future<Output = ()> {
        oer_portable_time::Timer::wait_until(&*self.clock, deadline)
    }
}

/// A future polled only when its own waker woke it, as an executor's
/// task is: a poll of the test's script beside it does not poll it again.
struct Woken<F> {
    future: core::pin::Pin<Box<F>>,
    flag: std::sync::Arc<WakeFlag>,
}

#[derive(Default)]
struct WakeFlag {
    woken: std::sync::atomic::AtomicBool,
    outer: std::sync::Mutex<Option<core::task::Waker>>,
}

impl std::task::Wake for WakeFlag {
    fn wake(self: std::sync::Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.woken.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(waker) = self.outer.lock().unwrap().take() {
            waker.wake();
        }
    }
}

impl<F: core::future::Future> Woken<F> {
    fn new(future: F) -> Self {
        let flag = std::sync::Arc::new(WakeFlag::default());
        flag.woken.store(true, std::sync::atomic::Ordering::SeqCst);
        Self {
            future: Box::pin(future),
            flag,
        }
    }
}

impl<F: core::future::Future> core::future::Future for Woken<F> {
    type Output = F::Output;
    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> {
        *self.flag.outer.lock().unwrap() = Some(cx.waker().clone());
        if !self
            .flag
            .woken
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return core::task::Poll::Pending;
        }
        let waker = core::task::Waker::from(self.flag.clone());
        let mut inner = core::task::Context::from_waker(&waker);
        self.future.as_mut().poll(&mut inner)
    }
}

/// Run `script` beside the runner, the runner polled only when woken; the
/// runner's end once the script asked it to stop and let it finish.
async fn drive<F>(
    run: impl core::future::Future<Output = ApEnd<F>>,
    script: impl core::future::Future<Output = ()>,
) -> ApEnd<F> {
    match select(Woken::new(run), script).await {
        Either::First(end) => end,
        Either::Second(()) => panic!("the runner does not stop by the script's end"),
    }
}

fn stopped<F: core::fmt::Debug>(end: ApEnd<F>) {
    match end {
        ApEnd::Stopped => {}
        ApEnd::StartRefused(refused) => panic!("the radio starts: {refused:?}"),
        ApEnd::StopUnconfirmed(unconfirmed) => panic!("the stop is confirmed: {unconfirmed:?}"),
        ApEnd::Faulted(fault) => panic!("the port faulted: {fault:?}"),
        ApEnd::Contract(contract) => panic!("the runner's contract: {contract:?}"),
        ApEnd::Refused(refusal) => panic!("the port takes every command: {refusal:?}"),
    }
}

/// Ask the runner to stop and let time run until it ended.
async fn stop_and_finish(
    counted: &Counted,
    air: &oer_protocols_ieee80211_testing_model::ModelAir,
    shared: &ApShared,
) {
    shared.request_stop();
    for _ in 0..2048 {
        counted.advance(air, Duration::from_millis(1)).await;
    }
}

#[test]
fn the_runner_beacons_answers_a_probe_and_stops_with_every_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, device, shared);
        let script = async {
            counted.idle(64).await;
            // Started in order: the receive rules, the group key, the TSF.
            let interface = air.access_point();
            assert!(interface.receive.is_some());
            assert!(matches!(interface.tsf, ModelAccessPointTsf::Running { .. }));
            assert!(shared.status().running);
            assert_eq!(
                oer_protocols_network_port::NetDevice::link(&device).state,
                oer_protocols_network_port::LinkState::Up
            );
            // One beacon at the first TBTT, then one at each later one.
            for _ in 0..3 {
                counted.advance(&air, Duration::from_micros(102_400)).await;
            }
            let beacons = sent_controls(&air)
                .iter()
                .filter(|control| **control == 0x80)
                .count();
            assert!(beacons >= 3, "{beacons} beacons");
            // A probe for the SSID is answered.
            air.receive(&probe_request(), ModelRxMeta::new(channel(), -40, -90));
            counted.idle(64).await;
            assert!(sent_controls(&air).contains(&0x50), "a probe response");
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        // Every ticket and buffer is home, the interface closed, its TSF
        // stopped, the beacon permission and the group key's owners back.
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert_eq!(
            runner.homes.buffers.len(),
            oer_protocols_ieee80211_testing_model::MODEL_LARGE_BUFFERS
        );
        assert!(runner.homes.beacon.is_some());
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
        assert!(runner.homes.configure.is_some());
        let interface = air.access_point();
        assert!(interface.receive.is_none());
        assert_eq!(interface.tsf, ModelAccessPointTsf::Stopped);
        assert!(!shared.status().running);
    }));
}

/// The turns a runner may take while nothing changes: the event that woke
/// it, its idle repeats and the one that settles.
const IDLE_TURNS: u32 = super::IDLE_REPEATS as u32 + 2;

#[test]
fn a_stop_behind_a_held_group_removal_waits_for_the_port_without_turning() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            air.hold_group_commands(true);
            shared.request_stop();
            counted.idle(8).await;
            // The removal waits at the port: the runner sleeps meanwhile,
            // its accepted stop no longer a source.
            let turns = counted.idle(256).await;
            assert!(
                turns <= IDLE_TURNS,
                "{turns} turns while the removal is held"
            );
            air.hold_group_commands(false);
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
    }));
}

#[test]
fn a_held_beacon_completion_waits_for_the_port_without_turning() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            air.hold_completion(WmmAccessCategory::Voice, true);
            counted.advance(&air, Duration::from_micros(102_400)).await;
            // The beacon is on the air, its end held: no turn without an
            // event, even with its next TBTT due and its permission out.
            counted.advance(&air, Duration::from_micros(102_400)).await;
            let turns = counted.idle(256).await;
            assert!(
                turns <= IDLE_TURNS,
                "{turns} turns while the beacon is held"
            );
            air.hold_completion(WmmAccessCategory::Voice, false);
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert!(runner.homes.beacon.is_some());
    }));
}

#[test]
fn a_probe_response_waiting_for_its_queue_waits_without_turning() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            air.hold(WmmAccessCategory::Voice, true);
            air.receive(&probe_request(), ModelRxMeta::new(channel(), -40, -90));
            counted.idle(16).await;
            let turns = counted.idle(256).await;
            assert!(
                turns <= IDLE_TURNS,
                "{turns} turns while the response waits"
            );
            assert!(!sent_controls(&air).contains(&0x50));
            air.hold(WmmAccessCategory::Voice, false);
            counted.idle(64).await;
            assert!(
                sent_controls(&air).contains(&0x50),
                "the response goes once the queue is free"
            );
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
    }));
}

#[test]
fn a_unit_the_group_removal_cuts_is_read_and_recycled() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            // A group frame the hardware holds under the installed GTK when
            // the stop removes it: the removal's cut hands it over first.
            let mut frame = std::vec![0x08, 0x02, 0, 0];
            frame.extend_from_slice(&[0xff; 6]);
            frame.extend_from_slice(&ACCESS_POINT);
            frame.extend_from_slice(&PEER);
            frame.extend_from_slice(&[0, 0]);
            assert!(air.receive_group(
                &frame,
                ModelRxMeta::new(channel(), -40, -90),
                oer_protocols_ieee80211_contracts_port::VifRole::AccessPoint,
                GroupKeyId::new(GROUP_KEY_ID).unwrap(),
                &[0x55; 16],
                oer_protocols_ieee80211_testing_model::GroupRxStage::Dma,
            ));
            stop_and_finish(&counted, &air, &shared).await;
        };
        // The stop ends with every owner: a cut unit kept from the port
        // would have quarantined it.
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
    }));
}

#[test]
fn a_refused_command_returns_the_runner_with_the_command_and_every_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        // The first command, the receive rules' configuration, is refused.
        air.refuse_access_point_commands(1);
        let script = async {
            loop {
                counted.idle(1).await;
            }
        };
        let end = match select(Woken::new(runner.run(&shared, &counted)), script).await {
            Either::First(end) => end,
            Either::Second(()) => unreachable!(),
        };
        let ApEnd::Refused(refusal) = end else {
            panic!("a refusal ends the run");
        };
        let Some(command) = runner.take_refused() else {
            panic!("the refused command comes back");
        };
        assert_eq!(
            refusal,
            oer_protocols_ieee80211_contracts_port::AccessPointRefusal::NotEnabled
        );
        let oer_protocols_ieee80211_contracts_port::AccessPointCommand::Configure { .. } = command
        else {
            panic!("the refused configuration, with its permission");
        };
        assert!(!shared.status().running);
        // Every other owner is the runner's: tickets, buffers, the beacon
        // permission, the group key's slot and permission.
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert_eq!(
            runner.homes.buffers.len(),
            oer_protocols_ieee80211_testing_model::MODEL_LARGE_BUFFERS
        );
        assert!(runner.homes.beacon.is_some());
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
        // The port stops with them.
        let stopped = core::future::poll_fn(|cx| {
            let now = oer_portable_time::Clock::now(&*counted.clock);
            runner.port_mut().poll_stop(cx, now)
        });
        assert!(embassy_futures::block_on(stopped).is_ok());
    }));
}

mod handshake;

mod ends;
