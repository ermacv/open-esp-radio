//! Where the runtime linker script placed the image's sections and stacks,
//! as an image checks them at boot.

use core::ops::Range;
use core::ptr::addr_of;

#[allow(unsafe_code, reason = "the linker script symbols")]
unsafe extern "C" {
    static __runtime_image_start: u8;
    static __runtime_payload_end: u8;
    static __runtime_bss_start: u8;
    static __runtime_bss_end: u8;
    static __runtime_data_load_start: u8;
    static __runtime_data_start: u8;
    static __runtime_data_end: u8;
    static __runtime_data_bss_start: u8;
    static __runtime_data_bss_end: u8;
    static __runtime_isr_start: u8;
    static __runtime_isr_end: u8;
    static __runtime_dma_data_start: u8;
    static __runtime_dma_data_end: u8;
    static __runtime_dma_bss_start: u8;
    static __runtime_dma_bss_end: u8;
    static __runtime_cpu0_irq_stack_bottom: u8;
    static __runtime_cpu0_irq_stack_top: u8;
    static __runtime_cpu1_irq_stack_bottom: u8;
    static __runtime_cpu1_irq_stack_top: u8;
    static _stack_end: u8;
    static _stack_start: u8;
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
    pub irq_stacks: [Range<usize>; 2],
    /// The CPU0 task stack, from its bottom to its top.
    pub task_stack: Range<usize>,
}

fn range(start: *const u8, end: *const u8) -> Range<usize> {
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
        irq_stacks: [
            range(
                addr_of!(__runtime_cpu0_irq_stack_bottom),
                addr_of!(__runtime_cpu0_irq_stack_top),
            ),
            range(
                addr_of!(__runtime_cpu1_irq_stack_bottom),
                addr_of!(__runtime_cpu1_irq_stack_top),
            ),
        ],
        task_stack: range(addr_of!(_stack_end), addr_of!(_stack_start)),
    }
}
