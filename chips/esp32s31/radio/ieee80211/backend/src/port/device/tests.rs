//! The radio device over the hardware model: its receive facts stay raised
//! until the pull that serves them, and a trace build's stall snapshot
//! comes before the turn's receive service, the resume after it, and the
//! request changes no ring work.
//!
//! The signals are deferred, as the executor's are: a raise pends the
//! executor, whose next round ([`service`]) wakes the task for the facts it
//! has not taken yet.

use super::*;
use crate::hardware_model::{
    Atoms, ModelDoorbell, ModelFrontier, ModelLine, ModelRegisters, ModelRx, RxPrefix, leak,
};
use crate::irq::HostActivation;
use crate::lower_mac::{ConnectionFramePriorities, RadioCoexPriorities};
use crate::port::{Device, DevicePull};
use crate::rx::arena::{WifiIrqSignals, WifiRxArena, WifiRxStatic};
#[cfg(oer_trace)]
use crate::rx::counters::stall::{RxStall, RxStallReading, RxStallTicket};
use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    rx_dma::model::{RxWalkerLanding, RxWalkerModel},
    space::HostDma,
};
use oer_chips_esp32s31_hal::system::memory::dma_static::Placed;
use oer_chips_family_platform_interrupt_table::signal::{
    host::{Deferred, deferred_signal},
    service,
};
use oer_portable_time::Clock;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::ConnectionFrame;
use oer_protocols_ieee80211_wire::channel::WifiChannelWidth;
use oer_protocols_radio_port::SlotIdentity;
use oer_protocols_radio_regulatory::Country;
use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
};

/// The product's ring and frame geometry, as the station's joined case.
const N: usize = 96;
const RB: usize = 1664;
const RS: usize = RB;
const Q: usize = 32;
/// The transmit buffers' size the device's port names; no frame is sent.
const TX: usize = 512;

struct Composed;

impl Composition for Composed {
    type Coex = Coex;
    type Entropy = fn() -> u32;
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedConfigureContext<Id: SlotIdentity> = ();
    type ConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

struct Coex;

impl RadioCoexPriorities for Coex {
    fn beacon_window_pti(&self) -> oer_chips_esp32s31_hal::types::MacPti {
        oer_chips_esp32s31_hal::types::MacPti::new(9).expect("a valid priority")
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}

/// The device's timer: one virtual clock the test moves.
#[derive(Clone)]
struct Shared(Rc<VirtualClock>);

impl Clock for Shared {
    fn now(&self) -> Instant {
        self.0.now()
    }
}

impl Timer for Shared {
    fn wait_until(&self, deadline: Instant) -> impl core::future::Future<Output = ()> {
        self.0.wait_until(deadline)
    }
}

type Model = RadioDevice<
    Composed,
    Shared,
    ModelFrontier,
    HostActivation,
    ModelDoorbell<Deferred>,
    N,
    RB,
    RS,
    Q,
    Deferred,
>;

/// A waker that counts its wakes.
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The device started on its channel, the DMA engine's port on its ring,
/// its interrupt line, its clock and a trace build's stall record.
struct Bench {
    device: &'static mut Model,
    rx: ModelRx<N, RB, RS>,
    line: Rc<ModelLine<Deferred>>,
    /// The power signal, which the radio handler raises a TBTT on.
    power: &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<Deferred>,
    clock: Shared,
    #[cfg(oer_trace)]
    stall: &'static RxStall,
    wakes: Arc<Count>,
    sequence: u16,
}

fn channel() -> oer_protocols_ieee80211_wire::channel::WifiChannel {
    oer_protocols_ieee80211_wire::channel::WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20)
        .expect("channel 6 is a 2.4 GHz channel")
}

impl Bench {
    fn new() -> Self {
        let clock = Shared(Rc::new(VirtualClock::new()));
        let reader = clock.clone();
        let atoms = Atoms::new(Rc::new(move || reader.now().as_micros()));
        let power = deferred_signal().0;
        let arena: &'static mut WifiRxStatic<N, RB, RS, Q, Deferred, HostDma> =
            leak(bytemuck::Zeroable::zeroed());
        let (rx, _stop, _frames) = WifiRxArena::bind(
            Placed::host(arena),
            WifiIrqSignals {
                mac: deferred_signal().0,
                receive: deferred_signal().0,
                power,
            },
        )
        .ok()
        .expect("a zeroed static binds");
        let storage = rx.rx();
        let requests = rx.requests();
        let walker: &'static RefCell<RxWalkerModel> = leak(RefCell::new(RxWalkerModel::new()));
        let cell: &'static HandlerCell<WifiIrqLine<Deferred>> = leak(HandlerCell::new());
        let line = Rc::new(ModelLine::new(cell, requests, atoms.clone()));
        #[cfg(oer_trace)]
        let stall: &'static RxStall = leak(RxStall::new());
        let device = leak(RadioDevice::new(RadioParts {
            wifi: ModelFrontier::new(ModelRegisters::new(atoms.clone(), walker), channel()),
            rx,
            interrupt_cell: cell,
            bound: HostActivation,
            actor: (),
            timer: clock.clone(),
            doorbell: ModelDoorbell(line.clone()),
            domain: RegulatoryDomain::of(Country::new(*b"DE").expect("a country code"))
                .expect("a regulatory domain"),
            power_limits: PowerLimits {
                antenna_gain_mbm: 0,
                chip_maximum_mbm: 2000,
                owner_cap_mbm: None,
            },
            #[cfg(oer_trace)]
            stall,
        }));
        let bench = Self {
            device,
            rx: ModelRx::new(storage, walker, atoms),
            line,
            power,
            clock,
            #[cfg(oer_trace)]
            stall,
            wakes: Arc::new(Count(AtomicUsize::new(0))),
            sequence: 0,
        };
        let waker = bench.waker();
        let started = <Model as Device<TX>>::poll_start(
            bench.device,
            &mut Context::from_waker(&waker),
            channel(),
        );
        assert!(matches!(started, Poll::Ready(Ok(()))), "{started:?}");
        bench
    }

