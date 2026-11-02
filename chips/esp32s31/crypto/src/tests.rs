#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
extern crate std;
use super::*;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use oer_crypto::{
    KeyData, KeyUnwrapError, KeyUnwrapRequest,
    block::BlockKeyUnwrap,
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Port {
    key: KeyWords,
    input: BlockWords,
    output: BlockWords,
    clocked: bool,
    encrypting: bool,
    started: bool,
    starts: usize,
    reads: usize,
    /// `STATE` reads after a start that still answer `WORK`.
    busy_reads: usize,
    busy_left: usize,
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
    fn decrypt128(&mut self) {
        self.0.borrow_mut().encrypting = false;
    }
    fn encrypt128(&mut self) {
        self.0.borrow_mut().encrypting = true;
    }
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
        if p.encrypting {
            cipher.encrypt_block(&mut block);
        } else {
            cipher.decrypt_block(&mut block);
        }
        p.output = core::array::from_fn(|index| {
            u32::from_le_bytes(block[index * 4..index * 4 + 4].try_into().unwrap())
        });
    }
    fn state(&self) -> u32 {
        let mut p = self.0.borrow_mut();
        p.reads += 1;
        if !p.started {
            return IDLE;
        }
        if let Some(invalid) = p.invalid {
            return invalid;
        }
        if p.stuck {
            return WORK;
        }
        if p.busy_left > 0 {
            p.busy_left -= 1;
            return WORK;
        }
        IDLE
    }
    fn read_block(&self) -> BlockWords {
        self.0.borrow().output
    }
}

/// A clock that advances one microsecond at every read, so a settle on a
/// stuck port ends.
#[derive(Default)]
struct Ticking(core::cell::Cell<u64>);
impl Clock for Ticking {
    fn now(&self) -> oer_time::Instant {
        let now = self.0.get() + 1;
        self.0.set(now);
        oer_time::Instant::from_micros(now)
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
    let port = Rc::new(RefCell::new(Port {
        busy_reads: 2,
        ..Port::default()
    }));
    let engine = BlockKeyUnwrap::new(AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default()));
    let engine = conformance::key_unwrap_rfc3394::<_, 32>(engine, || {
        panic!("a synchronous engine's operation is ready on its first poll")
    });
    let engine = conformance::key_unwrap_rfc3394::<_, 512>(engine, || {
        panic!("a synchronous engine's operation is ready on its first poll")
    });
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
    let response = oer_crypto::block::unwrap_now(&mut engine, request());
    assert_eq!(response.plaintext(), Ok(&RFC3394_4_1_KEY_DATA[..]));
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

#[test]
fn timeout_and_invalid_state_are_typed_and_poison_the_owned_engine() {
    for (stuck, invalid, error) in [
        (true, None, BlockError::Timeout { observation: WORK }),
        (false, Some(3), BlockError::InvalidState { observation: 3 }),
    ] {
        let port = Rc::new(RefCell::new(Port {
            stuck,
            invalid,
            ..Port::default()
        }));
        let mut engine = AesBlocks::new(Fake(Rc::clone(&port)), Ticking::default());
        let response = oer_crypto::block::unwrap_now(&mut engine, request());
        assert_eq!(response.plaintext(), Err(KeyUnwrapError::Engine(error)));
        assert_eq!(port.borrow().starts, 1, "no block after the refused one");
        if stuck {
            // One microsecond per clock read: the setup's read, then one
            // attempt per microsecond of the settle and the last one after
            // its end.
            assert_eq!(
                port.borrow().reads as u64,
                1 + BLOCK_SETTLE.limit().as_micros(),
                "the wait ends at its settle"
            );
        }
        assert_erased(&port);
        let starts = port.borrow().starts;
        let reads = port.borrow().reads;
        let response = oer_crypto::block::unwrap_now(&mut engine, request());
        assert_eq!(
            response.plaintext(),
            Err(KeyUnwrapError::Engine(BlockError::Unavailable))
        );
        assert_eq!(port.borrow().starts, starts);
        assert_eq!(port.borrow().reads, reads);
    }
}

#[test]
fn a_not_idle_accelerator_refuses_the_key_and_is_erased() {
    let port = Rc::new(RefCell::new(Port::default()));
    let mut engine = AesBlocks::new(StuckSetup(Fake(Rc::clone(&port))), Ticking::default());
    let response = oer_crypto::block::unwrap_now(&mut engine, request());
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(BlockError::InvalidState {
            observation: WORK
        }))
    );
    assert_eq!(port.borrow().starts, 0);
    assert_erased(&port);
}

/// A port whose `STATE` reads `WORK` before any start.
struct StuckSetup(Fake);
impl Registers for StuckSetup {
    fn clock(&mut self, on: bool) {
        self.0.clock(on);
    }
    fn reset(&mut self, asserted: bool) {
        self.0.reset(asserted);
    }
    fn release_parent_resets(&mut self) {}
    fn disable_interrupt(&mut self) {}
    fn disable_dma(&mut self) {}
    fn decrypt128(&mut self) {
        self.0.decrypt128();
    }
    fn encrypt128(&mut self) {
        self.0.encrypt128();
    }
    fn disable_pseudo_rounds(&mut self) {}
    fn hold(&mut self) {}
    fn write_key(&mut self, words: &KeyWords) {
        self.0.write_key(words);
    }
    fn write_block(&mut self, words: &BlockWords) {
        self.0.write_block(words);
    }
    fn start(&mut self) {
        self.0.start();
    }
    fn state(&self) -> u32 {
        WORK
    }
    fn read_block(&self) -> BlockWords {
        self.0.read_block()
    }
}
