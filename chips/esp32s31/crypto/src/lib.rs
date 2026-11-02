#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Value-owned Typical AES as a synchronous AES-128 engine
//! ([`oer_portable_crypto::block::Aes128Engine`]). No DMA, AES ISR, allocation or
//! software fallback. Each block is written, started and waited for on the
//! `STATE` flag in place, within the named settle [`BLOCK_SETTLE`]
//! (`oer_portable_time::settle`); a block still working at its end is a typed
//! timeout.
//! SOURCE: TRM_S31_V01_AES §10.5.3 and ESP-IDF 4d80c1b2 aes_ll.h/aes_hal.c
//! for the sequence; neither states a block latency.

use oer_portable_crypto::{
    Aes128Key,
    block::{Aes128Engine, Block, BlockEngine, BlockError},
};
use oer_chips_esp32s31_hal::peripherals::aes::{BlockWords, IDLE, KeyWords, Registers, WORK};
use oer_portable_time::{
    Clock, Duration,
    settle::{Settle, SettleLimit, settle},
};
use zeroize::Zeroize;

/// The named settle of one Typical block, from its start to `STATE`
/// reading idle.
///
/// SOURCE: the `esp32s31-aes-timing` probe on s31-c (leases #225 and #227,
/// 2026-11-02, 320.3 MHz CPU): the start to `STATE` reading idle, polled
/// back to back, took 270–279 cycles (0.87 µs) over 256 blocks. The settle
/// allows ten times that and stays under [`SettleLimit::MAX`].
pub const BLOCK_SETTLE: SettleLimit = match SettleLimit::new(Duration::from_micros(10)) {
    Some(limit) => limit,
    None => panic!("a block's settle lies within the longest a settle may wait"),
};

/// The accelerator as one owner's synchronous AES-128 engine, its settles
/// measured on `clock`. A failed engine retains its port and refuses later
/// keys without touching the hardware again.
pub struct AesBlocks<R: Registers, C: Clock> {
    registers: R,
    clock: C,
    active: bool,
    poisoned: bool,
}

impl<R: Registers, C: Clock> AesBlocks<R, C> {
    pub const fn new(registers: R, clock: C) -> Self {
        Self {
            registers,
            clock,
            active: false,
            poisoned: false,
        }
    }

    /// Erase the key and the blocks and gate the clock.
    fn clear(&mut self) {
        if self.active {
            // SOURCE: aes_ll_reset_register and the AES register reset values.
            // No DMA ever reaches request storage. Reset restores the documented
            // output state; explicit writes erase all software key/input words.
            self.registers.reset(true);
            self.registers.reset(false);
            self.registers.write_key(&KeyWords::default());
            self.registers.write_block(&BlockWords::default());
            self.registers.clock(false);
            self.active = false;
        }
    }

    fn initialize(&mut self, key: &Aes128Key) -> Result<(), BlockError> {
        if self.poisoned {
            return Err(BlockError::Unavailable);
        }
        self.active = true;
        self.registers.clock(true);
        self.registers.reset(true);
        self.registers.reset(false);
        self.registers.release_parent_resets();
        self.registers.disable_interrupt();
        self.registers.disable_dma();
        self.registers.disable_pseudo_rounds();
        self.registers.hold();
        let observation = self.registers.state();
        if observation != IDLE {
            self.poisoned = true;
            return Err(BlockError::InvalidState { observation });
        }
        let mut block = to_words(key.as_bytes());
        let mut words = KeyWords::default();
        for (out, word) in words.iter_mut().zip(&block) {
            *out = *word;
        }
        block.zeroize();
        self.registers.write_key(&words);
        words.zeroize();
        Ok(())
    }
}

impl<R: Registers, C: Clock> Drop for AesBlocks<R, C> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<R: Registers, C: Clock> Aes128Engine for AesBlocks<R, C> {
    type Keyed<'a>
        = Keyed<'a, R, C>
    where
        Self: 'a;

    fn load(&mut self, key: &Aes128Key) -> Result<Keyed<'_, R, C>, BlockError> {
        if let Err(error) = self.initialize(key) {
            self.clear();
            return Err(error);
        }
        Ok(Keyed {
            engine: self,
            encrypting: None,
        })
    }
}

/// The engine with a key loaded, in the direction of its last block.
/// Dropping it erases the key and gates the clock.
pub struct Keyed<'a, R: Registers, C: Clock> {
    engine: &'a mut AesBlocks<R, C>,
    encrypting: Option<bool>,
}

impl<R: Registers, C: Clock> Keyed<'_, R, C> {
    /// Write `block`, start it and wait for it on `STATE` within
    /// [`BLOCK_SETTLE`]; the output replaces `block`. A refusal poisons the
    /// engine.
    fn run(&mut self, block: &mut Block) -> Result<(), BlockError> {
        let engine = &mut *self.engine;
        if engine.poisoned {
            return Err(BlockError::Unavailable);
        }
        let mut words = to_words(block);
        engine.registers.write_block(&words);
        words.zeroize();
        engine.registers.start();
        let registers = &engine.registers;
        let mut observation = WORK;
        let settled = settle(Settle::new(BLOCK_SETTLE, &engine.clock), || {
            observation = registers.state();
            (observation != WORK).then_some(())
        });
        let result = match settled {
            Ok(()) if observation == IDLE => {
                let mut words = engine.registers.read_block();
                from_words(&words, block);
                words.zeroize();
                return Ok(());
            }
            Ok(()) => BlockError::InvalidState { observation },
            Err(_unsettled) => BlockError::Timeout { observation },
        };
        engine.poisoned = true;
        block.zeroize();
        Err(result)
    }
}

impl<R: Registers, C: Clock> BlockEngine for Keyed<'_, R, C> {
    fn encrypt(&mut self, block: &mut Block) -> Result<(), BlockError> {
        if self.encrypting != Some(true) {
            self.engine.registers.encrypt128();
            self.encrypting = Some(true);
        }
        self.run(block)
    }

    fn decrypt(&mut self, block: &mut Block) -> Result<(), BlockError> {
        if self.encrypting != Some(false) {
            self.engine.registers.decrypt128();
            self.encrypting = Some(false);
        }
        self.run(block)
    }
}

impl<R: Registers, C: Clock> Drop for Keyed<'_, R, C> {
    fn drop(&mut self) {
        self.engine.clear();
    }
}

fn to_words(bytes: &[u8; 16]) -> BlockWords {
    let [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] = *bytes;
    [
        u32::from_le_bytes([a, b, c, d]),
        u32::from_le_bytes([e, f, g, h]),
        u32::from_le_bytes([i, j, k, l]),
        u32::from_le_bytes([m, n, o, p]),
    ]
}

fn from_words(words: &BlockWords, bytes: &mut Block) {
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        for (out, byte) in chunk.iter_mut().zip(word.to_le_bytes()) {
            *out = byte;
        }
    }
}

#[cfg(test)]
mod tests;
