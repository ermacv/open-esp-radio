//! A receive load through the service: frames lent to a socket whose
//! consumer takes each and returns its buffer at once, in bursts. The counts
//! say what the turns did: frames and notices taken, empty or not, the
//! port's registrations, turns, yields and waits.
//!
//! Its conditions: 64 octets of UDP payload a datagram, the `Direct` egress
//! policy, a lending socket of credit 2 (the tests' `CREDITS`), bursts of
//! that credit, and every buffer returned as soon as it is taken. A host
//! case to weigh the service's receive turns on, not a device's load.

extern crate std;

use std::boxed::Box;

use oer_portable_events::EventSink;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{Interface, NetEvent, NetworkOwner, claim};
use oer_protocols_network_port::Holding;
use oer_protocols_network_wire::ip::Dscp;

use super::{
    BLOCKS, CREDITS, DEPTH, LOCAL, OUR_IP, RECEIVED, SIZE, WireHeld, WirePort, run_tasks, settle,
    udp_frame,
};
use crate::{Direct, LendingLanes, MemoryPolicy, Received, Service, ServiceEvent, StopLane};

const PORT: u16 = 6000;
const CREDIT: usize = CREDITS;
/// As many as the socket's credit lends at once.
const BURST: usize = CREDIT;
const BURSTS: usize = 2000;

#[derive(Default)]
struct Turns {
    turns: u64,
    yields: u64,
    waits: u64,
    frames: u64,
}

impl EventSink<NetEvent> for Turns {
    fn event(&mut self, _: NetEvent) {}
}

impl EventSink<ServiceEvent> for Turns {
    fn event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::TurnBegan => self.turns += 1,
            ServiceEvent::Yielded => self.yields += 1,
            ServiceEvent::Waited { .. } => self.waits += 1,
            ServiceEvent::FrameTaken => self.frames += 1,
            _ => {}
        }
    }
}

#[test]
fn a_receive_load_lends_every_frame_and_counts_its_turns() {
    #[derive(Debug)]
    struct Net;
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (mut pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    pool.loans().pool(CREDIT + 2, 2).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    {
        let mut state = wire.0.borrow_mut();
        state.answers = true;
        state.holding = Some(Holding::Lendable);
    }
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: Service<
        'static,
        'static,
        Net,
        SIZE,
        2,
        CREDITS,
        RECEIVED,
        2,
        BLOCKS,
        DEPTH,
        RECEIVED,
        WirePort,
        Direct,
    > = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let group = service.group(CREDIT).unwrap();
    let lanes: &'static mut LendingLanes<
        'static,
        'static,
        Net,
        SIZE,
        DEPTH,
        RECEIVED,
        CREDIT,
        WireHeld,
    > = Box::leak(Box::new(LendingLanes::new()));
    let mut socket = service
        .bind_lending::<CREDIT>(PORT, Dscp::default(), group, lanes)
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut turns = Turns::default();
    let mut lent = 0_usize;
    let drive = async {
        for burst in 0..BURSTS {
            {
                let mut state = wire.0.borrow_mut();
                for at in 0..BURST {
                    state
                        .incoming
                        .push_back(udp_frame(PORT, &[(burst * BURST + at) as u8; 64]));
                }
                state.wake();
            }
            for _ in 0..BURST {
                match socket.receive().await.unwrap() {
                    Received::Lent(datagram) => {
                        lent += 1;
                        drop(datagram);
                    }
                    Received::Copied(_) => panic!("lent, not copied"),
                }
            }
            settle().await;
        }
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut turns), drive);
    let state = wire.0.borrow();
    std::println!(
        "rx-load frames {} lent {lent} | turns {} yields {} waits {} | takes {} empty {} | notices {} empty {} | registers {}",
        BURSTS * BURST,
        turns.turns,
        turns.yields,
        turns.waits,
        state.takes,
        state.empty_takes,
        state.notice_takes,
        state.empty_notices,
        state.registers,
    );
    assert_eq!(lent, BURSTS * BURST, "every frame lent");
}