    fn waker(&self) -> Waker {
        Waker::from(self.wakes.clone())
    }

    /// The wakes the device's task got so far.
    fn woken(&self) -> usize {
        self.wakes.0.load(Ordering::Relaxed)
    }

    /// The port's facts: the device's receive work.
    fn facts(&mut self) -> bool {
        let waker = self.waker();
        <Model as Device<TX>>::poll_rx_work(self.device, &mut Context::from_waker(&waker))
            .expect("the receive work")
    }

    /// The turn's receive service at the clock's instant.
    fn pull(&mut self) -> DevicePull<StagedReceive<Q, RB>> {
        let now = self.clock.now();
        <Model as Device<TX>>::pull_unit(self.device, Q, now).expect("the receive pull")
    }

    /// A data frame lands in the ring; with `edge`, its RX edge runs the
    /// handler, which posts the receive fact.
    fn land(&mut self, edge: bool) {
        let mut mpdu = std::vec![0_u8; 24];
        mpdu[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
        mpdu[22..24].copy_from_slice(&(self.sequence << 4).to_le_bytes());
        mpdu.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4]);
        self.sequence += 1;
        let (landing, event) = self
            .rx
            .land(&RxPrefix::new(), &mpdu)
            .expect("the unit fits its buffer");
        assert!(matches!(landing, RxWalkerLanding::Landed(_)), "{landing:?}");
        if edge && let Some(event) = event {
            self.line.latch(event);
        }
    }

    #[cfg(oer_trace)]
    fn read(&self, ticket: RxStallTicket) -> Option<RxStallReading> {
        let now = self.clock.now().as_micros() as u32;
        self.stall.read(ticket, now)
    }

    #[cfg(oer_trace)]
    fn pass(&self, micros: u64) {
        self.clock
            .0
            .advance(Duration::from_micros(micros))
            .expect("the model's time fits");
    }
}

