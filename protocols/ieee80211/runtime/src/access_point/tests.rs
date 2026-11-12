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
    ModelAccessPointTsf, ModelCaps24, ModelContext, ModelInstall, ModelLarge, ModelNetworkMemory,
    ModelPort, ModelRxMeta,
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
    ssid::Ssid,
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

/// The access point's data frames on the air.
fn sent_data(air: &oer_protocols_ieee80211_testing_model::ModelAir) -> Vec<u8> {
    sent_controls(air)
        .into_iter()
        .filter(|control| control & 0x0c == 0x08)
        .collect()
}

/// Build a runner over the model's port of `$claim` on `$clock`: binds
/// `$runner`, its `$air`, its network `$device` and the shared `$shared`.
macro_rules! fixture {
    ($claim:ident, $clock:expr => $runner:ident, $air:ident, $device:ident, $shared:ident) => {
        fixture!($claim, $clock => $runner, $air, $device, _transmit, $shared);
    };
    ($claim:ident, $clock:expr => $runner:ident, $air:ident, $device:ident, $transmit:ident, $shared:ident) => {
        fixture!($claim, $clock => $runner, $air, $device, $transmit, _supply, $shared);
    };
    ($claim:ident, $clock:expr => $runner:ident, $air:ident, $device:ident, $transmit:ident, $supply:ident, $shared:ident) => {
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
            &Ssid::new(SSID).unwrap(),
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
        let $supply = ModelTx::new();
        let ($device, mut $transmit, end) = network
            .split::<oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame, _>(
                $supply.clone(),
            );
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

/// The network's transmit memory over the model: [`AP_TX_DEPTH`] memories,
/// each frame copied into a free one where the access point reads its
/// Ethernet header, as the stack's packets are written into theirs.
#[derive(Clone)]
struct ModelTx(Rc<core::cell::RefCell<Vec<ModelNetworkMemory>>>);

impl ModelTx {
    fn new() -> Self {
        Self(Rc::new(core::cell::RefCell::new(
            (0..super::network::AP_TX_DEPTH)
                .map(|_| Box::new([0; oer_protocols_ieee80211_testing_model::MODEL_MAX_MPDU]))
                .collect(),
        )))
    }
}

impl<F: oer_protocols_network_port::EthernetFrame> crate::station::TxSupply<F> for ModelTx {
    type Memory = ModelNetworkMemory;
    fn available(&self) -> bool {
        !self.0.borrow().is_empty()
    }
    fn write(&mut self, mut frame: F) -> Result<ModelNetworkMemory, crate::station::TxRefused<F>> {
        use oer_protocols_ieee80211_core_roles_sta::data::{
            MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET,
        };
        let Some(mut memory) = self.0.borrow_mut().pop() else {
            return Err(crate::station::TxRefused::Kept(
                oer_protocols_network_port::TransmitRefused {
                    frame,
                    reason: oer_protocols_network_port::TransmitError::QueueFull,
                },
            ));
        };
        let view = frame.view().unwrap();
        memory[MSDU_ETHERNET_OFFSET..][..6].copy_from_slice(&view.destination);
        memory[MSDU_ETHERNET_OFFSET + 6..][..6].copy_from_slice(&view.source);
        memory[MSDU_ETHERNET_OFFSET + 12..][..2].copy_from_slice(&view.ether_type.to_be_bytes());
        memory[MSDU_PAYLOAD_OFFSET..][..view.payload.len()].copy_from_slice(view.payload);
        Ok(memory)
    }
    fn give(&mut self, memory: ModelNetworkMemory) {
        self.0.borrow_mut().push(memory);
    }
}

impl ModelTx {
    /// The memories free for the network.
    fn free(&self) -> usize {
        self.0.borrow().len()
    }

    /// Where the memory the next frame goes into lies.
    fn next(&self) -> *const u8 {
        self.0.borrow().last().unwrap().as_ptr()
    }

    /// A copy of the free memory at `at`, if it is back.
    fn memory(&self, at: *const u8) -> Option<Vec<u8>> {
        self.0
            .borrow()
            .iter()
            .find(|memory| memory.as_ptr() == at)
            .map(|memory| memory.to_vec())
    }
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
        ApEnd::Unrecycled(fault) => panic!("the port takes every unit back: {fault:?}"),
        ApEnd::Unsupported(unsupported) => panic!("the model serves beacons: {unsupported:?}"),
        ApEnd::Unkept => panic!("the homes keep every item"),
        ApEnd::Unaccepted => panic!("the model accepts every submission"),
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
fn a_command_the_faulted_port_gives_back_ends_the_run_faulted_with_the_command_returned() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        // The first command, the receive rules' configuration, meets the
        // port's fault before the port takes it.
        air.fault_next_access_point_command();
        let script = async {
            loop {
                counted.idle(1).await;
            }
        };
        let end = match select(Woken::new(runner.run(&shared, &counted)), script).await {
            Either::First(end) => end,
            Either::Second(()) => unreachable!(),
        };
        let ApEnd::Faulted(_) = end else {
            panic!("the fault ends the run");
        };
        let Some(command) = runner.take_returned() else {
            panic!("the command comes back beside the fault");
        };
        let oer_protocols_ieee80211_contracts_port::AccessPointCommand::Configure { .. } = command
        else {
            panic!("the configuration, with its permission");
        };
        assert!(runner.homes.held.is_none(), "nothing is issued again");
    }));
}

