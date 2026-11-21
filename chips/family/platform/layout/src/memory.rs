//! The address map of a staged boot.
//!
//! Each chip's layout crate (`chips/<chip>/platform/layout`) generates its [`Layout`]
//! from the chip facts (`oer-chips-family-facts`); sizes describe the module's
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
    /// use oer_chips_family_platform_layout::Region;
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
    /// use oer_chips_family_platform_layout::Region;
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
    /// use oer_chips_family_platform_layout::Region;
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
    /// The internal SRAM stage two owns: all of it, the ROM's reserves
    /// included, as stage two calls no ROM function.
    pub sram: Region,
    /// The internal SRAM the bootstrap links in (ESP-IDF's
    /// `SRAM_START..SRAM_SEG_END`), below what the ROM and the boot chain keep
    /// until stage two starts; the bootstrap's stack ends at its top.
    pub bootstrap_sram: Region,
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
    /// Memory that no reset entry initializes, which keeps the panic record
    /// across a reset (`.rtc_fast.persistent`).
    pub retained: Region,
    /// Bytes at the end of [`Self::retained`] that ESP-IDF's bootloader and
    /// RTC timer keep.
    pub retained_reserved_tail: u32,
    /// Default task allocations indexed by hart; contained harts have zero bytes.
    pub task_stack_bytes: &'static [u32],
    /// What the first, analysis-only link places for every used interrupt
    /// stack: a placeholder of the stack policy, no limit. The image linker
    /// sizes each stack in the second link from its bound or the image's
    /// declared size.
    pub analysis_irq_stack_bytes: u32,
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

    /// The first part of the layout's contract it breaks, or none: every
    /// derived region stays inside its memory, the loader lies in SRAM, the
    /// interrupt stacks lie below every task stack (the trap entries tell the
    /// interrupt stack from a task stack by address) and the pages are pages.
    /// A chip's generated layout refuses to build on a fault, in a `const`
    /// item.
    pub const fn fault(&self) -> Option<LayoutFault> {
        let fault = if self.bootstrap_flash_text().is_none() {
            LayoutFault::BootstrapFlashText
        } else if self.runtime_psram().is_none() {
            LayoutFault::RuntimePsram
        } else if !self.mmu_page_bytes.is_power_of_two() {
            LayoutFault::PageSize
        } else if !self
            .bootstrap_psram_bytes
            .is_multiple_of(self.mmu_page_bytes)
        {
            LayoutFault::BootstrapPsramPages
        } else if self.image_segment_header_bytes >= self.mmu_page_bytes {
            LayoutFault::SegmentHeader
        } else if !self.sram.contains(self.bootstrap_sram) {
            LayoutFault::BootstrapSram
        } else if !self.bootstrap_sram.contains_range(
            self.second_stage_loader_start as u64,
            self.bootstrap_sram.end() as u64,
        ) {
            LayoutFault::Loader
        } else if self.sram.end() > self.psram.origin {
            LayoutFault::SramAbovePsram
        } else if self.harts == 0 || self.boot_hart >= self.harts {
            LayoutFault::Harts
        } else if self.task_stack_bytes.len() != self.harts as usize {
            LayoutFault::TaskStackCount
        } else if self.task_stack_bytes[self.boot_hart as usize] == 0 {
            LayoutFault::BootTaskStack
        } else if self.retained_reserved_tail > self.retained.length {
            LayoutFault::RetainedTail
        // Per-hart guards belong to chip.toml and are checked by the image
        // linker; this chip-neutral layout only constrains the placeholder.
        } else if self.analysis_irq_stack_bytes < 16
            || !self.analysis_irq_stack_bytes.is_multiple_of(16)
        {
            LayoutFault::AnalysisIrqStack
        } else {
            let mut hart = 0;
            while hart < self.task_stack_bytes.len() {
                if !self.task_stack_bytes[hart].is_multiple_of(16) {
                    return Some(LayoutFault::TaskStackAlignment);
                }
                hart += 1;
            }
            return None;
        };
        Some(fault)
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

/// A part of the layout's contract a [`Layout`] breaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutFault {
    /// The bootstrap's flash text does not fit its flash.
    BootstrapFlashText,
    /// Stage two does not fit the PSRAM after the bootstrap's probe page.
    RuntimePsram,
    /// The MMU page is not a power of two.
    PageSize,
    /// The bootstrap's PSRAM is not a whole number of pages.
    BootstrapPsramPages,
    /// An image segment's header does not fit a page.
    SegmentHeader,
    /// The bootstrap's SRAM is outside the SRAM.
    BootstrapSram,
    /// The second-stage loader is outside the bootstrap's SRAM.
    Loader,
    /// The SRAM ends above the PSRAM's start.
    SramAbovePsram,
    /// No harts, or the boot hart is not one of them.
    Harts,
    /// Not one task stack a hart.
    TaskStackCount,
    /// The boot hart has no task stack.
    BootTaskStack,
    /// The retained memory's reserved tail is longer than the memory.
    RetainedTail,
    /// The analysis placeholder of an interrupt stack is not 16-byte sized.
    AnalysisIrqStack,
    /// A task stack is not a multiple of 16 bytes.
    TaskStackAlignment,
}

impl LayoutFault {
    /// What the fault breaks, for the generated layout's refusal.
    pub const fn message(self) -> &'static str {
        match self {
            Self::BootstrapFlashText => "the bootstrap's flash text does not fit its flash",
            Self::RuntimePsram => "stage two does not fit the PSRAM",
            Self::PageSize => "the MMU page is not a power of two",
            Self::BootstrapPsramPages => "the bootstrap's PSRAM is not whole pages",
            Self::SegmentHeader => "an image segment's header does not fit a page",
            Self::BootstrapSram => "the bootstrap's SRAM is outside the SRAM",
            Self::Loader => "the second-stage loader is outside the bootstrap's SRAM",
            Self::SramAbovePsram => "the SRAM ends above the PSRAM",
            Self::Harts => "the boot hart is not one of the harts",
            Self::TaskStackCount => "not one task stack a hart",
            Self::BootTaskStack => "the boot hart has no task stack",
            Self::RetainedTail => "the retained memory's reserved tail is too long",
            Self::AnalysisIrqStack => "the interrupt-stack placeholder is not 16-byte sized",
            Self::TaskStackAlignment => "a task stack is not a multiple of 16 bytes",
        }
    }
}
