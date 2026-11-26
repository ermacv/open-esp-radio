extern crate std;
use super::*;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use oer_chips_esp32s31_hal::peripherals::aes::{AesState, AesStateField};
use oer_portable_crypto::{
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
    rfc3394::{KeyUnwrapError, unwrap},
};
use std::{cell::RefCell, rc::Rc};

/// The `STATE` codes the fake answers, by their generated names.
const IDLE: AesStateField = AesStateField::constant::<{ AesState::Idle.bits() }>();
const WORK: AesStateField = AesStateField::constant::<{ AesState::Work.bits() }>();

/// The fake's clock gate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Gate {
    #[default]
    Off,
    On,
}

/// The fake's mode word.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    Decrypt,
    Encrypt,
}

/// What `STATE` answers after a start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Answer {
    /// `WORK` for `busy_reads` reads, then `IDLE`.
    #[default]
    Normal,
    /// `WORK` for ever.
    Stuck,
    /// This code.
    Code(AesStateField),
}

#[derive(Default)]
struct Port {
    key: KeyWords,
    input: BlockWords,
    output: BlockWords,
    clock: Gate,
    mode: Mode,
    started: bool,
    starts: usize,
    reads: usize,
    /// `STATE` reads after a start that still answer `WORK`.
    busy_reads: usize,
    busy_left: usize,
    answer: Answer,
    /// What `STATE` answers before any start; `IDLE` unless set.
    before_start: Option<AesStateField>,
    /// Every atom called, in order.
    calls: std::vec::Vec<&'static str>,
}
struct Fake(Rc<RefCell<Port>>);
impl Registers for Fake {
    fn enable_clock(&mut self) {
        self.0.borrow_mut().calls.push("enable_clock");
        self.0.borrow_mut().clock = Gate::On;
    }
    fn disable_clock(&mut self) {
        self.0.borrow_mut().calls.push("disable_clock");
        self.0.borrow_mut().clock = Gate::Off;
    }
    fn assert_reset(&mut self) {
        self.0.borrow_mut().calls.push("assert_reset");
        let mut p = self.0.borrow_mut();
        p.key = [0; 8];
        p.input = [0; 4];
        p.output = [0; 4];
        p.started = false;
    }
    fn release_reset(&mut self) {
        self.0.borrow_mut().calls.push("release_reset");
    }
    fn release_parent_resets(&mut self) {
        self.0.borrow_mut().calls.push("release_parent_resets");
    }
    fn disable_interrupt(&mut self) {
        self.0.borrow_mut().calls.push("disable_interrupt");
    }
    fn disable_dma(&mut self) {
        self.0.borrow_mut().calls.push("disable_dma");
    }
    fn decrypt128(&mut self) {
        self.0.borrow_mut().calls.push("decrypt128");
        self.0.borrow_mut().mode = Mode::Decrypt;
    }
    fn encrypt128(&mut self) {
        self.0.borrow_mut().calls.push("encrypt128");
        self.0.borrow_mut().mode = Mode::Encrypt;
    }
    fn disable_pseudo_rounds(&mut self) {
        self.0.borrow_mut().calls.push("disable_pseudo_rounds");
    }
    fn hold(&mut self) {
        self.0.borrow_mut().calls.push("hold");
    }
    fn write_key(&mut self, words: &KeyWords) {
        self.0.borrow_mut().calls.push("write_key");
        self.0.borrow_mut().key = *words;
    }
    fn write_block(&mut self, words: &BlockWords) {
        self.0.borrow_mut().calls.push("write_block");
        self.0.borrow_mut().input = *words;
    }
    fn start(&mut self) {
        self.0.borrow_mut().calls.push("start");
        let mut p = self.0.borrow_mut();
        assert_eq!(p.clock, Gate::On);
        p.starts += 1;
        p.started = true;
        p.busy_left = p.busy_reads;
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
        let cipher = aes::Aes128::new(GenericArray::from_slice(&key));
        if p.mode == Mode::Encrypt {
            cipher.encrypt_block(&mut block);
        } else {
            cipher.decrypt_block(&mut block);
        }
        p.output = core::array::from_fn(|index| {
            u32::from_le_bytes(block[index * 4..index * 4 + 4].try_into().unwrap())
        });
    }
    fn state(&self) -> AesStateField {
        self.0.borrow_mut().calls.push("state");
        let mut p = self.0.borrow_mut();
        p.reads += 1;
        if !p.started {
            return p.before_start.unwrap_or(IDLE);
        }
        match p.answer {
            Answer::Code(code) => return code,
            Answer::Stuck => return WORK,
            Answer::Normal => {}
        }
        if p.busy_left > 0 {
            p.busy_left -= 1;
            return WORK;
        }
        IDLE
    }
    fn read_block(&self) -> BlockWords {
        self.0.borrow_mut().calls.push("read_block");
        self.0.borrow().output
    }
}

