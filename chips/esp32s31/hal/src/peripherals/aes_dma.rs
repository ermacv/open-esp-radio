//! DMA-AES over AXI-DMA channel 0: the channel the AES owns, its placed
//! descriptors and buffers, and one transfer of whole blocks from the input
//! buffer through the AES into the output buffer.
//!
//! SOURCE: ESP_IDF_4D80C1B2_S31_AES_DMA, ESP_IDF_4D80C1B2_S31_AXI_DMA_LL and
//! ESP_IDF_4D80C1B2_S31_AXI_DMA_DESCRIPTOR in registers/evidence: the TX
//! (OUT) channel reads the input into the AES and the RX (IN) channel writes
//! its output, both connected to the AES trigger, reset, linked and started
//! before the AES's `DMA_ENABLE`, `BLOCK_NUM` and `TRIGGER`; the transfer is
//! complete when `STATE` reads `DONE` and the output descriptor's `OWNER`
//! reads 0. SOURCE: ESP_IDF_4D80C1B2_S31_SOC_CAPS_CACHE: internal SRAM is not
//! reached through the L1 cache, so the placed storage needs no cache
//! maintenance.

use oer_esp32s31_pac::{axi_dma, hp_sys_clkrst, ownership::AxiDmaPeripherals};
use oer_memory::DmaBytes;
use oer_mmio::Bits;

use crate::layouts::{
    AxiDmaDescriptor, AxiDmaDescriptorBuffer, AxiDmaDescriptorControl, AxiDmaDescriptorNext,
    DmaRef, DmaWindowAddress,
};
use crate::memory::dma_address::ProjectionError;

/// AES blocks of the largest transfer: a CCMP-128 CBC-MAC over B0, two AAD
/// blocks and a 1500-byte body.
pub const AES_DMA_BLOCKS: usize = 98;
/// Bytes of the largest transfer.
pub const AES_DMA_BYTES: usize = AES_DMA_BLOCKS * 16;

const _: () = {
    if AES_DMA_BYTES > AxiDmaDescriptorControl::SIZE_MAX as usize {
        panic!("one descriptor carries a whole transfer");
    }
};

/// The AXI-DMA access ranges ESP-IDF presets: internal SRAM's DMA window and
/// the external flash and PSRAM. SOURCE: ESP_IDF_4D80C1B2_S31_AXI_DMA_LL.
const EXTERNAL_START: u32 = 0x4000_0000;
const EXTERNAL_END: u32 = 0x53ff_ffff;

/// AXI-DMA channel 0 and the AXI-PDMA bus clock and reset word: the AES's
/// DMA.
pub struct AesDmaChannel {
    clock: hp_sys_clkrst::AxiDmaClock,
    channel: axi_dma::AxiDmaChannel0,
}

impl AesDmaChannel {
    pub(crate) fn new(parts: AxiDmaPeripherals) -> Self {
        Self {
            clock: parts.axi_dma_clock,
            channel: parts.axi_dma_channel0,
        }
    }

    /// Clock and reset the controller, open its access ranges and connect
    /// both halves of channel 0 to the AES.
    pub(crate) fn prepare(&mut self) {
        self.clock.modify_axi_pdma_ctrl0(|w| {
            w.set_reg_axi_pdma_sys_clk_en(true);
            w.set_reg_axi_pdma_rst_en(true);
        });
        self.clock.modify_axi_pdma_ctrl0(|w| {
            w.set_reg_axi_pdma_rst_en(false);
        });
        self.channel.modify_misc_conf(|w| {
            w.set_clk_en(true);
        });
        let window = crate::layouts::DMA_WINDOW;
        self.channel.modify_intr_mem_start_addr(|w| {
            w.set_access_intr_mem_start_addr(window.start);
        });
        self.channel.modify_intr_mem_end_addr(|w| {
            w.set_access_intr_mem_end_addr(window.end.wrapping_sub(1));
        });
        self.channel.modify_extr_mem_start_addr(|w| {
            w.set_access_extr_mem_start_addr(EXTERNAL_START);
        });
        self.channel.modify_extr_mem_end_addr(|w| {
            w.set_access_extr_mem_end_addr(EXTERNAL_END);
        });
        let aes = Bits::truncate(axi_dma::in_peri_sel_ch0::PeriInSelCh0::Aes.bits());
        self.channel.modify_in_peri_sel_ch0(|w| {
            w.set_peri_in_sel_ch0(aes);
        });
        let aes = Bits::truncate(axi_dma::out_peri_sel_ch0::PeriOutSelCh0::Aes.bits());
        self.channel.modify_out_peri_sel_ch0(|w| {
            w.set_peri_out_sel_ch0(aes);
        });
        self.channel.modify_in_conf0_ch0(|w| {
            w.set_mem_trans_en_ch0(false);
        });
    }

