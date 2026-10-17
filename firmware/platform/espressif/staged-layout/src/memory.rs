//! The address map of a staged boot.
//!
//! A chip's platform states its [`Layout`] once; sizes describe the board's
//! fitted memories and the staged-boot contract, not universal chip
//! capabilities.

/// A contiguous address range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub origin: u32,
    pub length: u32,
}

impl Region {
    pub const fn new(origin: u32, length: u32) -> Self {
        Self { origin, length }
    }

    /// First address past the region.
    pub const fn end(self) -> u32 {
        self.origin + self.length
    }

    /// The remainder of this region after its first `offset` bytes.
    pub const fn after(self, offset: u32) -> Self {
        assert!(offset <= self.length);
        Self::new(self.origin + offset, self.length - offset)
    }

    /// The first `length` bytes of this region.
    pub const fn first(self, length: u32) -> Self {
        assert!(length <= self.length);
        Self::new(self.origin, length)
    }

    /// Whether `start..end` is a (possibly empty) range inside this region.
    pub const fn contains_range(self, start: u64, end: u64) -> bool {
        start >= self.origin as u64 && end >= start && end <= self.end() as u64
    }

    /// Whether `inner` lies inside this region.
    pub const fn contains(self, inner: Region) -> bool {
        self.contains_range(inner.origin as u64, inner.end() as u64)
    }
}

/// Bytes of the ESP image segment header preceding the bootstrap's
/// Flash-mapped text in its first MMU page.
pub const ESP_IMAGE_SEGMENT_HEADER_BYTES: u32 = 0x20;

/// PSRAM page the bootstrap probes before it copies stage two; stage two
/// starts on the next 64-KiB MMU page.
pub const BOOTSTRAP_PSRAM_BYTES: u32 = 0x1_0000;

/// One chip's staged-boot address map and stack sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    /// Application-owned internal SRAM (ESP-IDF's `SRAM_START..SRAM_SEG_END`).
    pub sram: Region,
    /// Start of the second-stage loader's `iram_loader_seg`. The loader runs
    /// from here while it loads the bootstrap image, so every segment of that
    /// image it loads must end below this address; memory it does not load
    /// (BSS, the stack and everything stage two copies) may overlap it.
    pub second_stage_loader_start: u32,
    /// The Flash XIP window the bootstrap's text and read-only data are
    /// mapped in.
    pub flash_xip: Region,
    /// The cached external RAM aperture of the fitted PSRAM, where the
    /// bootstrap maps it.
    pub psram: Region,
    /// Memory that no reset entry initializes, which keeps the panic record
    /// across a reset (`.rtc_fast.persistent`).
    pub retained: Region,
    /// Bytes at the end of [`Self::retained`] that ESP-IDF's bootloader and
    /// RTC timer keep.
    pub retained_reserved_tail: u32,
    /// Stage two's CPU0 task stack in PSRAM.
    pub cpu0_task_stack_bytes: u32,
    /// Each hart's dedicated SRAM interrupt stack; task stacks live in PSRAM.
    pub irq_stack_bytes: u32,
    /// The harts stage two runs on: 1 or 2.
    pub harts: u32,
    /// The Flash span the bootstrap keeps cold for its Flash timing tuning;
    /// zero on a chip whose bootstrap does not tune Flash.
    pub flash_tuning_reference_bytes: u32,
    /// The ROM symbol script of the chip's `linker` directory the images link
    /// with, such as `rom/esp32s31-eco0.x`.
    pub rom_script: &'static str,
}

impl Layout {
    /// The bootstrap's Flash-mapped text, after the ESP image segment header.
    pub const fn bootstrap_flash_text(&self) -> Region {
        self.flash_xip.after(ESP_IMAGE_SEGMENT_HEADER_BYTES)
    }

    /// Stage two relocated to PSRAM, following the bootstrap's probe page.
    pub const fn runtime_psram(&self) -> Region {
        self.psram.after(BOOTSTRAP_PSRAM_BYTES)
    }

    /// Panics unless every derived region stays inside its memory, the loader
    /// lies in SRAM and the interrupt stacks lie below every task stack (the
    /// trap entries tell the interrupt stack from a task stack by address).
    /// A chip's layout calls this in a `const` item.
    pub const fn check(&self) {
        assert!(self.flash_xip.contains(self.bootstrap_flash_text()));
        assert!(self.psram.contains(self.runtime_psram()));
        assert!(self.runtime_psram().origin >= self.psram.origin + BOOTSTRAP_PSRAM_BYTES);
        assert!(self.sram.contains_range(
            self.second_stage_loader_start as u64,
            self.sram.end() as u64
        ));
        assert!(self.sram.end() <= self.psram.origin);
        assert!(self.harts == 1 || self.harts == 2);
        assert!(self.retained_reserved_tail <= self.retained.length);
        assert!(self.irq_stack_bytes > crate::interrupts::IRQ_STACK_GUARD_BYTES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_containment_rejects_inverted_and_overhanging_ranges() {
        let region = Region::new(0x100, 0x100);
        assert!(region.contains_range(0x100, 0x100));
        assert!(region.contains_range(0x100, 0x200));
        assert!(!region.contains_range(0x180, 0x170));
        assert!(!region.contains_range(0x180, 0x201));
        assert!(!region.contains_range(0xff, 0x100));
    }
}
