//! The minimal SRAM-placement check of a linked runtime, reported as
//! warnings (it never fails a build): code that runs while cached PSRAM may
//! be unavailable must lie in internal SRAM.
//!
//! - every handler of the image's interrupt table (`__OER_INTERRUPT_TABLE`);
//! - the panic entry (`rust_begin_unwind`) and the image's panic hook;
//! - the critical sections (`.critical.*`).
//!
//! DMA storage is not checked here: the runtime link refuses it outside the
//! DMA window or at an address whose low DMA address bits are zero (the
//! generated `dma-window.x`).

use object::{Object as _, ObjectSection as _, ObjectSymbol as _, read::elf::ElfFile32};
use oer_firmware_platform_espressif_staged_layout::Region;

use crate::Result;

/// The interrupt table's exported slice: a pointer and a length.
pub const TABLE_SYMBOL: &str = "__OER_INTERRUPT_TABLE";
/// Bytes of one table entry and the handler word's offset in it
/// (`oer-firmware-platform-espressif-staged-layout`'s `interrupts`).
pub const ENTRY_BYTES: u32 = 12;
pub const HANDLER_OFFSET: u32 = 8;
/// The panic entry's symbol, under its mangled path.
pub const PANIC_ENTRY: &str = "rust_begin_unwind";
pub const PANIC_HOOK: &str = "oer_platform_panic_hook";
/// Sections whose every byte must be in SRAM.
pub const SRAM_SECTION_PREFIXES: [&str; 1] = [".critical"];

/// What the check reads of a linked image.
pub struct Image<'a> {
    /// Allocated sections: name, address and size.
    pub sections: Vec<(String, u32, u32)>,
    /// Symbols: name and address.
    pub symbols: Vec<(String, u32)>,
    /// The little-endian word the image initializes at an address.
    pub word: Box<dyn Fn(u32) -> Option<u32> + 'a>,
}

fn in_region(region: Region, start: u32, size: u32) -> bool {
    let end = u64::from(start) + u64::from(size);
    start >= region.origin && end <= u64::from(region.origin) + u64::from(region.length)
}

/// The warnings of `image` against the internal SRAM `sram`.
pub fn check(image: &Image<'_>, sram: Region) -> Vec<String> {
    let mut warnings = Vec::new();
    let symbol = |name: &str| {
        image
            .symbols
            .iter()
            .find(|(symbol, _)| symbol == name || symbol.ends_with(name))
            .map(|(_, address)| *address)
    };
    match symbol(TABLE_SYMBOL) {
        None => warnings.push(format!("no interrupt table `{TABLE_SYMBOL}`")),
        Some(table) => match ((image.word)(table), (image.word)(table + 4)) {
            (Some(entries), Some(count)) => {
                for index in 0..count {
                    let at = entries + index * ENTRY_BYTES + HANDLER_OFFSET;
                    match (image.word)(at) {
                        Some(0) => {}
                        Some(handler) if in_region(sram, handler, 4) => {}
                        Some(handler) => warnings.push(format!(
                            "interrupt table entry {index}'s handler {handler:#010x} is not in SRAM"
                        )),
                        None => warnings.push(format!(
                            "interrupt table entry {index} at {at:#010x} is not initialized data"
                        )),
                    }
                }
            }
            _ => warnings.push(format!("`{TABLE_SYMBOL}` is not initialized data")),
        },
    }
    match symbol(PANIC_ENTRY) {
        None => warnings.push(format!("no panic entry `{PANIC_ENTRY}`")),
        Some(address) if !in_region(sram, address, 4) => warnings.push(format!(
            "the panic entry `{PANIC_ENTRY}` at {address:#010x} is not in SRAM"
        )),
        Some(_) => {}
    }
    if let Some(address) = symbol(PANIC_HOOK)
        && !in_region(sram, address, 4)
    {
        warnings.push(format!(
            "the panic hook `{PANIC_HOOK}` at {address:#010x} is not in SRAM"
        ));
    }
    for (name, address, size) in &image.sections {
        if *size > 0
            && SRAM_SECTION_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
            && !in_region(sram, *address, *size)
        {
            warnings.push(format!(
                "section `{name}` at {address:#010x}+{size:#x} is not in SRAM"
            ));
        }
    }
    warnings
}

