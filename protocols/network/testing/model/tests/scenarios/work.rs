//! The transmit hot path's work per frame, as the image that loads the air
//! composes it: the real service, core and airtime egress with a local
//! producer of load datagrams, unpaced, to one peer, whose frames the medium
//! settles a Block Ack of 32 at a time. The service's diagnostics and the
//! producer count the work; each case bounds it per frame.
//!
//! The counts are the work's shape, not its time: what each step costs on a
//! hart the counts do not say. A host run of these cases under an
//! instruction counter weighs them (see the crate's README).

use super::*;

use oer_protocols_network_core::{Declined, Payload, SendRefusal};
use oer_protocols_network_port::PlanStop;
use oer_protocols_network_runtime::TurnEnd;
use oer_protocols_network_runtime::{
    Diagnosis, Diagnostics, Interest, LocalEnd, Next, Offer, Produce, Work,
};

/// The image's network geometry (`hil/images/wifi-station`).
const LOAD_SIZE: usize = 1472;
const LOAD_SOCKETS: usize = 2;
const LOAD_CREDITS: usize = 32;
const LOAD_RECEIVED: usize = 8;
const LOAD_NEIGHBORS: usize = 4;
const LOAD_BLOCKS: usize = 64;
const LOAD_DEPTH: usize = 32;
const LOAD_LANE: usize = 32;
/// The settlements one Block Ack tells.
const AGGREGATE: usize = 32;
/// Frames the port holds: the image's network buffers.
const AT_THE_PORT: usize = 63;
/// Accounts and reservations of the egress.
const ACCOUNTS: usize = 4;
const RESERVATIONS: usize = 63;
/// The UDP payload of a load datagram, which fills one frame.
const DATAGRAM: usize = 1472;
/// The load header's length (`hil/services/load`), and its filler.
const HEADER: usize = 28;
const FILLER: u8 = 0x5a;
const LOAD_PORT: u16 = 5001;
/// How long the load runs.
const RUN: u64 = SECOND / 10;

type LoadEgress = AirtimeEgress<'static, Medium, Ticket, ACCOUNTS, RESERVATIONS>;
type LoadOwner = NetworkOwner<
    'static,
    LoadNet,
    LOAD_SIZE,
    LOAD_SOCKETS,
    LOAD_CREDITS,
    LOAD_RECEIVED,
    LOAD_NEIGHBORS,
    LOAD_BLOCKS,
    Medium,
    // The load producer's tag, kept in each declared work's place.
    u64,
>;

struct LoadNet;

/// What the producer was asked.
#[derive(Clone, Copy, Debug, Default)]
struct Asked {
    offers: u64,
    fills: u64,
    currents: u64,
    generations: u64,
    registers: u64,
    ended: u64,
    refused: u64,
}

/// Datagrams without end, each a load header and filler, to one peer.
#[derive(Clone)]
struct Load {
    to: Remote,
    next: u64,
    asked: Rc<RefCell<Asked>>,
}

impl Produce for Load {
    type Tag = u64;

    fn generation(&self) -> u32 {
        self.asked.borrow_mut().generations += 1;
        0
    }

    fn current(&self, _tag: u64) -> bool {
        self.asked.borrow_mut().currents += 1;
        true
    }

    fn offer(&mut self, _now: Instant) -> Result<Offer<u64>, Next> {
        self.asked.borrow_mut().offers += 1;
        let tag = self.next;
        self.next += 1;
        Ok(Offer {
            remote: self.to,
            length: DATAGRAM,
            tag,
        })
    }

    fn fill(
        &mut self,
        _now: Instant,
        work: Work<u64>,
        data: &mut Payload<'_>,
    ) -> Result<(), Declined> {
        self.asked.borrow_mut().fills += 1;
        let mut header = [0; HEADER];
        header[12..20].copy_from_slice(&work.tag.to_be_bytes());
        data.put(&header).map_err(|_| Declined::Unwritable)?;
        data.repeat(FILLER, data.remaining())
            .map_err(|_| Declined::Unwritable)
    }

