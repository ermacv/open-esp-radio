//! CPU Typical AES atoms. Sequencing, completion waits and error policy are
//! the chip crypto driver's responsibility. The polling owner enables no DMA
//! and no AES IRQ; an [`InterruptingAes`] owner, whose interrupt part a
//! handler holds, may enable the IRQ and run DMA-AES transfers over the
//! AES's AXI-DMA channel ([`super::aes_dma`]).
//! SOURCE: ESP_IDF_4D80C1B2_S31_AES and TRM_S31_V01_AES in registers/evidence.

use oer_chips_esp32s31_pac::{
    aes, hp_sys_clkrst,
    ownership::{AesPeripherals, AxiDmaPeripherals, CryptoClockPeripherals},
};
use oer_chips_family_platform_interrupt_table::Silent;
use oer_portable_mmio::InterruptSource;

mod ops;
#[cfg(test)]
mod ops_tests;

use super::aes_dma::{AesDma, AesDmaChannel, DmaLengthError};

/// The block modes of a DMA-AES transfer.
pub use oer_chips_esp32s31_pac::aes::block_mode::BlockMode as DmaBlockMode;

/// The generated Typical/DMA status names; a code without one is no state
/// the driver accepts.
pub use oer_chips_esp32s31_pac::aes::state::State as AesState;
/// The `STATE` field as read: a code of [`AesState`] or an unnamed one.
pub type AesStateField = oer_chips_esp32s31_pac::Bits<2>;

/// One CPU block, with geometry checked against the generated register model.
pub type BlockWords = [u32; aes::text_in::COUNT];
pub type KeyWords = [u32; aes::key::COUNT];

/// Hardware atoms used by the S31 driver and its fake register port.
pub trait Registers {
    fn enable_clock(&mut self);
    fn disable_clock(&mut self);
    fn assert_reset(&mut self);
    fn release_reset(&mut self);
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
    fn state(&self) -> AesStateField;
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
        ops::clear_interrupt(&mut self.part);
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

impl InterruptingAes {
    /// Enable the AES interrupt (`INT_ENA`).
    pub fn enable_interrupt(&mut self) {
        ops::enable_interrupt(&mut self.program);
    }

    /// Clock and reset the AES's AXI-DMA, open its access ranges and connect
    /// channel 0 to the AES.
    pub fn prepare_dma(&mut self) {
        self.dma.prepare();
    }

    /// Write the IV (CBC) or initial counter block (CTR), as little-endian
    /// words of its bytes.
    pub fn write_iv(&mut self, words: &BlockWords) {
        ops::write_iv(&mut self.program, words);
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
        ops::start_dma(&mut self.program, mode, length);
        Ok(())
    }

    /// Whether the started transfer is complete: `STATE` reads `DONE` and
    /// the DMA handed the output descriptor back.
    pub fn dma_done(&self, storage: &AesDma) -> bool {
        ops::is_done(&self.program) && storage.output_returned()
    }

    /// End the complete transfer: `DMA_EXIT`, then `DMA_ENABLE` cleared.
    pub fn finish_dma(&mut self) {
        ops::finish_dma(&mut self.program);
    }
}

/// The atoms over an owner's program and clock words, for each owner.
macro_rules! typical_atoms {
    ($owner:ty) => {
        impl Registers for $owner {
            fn enable_clock(&mut self) {
                ops::enable_clock(&mut self.clock);
            }
            fn disable_clock(&mut self) {
                ops::disable_clock(&mut self.clock);
            }
            fn assert_reset(&mut self) {
                ops::assert_reset(&mut self.clock);
            }
            fn release_reset(&mut self) {
                ops::release_reset(&mut self.clock);
            }
            fn release_parent_resets(&mut self) {
                ops::release_parent_resets(&mut self.clock);
            }
            fn disable_interrupt(&mut self) {
                ops::disable_interrupt(&mut self.program);
            }
            fn disable_dma(&mut self) {
                ops::disable_dma(&mut self.program);
            }
            fn decrypt128(&mut self) {
                ops::decrypt128(&mut self.program);
            }
            fn encrypt128(&mut self) {
                ops::encrypt128(&mut self.program);
            }
            fn disable_pseudo_rounds(&mut self) {
                ops::disable_pseudo_rounds(&mut self.program);
            }
            fn hold(&mut self) {
                ops::hold(&mut self.program);
            }
            fn write_key(&mut self, words: &KeyWords) {
                ops::write_key(&mut self.program, words);
            }
            fn write_block(&mut self, words: &BlockWords) {
                ops::write_block(&mut self.program, words);
            }
            fn start(&mut self) {
                ops::start(&mut self.program);
            }
            fn state(&self) -> AesStateField {
                ops::state(&self.program)
            }
            fn read_block(&self) -> BlockWords {
                ops::read_block(&self.program)
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