/// A counter of one cycle a microsecond that advances one at every sample,
/// so a settle on a stuck port ends.
#[derive(Default)]
struct Ticking(core::cell::Cell<u32>);
impl SettleCounter for Ticking {
    fn cycles(&self, limit: SettleLimit) -> u32 {
        limit.micros()
    }
    fn sample(&self) -> u32 {
        let now = self.0.get() + 1;
        self.0.set(now);
        now
    }
}

/// The RFC 3394 §4.1 unwrap, in place in its own buffer: whether it
/// answered the key data, or its refusal, and the buffer after it.
fn unwrapped<E: Aes128Engine>(engine: &mut E) -> (Result<bool, KeyUnwrapError<E::Stop>>, [u8; 24]) {
    let mut buffer = RFC3394_4_1_CIPHERTEXT;
    let outcome = unwrap(engine, &Aes128Key::copy_from(&RFC3394_4_1_KEK), &mut buffer)
        .map(|plaintext| *plaintext == RFC3394_4_1_KEY_DATA);
    (outcome, buffer)
}
fn assert_erased(port: &Rc<RefCell<Port>>) {
    let p = port.borrow();
    assert_eq!(p.clock, Gate::Off);
    assert_eq!(p.key, [0; 8]);
    assert_eq!(p.input, [0; 4]);
    assert_eq!(p.output, [0; 4]);
}

#[test]
fn fake_register_engine_passes_the_shared_rfc3394_conformance() {
    let port = Rc::new(RefCell::new(Port {
        busy_reads: 2,
        ..Port::default()
    }));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    conformance::key_unwrap_rfc3394(&mut engine);
    conformance::key_unwrap_rfc3394(&mut engine);
    assert_eq!(port.borrow().starts, 72);
    assert_erased(&port);
    drop(engine);
}

#[test]
fn a_block_waits_on_its_state_flag_and_the_key_is_erased_after_the_run() {
    let port = Rc::new(RefCell::new(Port {
        busy_reads: 3,
        ..Port::default()
    }));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    let (outcome, _) = unwrapped(&mut engine);
    assert_eq!(outcome, Ok(true));
    let p = port.borrow();
    assert_eq!(p.starts, 12);
    // The setup's read, then each block's three busy reads and its idle one.
    assert_eq!(p.reads, 1 + 12 * 4);
    drop(p);
    assert_erased(&port);
}

#[test]
fn encryption_and_decryption_select_their_mode_and_agree_with_the_cipher() {
    let port = Rc::new(RefCell::new(Port::default()));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    let key = Aes128Key::copy_from(&RFC3394_4_1_KEK);
    let reference = aes::Aes128::new(GenericArray::from_slice(&RFC3394_4_1_KEK));
    let plain: Block = *b"0123456789abcdef";
    let mut keyed = engine.load(&key).unwrap();
    let mut block = plain;
    keyed.encrypt(&mut block).unwrap();
    let mut expected = GenericArray::clone_from_slice(&plain);
    reference.encrypt_block(&mut expected);
    assert_eq!(block.as_slice(), expected.as_slice());
    keyed.decrypt(&mut block).unwrap();
    assert_eq!(block, plain);
    drop(keyed);
    assert_erased(&port);
}

/// The stop's calls: the block reset, the key and block words erased, the
/// clock gated.
const STOP: [&str; 5] = [
    "assert_reset",
    "release_reset",
    "write_key",
    "write_block",
    "disable_clock",
];

/// A block that times out or ends in another state returns its typed error
/// and erases the block; it ends its run, whose later blocks answer the same
/// error and touch nothing; the drop is the whole stop; and the next load
/// starts the accelerator again, failing again while the port does and
/// running a block once it answers idle.
#[test]
fn a_failed_block_ends_its_run_and_the_next_load_starts_again() {
    for (answer, observed) in [
        (
            Answer::Stuck,
            AesStop::Timeout {
                observation: WORK.get(),
            },
        ),
        (
            Answer::Code(AesStateField::constant::<3>()),
            AesStop::InvalidState { observation: 3 },
        ),
    ] {
        let error = EngineStopped;
        let port = Rc::new(RefCell::new(Port {
            answer,
            ..Port::default()
        }));
        let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
        let key = Aes128Key::copy_from(&RFC3394_4_1_KEK);
        let plain: Block = *b"0123456789abcdef";

        let mut keyed = engine.load(&key).expect("an idle accelerator");
        let mut block = plain;
        assert_eq!(keyed.decrypt(&mut block), Err(error));
        assert_eq!(block, [0; 16], "the failed block is erased");
        if answer == Answer::Stuck {
            // One microsecond per counter sample: the setup's read, then one
            // attempt per microsecond of the settle and the last one after
            // its end.
            assert_eq!(
                port.borrow().reads as u64,
                1 + BLOCK_SETTLE.limit().as_micros(),
                "the wait ends at its settle"
            );
        }
        port.borrow_mut().calls.clear();
        let mut later = plain;
        assert_eq!(keyed.decrypt(&mut later), Err(error));
        assert_eq!(keyed.encrypt(&mut later), Err(error));
        assert_eq!(later, plain, "a later block is not touched");
        assert!(port.borrow().calls.is_empty(), "nor is the hardware");
        drop(keyed);
        assert_eq!(
            engine.last_stop(),
            Some(observed),
            "the driver keeps what the stop observed"
        );
        assert_eq!(port.borrow().calls, STOP);
        assert_erased(&port);

        // The port still fails: the next load starts, and the block fails
        // again with its own error.
        port.borrow_mut().calls.clear();
        let mut keyed = engine.load(&key).expect("a start like the first");
        assert_eq!(
            port.borrow().calls[..3],
            ["enable_clock", "assert_reset", "release_reset"]
        );
        let mut block = plain;
        assert_eq!(keyed.decrypt(&mut block), Err(error));
        drop(keyed);

        // The port answers idle: the next load runs a block.
        port.borrow_mut().answer = Answer::Normal;
        let mut keyed = engine.load(&key).expect("a start like the first");
        let mut block = plain;
        keyed.encrypt(&mut block).expect("the block runs");
        let mut expected = GenericArray::clone_from_slice(&plain);
        aes::Aes128::new(GenericArray::from_slice(&RFC3394_4_1_KEK)).encrypt_block(&mut expected);
        assert_eq!(block.as_slice(), expected.as_slice());
        drop(keyed);
        assert_erased(&port);

        // Through the unwrap: every failed request gives its typed error.
        port.borrow_mut().answer = answer;
        for _ in 0..2 {
            let (outcome, buffer) = unwrapped(&mut engine);
            assert_eq!(outcome, Err(KeyUnwrapError::Engine(error)));
            assert_eq!(buffer, [0; 24], "a stopped unwrap clears its buffer");
            assert_erased(&port);
        }
    }
}

