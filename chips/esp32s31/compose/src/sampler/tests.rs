use super::*;
use core::cell::RefCell;
use oer_memory::AffineSpscQueue;
use std::rc::Rc;

/// What the fake timer was asked, in order.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    Start(u64),
    At(u64),
    Disarm,
}

#[derive(Clone, Default)]
struct Fake {
    calls: Rc<RefCell<Vec<Call>>>,
    refuse_at: Option<u64>,
    refuse_start: bool,
}

impl Alarm for Fake {
    fn start(&mut self, period_us: u64) -> bool {
        self.calls.borrow_mut().push(Call::Start(period_us));
        !self.refuse_start
    }
    fn alarm_at(&mut self, target_us: u64) -> bool {
        self.calls.borrow_mut().push(Call::At(target_us));
        self.refuse_at != Some(target_us)
    }
    fn disarm(&mut self) {
        self.calls.borrow_mut().push(Call::Disarm);
    }
}

type Queue = AffineSpscQueue<Session<Fake>, 1, CompactSpscCursor>;

fn sampler(fake: Fake) -> (Sampler<Fake>, Handler<Fake>, &'static Shared) {
    let request: &'static mut Queue = Box::leak(Box::new(Queue::new()));
    let reply: &'static mut Queue = Box::leak(Box::new(Queue::new()));
    let shared: &'static Shared = Box::leak(Box::new(Shared::new()));
    let (send, receive) = request.split();
    let (back, replies) = reply.split();
    (
        Sampler::new(fake, send, replies, shared),
        Handler::new(receive, back),
        shared,
    )
}

fn unmasked(body: &mut dyn FnMut()) {
    body();
}

#[test]
fn every_tick_is_due_a_whole_period_after_the_last_whatever_the_latency() {
    let fake = Fake::default();
    let (mut sampler, mut handler, shared) = sampler(fake.clone());
    sampler.start(1000, unmasked).unwrap();
    assert!(!sampler.available());
    let samples: Vec<_> = (0..3)
        .map(|pc| handler.tick(shared, true, 0x4000_0000 + pc, 3).unwrap())
        .collect();
    assert_eq!(
        samples
            .iter()
            .map(|sample| (sample.tick(), sample.pc(), sample.level()))
            .collect::<Vec<_>>(),
        [
            (0, 0x4000_0000, 3),
            (1, 0x4000_0001, 3),
            (2, 0x4000_0002, 3)
        ]
    );
    assert_eq!(
        *fake.calls.borrow(),
        [
            Call::Start(1000),
            Call::At(2000),
            Call::At(3000),
            Call::At(4000)
        ]
    );
}

#[test]
fn a_stop_takes_no_sample_and_hands_the_stopped_session_back() {
    let fake = Fake::default();
    let (mut sampler, mut handler, shared) = sampler(fake.clone());
    sampler.start(500, unmasked).unwrap();
    assert!(handler.tick(shared, true, 1, 0).is_some());
    sampler.request_stop();
    assert_eq!(
        sampler.take_back(),
        None,
        "the handler keeps it until its tick"
    );
    assert_eq!(handler.tick(shared, true, 2, 0), None);
    assert_eq!(fake.calls.borrow().last(), Some(&Call::Disarm));
    assert_eq!(
        sampler.take_back(),
        Some(Ended {
            ticks: 1,
            cut: false
        })
    );
    assert!(sampler.available());
    // The next session counts from zero again.
    sampler.start(500, unmasked).unwrap();
    assert_eq!(handler.tick(shared, true, 3, 0).unwrap().tick(), 0);
}

#[test]
fn a_tick_without_a_session_or_pending_event_reports_nothing() {
    let (_sampler, mut handler, shared) = sampler(Fake::default());
    assert_eq!(handler.tick(shared, false, 1, 0), None);
    assert_eq!(shared.stray(), 0);
    assert_eq!(handler.tick(shared, true, 1, 0), None);
    assert_eq!(shared.stray(), 1);
}

#[test]
fn a_refused_target_cuts_the_session_and_hands_it_back() {
    let fake = Fake {
        refuse_at: Some(3000),
        ..Fake::default()
    };
    let (mut sampler, mut handler, shared) = sampler(fake.clone());
    sampler.start(1000, unmasked).unwrap();
    assert!(handler.tick(shared, true, 1, 0).is_some());
    assert!(
        handler.tick(shared, true, 2, 0).is_some(),
        "the due tick is still reported"
    );
    assert_eq!(
        sampler.take_back(),
        Some(Ended {
            ticks: 2,
            cut: true
        })
    );
}

#[test]
fn a_refused_period_or_a_running_session_leaves_the_sampler_as_it_was() {
    let fake = Fake {
        refuse_start: true,
        ..Fake::default()
    };
    let (mut sampler, _handler, _) = sampler(fake.clone());
    assert_eq!(sampler.start(1000, unmasked), Err(StartError::Period));
    assert!(sampler.available());
    assert_eq!(fake.calls.borrow().last(), Some(&Call::Disarm));
    let (mut running, _handler, _) = self::sampler(Fake::default());
    running.start(1000, unmasked).unwrap();
    assert_eq!(running.start(1000, unmasked), Err(StartError::Unavailable));
}