/// The owner's question 1: a port that keeps a fault it could not take
/// back never confirms a stop. The stop's watch ends on the port's
/// unconfirmed answer, with the fault and what the port observed, and
/// every owner stays in the runner; a port that takes its fault back stops
/// as before.
#[test]
fn a_fault_the_port_keeps_ends_the_stops_watch_unconfirmed_with_every_owner() {
    for recovers in [false, true] {
        embassy_futures::block_on(with_local_slot(async |claim| {
            let counted = Counted::new();
            fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
            air.fault_next_access_point_command();
            if recovers {
                air.allow_recovery();
            }
            let idle = async {
                loop {
                    counted.idle(1).await;
                }
            };
            let end = match select(Woken::new(runner.run(&shared, &counted)), idle).await {
                Either::First(end) => end,
                Either::Second(()) => unreachable!(),
            };
            let ApEnd::Faulted(fault) = end else {
                panic!("the fault ends the run");
            };
            let observed = Cell::new(0_u32);
            let finish = runner.finish_stop(&counted, |_| observed.set(observed.get() + 1));
            let bounded = async {
                for _ in 0..256 {
                    counted.advance(&air, Duration::from_millis(1)).await;
                }
            };
            let watched = match select(Woken::new(finish), bounded).await {
                Either::First(watched) => watched,
                Either::Second(()) => panic!("the watch of a kept fault never ends"),
            };
            if recovers {
                assert_eq!(watched, Ok(super::StopWatch::Confirmed));
                return;
            }
            let Ok(super::StopWatch::Faulted {
                fault: kept,
                observed: unconfirmed,
            }) = watched
            else {
                panic!("a kept fault ends the watch unconfirmed: {watched:?}");
            };
            assert_eq!(kept, Some(fault), "the run's fault is kept");
            assert_eq!(
                unconfirmed.cause,
                oer_protocols_ieee80211_contracts_port::StopCause::Device(fault),
                "the stop's answer names the fault it met"
            );
            assert_eq!(observed.get(), 1, "the observation was published once");
            assert!(
                runner.take_returned().is_some(),
                "the command comes back beside the fault"
            );
            assert_eq!(
                runner.homes.tickets.len(),
                oer_protocols_ieee80211_testing_model::MODEL_TICKETS
            );
            assert!(runner.homes.beacon.is_some() && runner.homes.configure.is_none());
        }));
    }
}

