//! A paced sender, as the load image's: each millisecond a batch of
//! datagrams, written without a yield between them, so the service runs only
//! when the sender waits. A send that finds no block ready is refused, and the
//! sender then waits for one, as `Sending::try_send` and `Sending::send` do.
//! The pool and the lane have room throughout, so a refusal is the look-ahead
//! running dry and nothing else.

use oer_portable_events::EventSink;
use oer_portable_time::{Clock, Duration, Timer};
use oer_protocols_network_core::NetEvent;

use super::*;
use crate::{RequestKind, ServiceEvent, TurnEnd};

/// The device's geometry (`chips/esp32s31/app/src/wifi/network.rs`).
const PACED_BLOCKS: usize = 64;
const PACED_CREDITS: usize = 32;
const PACED_DEPTH: usize = 32;
/// About 100 Mbit/s of 1400-octet payloads: nine a millisecond.
const PER_BATCH: usize = 9;
/// The most the image's paced sender sends before it yields
/// (`load::pace::MAX_BATCH`): the whole look-ahead.
const MAX_BATCH: usize = 16;
const BATCHES: usize = 64;

type PacedSocket<B> = Socket<'static, 'static, B, SIZE, PACED_DEPTH, RECEIVED, PACED_CREDITS>;
type PacedService<B> = Service<
    'static,
    'static,
    B,
    SIZE,
    1,
    PACED_CREDITS,
    RECEIVED,
    2,
    PACED_BLOCKS,
    PACED_DEPTH,
    RECEIVED,
    WirePort,
    Direct,
>;

/// The blocks each reservation the service took asked for, and how each
/// turn ended; the sender reads the count of turns as they go.
#[derive(Default)]
struct Asked(Vec<u8>, Vec<TurnEnd>, Rc<core::cell::Cell<usize>>);

impl EventSink<ServiceEvent> for Asked {
    fn event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Took(RequestKind::Reserve { blocks }) => self.0.push(blocks),
            ServiceEvent::TurnEnded(end) => {
                self.1.push(end);
                self.2.set(self.2.get() + 1);
            }
            _ => {}
        }
    }
}

impl EventSink<NetEvent> for Asked {
    fn event(&mut self, _: NetEvent) {}
}

/// The datagrams of `BATCHES` bursts of `bursts`, in turn.
fn total(bursts: &[usize]) -> usize {
    (0..BATCHES).map(|batch| bursts[batch % bursts.len()]).sum()
}

/// What the sender saw: the sends that found no block ready, by batch and
/// place within it.
#[derive(Debug, Default)]
struct Paced {
    refused_at: Vec<(usize, usize)>,
    /// For each refusal, the service's turns until its block came.
    turns_to_block: Vec<usize>,
}

