//! Register-local AES operations over the generated parts, for any
//! placement: the owners call them, and host tests run them on memory.
#![forbid(unsafe_code)]

use oer_chips_esp32s31_pac::Bits;
use oer_chips_esp32s31_pac::{aes, hp_sys_clkrst};
use oer_portable_bounded::Index;
use oer_portable_mmio::Placement;

use super::{BlockWords, DmaBlockMode, KeyWords};

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
const IV_WORDS: [Index<{ aes::iv::COUNT }>; 4] = [
    Index::constant::<0>(),
    Index::constant::<1>(),
    Index::constant::<2>(),
    Index::constant::<3>(),
];
const _: () = if aes::text_in::COUNT != 4 || aes::text_out::COUNT != 4 || aes::key::COUNT != 8 {
    panic!("AES block/key geometry");
};

pub(super) fn enable_clock<P: Placement>(clock: &mut hp_sys_clkrst::CryptoClock<P>) {
    clock.set_aes_bus_clock(true);
}
pub(super) fn disable_clock<P: Placement>(clock: &mut hp_sys_clkrst::CryptoClock<P>) {
    clock.set_aes_bus_clock(false);
}
pub(super) fn assert_reset<P: Placement>(clock: &mut hp_sys_clkrst::CryptoClock<P>) {
    clock.set_aes_module_reset(true);
}
pub(super) fn release_reset<P: Placement>(clock: &mut hp_sys_clkrst::CryptoClock<P>) {
    clock.set_aes_module_reset(false);
}
/// SOURCE: S31 aes_ll_reset_register: parent crypto and DS resets hold AES.
pub(super) fn release_parent_resets<P: Placement>(clock: &mut hp_sys_clkrst::CryptoClock<P>) {
    clock.modify_crypto_ctrl0(|w| {
        w.set_reg_crypto_rst_en(false);
        w.set_reg_crypto_ds_rst_en(false);
    });
}
pub(super) fn disable_interrupt<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_int_ena(|w| {
        w.set_int_ena(false);
    });
}
pub(super) fn enable_interrupt<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_int_ena(|w| {
        w.set_int_ena(true);
    });
}
pub(super) fn disable_dma<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_dma_enable(|w| {
        w.set_dma_enable(false);
    });
}
pub(super) fn decrypt128<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_mode(|w| {
        w.set_mode(aes::mode::Mode::Decrypt128);
        w.set_use_hardware_key(false);
    });
}
pub(super) fn encrypt128<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_mode(|w| {
        w.set_mode(aes::mode::Mode::Encrypt128);
        w.set_use_hardware_key(false);
    });
}
pub(super) fn disable_pseudo_rounds<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_pseudo(|w| {
        w.set_pseudo_en(false);
    });
}
pub(super) fn hold<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.modify_free(|w| {
        w.set_free(false);
    });
}
pub(super) fn write_key<P: Placement>(program: &mut aes::AesProgram<P>, words: &KeyWords) {
    for (index, word) in KEY_WORDS.into_iter().zip(words) {
        program.modify_key(index, |w| {
            w.set_data(*word);
        });
    }
}
pub(super) fn write_block<P: Placement>(program: &mut aes::AesProgram<P>, words: &BlockWords) {
    for (index, word) in WORDS.into_iter().zip(words) {
        program.modify_text_in(index, |w| {
            w.set_data(*word);
        });
    }
}
pub(super) fn start<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.trigger_trigger_trigger();
}
pub(super) fn state<P: Placement>(program: &aes::AesProgram<P>) -> Bits<2> {
    program.read_state().state()
}
pub(super) fn is_done<P: Placement>(program: &aes::AesProgram<P>) -> bool {
    matches!(
        program.read_state().state_variant(),
        Some(aes::state::State::Done)
    )
}
pub(super) fn read_block<P: Placement>(program: &aes::AesProgram<P>) -> BlockWords {
    WORDS.map(|index| program.read_text_out(index).data())
}
/// Write the IV (CBC) or initial counter block (CTR), as little-endian
/// words of its bytes.
pub(super) fn write_iv<P: Placement>(program: &mut aes::AesProgram<P>, words: &BlockWords) {
    for (index, word) in IV_WORDS.into_iter().zip(words) {
        program.modify_iv(index, |w| {
            w.set_data(*word);
        });
    }
}
/// The AES half of a DMA transfer's start, after the channel's.
pub(super) fn start_dma<P: Placement>(
    program: &mut aes::AesProgram<P>,
    mode: DmaBlockMode,
    length: u16,
) {
    program.modify_block_mode(|w| {
        w.set_block_mode(mode.into());
    });
    // SOURCE: ESP_IDF_4D80C1B2_S31_AES_DMA: INC_SEL 0 increments the
    // counter block's last 32 bits.
    program.modify_inc_sel(|w| {
        w.set_inc_sel(false);
    });
    program.modify_dma_enable(|w| {
        w.set_dma_enable(true);
    });
    program.modify_block_num(|w| {
        w.set_block_num(u32::from(length / 16));
    });
    program.trigger_trigger_trigger();
}
/// End the complete transfer: `DMA_EXIT`, then `DMA_ENABLE` cleared.
pub(super) fn finish_dma<P: Placement>(program: &mut aes::AesProgram<P>) {
    program.trigger_dma_exit_dma_exit();
    program.modify_dma_enable(|w| {
        w.set_dma_enable(false);
    });
}
pub(super) fn clear_interrupt<P: Placement>(part: &mut aes::AesInterrupt<P>) {
    part.trigger_int_clear_int_clear();
}