    fn refused(&mut self, _tag: u64, _reason: SendRefusal) {
        self.asked.borrow_mut().refused += 1;
    }

    fn ended(&mut self, _work: Work<u64>, _outcome: Outcome) {
        self.asked.borrow_mut().ended += 1;
    }

    fn register(&mut self, _cx: &mut Context<'_>, _interest: Interest) -> bool {
        self.asked.borrow_mut().registers += 1;
        // Always more: the sender is unpaced.
        true
    }
}

/// Diagnostics always on, summing every turn's count.
#[derive(Clone, Default)]
struct Counted(Rc<RefCell<Diagnosis>>);

impl Diagnostics for Counted {
    fn on(&mut self) -> bool {
        true
    }

    fn clock(&mut self) -> u64 {
        0
    }

    fn add(&mut self, counted: &Diagnosis) {
        self.0.borrow_mut().accumulate(counted);
    }
}

/// The frames the port took and the turns that took them.
#[derive(Clone, Copy, Debug, Default)]
struct Moved {
    accepted: u64,
    turns: u64,
    /// Passes of the service's wait, and its yields after a turn.
    waits: u64,
    ready_waits: u64,
    yields: u64,
}

struct MovedSink(Rc<RefCell<Moved>>);

impl EventSink<NetEvent> for MovedSink {
    fn event(&mut self, _: NetEvent) {}
}

impl EventSink<ServiceEvent> for MovedSink {
    fn event(&mut self, event: ServiceEvent) {
        let mut moved = self.0.borrow_mut();
        match event {
            ServiceEvent::Accepted => moved.accepted += 1,
            ServiceEvent::TurnEnded(TurnEnd { .. }) => moved.turns += 1,
            ServiceEvent::Waited { ready } => {
                moved.waits += 1;
                moved.ready_waits += u64::from(ready);
            }
            ServiceEvent::Yielded => moved.yields += 1,
            _ => {}
        }
    }
}

/// What a run of the load did.
#[derive(Debug)]
struct Account {
    moved: Moved,
    asked: Asked,
    diagnosis: Diagnosis,
    /// Frames the air carried.
    carried: u64,
}

impl Account {
    /// `count` per frame the port took.
    fn per_frame(&self, count: u64) -> f64 {
        count as f64 / self.moved.accepted.max(1) as f64
    }

    fn report(&self) {
        let d = &self.diagnosis;
        println!(
            "frames {} carried {} turns {} | per frame: turns {:.3} waits {:.3} ready {:.3} yields {:.3} collections {:.3} \
             walked {:.3} continued {:.3} told {:.3} materialised {:.3} | stops spent {} \
             changed {} grant {} account {} | producer per frame: offers {:.3} fills {:.3} \
             currents {:.3} generations {:.3} registers {:.3} ended {:.3} refused {}",
            self.moved.accepted,
            self.carried,
            self.moved.turns,
            self.per_frame(self.moved.turns),
            self.per_frame(self.moved.waits),
            self.per_frame(self.moved.ready_waits),
            self.per_frame(self.moved.yields),
            self.per_frame(u64::from(d.collections)),
            self.per_frame(u64::from(d.walked)),
            self.per_frame(u64::from(d.continued)),
            self.per_frame(u64::from(d.told)),
            self.per_frame(u64::from(d.materialised)),
            d.stopped_spent,
            d.stopped_changed,
            d.stopped_grant,
            d.stopped_account,
            self.per_frame(self.asked.offers),
            self.per_frame(self.asked.fills),
            self.per_frame(self.asked.currents),
            self.per_frame(self.asked.generations),
            self.per_frame(self.asked.registers),
            self.per_frame(self.asked.ended),
            self.asked.refused,
        );
    }
}

