//! Exclusive semantic access to the indexed Flash MMU selector.

use esp_hal::peripherals::SPI0;

pub const FLASH_XIP_START: usize = 0x4000_0000;
pub const FLASH_XIP_END: usize = 0x4400_0000;

/// Unique owner of the SPI0 Flash-MMU selector.
pub struct FlashMmu {
    _owner: SPI0<'static>,
}

impl FlashMmu {
    /// Bind the official ESP-HAL singleton to this semantic PAC owner.
    pub const fn new(owner: SPI0<'static>) -> Self {
        Self { _owner: owner }
    }

    /// Translate one currently mapped Flash XIP address to its physical
    /// address.
    ///
    /// The mutable borrow serializes the shared indexed selector. Callers do
    /// not observe its register identity, field placement, or raw image.
    #[allow(
        unsafe_code,
        reason = "the generated full-width MMU index field accepts every u32 value; the owned singleton and mutable borrow serialize the selector"
    )]
    pub fn physical_address(&mut self, virtual_address: usize) -> Option<u32> {
        let relative = virtual_address.checked_sub(FLASH_XIP_START)?;
        if virtual_address >= FLASH_XIP_END {
            return None;
        }

        let registers = SPI0::regs();
        let page_size = match registers.mmu_power_ctrl().read().mmu_page_size().bits() {
            0 => 0x4_0000_usize,
            1 => 0x2_0000,
            2 => 0x1_0000,
            3 => 0x8000,
            _ => return None,
        };
        let entry = u32::try_from(relative / page_size).ok()?;
        let within_page = u32::try_from(relative % page_size).ok()?;

        // SAFETY: the full-width index field accepts every `u32`; the owned
        // SPI0 singleton and `&mut self` serialize this index/content pair.
        registers
            .mmu_item_index()
            .write(|writer| unsafe { writer.mmu_item_index().bits(entry) });
        let physical_page = u32::from(registers.mmu_item_content().read().paddr().bits())
            .checked_mul(u32::try_from(page_size).ok()?)?;
        physical_page.checked_add(within_page)
    }
}
