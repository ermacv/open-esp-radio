//! Receive loans over a radio with a finite pool, before the service lends:
//! a slow consumer deprives neither others nor control traffic of buffers, and
//! a full stop handles a buffer held for ever. The pool, the arrivals and the
//! work per turn are this file's; [`Loans`] is the production accounting, in
//! its production owner.
//!
//! Each scenario has a control without credits, where the same load has to go
//! wrong: a model that cannot fail proves nothing.

use oer_protocols_network_core::{GROUPS, Loan, Loans, NotLent, Pool, claim};

const SLOW: usize = 0;
const FAST: usize = 1;
const BUFFERS: usize = 32;
const RESERVE: usize = 8;
const CREDIT: usize = 12;

/// What an arrival is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Traffic {
    Socket(usize),
    /// Handled in the turn it is read, with no loan: ARP, a handshake.
    Control,
}

/// How frames meet the sockets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Policy {
    Credits,
    /// Every frame is lent: the pool is the only limit.
    NoLimit,
}

#[derive(Debug, Default)]
struct Counts {
    /// The radio had no buffer for a frame.
    lost_slow: u32,
    lost_fast: u32,
    lost_control: u32,
    /// A frame came, had no credit and went back.
    dropped_slow: u32,
    dropped_fast: u32,
    highest_slow: usize,
}

/// The brand of this file's networks: the accounting belongs to a core.
macro_rules! network {
    ($policy:expr) => {{
        struct Net;
        Radio::new(pool_of::<Net>(), $policy)
    }};
}

type TestPool<B> = Pool<'static, B, 64, 1>;

fn pool_of<B: 'static>() -> TestPool<B> {
    let memory = Box::leak(Box::new([[0; 64]; 1]));
    claim::<B, 64, 1>(memory).unwrap().0
}

struct Radio<B: 'static> {
    free: usize,
    ring: Vec<Traffic>,
    pool: TestPool<B>,
    held_slow: Vec<Option<Loan<B>>>,
    held_fast: Vec<Option<Loan<B>>>,
    policy: Policy,
    counts: Counts,
}

impl<B: 'static> Radio<B> {
    fn new(mut pool: TestPool<B>, policy: Policy) -> Self {
        let loans = pool.loans();
        loans.pool(BUFFERS, RESERVE).unwrap();
        loans.grant(SLOW, CREDIT).unwrap();
        loans.grant(FAST, CREDIT).unwrap();
        Self {
            free: BUFFERS,
            ring: Vec::new(),
            pool,
            held_slow: Vec::new(),
            held_fast: Vec::new(),
            policy,
            counts: Counts::default(),
        }
    }

    fn loans(&mut self) -> &mut Loans<B, GROUPS> {
        self.pool.loans()
    }

    fn arrive(&mut self, traffic: Traffic) {
        if self.free == 0 {
            match traffic {
                Traffic::Socket(SLOW) => self.counts.lost_slow += 1,
                Traffic::Socket(_) => self.counts.lost_fast += 1,
                Traffic::Control => self.counts.lost_control += 1,
            }
            return;
        }
        self.free -= 1;
        self.ring.push(traffic);
    }

    /// One turn of the stack: at most `budget` frames, a limit of its own.
    fn turn(&mut self, budget: usize) {
        for _ in 0..budget.min(self.ring.len()) {
            match self.ring.remove(0) {
                Traffic::Control => self.free += 1,
                Traffic::Socket(socket) => self.deliver(socket),
            }
        }
    }

    fn deliver(&mut self, socket: usize) {
        let lent = match self.policy {
            Policy::Credits => self.loans().lend(socket).map(Some),
            Policy::NoLimit => Ok(None),
        };
        match lent {
            Ok(loan) if socket == SLOW => self.held_slow.push(loan),
            Ok(loan) => self.held_fast.push(loan),
            Err(NotLent::NoCredit) => {
                self.free += 1;
                if socket == SLOW {
                    self.counts.dropped_slow += 1;
                } else {
                    self.counts.dropped_fast += 1;
                }
            }
            Err(other) => panic!("{other:?}"),
        }
        self.counts.highest_slow = self.counts.highest_slow.max(self.held_slow.len());
    }

    fn fast_reads_all(&mut self) {
        while !self.held_fast.is_empty() {
            self.free += 1;
            if let Some(loan) = self.held_fast.remove(0) {
                self.loans().give_back(loan);
            }
        }
    }

    fn slow_reads_one(&mut self) {
        if !self.held_slow.is_empty() {
            self.free += 1;
            if let Some(loan) = self.held_slow.remove(0) {
                self.loans().give_back(loan);
            }
        }
    }
}