/// Run the load for [`RUN`] on a medium where a frame costs
/// `micros_per_frame` of air, and count its work.
fn load(micros_per_frame: u32) -> Account {
    let mut spec = peer(1, 0);
    spec.overhead_micros = micros_per_frame;
    let medium = Medium::new(LOCAL, OUR_IP, AT_THE_PORT, &[spec]);
    medium.batch(AGGREGATE);
    let clock = clock();
    let memory = Box::leak(Box::new([[0; LOAD_SIZE]; LOAD_BLOCKS]));
    let (pool, brand) = claim::<LoadNet, LOAD_SIZE, LOAD_BLOCKS>(memory).unwrap();
    let owner: LoadOwner = NetworkOwner::new(brand, interface(), pool, medium.clone());
    let storage = Box::leak(Box::new(AirtimeStorage::new(
        NonZeroU32::new(QUANTUM).unwrap(),
    )));
    let egress: LoadEgress = AirtimeEgress::new(storage, medium.clone());
    let (stop, stopping): (Stopper, _) = Box::leak(Box::new(StopLane::new())).split();
    let asked = Rc::new(RefCell::new(Asked::default()));
    let producer = Load {
        to: Remote {
            address: spec.address,
            port: 9,
        },
        next: 0,
        asked: asked.clone(),
    };
    let end: &'static LocalEnd = Box::leak(Box::new(LocalEnd::new()));
    let counted = Counted::default();
    let mut service: Service<
        'static,
        'static,
        LoadNet,
        LOAD_SIZE,
        LOAD_SOCKETS,
        LOAD_CREDITS,
        LOAD_RECEIVED,
        LOAD_NEIGHBORS,
        LOAD_BLOCKS,
        LOAD_DEPTH,
        LOAD_LANE,
        Medium,
        LoadEgress,
        _,
        Counted,
    > = Service::with_producer(owner, egress, MemoryPolicy::KEEP, stopping, producer, end)
        .with_diagnostics(counted.clone());
    service.bind_local(LOAD_PORT, Dscp::default()).unwrap();
    let moved = Rc::new(RefCell::new(Moved::default()));
    let mut sink = MovedSink(moved.clone());
    let tasks: Vec<Task<'_>> = vec![Box::pin(async move {
        let _cause = service.run(clock, &mut sink).await;
    })];
    let stop = Cell::new(Some(stop));
    let script: Vec<(u64, Box<dyn FnOnce() + '_>)> = vec![(
        RUN,
        Box::new(|| {
            if let Some(stop) = stop.take() {
                stop.try_send(()).ok().unwrap();
            }
        }),
    )];
    run_tasks(clock, &medium, tasks, script);
    let carried = medium.spent().iter().map(|peer| peer.frames).sum();
    let work = Account {
        moved: *moved.borrow(),
        asked: *asked.borrow(),
        diagnosis: *counted.0.borrow(),
        carried,
    };
    work.report();
    work
}

/// Frames the port takes per microsecond of the medium's air: so fast that
/// the service, never the air, is what bounds the load, as on a hart it
/// saturates.
const FREE_AIR: u32 = 1;

#[test]
fn an_unpaced_load_takes_each_frame_once_through_a_plan_of_several() {
    let account = load(FREE_AIR);
    let frames = account.moved.accepted;
    assert!(frames > 50_000, "{account:?}");
    let d = &account.diagnosis;
    // Every frame is built whole in its slot. Each but the one request
    // that resolved the peer is the producer's, filled there once.
    assert_eq!(u64::from(d.materialised), frames, "{account:?}");
    assert_eq!(account.asked.fills + 1, frames, "{account:?}");
    // Each offer is taken: an unpaced producer is never refused.
    assert_eq!(account.asked.refused, 0, "{account:?}");
    // A collection's plan sends a turn's budget, not one frame: at most one
    // collection a turn, and fewer collections than a fifth of the frames.
    assert!(
        u64::from(d.collections) <= account.moved.turns,
        "{account:?}"
    );
    assert!(u64::from(d.collections) * 5 <= frames, "{account:?}");
    // No plan of one account stops before its turns are spent.
    assert_eq!(d.stopped(PlanStop::Changed), 0, "{account:?}");
    assert_eq!(d.stopped(PlanStop::Grant), 0, "{account:?}");
    assert_eq!(d.stopped(PlanStop::Account), 0, "{account:?}");
}