    /// Reset both halves, link them to `input` (OUT) and `output` (IN) and
    /// start them.
    pub(crate) fn start(
        &mut self,
        input: DmaRef<AxiDmaDescriptor>,
        output: DmaRef<AxiDmaDescriptor>,
    ) {
        self.channel.modify_in_conf0_ch0(|w| {
            w.set_in_rst_ch0(true);
        });
        self.channel.modify_in_conf0_ch0(|w| {
            w.set_in_rst_ch0(false);
        });
        self.channel.modify_out_conf0_ch0(|w| {
            w.set_out_rst_ch0(true);
        });
        self.channel.modify_out_conf0_ch0(|w| {
            w.set_out_rst_ch0(false);
        });
        self.channel.modify_out_link2_ch0(|w| {
            w.set_outlink_addr_ch0(input);
        });
        self.channel.modify_in_link2_ch0(|w| {
            w.set_inlink_addr_ch0(output);
        });
        self.channel.trigger_out_link1_ch0_outlink_start_ch0();
        self.channel.trigger_in_link1_ch0_inlink_start_ch0();
    }
}

/// Bytes of the small and middle buffer pairs: one block, and the eight
/// blocks of a CCMP-128 CBC-MAC over a 64-byte body.
pub const AES_DMA_SMALL_BYTES: usize = 16;
pub const AES_DMA_MIDDLE_BYTES: usize = 128;

/// The placed storage of one transfer: the OUT (input) and IN (output)
/// descriptors and three buffer pairs, so a transfer copies about its own
/// length; 16-byte aligned.
#[repr(C, align(16))]
#[derive(bytemuck::Zeroable)]
pub struct AesDmaStorage {
    input_descriptor: AxiDmaDescriptor,
    output_descriptor: AxiDmaDescriptor,
    small: [DmaBytes<AES_DMA_SMALL_BYTES>; 2],
    middle: [DmaBytes<AES_DMA_MIDDLE_BYTES>; 2],
    large: [DmaBytes<AES_DMA_BYTES>; 2],
}

const _: () = {
    if !AES_DMA_MIDDLE_BYTES.is_multiple_of(16) || !AES_DMA_BYTES.is_multiple_of(16) {
        panic!("the buffers keep their successors 16-byte aligned");
    }
};

crate::dma_static! {
    #[allow(unsafe_code, reason = "DMA storage is placed through the HAL macro")]
    static STORAGE: AesDmaStorage = zeroed in ".dma.bss.aes";
}

/// Why the DMA storage is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AesDmaError {
    /// The storage was claimed before.
    Claimed,
    /// A part of the storage has no DMA address.
    Projection(ProjectionError),
}

/// A transfer length that is no whole number of blocks, is empty or exceeds
/// the largest buffers ([`AES_DMA_BYTES`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaLengthError(pub usize);

/// The buffer pair a transfer uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pair {
    Small,
    Middle,
    Large,
}

/// The placed storage with the DMA addresses of its parts, and the length
/// of the input it holds.
pub struct AesDma {
    storage: &'static mut AesDmaStorage,
    input_descriptor: DmaRef<AxiDmaDescriptor>,
    output_descriptor: DmaRef<AxiDmaDescriptor>,
    /// Input and output buffer addresses of the small, middle and large pair.
    buffers: [[DmaWindowAddress; 2]; 3],
    loaded: Option<(Pair, u16)>,
}

impl AesDma {
    /// Claim the storage once.
    ///
    /// # Errors
    /// [`AesDmaError::Claimed`] for a second claim,
    /// [`AesDmaError::Projection`] when a part lies outside the DMA window.
    pub fn claim() -> Result<Self, AesDmaError> {
        let placed = STORAGE.take().ok_or(AesDmaError::Claimed)?;
        let projected = crate::memory::dma_address::PlacementProjection::try_with::<
            crate::ieee80211::dma::space::NativeDma,
            _,
            _,
        >(placed, |storage, projection| {
            let [small_in, small_out] = &storage.small;
            let [middle_in, middle_out] = &storage.middle;
            let [large_in, large_out] = &storage.large;
            Ok((
                projection.link(&storage.input_descriptor)?,
                projection.link(&storage.output_descriptor)?,
                [
                    [
                        projection.address(small_in)?,
                        projection.address(small_out)?,
                    ],
                    [
                        projection.address(middle_in)?,
                        projection.address(middle_out)?,
                    ],
                    [
                        projection.address(large_in)?,
                        projection.address(large_out)?,
                    ],
                ],
            ))
        });
        match projected {
            Ok((storage, (input_descriptor, output_descriptor, buffers))) => Ok(Self {
                storage,
                input_descriptor,
                output_descriptor,
                buffers,
                loaded: None,
            }),
            Err((error, _placed)) => Err(AesDmaError::Projection(error)),
        }
    }

    /// Copy `input` (whole blocks) into the smallest buffer it fits.
    ///
    /// # Errors
    /// [`DmaLengthError`] when `input` is empty, no whole number of blocks
    /// or longer than [`AES_DMA_BYTES`]; nothing is loaded.
    pub fn load(&mut self, input: &[u8]) -> Result<(), DmaLengthError> {
        let bytes = input.len();
        if bytes == 0 || !bytes.is_multiple_of(16) || bytes > AES_DMA_BYTES {
            self.loaded = None;
            return Err(DmaLengthError(bytes));
        }
        let length = u16::try_from(bytes).map_err(|_| DmaLengthError(bytes))?;
        let pair = if bytes <= AES_DMA_SMALL_BYTES {
            let [buffer, _] = &mut self.storage.small;
            buffer.write(input);
            Pair::Small
        } else if bytes <= AES_DMA_MIDDLE_BYTES {
            let [buffer, _] = &mut self.storage.middle;
            buffer.write(input);
            Pair::Middle
        } else {
            let [buffer, _] = &mut self.storage.large;
            buffer.write(input);
            Pair::Large
        };
        self.loaded = Some((pair, length));
        Ok(())
    }