/// A fault that first appears inside the stop's watch (the run ended on a
/// refusal, with no fault): the port is asked to take it back once, per
/// the `recover_for_stop` contract. A port that takes it back confirms its
/// stop with every owner; one that refuses keeps it, and the watch ends
/// unconfirmed on the port's next answer, naming that fault, with every
/// owner in the runner.
#[test]
fn a_fault_first_seen_in_the_stops_watch_is_taken_back_once_or_kept() {
    for recovers in [true, false] {
        embassy_futures::block_on(with_local_slot(async |claim| {
            let counted = Counted::new();
            fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
            air.refuse_access_point_commands(1);
            let idle = async {
                loop {
                    counted.idle(1).await;
                }
            };
            let end = match select(Woken::new(runner.run(&shared, &counted)), idle).await {
                Either::First(end) => end,
                Either::Second(()) => unreachable!(),
            };
            assert!(matches!(end, ApEnd::Refused(_)), "no fault ends the run");
            assert!(runner.homes.fault.is_none() && !runner.homes.faulted);
            // The stop is underway, held on the air, when the fault first
            // appears: the watch's next service meets it.
            air.hold_stop(true);
            let observed = Cell::new(0_u32);
            let finish = runner.finish_stop(&counted, |_| observed.set(observed.get() + 1));
            let script = async {
                counted.idle(1).await;
                // The watch's next service meets the fault before the
                // stop's answer does: a fault the stop's answer names first
                // ends the watch named instead
                // (`a_device_fault_first_named_by_the_stops_answer_ends_the_watch_named`).
                air.poison_at_next_service();
                if recovers {
                    air.allow_recovery();
                }
                for _ in 0..256 {
                    counted.advance(&air, Duration::from_millis(1)).await;
                    // The stop goes quiet once the fault was taken back.
                    if recovers && air.recovery_asks() > 0 {
                        air.hold_stop(false);
                    }
                }
            };
            let watched = match select(Woken::new(finish), script).await {
                Either::First(watched) => watched,
                Either::Second(()) => panic!("the stop's watch never ends"),
            };
            let seen = runner.homes.fault;
            // The port's fault, as the watch's service answered it.
            assert!(
                matches!(
                    seen,
                    Some(oer_protocols_ieee80211_testing_model::ModelPortFault::Quarantined { .. })
                ),
                "the watch's service met the fault: {seen:?}"
            );
            assert_eq!(air.recovery_asks(), 1, "the port is asked once");
            if recovers {
                assert_eq!(watched, Ok(super::StopWatch::Confirmed));
                assert!(!runner.homes.fault_kept, "taken back");
            } else {
                let Ok(super::StopWatch::Faulted { fault, .. }) = watched else {
                    panic!("a kept fault ends the watch unconfirmed: {watched:?}");
                };
                assert_eq!(fault, seen, "the watch's fault is named");
                assert!(runner.homes.fault_kept);
            }
            assert!(runner.take_returned().is_some(), "the refused command");
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
        }));
    }
}

/// A device fault the stop's own answer names
/// first (the port poisoned before the watch, no service answering it)
/// ends the watch named by that very fault, not by the last one a port
/// call answered (none here), with the port's observations and every owner
/// in the runner: the port is asked to take nothing back, and the watch
/// does not wait on.
#[test]
fn a_device_fault_first_named_by_the_stops_answer_ends_the_watch_named() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        air.refuse_access_point_commands(1);
        let idle = async {
            loop {
                counted.idle(1).await;
            }
        };
        let end = match select(Woken::new(runner.run(&shared, &counted)), idle).await {
            Either::First(end) => end,
            Either::Second(()) => unreachable!(),
        };
        assert!(matches!(end, ApEnd::Refused(_)), "no fault ends the run");
        air.poison();
        let observed = Cell::new(0_u32);
        let finish = runner.finish_stop(&counted, |_| observed.set(observed.get() + 1));
        let script = async {
            for _ in 0..256 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
        };
        let watched = match select(Woken::new(finish), script).await {
            Either::First(watched) => watched,
            Either::Second(()) => panic!("the stop's watch never ends"),
        };
        let Ok(super::StopWatch::DeviceFailed { fault, .. }) = watched else {
            panic!("a device fault ends the watch named: {watched:?}");
        };
        assert!(
            matches!(
                fault,
                oer_protocols_ieee80211_testing_model::ModelPortFault::Quarantined {
                    call: oer_protocols_ieee80211_testing_model::ModelPortCall::Stop,
                    ..
                }
            ),
            "the fault the stop's answer met: {fault:?}"
        );
        assert_eq!(runner.homes.fault, None, "no port call answered a fault");
        assert_eq!(observed.get(), 1, "its observation was reported");
        assert_eq!(air.recovery_asks(), 0, "nothing is asked back");
        assert!(!runner.homes.fault_kept);
        assert!(runner.take_returned().is_some(), "the refused command");
        assert!(runner.homes.beacon.is_some());
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
    }));
}

