#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
extern crate std;
use super::*;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use core::task::Waker;
use oer_crypto::{
    Aes128Key, KeyData, KeyUnwrapError,
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK},
};
use oer_time::{Instant, Timer};
use oer_time_virtual::VirtualClock;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Port {
    key: KeyWords,
    input: BlockWords,
    output: BlockWords,
    clocked: bool,
    started: bool,
    starts: usize,
    reads: usize,
    stuck: bool,
    invalid: Option<u32>,
}
struct Fake(Rc<RefCell<Port>>);
impl Registers for Fake {
    fn clock(&mut self, on: bool) {
        self.0.borrow_mut().clocked = on;
    }
    fn reset(&mut self, asserted: bool) {
        if asserted {
            let mut p = self.0.borrow_mut();
            p.key = [0; 8];
            p.input = [0; 4];
            p.output = [0; 4];
            p.started = false;
        }
    }
    fn release_parent_resets(&mut self) {}
    fn disable_interrupt(&mut self) {}
    fn disable_dma(&mut self) {}
    fn decrypt128(&mut self) {}
    fn encrypt128(&mut self) {}
    fn disable_pseudo_rounds(&mut self) {}
    fn hold(&mut self) {}
    fn write_key(&mut self, words: &KeyWords) {
        self.0.borrow_mut().key = *words;
    }
    fn write_block(&mut self, words: &BlockWords) {
        self.0.borrow_mut().input = *words;
    }
    fn start(&mut self) {
        let mut p = self.0.borrow_mut();
        assert!(p.clocked);
        p.starts += 1;
        p.started = true;
        // The fake register oracle interprets the vendor's little-endian
        // word interface independently of the driver's packing helpers.
        let key: std::vec::Vec<u8> = p
            .key
            .iter()
            .take(4)
            .flat_map(|word| word.to_le_bytes())
            .collect();
        let input: std::vec::Vec<u8> = p.input.iter().flat_map(|word| word.to_le_bytes()).collect();
        let mut block = GenericArray::clone_from_slice(&input);
        aes::Aes128::new(GenericArray::from_slice(&key)).decrypt_block(&mut block);
        p.output = core::array::from_fn(|index| {
            u32::from_le_bytes(block[index * 4..index * 4 + 4].try_into().unwrap())
        });
    }
    fn state(&self) -> u32 {
        let mut p = self.0.borrow_mut();
        p.reads += 1;
        if !p.started {
            IDLE
        } else {
            p.invalid.unwrap_or(if p.stuck { WORK } else { IDLE })
        }
    }
    fn read_block(&self) -> BlockWords {
        self.0.borrow().output
    }
}

struct Wait {
    clock: Rc<VirtualClock>,
    at: Instant,
}
impl Future for Wait {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut wait = core::pin::pin!(self.clock.wait_until(self.at));
        wait.as_mut().poll(cx)
    }
}
fn request() -> KeyUnwrapRequest<32> {
    KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap(),
    )
}
fn assert_erased(port: &Rc<RefCell<Port>>) {
    let p = port.borrow();
    assert!(!p.clocked);
    assert_eq!(p.key, [0; 8]);
    assert_eq!(p.input, [0; 4]);
    assert_eq!(p.output, [0; 4]);
}

#[test]
fn fake_register_engine_passes_the_shared_rfc3394_conformance() {
    let clock = Rc::new(VirtualClock::new());
    let port = Rc::new(RefCell::new(Port::default()));
    let factory = Rc::clone(&clock);
    let engine = AesKeyUnwrap::new(
        Fake(Rc::clone(&port)),
        RcClock(Rc::clone(&clock)),
        move |at| Wait {
            clock: Rc::clone(&factory),
            at,
        },
    );
    let engine = conformance::key_unwrap_rfc3394::<_, 32>(engine, || {
        clock.advance_to_next_deadline().unwrap();
    });
    let engine = conformance::key_unwrap_rfc3394::<_, 512>(engine, || {
        clock.advance_to_next_deadline().unwrap();
    });
    assert_eq!(port.borrow().starts, 72);
    assert_erased(&port);
    drop(engine);
}
struct RcClock(Rc<VirtualClock>);
impl Clock for RcClock {
    fn now(&self) -> Instant {
        self.0.now()
    }
}