/// `STATE` not idle at a start refuses the key with its observation and
/// leaves the block stopped; the next load starts again.
#[test]
fn a_not_idle_accelerator_refuses_the_key_and_the_next_load_starts_again() {
    let port = Rc::new(RefCell::new(Port {
        before_start: Some(WORK),
        ..Port::default()
    }));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    let key = Aes128Key::copy_from(&RFC3394_4_1_KEK);
    assert_eq!(engine.load(&key).err(), Some(EngineStopped));
    assert_eq!(
        engine.last_stop(),
        Some(AesStop::InvalidState {
            observation: WORK.get()
        })
    );
    let calls = port.borrow().calls.clone();
    assert_eq!(
        calls[calls.len() - STOP.len()..],
        STOP,
        "the start is stopped"
    );
    assert!(
        !calls[..calls.len() - STOP.len()].contains(&"write_key"),
        "no key reaches the accelerator before the stop"
    );
    assert_eq!(port.borrow().starts, 0);
    assert_erased(&port);

    port.borrow_mut().before_start = None;
    let mut keyed = engine.load(&key).expect("a start like the first");
    let mut block = *b"0123456789abcdef";
    keyed.encrypt(&mut block).expect("the block runs");
    assert_eq!(port.borrow().starts, 1);
    drop(keyed);
    assert_erased(&port);
}

/// A key's load clocks and resets the accelerator before its configuration
/// and checks `STATE` before the key; the clear erases, then gates.
#[test]
fn the_load_and_the_clear_call_the_atoms_in_the_vendor_order() {
    let port = Rc::new(RefCell::new(Port::default()));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    let key = Aes128Key::copy_from(&RFC3394_4_1_KEK);
    let keyed = engine.load(&key).expect("an idle accelerator");
    assert_eq!(
        port.borrow().calls,
        [
            "enable_clock",
            "assert_reset",
            "release_reset",
            "release_parent_resets",
            "disable_interrupt",
            "disable_dma",
            "disable_pseudo_rounds",
            "hold",
            "state",
            "write_key",
        ]
    );
    port.borrow_mut().calls.clear();
    drop(keyed);
    assert_eq!(
        port.borrow().calls,
        [
            "assert_reset",
            "release_reset",
            "write_key",
            "write_block",
            "disable_clock",
        ]
    );
}

/// A run writes the mode word once a direction: blocks in one direction
/// write it once, and each change of direction writes it once more.
#[test]
fn the_mode_word_is_written_once_a_direction() {
    let port = Rc::new(RefCell::new(Port::default()));
    let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
    let key = Aes128Key::copy_from(&RFC3394_4_1_KEK);
    let mut keyed = engine.load(&key).expect("an idle accelerator");
    port.borrow_mut().calls.clear();
    let mut block = *b"0123456789abcdef";
    for _ in 0..3 {
        keyed.encrypt(&mut block).expect("the block runs");
    }
    for _ in 0..2 {
        keyed.decrypt(&mut block).expect("the block runs");
    }
    keyed.encrypt(&mut block).expect("the block runs");
    let modes: std::vec::Vec<&str> = port
        .borrow()
        .calls
        .iter()
        .copied()
        .filter(|call| *call == "encrypt128" || *call == "decrypt128")
        .collect();
    assert_eq!(modes, ["encrypt128", "decrypt128", "encrypt128"]);
    assert_eq!(port.borrow().starts, 6);
}
