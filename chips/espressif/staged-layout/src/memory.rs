//! The address map of a staged boot.
//!
//! Each chip's layout crate (`chips/<chip>/layout`) generates its [`Layout`]
//! from the chip facts (`oer-espressif-facts`); sizes describe the module's
//! fitted memories and the staged-boot contract, not universal chip
//! capabilities.

/// A contiguous address range whose exclusive end fits in a `u32`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    origin: u32,
    length: u32,
}

impl Region {
    /// Admit a range without address wraparound. Empty ranges are valid.
    /// Fixed maps check admission in a const initializer.
    ///
    /// ```compile_fail,E0080
    /// use oer_espressif_staged_layout::Region;
    /// const INVALID: Region = Region::new(u32::MAX, 1).expect("region fits");
    /// ```
    pub const fn new(origin: u32, length: u32) -> Option<Self> {
        match origin.checked_add(length) {
            Some(_) => Some(Self { origin, length }),
            None => None,
        }
    }

    pub const fn origin(self) -> u32 {
        self.origin
    }

    pub const fn length(self) -> u32 {
        self.length
    }

    /// First address past the region.
    pub const fn end(self) -> u32 {
        self.origin + self.length
    }

    /// The remainder of this region after its first `offset` bytes.
    /// Returns `None` when `offset` lies beyond the region.
    ///
    /// ```compile_fail,E0080
    /// use oer_espressif_staged_layout::Region;
    /// const REGION: Region = Region::new(0x100, 0x100).expect("region fits");
    /// const INVALID: Region = REGION.after(0x101).expect("subregion fits");
    /// ```
    pub const fn after(self, offset: u32) -> Option<Self> {
        if offset > self.length {
            return None;
        }
        Some(Self {
            origin: self.origin + offset,
            length: self.length - offset,
        })
    }

    /// The first `length` bytes of this region.
    /// Returns `None` when `length` exceeds this region's length.
    ///
    /// ```compile_fail,E0080
    /// use oer_espressif_staged_layout::Region;
    /// const REGION: Region = Region::new(0x100, 0x100).expect("region fits");
    /// const INVALID: Region = REGION.first(0x101).expect("subregion fits");
    /// ```
    pub const fn first(self, length: u32) -> Option<Self> {
        if length > self.length {
            return None;
        }
        Some(Self {
            origin: self.origin,
            length,
        })
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
    /// PSRAM page the bootstrap probes before it copies stage two; stage two
    /// starts after it.
    pub bootstrap_psram_bytes: u32,
    /// The MMU page that maps flash and PSRAM: stage two's payload fills at
    /// least one, and the bootstrap's flash text starts on one.
    pub mmu_page_bytes: u32,
    /// Bytes of the ESP image's headers preceding the bootstrap's
    /// flash-mapped text in its first MMU page.
    pub image_segment_header_bytes: u32,
    /// SRAM at the top the runtime's SRAM sections leave free while the boot hart
    /// still runs on the bootstrap's stack.
    pub handoff_sram_margin_bytes: u32,
    /// Memory that no reset entry initializes, which keeps the panic record
    /// across a reset (`.rtc_fast.persistent`).
    pub retained: Region,
    /// Bytes at the end of [`Self::retained`] that ESP-IDF's bootloader and
    /// RTC timer keep.
    pub retained_reserved_tail: u32,
    /// Default task allocations indexed by hart; contained harts have zero bytes.
    pub task_stack_bytes: &'static [u32],
    /// Maximum IRQ allocation per used hart; proven images are sized at link time.
    pub irq_stack_bytes: u32,
    /// The complete hart geometry, derived from the chip's facts.
    pub harts: u32,
    /// The hart that performs primary initialization.
    pub boot_hart: u32,
    /// The Flash span the bootstrap keeps cold for its Flash timing tuning;
    /// zero on a chip whose bootstrap does not tune Flash.
    pub flash_tuning_reference_bytes: u32,
    /// The ROM symbol script of the chip's `linker` directory the images link
    /// with, such as `rom/esp32s31-eco0.x` (`[rom] linker-script`).
    pub rom_script: &'static str,
}

impl Layout {
    /// The bootstrap's Flash-mapped text, after the ESP image segment header.
    pub const fn bootstrap_flash_text(&self) -> Option<Region> {
        self.flash_xip.after(self.image_segment_header_bytes)
    }

    /// Stage two relocated to PSRAM, following the bootstrap's probe page.
    pub const fn runtime_psram(&self) -> Option<Region> {
        self.psram.after(self.bootstrap_psram_bytes)
    }

    /// Panics unless every derived region stays inside its memory, the loader
    /// lies in SRAM, the interrupt stacks lie below every task stack (the
    /// trap entries tell the interrupt stack from a task stack by address)
    /// and the pages are pages. A chip's generated layout calls this in a
    /// `const` item.
    pub const fn check(&self) {
        assert!(self.bootstrap_flash_text().is_some());
        assert!(self.runtime_psram().is_some());
        assert!(self.mmu_page_bytes.is_power_of_two());
        assert!(
            self.bootstrap_psram_bytes
                .is_multiple_of(self.mmu_page_bytes)
        );
        assert!(self.image_segment_header_bytes < self.mmu_page_bytes);
        assert!(self.handoff_sram_margin_bytes < self.sram.length);
        assert!(self.sram.contains_range(
            self.second_stage_loader_start as u64,
            self.sram.end() as u64
        ));
        assert!(self.sram.end() <= self.psram.origin);
        assert!(self.harts > 0 && self.boot_hart < self.harts);
        assert!(self.task_stack_bytes.len() == self.harts as usize);
        assert!(self.task_stack_bytes[self.boot_hart as usize] > 0);
        assert!(self.retained_reserved_tail <= self.retained.length);
        // Per-hart guards belong to chip.toml and are checked by the image
        // linker; this chip-neutral layout only constrains the allocation.
        assert!(self.irq_stack_bytes >= 16 && self.irq_stack_bytes % 16 == 0);
        let mut hart = 0;
        while hart < self.task_stack_bytes.len() {
            assert!(self.task_stack_bytes[hart].is_multiple_of(16));
            hart += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_containment_rejects_inverted_and_overhanging_ranges() {
        const REGION: Region = Region::new(0x100, 0x100).expect("region fits");
        let region = REGION;
        assert!(region.contains_range(0x100, 0x100));
        assert!(region.contains_range(0x100, 0x200));
        assert!(!region.contains_range(0x180, 0x170));
        assert!(!region.contains_range(0x180, 0x201));
        assert!(!region.contains_range(0xff, 0x100));
    }

    #[test]
    fn checked_geometry_preserves_bounds_and_admits_empty_edges() {
        const REGION: Region = Region::new(u32::MAX - 16, 16).expect("region fits");
        const FIRST: Region = REGION.first(8).expect("prefix fits");
        const AFTER: Region = REGION.after(8).expect("suffix fits");
        assert_eq!(
            (REGION.origin(), REGION.length(), REGION.end()),
            (u32::MAX - 16, 16, u32::MAX)
        );
        assert_eq!(FIRST.end(), AFTER.origin());
        assert!(REGION.contains(FIRST) && REGION.contains(AFTER));
        assert_eq!(REGION.first(17), None);
        assert_eq!(REGION.after(17), None);
        assert_eq!(Region::new(u32::MAX, 1), None);
        assert_eq!(Region::new(1, u32::MAX), None);
        assert_eq!(
            REGION.first(0).expect("empty prefix").end(),
            REGION.origin()
        );
        assert_eq!(REGION.after(16), Region::new(u32::MAX, 0));
    }
}
