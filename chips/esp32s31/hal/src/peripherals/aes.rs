//! CPU Typical AES atoms. Sequencing, completion waits and error policy are
//! the chip crypto driver's responsibility. The polling owner enables no DMA
//! and no AES IRQ; an [`InterruptingAes`] owner, whose interrupt part a
//! handler holds, may enable the IRQ and run DMA-AES transfers over the
//! AES's AXI-DMA channel ([`super::aes_dma`]).
//! SOURCE: ESP_IDF_4D80C1B2_S31_AES and TRM_S31_V01_AES in registers/evidence.

use oer_esp32s31_pac::{
    aes, hp_sys_clkrst,
    ownership::{AesPeripherals, AxiDmaPeripherals, CryptoClockPeripherals},
};
use oer_espressif_interrupt_table::Silent;
use oer_mmio::{Bits, Index, InterruptSource};

use super::aes_dma::{AesDma, AesDmaChannel, DmaLengthError};

/// The block modes of a DMA-AES transfer.
pub use aes::block_mode::BlockMode as DmaBlockMode;

/// The generated Typical/DMA status codes; the driver rejects other values.
pub const IDLE: u32 = aes::state::State::Idle.bits();
pub const WORK: u32 = aes::state::State::Work.bits();
pub const DONE: u32 = aes::state::State::Done.bits();

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
    dma: AesDmaChannel,
}

impl AesParts {
    pub(crate) fn new(
        aes: AesPeripherals,
        clock: CryptoClockPeripherals,
        dma: AxiDmaPeripherals,
    ) -> Self {
        Self {
            program: aes.aes_program,
            clock: clock.crypto_clock,
            interrupt: AesInterrupt {
                part: aes.aes_interrupt,
            },
            dma: AesDmaChannel::new(dma),
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
                dma: self.dma,
            },
            self.interrupt,
        )
    }

    /// Retain the unbound DMA interrupt capability and the unused DMA
    /// channel while polling Typical AES.
    pub fn into_polled(self, silent: Silent<AesInterrupt>) -> AesRegisters {
        AesRegisters {
            program: self.program,
            clock: self.clock,
            _interrupt: self.interrupt,
            _silent: silent,
            _dma: self.dma,
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
    _dma: AesDmaChannel,
}

/// The sole program owner whose interrupt part a handler holds: it may
/// enable the AES interrupt.
pub struct InterruptingAes {
    program: aes::AesProgram,
    clock: hp_sys_clkrst::CryptoClock,
    dma: AesDmaChannel,
}

const IV_WORDS: [Index<{ aes::iv::COUNT }>; 4] = [
    Index::constant::<0>(),
    Index::constant::<1>(),
    Index::constant::<2>(),
    Index::constant::<3>(),
];

impl InterruptingAes {
    /// Enable the AES interrupt (`INT_ENA`).
    pub fn enable_interrupt(&mut self) {
        self.program.modify_int_ena(|w| {
            w.set_int_ena(true);
        });
    }

    /// Clock and reset the AES's AXI-DMA, open its access ranges and connect
    /// channel 0 to the AES.
    pub fn prepare_dma(&mut self) {
        self.dma.prepare();
    }

    /// Write the IV (CBC) or initial counter block (CTR), as little-endian
    /// words of its bytes.
    pub fn write_iv(&mut self, words: &BlockWords) {
        for (index, word) in IV_WORDS.into_iter().zip(words) {
            self.program.modify_iv(index, |w| {
                w.set_data(*word);
            });
        }
    }

    /// Start a transfer of the input `storage` holds through the AES in
    /// `mode` (CTR increments the counter's last 32 bits) into its output
    /// buffer, under the loaded key and encryption mode.
    ///
    /// # Errors
    /// [`DmaLengthError`] when `storage` holds no input; nothing is started.
    pub fn start_dma(
        &mut self,
        storage: &AesDma,
        mode: DmaBlockMode,
    ) -> Result<(), DmaLengthError> {
        let length = storage.arm().ok_or(DmaLengthError(0))?;
        let (input, output) = storage.links();
        self.dma.start(input, output);
        self.program.modify_block_mode(|w| {
            w.set_block_mode(Bits::truncate(mode.bits()));
        });
        // SOURCE: ESP_IDF_4D80C1B2_S31_AES_DMA: INC_SEL 0 increments the
        // counter block's last 32 bits.
        self.program.modify_inc_sel(|w| {
            w.set_inc_sel(false);
        });
        self.program.modify_dma_enable(|w| {
            w.set_dma_enable(true);
        });
        self.program.modify_block_num(|w| {
            w.set_block_num(u32::from(length / 16));
        });
        self.program.trigger_trigger_trigger();
        Ok(())
    }

    /// Whether the started transfer is complete: `STATE` reads `DONE` and
    /// the DMA handed the output descriptor back.
    pub fn dma_done(&self, storage: &AesDma) -> bool {
        self.program.read_state().state().get() == DONE && storage.output_returned()
    }

    /// End the complete transfer: `DMA_EXIT`, then `DMA_ENABLE` cleared.
    pub fn finish_dma(&mut self) {
        self.program.trigger_dma_exit_dma_exit();
        self.program.modify_dma_enable(|w| {
            w.set_dma_enable(false);
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