/// A device fault the stop's answer meets after another fault a service
/// answered and the port took back: the watch ends named by the stop's own
/// fault, never by the earlier one the runner recorded.
#[test]
fn a_device_fault_the_stop_meets_after_a_taken_back_one_is_named_by_its_own_cause() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        air.refuse_access_point_commands(1);
        let idle = async {
            loop {
                counted.idle(1).await;
            }
        };
        let end = match select(Woken::new(runner.run(&shared, &counted)), idle).await {
            Either::First(end) => end,
            Either::Second(()) => unreachable!(),
        };
        assert!(matches!(end, ApEnd::Refused(_)), "no fault ends the run");
        // The stop is held on the air; the watch's next service meets a
        // first fault, which the port takes back.
        air.hold_stop(true);
        air.allow_recovery();
        let finish = runner.finish_stop(&counted, |_| {});
        let script = async {
            counted.idle(1).await;
            air.poison_at_next_service();
            let mut poisoned = false;
            for _ in 0..256 {
                counted.advance(&air, Duration::from_millis(1)).await;
                // Once the first is back, the stop's own answer meets the
                // next one at the watch's next wake.
                if !poisoned && air.recovery_asks() > 0 {
                    air.poison();
                    poisoned = true;
                }
            }
        };
        let watched = match select(Woken::new(finish), script).await {
            Either::First(watched) => watched,
            Either::Second(()) => panic!("the stop's watch never ends"),
        };
        let first = runner.homes.fault;
        assert!(
            matches!(
                first,
                Some(
                    oer_protocols_ieee80211_testing_model::ModelPortFault::Quarantined {
                        call: oer_protocols_ieee80211_testing_model::ModelPortCall::ServiceTx,
                        ..
                    }
                )
            ),
            "the service answered the first fault: {first:?}"
        );
        assert_eq!(air.recovery_asks(), 1, "the first fault is taken back once");
        let Ok(super::StopWatch::DeviceFailed { fault, .. }) = watched else {
            panic!("the stop's device fault ends the watch named: {watched:?}");
        };
        assert!(
            matches!(
                fault,
                oer_protocols_ieee80211_testing_model::ModelPortFault::Quarantined {
                    call: oer_protocols_ieee80211_testing_model::ModelPortCall::Stop,
                    ..
                }
            ),
            "the fault the stop's answer met, not the earlier one: {fault:?}"
        );
        assert!(!runner.homes.fault_kept);
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
        let Some(command) = runner.take_returned() else {
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

/// Run the runner beside time moving on in 1 ms steps until the run ends;
/// its end.
async fn run_to_end<F>(
    run: impl core::future::Future<Output = ApEnd<F>>,
    counted: &Counted,
    air: &oer_protocols_ieee80211_testing_model::ModelAir,
    before: impl core::future::Future<Output = ()>,
) -> ApEnd<F> {
    let script = async {
        before.await;
        loop {
            counted.advance(air, Duration::from_millis(1)).await;
        }
    };
    match select(Woken::new(run), script).await {
        Either::First(end) => end,
        Either::Second(()) => unreachable!(),
    }
}

/// The end of a run whose held command passed the port's bound: the run
/// ends refused `Expired` with the command of `kind`, nothing kept beside
/// it, and the port's stop completes without an unconfirmed report.
macro_rules! expired {
    ($runner:ident, $end:expr, $kind:ident, $counted:expr) => {{
        let ApEnd::Refused(refusal) = $end else {
            panic!("an expired command ends the run refused");
        };
        assert_eq!(
            refusal,
            oer_protocols_ieee80211_contracts_port::AccessPointRefusal::Expired
        );
        let Some(command) = $runner.take_returned() else {
            panic!("the expired command comes back");
        };
        assert_eq!(
            command.kind(),
            oer_protocols_ieee80211_contracts_port::AccessPointCommandKind::$kind
        );
        assert!($runner.take_kept().is_none(), "nothing kept beside it");
        let mut reported = 0;
        assert_eq!(
            $runner.finish_stop($counted, |_| reported += 1).await,
            Ok(super::StopWatch::Confirmed)
        );
        assert_eq!(reported, 0, "the stop is never unconfirmed");
        drop(command);
    }};
}

#[test]
fn an_expired_configuration_returns_its_permission_and_the_stop_completes() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        // The first command, the receive rules' configuration, stays held.
        air.hold_access_point_configure(true);
        let end = run_to_end(runner.run(&shared, &counted), &counted, &air, async {}).await;
        expired!(runner, end, Configure, &counted);
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert!(runner.homes.beacon.is_some());
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
    }));
}