/// The warnings of the runtime ELF `elf` against `sram`.
pub fn check_elf(elf: &[u8], sram: Region) -> Result<Vec<String>> {
    let file = ElfFile32::<object::LittleEndian>::parse(elf)?;
    let mut sections = Vec::new();
    let mut initialized = Vec::new();
    for section in file.sections() {
        let flags = section.flags();
        let object::SectionFlags::Elf { sh_flags } = flags else {
            continue;
        };
        if sh_flags & u64::from(object::elf::SHF_ALLOC) == 0 {
            continue;
        }
        let name = section.name().unwrap_or_default().to_owned();
        let address = u32::try_from(section.address())?;
        let size = u32::try_from(section.size())?;
        sections.push((name, address, size));
        if section.kind() != object::SectionKind::UninitializedData
            && let Ok(data) = section.data()
        {
            initialized.push((address, data));
        }
    }
    let symbols = file
        .symbols()
        .filter_map(|symbol| {
            Some((
                symbol.name().ok()?.to_owned(),
                u32::try_from(symbol.address()).ok()?,
            ))
        })
        .collect();
    let word = move |address: u32| {
        initialized.iter().find_map(|(start, data)| {
            let offset = usize::try_from(address.checked_sub(*start)?).ok()?;
            let bytes = data.get(offset..offset + 4)?;
            Some(u32::from_le_bytes(bytes.try_into().ok()?))
        })
    };
    Ok(check(
        &Image {
            sections,
            symbols,
            word: Box::new(word),
        },
        sram,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SRAM: Region = Region::new(0x2f00_0000, 0x0007_afc0);

    fn image(
        words: HashMap<u32, u32>,
        symbols: &[(&str, u32)],
        sections: &[(&str, u32, u32)],
    ) -> Image<'static> {
        Image {
            sections: sections
                .iter()
                .map(|(name, address, size)| ((*name).to_owned(), *address, *size))
                .collect(),
            symbols: symbols
                .iter()
                .map(|(name, address)| ((*name).to_owned(), *address))
                .collect(),
            word: Box::new(move |address| words.get(&address).copied()),
        }
    }

    #[test]
    fn a_placed_image_has_no_warnings() {
        let table = 0x5001_0000;
        let entries = 0x5001_0100;
        let words = HashMap::from([
            (table, entries),
            (table + 4, 2),
            (entries + HANDLER_OFFSET, 0x2f00_1000),
            (entries + ENTRY_BYTES + HANDLER_OFFSET, 0),
        ]);
        let image = image(
            words,
            &[
                (TABLE_SYMBOL, table),
                ("_RNvCs_7___rustc17rust_begin_unwind", 0x2f00_2000),
            ],
            &[
                (".dma.bss", 0x2f01_0000, 0x100),
                (".critical.data", 0x2f00_3000, 4),
                (".text", 0x5001_3c00, 0x1000),
            ],
        );
        assert_eq!(check(&image, SRAM), Vec::<String>::new());
    }

    #[test]
    fn psram_handlers_panic_paths_and_critical_sections_warn() {
        let table = 0x5001_0000;
        let entries = 0x5001_0100;
        let words = HashMap::from([
            (table, entries),
            (table + 4, 1),
            (entries + HANDLER_OFFSET, 0x5001_4000),
        ]);
        let image = image(
            words,
            &[
                (TABLE_SYMBOL, table),
                (PANIC_ENTRY, 0x5001_5000),
                (PANIC_HOOK, 0x5001_6000),
            ],
            &[
                (".dma.data", 0x5002_0000, 0x10),
                (".critical.bss", 0x2f07_afb0, 0x20),
            ],
        );
        let warnings = check(&image, SRAM);
        assert_eq!(warnings.len(), 4, "{warnings:#?}");
        assert!(warnings[0].contains("handler 0x50014000"));
        assert!(warnings[1].contains("panic entry"));
        assert!(warnings[2].contains("panic hook"));
        assert!(warnings[3].contains(".critical.bss"));
    }

    #[test]
    fn dma_sections_are_left_to_the_link() {
        // Outside SRAM and at a zero low address: the link refuses both, so
        // the placement check does not report them.
        let image = image(
            HashMap::new(),
            &[(TABLE_SYMBOL, 0), (PANIC_ENTRY, 0x2f00_1000)],
            &[
                (".dma.bss", 0x2f00_0000, 0x100),
                (".dma.data", 0x5002_0000, 0x10),
            ],
        );
        let warnings = check(&image, SRAM);
        assert!(
            !warnings.iter().any(|warning| warning.contains(".dma")),
            "{warnings:#?}"
        );
    }

    #[test]
    fn a_missing_table_or_panic_entry_warns() {
        let warnings = check(&image(HashMap::new(), &[], &[]), SRAM);
        assert_eq!(warnings.len(), 2, "{warnings:#?}");
    }
}