/// A unit lands after the facts call that registered the device's task, as
/// one does while the actor's turn runs: the next facts call sees its fact
/// but leaves it raised, so the executor's next round still wakes the task,
/// whether or not the caller turns again. The pull that serves it takes it.
#[test]
fn a_receive_fact_stays_raised_until_the_pull_that_serves_it() {
    let mut bench = Bench::new();
    assert!(!bench.facts(), "a quiet ring has no work");
    service::<Deferred>();
    let woken = bench.woken();
    bench.land(true);
    assert!(bench.facts(), "the facts see the raised fact");
    assert!(bench.facts(), "and leave it raised");
    service::<Deferred>();
    assert_eq!(bench.woken(), woken + 1, "the raise's wake stays owed");
    assert!(matches!(bench.pull(), DevicePull::Unit(_)));
    // Served: no fact is left, and no round wakes the task for it again.
    service::<Deferred>();
    assert_eq!(bench.woken(), woken + 1);
    assert!(matches!(bench.pull(), DevicePull::Empty));
    assert!(!bench.facts());
}

/// A TBTT comes after the device's facts found no power fact and before
/// they look at the MAC's: the MAC's look leaves it, and the next facts give
/// it.
#[test]
fn a_tbtt_between_the_power_and_the_mac_facts_is_kept() {
    let mut bench = Bench::new();
    let waker = bench.waker();
    let cx = &mut Context::from_waker(&waker);
    let session = bench.device.session.as_mut().expect("the session");
    assert!(session.port_power(cx).is_pending());
    // The radio handler's typed publication on the device's power signal.
    let mut storage = crate::mac::irq::consumer::PowerIrqStorage::new(bench.power);
    let (publisher, _consumer) = storage.split();
    publisher.publish(
        oer_chips_esp32s31_hal::types::MacPowerInterruptObservation::from_semantic_events(
            false, false, false, false, true, false,
        ),
    );
    assert!(session.poll(cx).is_pending(), "no MAC fact");
    assert!(
        matches!(session.port_power(cx), Poll::Ready(Ok(facts)) if facts.sta_tbtt()),
        "the TBTT waits for the power facts"
    );
    // Through the device: the facts give it as its event once.
    publisher.publish(
        oer_chips_esp32s31_hal::types::MacPowerInterruptObservation::from_semantic_events(
            false, false, false, false, true, false,
        ),
    );
    assert!(matches!(
        <Model as Device<TX>>::poll_event(bench.device, cx),
        Poll::Ready(Ok(DeviceEvent::Tbtt))
    ));
    assert!(<Model as Device<TX>>::poll_event(bench.device, cx).is_pending());
}

/// A pull without room serves nothing, so it takes no fact: the facts keep
/// telling of the ring's work until a pull with room serves it.
#[test]
fn a_pull_without_room_leaves_the_receive_fact_raised() {
    let mut bench = Bench::new();
    assert!(!bench.facts());
    bench.land(true);
    let now = bench.clock.now();
    assert!(matches!(
        <Model as Device<TX>>::pull_unit(bench.device, 0, now).expect("the receive pull"),
        DevicePull::Empty
    ));
    assert!(bench.facts(), "the fact still waits");
    assert!(matches!(bench.pull(), DevicePull::Unit(_)));
    assert!(matches!(bench.pull(), DevicePull::Empty));
    assert!(!bench.facts());
}