    /// Copy the output of the loaded length's transfer into `out`, as far
    /// as both reach.
    pub fn output(&self, out: &mut [u8]) {
        let Some((pair, _)) = self.loaded else {
            return;
        };
        match pair {
            Pair::Small => {
                let [_, buffer] = &self.storage.small;
                copy(out, &buffer.read());
            }
            Pair::Middle => {
                let [_, buffer] = &self.storage.middle;
                copy(out, &buffer.read());
            }
            Pair::Large => {
                let [_, buffer] = &self.storage.large;
                copy(out, &buffer.read());
            }
        }
    }

    /// The loaded length, once [`Self::load`] accepted an input.
    pub fn loaded_bytes(&self) -> Option<u16> {
        self.loaded.map(|(_, length)| length)
    }

    /// Hand the loaded input and the output buffer of its pair to the DMA:
    /// the input descriptor ends the OUT chain, the output descriptor waits
    /// for the IN data. The ownership words are written last.
    pub(crate) fn arm(&self) -> Option<u16> {
        let (pair, bytes) = self.loaded?;
        let [small, middle, large] = self.buffers;
        let [input_buffer, output_buffer] = match pair {
            Pair::Small => small,
            Pair::Middle => middle,
            Pair::Large => large,
        };
        let input = AxiDmaDescriptorControl::from_bits(0)
            .with_size(bytes)?
            .with_length(bytes)?
            .with_suc_eof(true)
            .with_owner(true);
        let output = AxiDmaDescriptorControl::from_bits(0)
            .with_size(bytes)?
            .with_owner(true);
        for (descriptor, buffer) in [
            (&self.storage.input_descriptor, input_buffer),
            (&self.storage.output_descriptor, output_buffer),
        ] {
            descriptor
                .buffer()
                .write(AxiDmaDescriptorBuffer::from_bits(0).with_address(buffer));
            descriptor
                .next()
                .write(AxiDmaDescriptorNext::from_bits(0).with_address(None));
        }
        self.storage.input_descriptor.control().write(input);
        self.storage.output_descriptor.control().write(output);
        // The descriptor and buffer stores precede the channel's start.
        oer_riscv::fence();
        Some(bytes)
    }

    /// Whether the DMA has handed the output descriptor back.
    pub(crate) fn output_returned(&self) -> bool {
        !self.storage.output_descriptor.control().read().owner()
    }

    pub(crate) fn links(&self) -> (DmaRef<AxiDmaDescriptor>, DmaRef<AxiDmaDescriptor>) {
        (self.input_descriptor, self.output_descriptor)
    }
}

fn copy(out: &mut [u8], bytes: &[u8]) {
    for (to, from) in out.iter_mut().zip(bytes) {
        *to = *from;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_storage_is_claimed_once_and_arms_the_smallest_pair_a_whole_block_input_fits() {
        let mut dma = AesDma::claim().expect("the first claim");
        assert!(matches!(AesDma::claim(), Err(AesDmaError::Claimed)));
        let input: [u8; AES_DMA_BYTES + 16] = core::array::from_fn(|index| index as u8);
        for bytes in [0, 15, 17, AES_DMA_BYTES + 16] {
            assert_eq!(dma.load(&input[..bytes]), Err(DmaLengthError(bytes)));
            assert_eq!(dma.arm(), None, "a refused input arms nothing");
        }
        let [small, middle, large] = dma.buffers;
        for (bytes, pair) in [(16, small), (48, middle), (128, middle), (144, large)] {
            dma.load(&input[..bytes]).expect("whole blocks");
            assert_eq!(dma.arm(), Some(bytes as u16));
            let control = dma.storage.input_descriptor.control().read();
            assert_eq!(
                (
                    control.size(),
                    control.length(),
                    control.suc_eof(),
                    control.owner()
                ),
                (bytes as u16, bytes as u16, true, true)
            );
            let control = dma.storage.output_descriptor.control().read();
            assert_eq!((control.size(), control.owner()), (bytes as u16, true));
            assert!(!dma.output_returned());
            let [input_buffer, output_buffer] = pair;
            assert_eq!(
                dma.storage.input_descriptor.buffer().read().address(),
                input_buffer.get()
            );
            assert_eq!(
                dma.storage.output_descriptor.buffer().read().address(),
                output_buffer.get()
            );
            assert_eq!(dma.storage.input_descriptor.next().read().address(), 0);
        }
        let mut out = [0xff_u8; 4];
        dma.output(&mut out);
        assert_eq!(out, [0; 4], "the output buffer of the loaded pair");
    }
}
