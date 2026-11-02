//! CPU Typical AES atoms. Sequencing, completion waits and error policy are
//! the chip crypto driver's responsibility. No DMA is enabled; the AES IRQ
//! is enabled only by an [`InterruptingAes`] owner, whose interrupt part a
//! handler holds.
//! SOURCE: ESP_IDF_4D80C1B2_S31_AES and TRM_S31_V01_AES in registers/evidence.

use oer_esp32s31_pac::{
    aes, hp_sys_clkrst,
    ownership::{AesPeripherals, CryptoClockPeripherals},
};
use oer_espressif_interrupt_table::Silent;
use oer_mmio::{Index, InterruptSource};

/// The generated Typical/DMA status codes; the driver rejects other values.
pub const IDLE: u32 = aes::state::State::Idle.bits();
pub const WORK: u32 = aes::state::State::Work.bits();

/// One CPU block, with geometry checked against the generated register model.
pub type BlockWords = [u32; aes::text_in::COUNT];
pub type KeyWords = [u32; aes::key::COUNT];

/// Hardware atoms used by the S31 driver and its fake register port.
pub trait Registers {
    fn clock(&mut self, enabled: bool);
    fn reset(&mut self, asserted: bool);
    fn release_parent_resets(&mut self);
    fn disable_interrupt(&mut self);
    fn disable_dma(&mut self);
    fn decrypt128(&mut self);
    fn encrypt128(&mut self);
    fn disable_pseudo_rounds(&mut self);
    fn hold(&mut self);
    fn write_key(&mut self, words: &KeyWords);
    fn write_block(&mut self, words: &BlockWords);
    fn start(&mut self);
    fn state(&self) -> u32;
    fn read_block(&self) -> BlockWords;
}

/// AES and its exclusive crypto clock/reset word, split once from Root.
/// The image's composition places it: the boot hart keeps it in `Hardware`,
/// or the image moves it to a secondary hart through that hart's launch
/// parts, whose router then gives the polling engine its `Silent` proof.
pub struct AesParts {
    program: aes::AesProgram,
    clock: hp_sys_clkrst::CryptoClock,
    interrupt: AesInterrupt,
}

impl AesParts {
    pub(crate) fn new(aes: AesPeripherals, clock: CryptoClockPeripherals) -> Self {
        Self {
            program: aes.aes_program,
            clock: clock.crypto_clock,
            interrupt: AesInterrupt {
                part: aes.aes_interrupt,
            },
        }
    }

    /// The program owner and the interrupt part apart: the image's router
    /// binds the part to a handler, which clears it ([`AesInterrupt::clear`]),
    /// and the owner may enable the interrupt
    /// ([`InterruptingAes::enable_interrupt`]).
    pub fn into_interrupting(self) -> (InterruptingAes, AesInterrupt) {
        (
            InterruptingAes {
                program: self.program,
                clock: self.clock,
            },
            self.interrupt,
        )
    }

    /// Retain the unbound DMA interrupt capability while polling Typical AES.
    pub fn into_polled(self, silent: Silent<AesInterrupt>) -> AesRegisters {
        AesRegisters {
            program: self.program,
            clock: self.clock,
            _interrupt: self.interrupt,
            _silent: silent,
        }
    }
}

/// The AES interrupt (DMA completion, TRM_S31_V01_AES): a polling owner keeps
/// it unbound; an interrupting owner's handler holds it.
pub struct AesInterrupt {
    part: aes::AesInterrupt,
}

impl AesInterrupt {
    /// Acknowledge the interrupt (`INT_CLEAR`).
    pub fn clear(&mut self) {
        self.part.trigger_int_clear_int_clear();
    }
}
impl InterruptSource for AesInterrupt {
    const SOURCE: u16 = <aes::AesInterrupt as InterruptSource>::SOURCE;
}

/// The sole program owner, retaining proof that no CPU line receives AES IRQ.
pub struct AesRegisters {
    program: aes::AesProgram,
    clock: hp_sys_clkrst::CryptoClock,
    _interrupt: AesInterrupt,
    _silent: Silent<AesInterrupt>,
}