/// Two frames for each socket and one of control each step, and a consumer
/// that reads one frame in `slow_every` steps, or never at zero. The burst
/// starts one frame later each step, or a fixed order would decide who finds
/// the pool empty.
fn run<B: 'static>(mut radio: Radio<B>, slow_every: usize, budget: usize) -> Radio<B> {
    let burst = [
        Traffic::Socket(SLOW),
        Traffic::Socket(FAST),
        Traffic::Control,
        Traffic::Socket(SLOW),
        Traffic::Socket(FAST),
    ];
    for step in 1..=2000 {
        for at in 0..burst.len() {
            radio.arrive(burst[(step + at) % burst.len()]);
        }
        radio.turn(budget);
        radio.fast_reads_all();
        if slow_every != 0 && step % slow_every == 0 {
            radio.slow_reads_one();
        }
    }
    radio
}

#[test]
fn a_consumer_that_never_reads_costs_others_no_buffer() {
    let radio = run(network!(Policy::Credits), 0, 8);
    assert_eq!(radio.counts.lost_fast, 0);
    assert_eq!(radio.counts.lost_control, 0);
    assert_eq!(radio.counts.dropped_fast, 0);
    assert_eq!(radio.counts.highest_slow, CREDIT);
    assert_eq!(
        radio.counts.lost_slow, 0,
        "its frames end at its own credit"
    );
    assert!(radio.counts.dropped_slow > 0);
}

#[test]
fn a_slow_reader_costs_others_no_buffer_either() {
    let radio = run(network!(Policy::Credits), 10, 8);
    assert_eq!(radio.counts.lost_fast, 0);
    assert_eq!(radio.counts.lost_control, 0);
    assert_eq!(radio.counts.dropped_fast, 0);
    assert!(radio.counts.highest_slow <= CREDIT);
}

#[test]
fn with_no_credit_the_same_consumer_takes_everyones_buffers() {
    let radio = run(network!(Policy::NoLimit), 0, 8);
    assert!(radio.counts.lost_fast > 0);
    assert!(radio.counts.lost_control > 0);
    assert_eq!(radio.free, 0);
}

/// A credit bounds memory only: more arrivals than a turn's budget lose
/// frames of every class with all consumers fast, and this names that limit.
#[test]
fn a_credit_does_not_bound_the_work() {
    let radio = run(network!(Policy::Credits), 1, 4);
    assert!(radio.counts.lost_control > 0);
    assert!(radio.counts.lost_fast > 0);
}

/// One frame in the slow socket's hands, for ever.
fn holding<B: 'static>(mut radio: Radio<B>) -> Radio<B> {
    radio.arrive(Traffic::Socket(SLOW));
    radio.turn(1);
    assert_eq!(radio.held_slow.len(), 1);
    radio
}

#[test]
fn a_stop_is_not_held_up_by_a_buffer_held_for_ever() {
    let mut radio = holding(network!(Policy::Credits));
    radio.loans().stop();
    assert_eq!(radio.loans().lend(FAST).err(), Some(NotLent::Stopping));
    assert_eq!(radio.loans().outstanding(), 1, "the stop itself is done");
    assert!(
        !radio.loans().released(),
        "and the pool is not free to reuse"
    );
}

#[test]
fn a_held_buffer_stays_its_holders_and_frees_the_pool_when_it_returns() {
    let mut radio = holding(network!(Policy::Credits));
    let before = radio.free;
    radio.loans().stop();
    for _ in 0..1000 {
        assert!(!radio.loans().released(), "no expiry frees it");
        assert_eq!(radio.free, before);
    }
    radio.slow_reads_one();
    assert!(radio.loans().released());
    assert_eq!(radio.free, BUFFERS);
}

#[test]
fn a_new_installation_counts_what_the_old_one_still_holds() {
    let mut radio = holding(network!(Policy::Credits));
    radio.loans().stop();
    radio.loans().reinstall();
    let room = BUFFERS - RESERVE - 1;
    assert!(radio.loans().grant(FAST, room + 1).is_err());
    radio.loans().grant(FAST, room).unwrap();
    radio.slow_reads_one();
    assert_eq!(radio.loans().outstanding(), 0);
    radio.loans().grant(FAST, room + 1).unwrap();
}