#[cfg(oer_trace)]
#[test]
fn the_snapshot_precedes_the_turn_s_service_and_the_resume_follows_it() {
    let mut bench = Bench::new();
    assert!(!bench.facts(), "a quiet ring has no work");
    // One unit through the ordinary path: the boundaries have moved once.
    bench.land(true);
    assert!(bench.facts());
    assert!(matches!(bench.pull(), DevicePull::Unit(_)));
    bench.pass(1_000);

    // A unit completes and its edge posts the fact; then a reader asks.
    bench.land(true);
    bench.pass(500);
    let wakes = bench.wakes.0.load(Ordering::Relaxed);
    let ticket = bench.stall.request();
    assert_eq!(
        bench.wakes.0.load(Ordering::Relaxed),
        wakes + 1,
        "the request wakes the device's task"
    );
    assert_eq!(bench.read(ticket), None, "the owner has not turned yet");
    let asked = bench.clock.now().as_micros() as u32;

    // The turn's facts take the snapshot before any of its receive work.
    assert!(bench.facts());
    let taken = bench.read(ticket).expect("taken at the turn's facts");
    let snapshot = taken.snapshot;
    assert_eq!(snapshot.at, asked);
    assert_eq!(
        snapshot.driver.count, 1,
        "only the first unit was handed on"
    );
    let first_dma = snapshot.dma;
    assert!(first_dma.seen() && first_dma.at < asked);
    assert!(!snapshot.device.rx_pending, "the fact was not taken yet");
    let hardware = snapshot.hardware.expect("the owner holds its registers");
    assert_eq!(
        hardware.session.receive_fact_pending,
        Some(true),
        "the posted fact still waits"
    );
    assert_eq!(hardware.session.descriptors.total, N as u32);
    assert_eq!(hardware.session.descriptors.done, 1, "the completed unit");
    assert!(hardware.walker.enabled);
    assert!(hardware.ring.is_some_and(|ring| ring.completed == 1));
    assert_eq!(snapshot.mac, None, "the model has no statistics bank");
    assert_eq!(taken.resume, None, "the service has not run");

    // The turn's service hands the unit on; the next facts record that.
    bench.pass(20);
    assert!(matches!(bench.pull(), DevicePull::Unit(_)));
    assert_eq!(bench.read(ticket).unwrap().resume, None);
    bench.pass(30);
    bench.facts();
    let resume = bench
        .read(ticket)
        .unwrap()
        .resume
        .expect("recorded after the service");
    assert!(resume.serviced);
    assert_eq!(resume.at, asked + 50);
    assert_eq!(resume.driver.count, 2);
    assert_eq!(resume.driver.at, asked + 20, "at the turn's instant");
    assert!(resume.dma.count > first_dma.count);
    assert_eq!(resume.dma.at, asked + 20);
    // Served once: the next facts take no snapshot again.
    assert!(!bench.facts());
}

#[cfg(oer_trace)]
#[test]
fn a_request_without_ring_work_takes_a_snapshot_but_no_service() {
    let mut bench = Bench::new();
    assert!(!bench.facts());
    // A unit lands but its edge never runs the handler: no fact, no work.
    bench.land(false);
    let ticket = bench.stall.request();
    assert!(bench.facts(), "the snapshot's turn runs");
    let snapshot = bench.read(ticket).unwrap().snapshot;
    let hardware = snapshot.hardware.unwrap();
    // The epoch's first unit lands in the reserved descriptor the walker
    // starts from.
    assert!(hardware.session.descriptors.reserved_done, "the unit waits");
    assert_eq!(hardware.session.descriptors.done, 0);
    assert_eq!(hardware.session.receive_fact_pending, Some(false));
    assert!(!snapshot.device.rx_pending);
    assert!(!snapshot.dma.seen() && !snapshot.driver.seen());
    // Facts before the turn's pull record the boundaries, not yet served.
    bench.pass(5);
    assert!(!bench.facts(), "served once: no second snapshot");
    let early = bench.read(ticket).unwrap().resume.unwrap();
    assert!(!early.serviced);
    assert_eq!(early.at, snapshot.at + 5);
    // The turn's pull finds no ring work: the request forced none.
    assert!(matches!(bench.pull(), DevicePull::Empty));
    bench.pass(5);
    assert!(!bench.facts());
    let resume = bench.read(ticket).unwrap().resume.unwrap();
    assert!(resume.serviced);
    assert_eq!(resume.dma, snapshot.dma, "nothing moved");
    assert_eq!(resume.driver, snapshot.driver);
    assert_eq!(resume.at, snapshot.at + 10);
    // Recorded for the last time: later facts leave it.
    bench.pass(5);
    bench.facts();
    assert_eq!(bench.read(ticket).unwrap().resume, Some(resume));
}