#[test]
fn an_expired_group_install_returns_its_slot_and_permission_and_the_stop_completes() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        // The configuration runs; the group key's installation stays held.
        air.hold_group_commands(true);
        let end = run_to_end(runner.run(&shared, &counted), &counted, &air, async {}).await;
        expired!(runner, end, InstallGroup, &counted);
        assert!(!shared.status().running);
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert!(runner.homes.beacon.is_some());
    }));
}

#[test]
fn an_expired_group_removal_at_the_stop_returns_its_key_and_the_stop_completes() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let before = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            // The stop's removal of the group key stays held.
            air.hold_group_commands(true);
            shared.request_stop();
        };
        let end = run_to_end(runner.run(&shared, &counted), &counted, &air, before).await;
        expired!(runner, end, RemoveGroup, &counted);
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert!(runner.homes.beacon.is_some());
    }));
}

mod handshake;

mod ends;

mod homes;
mod network_memory;

#[test]
fn a_port_with_the_callers_timestamp_rule_serves_beacons() {
    assert_eq!(serves_beacons::<ModelCaps24>(), Ok(()));
}

/// Frames the network gave and none carried come home when the access point
/// stops: the transport's places free again, every frame once.
#[test]
fn a_stop_returns_the_network_frames_it_never_carried() {
    use oer_protocols_ieee80211_core_roles_sta::data::{EthernetFrame, MsduParts};
    use oer_protocols_network_port::NetTransmit;
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, _air, _device, transmit, _shared);
        let frame = || {
            let mut frame = EthernetFrame::empty();
            frame
                .set_parts(MsduParts {
                    destination: [0x02, 0, 0, 0, 0, 0x09],
                    source: ACCESS_POINT,
                    ether_type: 0x0800,
                    payload: &[0x45; 28],
                })
                .unwrap();
            frame
        };
        runner.homes.network.set_link(true);
        let mut sent = 0;
        while transmit.try_transmit(frame()).is_ok() {
            sent += 1;
        }
        assert_eq!(sent, super::network::AP_TX_DEPTH, "every place taken");
        // The burst is the access point's once published.
        transmit.flush();
        assert!(runner.homes.begin_stop().is_ok());
        // Up again, as a later run would be: every place is free.
        runner.homes.network.set_link(true);
        // The network takes its returns as its turn does.
        transmit.reclaim();
        let mut again = 0;
        while transmit.try_transmit(frame()).is_ok() {
            again += 1;
        }
        let why = transmit
            .try_transmit(frame())
            .err()
            .map(|refused| refused.reason);
        assert_eq!(
            again,
            super::network::AP_TX_DEPTH,
            "every frame came home: {why:?}"
        );
    }));
}