/// The sole program owner whose interrupt part a handler holds: it may
/// enable the AES interrupt.
pub struct InterruptingAes {
    program: aes::AesProgram,
    clock: hp_sys_clkrst::CryptoClock,
}

impl InterruptingAes {
    /// Enable the AES interrupt (`INT_ENA`).
    pub fn enable_interrupt(&mut self) {
        self.program.modify_int_ena(|w| {
            w.set_int_ena(true);
        });
    }
}

const WORDS: [Index<{ aes::text_in::COUNT }>; 4] = [
    Index::constant::<0>(),
    Index::constant::<1>(),
    Index::constant::<2>(),
    Index::constant::<3>(),
];
const KEY_WORDS: [Index<{ aes::key::COUNT }>; aes::key::COUNT] = [
    Index::constant::<0>(),
    Index::constant::<1>(),
    Index::constant::<2>(),
    Index::constant::<3>(),
    Index::constant::<4>(),
    Index::constant::<5>(),
    Index::constant::<6>(),
    Index::constant::<7>(),
];
const _: () = if aes::text_in::COUNT != 4 || aes::text_out::COUNT != 4 || aes::key::COUNT != 8 {
    panic!("AES block/key geometry");
};

/// The atoms over an owner's program and clock words, for each owner.
macro_rules! typical_atoms {
    ($owner:ty) => {
        impl Registers for $owner {
            fn clock(&mut self, enabled: bool) {
                self.clock.set_aes_bus_clock(enabled);
            }
            fn reset(&mut self, asserted: bool) {
                self.clock.set_aes_module_reset(asserted);
            }
            /// SOURCE: S31 aes_ll_reset_register: parent crypto and DS resets hold AES.
            fn release_parent_resets(&mut self) {
                self.clock.modify_crypto_ctrl0(|w| {
                    w.set_reg_crypto_rst_en(false);
                    w.set_reg_crypto_ds_rst_en(false);
                });
            }
            fn disable_interrupt(&mut self) {
                self.program.modify_int_ena(|w| {
                    w.set_int_ena(false);
                });
            }
            fn disable_dma(&mut self) {
                self.program.modify_dma_enable(|w| {
                    w.set_dma_enable(false);
                });
            }
            fn decrypt128(&mut self) {
                self.program.modify_mode(|w| {
                    w.set_mode(aes::mode::Mode::Decrypt128);
                    w.set_use_hardware_key(false);
                });
            }
            fn encrypt128(&mut self) {
                self.program.modify_mode(|w| {
                    w.set_mode(aes::mode::Mode::Encrypt128);
                    w.set_use_hardware_key(false);
                });
            }
            fn disable_pseudo_rounds(&mut self) {
                self.program.modify_pseudo(|w| {
                    w.set_pseudo_en(false);
                });
            }
            fn hold(&mut self) {
                self.program.modify_free(|w| {
                    w.set_free(false);
                });
            }
            fn write_key(&mut self, words: &KeyWords) {
                for (index, word) in KEY_WORDS.into_iter().zip(words) {
                    self.program.modify_key(index, |w| {
                        w.set_data(*word);
                    });
                }
            }
            fn write_block(&mut self, words: &BlockWords) {
                for (index, word) in WORDS.into_iter().zip(words) {
                    self.program.modify_text_in(index, |w| {
                        w.set_data(*word);
                    });
                }
            }
            fn start(&mut self) {
                self.program.trigger_trigger_trigger();
            }
            fn state(&self) -> u32 {
                self.program.read_state().state().get()
            }
            fn read_block(&self) -> BlockWords {
                WORDS.map(|index| self.program.read_text_out(index).data())
            }
        }
    };
}

typical_atoms!(AesRegisters);
typical_atoms!(InterruptingAes);

#[cfg(test)]
mod tests {
    /// The image's composition places AES on a hart: the parts move by value
    /// through a secondary hart's launch mailbox, and the polled owner and
    /// its silence proof into that hart's task.
    #[test]
    fn aes_owners_move_to_the_hart_the_composition_places_them_on() {
        fn movable<T: Send>() {}
        movable::<super::AesParts>();
        movable::<super::AesRegisters>();
        movable::<super::InterruptingAes>();
        movable::<super::AesInterrupt>();
    }
}
