//! The external-memory MMU of the boot: one entry per page of a virtual
//! window, each naming a physical page of flash or PSRAM.
//!
//! An entry is read and written through an index selector and a content
//! word, so even a read changes the selector: every operation takes the MMU
//! by `&mut`, and an index and its content are one call of the chip's
//! registers, never interleaved with another entry's. A read checks the
//! entry's valid flag and its target; an invalid entry is never taken for
//! page 0. A mapping refuses a misaligned or out-of-window range and an
//! entry that already maps something else, and reads every entry it wrote
//! back.
//!
//! SOURCE: ESP-IDF 4d80c1b2
//! `components/esp_hal_cache/esp32s31/include/hal/mmu_ll.h:309-366`
//! (index then content, for a read and a write; the valid and target
//! flags), `components/soc/esp32s31/include/soc/ext_mem_defs.h:57-94`
//! (flags, 1024 entries, the window bases);
//! `components/esp_hal_cache/esp32c5/include/hal/mmu_ll.h:214-268` (C5: one
//! table for flash and PSRAM).

use core::num::NonZeroU32;

use crate::{Access, BootFailure};

/// What an MMU entry maps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmuTarget {
    Flash,
    Psram,
}

/// A valid MMU entry: a physical page of a target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmuEntry {
    pub target: MmuTarget,
    /// The physical page number, in pages of the window's size.
    pub page: u32,
}

/// One MMU table's index selector and content word.
pub trait MmuRegisters {
    /// Select `index` and read its entry: `None` when it is not valid.
    fn read_entry(&mut self, access: Access<'_>, index: u16) -> Option<MmuEntry>;
    /// Select `index` and write `entry`, valid.
    fn write_entry(&mut self, access: Access<'_>, index: u16, entry: MmuEntry);
}

/// One virtual window an MMU table serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmuWindow {
    /// The window's first virtual address, page aligned.
    pub virtual_base: u32,
    /// The page, a power of two.
    pub page_bytes: u32,
    /// The table's first entry for this window.
    pub first_entry: u16,
    /// The window's entries.
    pub entries: u16,
    /// The physical pages the entry's page field holds.
    pub physical_pages: u32,
}

impl MmuWindow {
    /// The entry of `address` and its offset in the page, or `None` outside
    /// the window.
    pub const fn locate(self, address: u32) -> Option<(u16, u32)> {
        if !self.page_bytes.is_power_of_two() {
            return None;
        }
        let Some(offset) = address.checked_sub(self.virtual_base) else {
            return None;
        };
        let Some(page) = offset.checked_div(self.page_bytes) else {
            return None;
        };
        if page >= self.entries as u32 {
            return None;
        }
        let Some(index) = self.first_entry.checked_add(page as u16) else {
            return None;
        };
        let Some(in_page) = offset.checked_rem(self.page_bytes) else {
            return None;
        };
        Some((index, in_page))
    }
}

/// Why an MMU entry or range was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmuFailure {
    /// The address lies outside the window.
    OutsideWindow,
    /// The entry is not valid.
    Unmapped,
    /// The entry maps the other target.
    WrongTarget,
    /// The range or its physical start is not page aligned, or is empty.
    Misaligned,
    /// The physical range exceeds the pages an entry holds.
    PhysicalRange,
    /// An entry of the range already maps another page.
    Occupied,
    /// An entry read back otherwise than written.
    Readback,
}

impl MmuFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::OutsideWindow => "mmu-outside-window",
            Self::Unmapped => "mmu-unmapped",
            Self::WrongTarget => "mmu-wrong-target",
            Self::Misaligned => "mmu-misaligned",
            Self::PhysicalRange => "mmu-physical-range",
            Self::Occupied => "mmu-occupied",
            Self::Readback => "mmu-readback",
        }
    }
}

/// The physical address `address` maps to in `target`.
pub fn physical_address<R: MmuRegisters>(
    registers: &mut R,
    window: MmuWindow,
    target: MmuTarget,
    address: u32,
) -> Result<u32, BootFailure> {
    let (index, offset) = window.locate(address).ok_or(MmuFailure::OutsideWindow)?;
    let entry = registers
        .read_entry(Access::new(), index)
        .ok_or(MmuFailure::Unmapped)?;
    if entry.target != target {
        return Err(MmuFailure::WrongTarget.into());
    }
    entry
        .page
        .checked_mul(window.page_bytes)
        .and_then(|base| base.checked_add(offset))
        .ok_or(MmuFailure::PhysicalRange.into())
}

/// A range the boot mapped and read back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a mapped range is used through its virtual address"]
pub struct MappedRange {
    pub virtual_start: u32,
    pub len: u32,
}

/// Map `len` bytes of `target` from `physical` at `virtual_start`, every
/// entry free or already mapping the same page, and read each back.
pub fn map<R: MmuRegisters>(
    registers: &mut R,
    window: MmuWindow,
    target: MmuTarget,
    virtual_start: u32,
    physical: u32,
    len: u32,
) -> Result<MappedRange, BootFailure> {
    let page_bytes = window.page_bytes;
    if !page_bytes.is_power_of_two()
        || len == 0
        || !len.is_multiple_of(page_bytes)
        || !physical.is_multiple_of(page_bytes)
    {
        return Err(MmuFailure::Misaligned.into());
    }
    let (first, offset) = window
        .locate(virtual_start)
        .ok_or(MmuFailure::OutsideWindow)?;
    if offset != 0 {
        return Err(MmuFailure::Misaligned.into());
    }
    let page_bytes = NonZeroU32::new(page_bytes).ok_or(MmuFailure::Misaligned)?;
    let pages = len / page_bytes;
    let last_offset = len.checked_sub(1).ok_or(MmuFailure::Misaligned)?;
    let last = virtual_start
        .checked_add(last_offset)
        .ok_or(MmuFailure::OutsideWindow)?;
    let (last, _) = window.locate(last).ok_or(MmuFailure::OutsideWindow)?;
    let first_page = physical / page_bytes;
    let physical_end = first_page
        .checked_add(pages)
        .ok_or(MmuFailure::PhysicalRange)?;
    if physical_end > window.physical_pages {
        return Err(MmuFailure::PhysicalRange.into());
    }
    // Both ends are admitted before any register access. The paired
    // ranges have one index and physical page per virtual page.
    let entries = (first..=last).zip(first_page..physical_end);
    for (index, page) in entries.clone() {
        let wanted = MmuEntry { target, page };
        match registers.read_entry(Access::new(), index) {
            None => {}
            Some(entry) if entry == wanted => {}
            Some(_) => return Err(MmuFailure::Occupied.into()),
        }
    }
    for (index, page) in entries {
        let wanted = MmuEntry { target, page };
        registers.write_entry(Access::new(), index, wanted);
        if registers.read_entry(Access::new(), index) != Some(wanted) {
            return Err(MmuFailure::Readback.into());
        }
    }
    Ok(MappedRange { virtual_start, len })
}