/// A client place's queues belong to one association. A later one of the
/// same address in the same place (its epoch advanced) takes the place
/// over: the earlier one's frames go back to the network, none of them on
/// the air, and its late completions charge nothing to the new owner. A
/// frame of an association no longer current goes back before it is
/// written.
#[test]
fn a_client_place_s_queues_belong_to_one_association() {
    use crate::datapath::queues::QueueId;
    use oer_protocols_ieee80211_core_roles_ap::ApAssociationIdentity;
    use oer_protocols_ieee80211_core_roles_sta::data::plane::OutboundMsdu;
    use oer_protocols_ieee80211_core_roles_sta::data::{EthernetFrame, MsduParts, WmmUserPriority};
    use oer_protocols_network_port::NetTransmit;
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, _shared);
        let frame = || {
            let mut frame = EthernetFrame::empty();
            frame
                .set_parts(MsduParts {
                    destination: STATION,
                    source: ACCESS_POINT,
                    ether_type: 0x0800,
                    payload: &[0x45; 28],
                })
                .unwrap();
            frame
        };
        // The network's frames, taken from its transport as a turn would.
        runner.homes.network.set_link(true);
        let mut send = |count: usize| {
            for _ in 0..count {
                assert!(transmit.try_transmit(frame()).is_ok());
            }
            transmit.flush();
        };
        send(2);
        let old = ApAssociationIdentity::new(STATION, 1, 7).unwrap();
        let new = ApAssociationIdentity::new(STATION, 1, 8).unwrap();
        let place = runner.homes.place_of(old).unwrap();
        let queue = QueueId::new(place, WmmUserPriority::UP0).unwrap();
        while let Some(outgoing) = runner.homes.network.take() {
            let msdu = OutboundMsdu {
                memory: outgoing.memory,
                length: usize::from(outgoing.length),
                priority: WmmUserPriority::UP0,
            };
            assert!(runner.homes.queues.push(queue, msdu).is_ok());
        }
        assert!(runner.homes.owns(queue, Some(old)));
        // The same address reassociates in the same place.
        assert_eq!(runner.homes.place_of(new), Some(place));
        assert!(runner.homes.queues.is_empty(), "the old frames went back");
        assert!(
            !runner.homes.owns(queue, Some(old)),
            "a late completion charges nothing"
        );
        assert!(runner.homes.owns(queue, Some(new)));
        transmit.reclaim();
        let mut free = 0;
        while transmit.try_transmit(frame()).is_ok() {
            free += 1;
        }
        assert_eq!(
            free,
            super::network::AP_TX_DEPTH,
            "each old frame came home once"
        );
        transmit.flush();
        // Queued under `new`, which the engine never authorized: before any
        // frame is written, the place's frames go back.
        while let Some(outgoing) = runner.homes.network.take() {
            let msdu = OutboundMsdu {
                memory: outgoing.memory,
                length: usize::from(outgoing.length),
                priority: WmmUserPriority::UP0,
            };
            assert!(runner.homes.queues.push(queue, msdu).is_ok());
        }
        let now = oer_portable_time::Clock::now(&*counted.clock);
        assert!(runner.transmit_network(now).is_ok());
        assert!(runner.homes.queues.is_empty());
        assert!(!runner.homes.owns(queue, Some(new)), "the place is free");
        assert!(sent_data(&air).is_empty(), "nothing went on the air");
        transmit.reclaim();
        let mut again = 0;
        while transmit.try_transmit(frame()).is_ok() {
            again += 1;
        }
        assert_eq!(again, super::network::AP_TX_DEPTH, "every frame came home");
    }));
}
