//! Where the runtime linker script placed the image's sections and stacks,
//! as an image checks them at boot.

use core::ops::Range;
use core::ptr::addr_of;

include!(concat!(env!("OUT_DIR"), "/stack-symbols.rs"));
include!(concat!(env!("OUT_DIR"), "/vector-markers.rs"));

#[allow(unsafe_code, reason = "the linker script symbols")]
// SAFETY: the runtime linker script and generated vectors define these address
// markers. Zero-size byte arrays and repr(C) VectorEntry markers read no code,
// stack memory or memory at an absolute symbol's value. The one sized item,
// `__runtime_payload_crc32`, is the stage-two header's 4-byte-aligned CRC word
// in the loaded image (`.runtime.header`), readable for the whole runtime;
// every bit pattern is a valid `u32`.
unsafe extern "C" {
    safe static __runtime_image_start: [u8; 0];
    safe static __runtime_payload_end: [u8; 0];
    safe static __runtime_bss_start: [u8; 0];
    safe static __runtime_bss_end: [u8; 0];
    safe static __runtime_data_load_start: [u8; 0];
    safe static __runtime_data_start: [u8; 0];
    safe static __runtime_data_end: [u8; 0];
    safe static __runtime_data_bss_start: [u8; 0];
    safe static __runtime_data_bss_end: [u8; 0];
    safe static __runtime_isr_start: [u8; 0];
    safe static __runtime_isr_end: [u8; 0];
    safe static __runtime_dma_data_start: [u8; 0];
    safe static __runtime_dma_data_end: [u8; 0];
    safe static __runtime_dma_bss_start: [u8; 0];
    safe static __runtime_dma_bss_end: [u8; 0];
    stack_markers!();
    vector_markers!();
    pub(crate) safe static _runtime_mtvt: [u8; 0];
    pub(crate) safe static _start_trap: [u8; 0];
    safe static __runtime_payload_crc32: u32;
}

/// The image's identity: the stage-two payload CRC-32 the image packer wrote
/// into the header and the bootstrap checked before it started the runtime
/// (`oer_espressif_staged_layout::stage_two`). Two builds of different
/// payloads differ in it but by chance.
pub fn payload_crc32() -> u32 {
    // The packer writes the word after the link: never the linked zero.
    oer_memory::read_volatile(&__runtime_payload_crc32)
}

/// The linked image's sections and stacks, as address ranges.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sections {
    /// The loaded image, from its first byte to the end of its payload.
    pub image: Range<usize>,
    /// Where the initialized data's image is loaded.
    pub data_load_start: usize,
    pub data: Range<usize>,
    pub bss: Range<usize>,
    /// The PSRAM BSS the bootstrap clears.
    pub data_bss: Range<usize>,
    /// The SRAM interrupt text and data.
    pub isr: Range<usize>,
    pub dma_data: Range<usize>,
    pub dma_bss: Range<usize>,
    /// Each hart's SRAM interrupt stack.
    pub irq_stacks: [Range<usize>; oer_espressif_platform_port::HARTS],
    /// Each hart's PSRAM task stack; an unused range is empty.
    pub task_stacks: [Range<usize>; oer_espressif_platform_port::HARTS],
}

fn range(start: *const [u8; 0], end: *const [u8; 0]) -> Range<usize> {
    start as usize..end as usize
}

/// The linked image's sections and stacks.
pub fn sections() -> Sections {
    Sections {
        image: range(
            addr_of!(__runtime_image_start),
            addr_of!(__runtime_payload_end),
        ),
        data_load_start: addr_of!(__runtime_data_load_start) as usize,
        data: range(addr_of!(__runtime_data_start), addr_of!(__runtime_data_end)),
        bss: range(addr_of!(__runtime_bss_start), addr_of!(__runtime_bss_end)),
        data_bss: range(
            addr_of!(__runtime_data_bss_start),
            addr_of!(__runtime_data_bss_end),
        ),
        isr: range(addr_of!(__runtime_isr_start), addr_of!(__runtime_isr_end)),
        dma_data: range(
            addr_of!(__runtime_dma_data_start),
            addr_of!(__runtime_dma_data_end),
        ),
        dma_bss: range(
            addr_of!(__runtime_dma_bss_start),
            addr_of!(__runtime_dma_bss_end),
        ),
        irq_stacks: stack_symbols().map(|stack| stack.irq),
        task_stacks: stack_symbols().map(|stack| stack.task),
    }
}

/// Linker markers indexed by the chip's complete hart geometry.
pub(crate) struct HartStacks {
    pub irq: Range<usize>,
    pub task: Range<usize>,
    pub guard: usize,
}

pub(crate) fn stack_symbols() -> [HartStacks; oer_espressif_platform_port::HARTS] {
    stack_addresses!()
}