#[test]
fn each_block_yields_before_completion_and_cancellation_erases_hardware() {
    let clock = Rc::new(VirtualClock::new());
    let port = Rc::new(RefCell::new(Port::default()));
    let factory = Rc::clone(&clock);
    let engine = AesKeyUnwrap::new(Fake(Rc::clone(&port)), RcClock(clock), move |at| Wait {
        clock: Rc::clone(&factory),
        at,
    });
    let mut operation = engine.unwrap_key(request());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut operation).poll(&mut cx).is_pending());
    assert_eq!(port.borrow().starts, 1);
    assert_eq!(port.borrow().reads, 1);
    assert_ne!(port.borrow().key, [0; 8]);
    drop(operation);
    assert_erased(&port);
}

#[test]
fn timeout_and_invalid_state_are_typed_and_poison_the_owned_engine() {
    for (stuck, invalid, error) in [
        (true, None, BlockError::Timeout { observation: WORK }),
        (false, Some(3), BlockError::InvalidState { observation: 3 }),
    ] {
        let clock = Rc::new(VirtualClock::new());
        let port = Rc::new(RefCell::new(Port {
            stuck,
            invalid,
            ..Port::default()
        }));
        let factory = Rc::clone(&clock);
        let engine = AesKeyUnwrap::new(
            Fake(Rc::clone(&port)),
            RcClock(Rc::clone(&clock)),
            move |at| Wait {
                clock: Rc::clone(&factory),
                at,
            },
        );
        let (engine, response) = conformance::complete(engine, request(), &mut || {
            clock.advance(Duration::from_millis(1)).unwrap();
        });
        assert_eq!(response.plaintext(), Err(KeyUnwrapError::Engine(error)));
        assert_erased(&port);
        let starts = port.borrow().starts;
        let reads = port.borrow().reads;
        let (_, response) =
            conformance::complete(engine, request(), &mut || panic!("poison refusal is ready"));
        assert_eq!(
            response.plaintext(),
            Err(KeyUnwrapError::Engine(BlockError::Unavailable))
        );
        assert_eq!(port.borrow().starts, starts);
        assert_eq!(port.borrow().reads, reads);
    }
}

#[test]
fn dropping_before_first_poll_does_not_touch_hardware() {
    let clock = Rc::new(VirtualClock::new());
    let port = Rc::new(RefCell::new(Port::default()));
    let engine = AesKeyUnwrap::new(
        Fake(Rc::clone(&port)),
        RcClock(Rc::clone(&clock)),
        move |at| Wait {
            clock: Rc::clone(&clock),
            at,
        },
    );
    drop(engine.unwrap_key(request()));
    assert_eq!(port.borrow().starts, 0);
    assert_eq!(port.borrow().reads, 0);
    assert_erased(&port);
}

#[test]
fn a_busy_block_waits_for_time_between_status_samples() {
    let clock = Rc::new(VirtualClock::new());
    let port = Rc::new(RefCell::new(Port {
        stuck: true,
        ..Port::default()
    }));
    let factory = Rc::clone(&clock);
    let engine = AesKeyUnwrap::new(
        Fake(Rc::clone(&port)),
        RcClock(Rc::clone(&clock)),
        move |at| Wait {
            clock: Rc::clone(&factory),
            at,
        },
    );
    let mut operation = engine.unwrap_key(request());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut operation).poll(&mut cx).is_pending());
    for _ in 0..4 {
        assert!(Pin::new(&mut operation).poll(&mut cx).is_pending());
    }
    assert_eq!(
        port.borrow().reads,
        1,
        "no status polling while the timer is pending"
    );
    clock.advance_to_next_deadline().unwrap();
    assert!(Pin::new(&mut operation).poll(&mut cx).is_pending());
    assert_eq!(port.borrow().reads, 2);
    assert!(clock.next_deadline().is_some());
    drop(operation);
    assert_erased(&port);
}

#[test]
fn a_clock_overflow_refuses_before_start_and_erases_the_key() {
    let clock = Rc::new(VirtualClock::starting_at(Instant::from_micros(u64::MAX)));
    let port = Rc::new(RefCell::new(Port::default()));
    let factory = Rc::clone(&clock);
    let engine = AesKeyUnwrap::new(Fake(Rc::clone(&port)), RcClock(clock), move |at| Wait {
        clock: Rc::clone(&factory),
        at,
    });
    let (_, response) =
        conformance::complete(engine, request(), &mut || panic!("overflow is ready"));
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(BlockError::TimeOverflow))
    );
    assert_eq!(port.borrow().starts, 0);
    assert_erased(&port);
}