/// The sender of `bursts`, one a millisecond, in turn.
fn paced<B: 'static>(bursts: &[usize]) -> (Paced, Vec<u8>, usize, Vec<TurnEnd>) {
    let memory = Box::leak(Box::new([[0; SIZE]; PACED_BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, PACED_BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: PacedService<B> = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let mut socket: PacedSocket<B> = service
        .bind(
            5000,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let start = clock.now();
    let mut asked = Asked::default();
    let turns = asked.2.clone();
    let mut seen = Paced::default();
    let drive = async {
        socket.reserve_ahead(RESERVE_AHEAD);
        for batch in 0..BATCHES {
            let due = start.saturating_add(Duration::from_micros(1000 * batch as u64));
            clock.wait_until(due).await;
            for at in 0..bursts[batch % bursts.len()] {
                let mut block = match socket.try_reserve().unwrap() {
                    Some(block) => block,
                    None => {
                        seen.refused_at.push((batch, at));
                        let before = turns.get();
                        let block = socket.reserve().await.unwrap();
                        seen.turns_to_block.push(turns.get() - before);
                        block
                    }
                };
                block.fill(&[0; 32]).unwrap();
                socket.credit().await.unwrap();
                drop(socket.send(peer(), block).ok().unwrap());
            }
        }
        while datagrams(&wire.0.borrow()).len() < total(bursts) {
            yield_now().await;
        }
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut asked), drive);
    assert_eq!(cause, EndCause::Stopped);
    let sent = datagrams(&wire.0.borrow()).len();
    (seen, asked.0, sent, asked.1)
}

/// A paced sender of `per_batch` datagrams between two turns of the service:
/// only its first send, before any request, finds none ready.
fn never_dry<B: 'static>(per_batch: usize) {
    let (seen, asked, sent, turns_ended) = paced::<B>(&[per_batch]);
    let reserves = asked.len();
    let asked_blocks: usize = asked.iter().map(|blocks| usize::from(*blocks)).sum();
    std::eprintln!(
        "paced: {sent} datagrams, {reserves} reservations: {:.3} messages per datagram, {} refused",
        (sent + reserves) as f64 / sent as f64,
        seen.refused_at.len()
    );
    assert_eq!(sent, BATCHES * per_batch);
    assert!((sent..=sent + RESERVE_AHEAD).contains(&asked_blocks));
    assert!(
        turns_ended.iter().all(|end| end.blocks_free),
        "the pool never ran out"
    );
    // Only the first send, before any request, finds none ready: no batch
    // boundary waits for a new request.
    assert_eq!(seen.refused_at, [(0, 0)], "{:?}", seen.turns_to_block);
    // After the first, each request asks for half the look-ahead: a message
    // and an eighth per datagram.
    assert_eq!(usize::from(asked[0]), RESERVE_AHEAD);
    assert!(
        asked[1..]
            .iter()
            .all(|blocks| usize::from(*blocks) == RESERVE_AHEAD / 2),
        "{asked:?}"
    );
}

/// Each brand's own constant burst.
macro_rules! constant {
    ($($brand:ident: $burst:expr),* $(,)?) => {
        $({
            brand!($brand);
            never_dry::<$brand>($burst);
        })*
    };
}

#[test]
fn a_constant_burst_that_the_half_look_ahead_covers_never_runs_dry() {
    // A burst of c a turn leaves a deficit of c, 2c, ... modulo the half
    // look-ahead of eight once its requests went: at most 8 - gcd(c, 8). The
    // look-ahead covers the next burst while that is at most 16 - c: for 1
    // to 10, 12 and 16. Nine is the stand's TX 100; sixteen, the image's
    // `MAX_BATCH`, the whole look-ahead.
    constant!(
        B1: 1, B2: 2, B3: 3, B4: 4, B5: 5, B6: 6, B7: 7, B8: 8,
        B9: PER_BATCH, B10: 10, B12: 12, B16: MAX_BATCH,
    );
}

/// The sends of `bursts` that found none ready, after the first.
fn dry<B: 'static>(bursts: &[usize]) -> Vec<(usize, usize)> {
    let (seen, _, sent, _) = paced::<B>(bursts);
    assert_eq!(sent, total(bursts));
    assert_eq!(seen.refused_at.first(), Some(&(0, 0)));
    seen.refused_at[1..].to_vec()
}

#[test]
fn a_constant_burst_the_half_look_ahead_does_not_cover_runs_dry_as_counted() {
    // As it is: 11, 13, 14 and 15 a turn outrun what the half look-ahead
    // leaves ready, and wait once in so many bursts.
    brand!(Eleven);
    brand!(Thirteen);
    brand!(Fourteen);
    brand!(Fifteen);
    let counts = [
        dry::<Eleven>(&[11]).len(),
        dry::<Thirteen>(&[13]).len(),
        dry::<Fourteen>(&[14]).len(),
        dry::<Fifteen>(&[15]).len(),
    ];
    assert_eq!(counts, [16, 32, 32, 48], "of {BATCHES} bursts each");
}

#[test]
fn a_burst_of_sixteen_after_nine_waits_once_as_it_is() {
    // As it is: after a burst of nine, a burst of sixteen finds fifteen or
    // fewer ready, and its last sends wait for the next request. Seven of
    // each eight such pairs do.
    brand!(Net);
    let dry = dry::<Net>(&[PER_BATCH, MAX_BATCH]);
    assert_eq!(dry.len(), 28, "{dry:?}");
    assert!(
        dry.iter().all(|(batch, _)| batch % 2 == 1),
        "only bursts of sixteen wait: {dry:?}"
    );
}
