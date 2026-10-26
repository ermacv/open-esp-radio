//! The placement check: where an image's linked symbols lie in the chip's
//! `[memory]` map, as the chip profile's `[placement]` contract names them.
//! Every rule is data: the flattened image's span and origin, the entry
//! point's text, symbol ranges, symbols and sections with the region each
//! lies in, the regions the interrupt table's handlers lie in, and the
//! entries that must swap to their own stack first.

#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use oer_espressif_facts::{InterruptContract, Memory, Placement, Region, Symbols};
use std::{collections::BTreeMap, fs, path::Path};

/// The region name of the mask ROM's code in an interrupt handler rule.
pub const ROM: &str = "rom";

/// Mandatory for every staged bootstrap, including a diagnostic build.
pub const BOOTSTRAP_REPORT: &str = "bootstrap-placement.txt";

/// The bootstrap may suspend instruction fetch while tuning flash. Its
/// complete executable image and ordinary constants must therefore be in
/// internal SRAM, including out-of-line callees from other crates. Explicit
/// flash payload/reference sections remain sources read outside suspension.
/// Every allocated section overlapping flash must be a non-writable,
/// non-executable section on the shared script's exact allowlist.
pub fn bootstrap(sram: Region, flash: Region, path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let elf = oer_analysis_elf::Elf::executable(&bytes)?;
    let mut violations = Vec::new();
    let mut code_bytes = 0_u64;
    let mut constant_bytes = 0_u64;
    for section in elf
        .sections()
        .filter(|section| section.allocated && section.size > 0)
    {
        let end = section
            .address
            .checked_add(section.size)
            .ok_or("bootstrap section range overflow")?;
        let in_sram = sram.contains_range(section.address, end);
        let overlaps_flash =
            section.address < u64::from(flash.end()) && end > u64::from(flash.origin);
        // Exact output-section names owned by the shared bootstrap script.
        // Neither a new name nor a prefix of an allowed name grants access.
        // The S31 loader needs two mapped image records, not flash code:
        // payload and tuning reference are separate read-only sources.
        // SOURCE: ESP-IDF e9da155a bootloader_utility.c:802-851;
        // chips/esp32s31/platform/chip.toml [boot-chain]. The staged encoder
        // audits the actual ESP segment count after adjacent sections merge.
        let allowed_flash = flash.contains_range(section.address, end)
            && !section.executable
            && !section.writable
            && !section.nobits
            && matches!(
                section.name,
                ".flash.appdesc"
                    | ".flash.rodata"
                    | ".flash.tuning.reference"
                    | ".eh_frame"
                    | ".eh_frame_hdr"
                    | ".metadata"
            );
        if section.executable {
            code_bytes = code_bytes
                .checked_add(section.size)
                .ok_or("bootstrap code size overflow")?;
        } else if in_sram && !section.writable && !section.nobits {
            constant_bytes = constant_bytes
                .checked_add(section.size)
                .ok_or("bootstrap constant size overflow")?;
        }
        let sram_required =
            section.executable || (!section.writable && !section.nobits && !allowed_flash);
        if (!sram_required || in_sram) && (!overlaps_flash || allowed_flash) {
            continue;
        }
        let names = elf
            .symbols()
            .filter(|symbol| symbol.defined && symbol.section == Some(section.index))
            .map(|symbol| symbol.demangled())
            .collect::<Vec<_>>()
            .join(", ");
        violations.push(format!(
            "section `{}` ({:#010x}+{:#x}) violates bootstrap SRAM/flash placement; symbols: {names}",
            section.name, section.address, section.size,
        ));
    }
    if !violations.is_empty() {
        return Err(format!(
            "bootstrap cache-off placement failed:\n  {}",
            violations.join("\n  ")
        )
        .into());
    }
    Ok(format!(
        "bootstrap cache-off placement: code={code_bytes} constants={constant_bytes} bytes in SRAM\nresult=PASS\n"
    ))
}

/// What the placement check reads besides the image.
pub struct Contract<'a> {
    pub memory: &'a Memory,
    pub placement: &'a Placement,
    /// The interrupt table's layout, which handler rules need.
    pub interrupts: Option<&'a InterruptContract>,
    /// The mask ROM's code, which [`ROM`] names.
    pub rom: &'a [Region],
}

/// Audit the ELF `elf` and, when the contract names a flattened image, its
/// flattened `binary` against `contract`; returns the placement report.
pub fn audit(contract: &Contract<'_>, elf: &Path, binary: Option<&Path>) -> Result<String> {
    let (memory, placement) = (contract.memory, contract.placement);
    let bytes = fs::read(elf)?;
    let elf = oer_analysis_elf::Elf::executable(&bytes)?;
    let symbols = elf.addresses();
    let symbol = |name: &str| -> Result<u64> {
        symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("the ELF lacks `{name}`").into())
    };
    let region = |name: &str| -> Result<Region> {
        memory.region(name).ok_or_else(|| {
            format!("the placement contract names `{name}`, a region the chip's [memory] map lacks")
                .into()
        })
    };
    let mut report = format!("profile={}\n", placement.name);
    let mut violations = Vec::new();
    if let Some(image) = &placement.image {
        let binary = binary
            .ok_or("the placement contract names a flattened image the build left none of")?;
        let start = symbol(&image.start)?;
        let end = symbol(&image.end)?;
        let origin = u64::from(region(&image.origin)?.origin);
        let binary_bytes = fs::metadata(binary)?.len();
        if start != origin || end <= start || end - start != binary_bytes {
            violations.push(format!(
                "the image {start:#010x}..{end:#010x} is not the {binary_bytes}-byte flat file at {origin:#010x}"
            ));
        }
        report.push_str(&format!("image={start:#010x}..{end:#010x}\n"));
    }
    if let Some(entry) = &placement.entry {
        let at = symbol(&entry.symbol)?;
        let start = symbol(&entry.text[0])?;
        let end = symbol(&entry.text[1])?;
        if !(start..end).contains(&at) {
            violations.push(format!(
                "the entry `{}` lies outside its text",
                entry.symbol
            ));
        }
        report.push_str(&format!("text={start:#010x}..{end:#010x}\n"));
    }
    for rule in &placement.range {
        let [start_name, end_name] = &rule.range;
        let bounds = oer_analysis_elf::StorageBounds::range(&elf, start_name, end_name)?
            .ok_or_else(|| format!("the ELF lacks `{start_name}`..`{end_name}`"))?;
        let (start, end) = (bounds.start, bounds.end);
        if !region(&rule.region)?.contains_range(start, end) {
            violations.push(format!(
                "`{start_name}`..`{end_name}` ({start:#010x}..{end:#010x}) is not in {}",
                rule.region
            ));
        }
        if let Some(bytes) = rule.bytes
            && bounds.capacity != bytes
        {
            violations.push(format!(
                "`{start_name}`..`{end_name}` holds {} bytes, not {bytes}",
                bounds.capacity
            ));
        }
        if let Some(maximum) = rule.maximum_bytes
            && bounds.capacity > maximum
        {
            violations.push(format!(
                "`{start_name}`..`{end_name}` holds {} bytes, above its maximum {maximum}",
                bounds.capacity
            ));
        }
        report.push_str(&format!(
            "{start_name}..{end_name}={start:#010x}..{end:#010x}\n"
        ));
    }
    for rule in &placement.symbol {
        let within = region(&rule.region)?;
        for name in rule.symbols().names() {
            let found = symbols
                .get(name.as_str())
                .copied()
                .or_else(|| by_demangled_name(&elf, &name));
            let Some(at) = found else {
                if rule.optional {
                    continue;
                }
                return Err(format!("the ELF lacks `{name}`").into());
            };
            if !within.contains_range(at, at + rule.bytes) {
                violations.push(format!("`{name}` ({at:#010x}) is not in {}", rule.region));
            }
        }
    }
    for rule in &placement.section {
        let within = region(&rule.region)?;
        violations.extend(section_violations(
            elf.sections()
                .filter(|section| section.allocated)
                .map(|section| (section.name, section.address, section.size)),
            &rule.prefix,
            (within, &rule.region),
        ));
    }
    if !placement.interrupt_handlers.is_empty() {
        let interrupts = contract.interrupts.ok_or(
            "the placement contract names interrupt handler regions, the chip names no [interrupts] table",
        )?;
        let mut regions = Vec::new();
        for name in &placement.interrupt_handlers {
            if name == ROM {
                if contract.rom.is_empty() {
                    return Err(
                        "the placement contract names the ROM, the chip names no [rom] ELF".into(),
                    );
                }
                regions.extend_from_slice(contract.rom);
            } else {
                regions.push(region(name)?);
            }
        }
        let handlers = interrupt_handlers(&elf, symbol(&interrupts.table_symbol)?, interrupts)?;
        violations.extend(handler_violations(
            &handlers,
            &regions,
            &placement.interrupt_handlers,
        ));
        report.push_str(&format!(
            "interrupt-handlers={}\n",
            placement.interrupt_handlers.join(",")
        ));
    }
    if !violations.is_empty() {
        return Err(format!(
            "the image violates the {} placement contract:\n  {}",
            placement.name,
            violations.join("\n  ")
        )
        .into());
    }
    audit_stack_swap_entries(&elf, &symbols, &placement.stack_swap)?;
    report.push_str("result=PASS\n");
    Ok(report)
}

/// The allocated `sections` (name, address, bytes) whose name starts with
/// `prefix` and that do not lie wholly in `region`.
fn section_violations<'a>(
    sections: impl Iterator<Item = (&'a str, u64, u64)>,
    prefix: &str,
    (region, name): (Region, &str),
) -> Vec<String> {
    sections
        .filter(|(section, address, size)| {
            *size > 0
                && section.starts_with(prefix)
                && !region.contains_range(*address, address + size)
        })
        .map(|(section, address, size)| {
            format!("section `{section}` ({address:#010x}+{size:#x}) is not in {name}")
        })
        .collect()
}

/// The interrupt table `handlers` (entry index, address) outside every one
/// of `regions`, which `names` name.
fn handler_violations(
    handlers: &[(u32, u64)],
    regions: &[Region],
    names: &[String],
) -> Vec<String> {
    handlers
        .iter()
        .filter(|(_, handler)| {
            !regions
                .iter()
                .any(|region| region.contains_range(*handler, handler + 4))
        })
        .map(|(index, handler)| {
            format!(
                "interrupt table entry {index}'s handler {handler:#010x} is not in {}",
                names.join(" or ")
            )
        })
        .collect()
}

/// The address of the defined symbol whose demangled name is `name`: a
/// mangled Rust symbol, whose hash differs between builds.
fn by_demangled_name(elf: &oer_analysis_elf::Elf<'_>, name: &str) -> Option<u64> {
    elf.symbols()
        .find(|symbol| symbol.defined && symbol.demangled() == name)
        .map(|symbol| symbol.address)
}

/// The handlers of the interrupt table at `table` (its entries' pointer and
/// count) by entry index; an empty entry has none.
fn interrupt_handlers(
    elf: &oer_analysis_elf::Elf<'_>,
    table: u64,
    interrupts: &InterruptContract,
) -> Result<Vec<(u32, u64)>> {
    let word = |address: u64| -> Result<u32> {
        elf.sections()
            .filter(|section| section.allocated && !section.nobits)
            .find_map(|section| section.word(address))
            .ok_or_else(|| format!("{address:#010x} holds no initialized word").into())
    };
    let entries = u64::from(word(table)?);
    let count = word(table + 4)?;
    let mut handlers = Vec::new();
    for index in 0..count {
        let at = entries
            + u64::from(index) * u64::from(interrupts.table_entry_bytes)
            + u64::from(interrupts.table_handler);
        match word(at)? {
            0 => {}
            handler => handlers.push((index, u64::from(handler))),
        }
    }
    Ok(handlers)
}

/// Each named entry's first instruction swaps `sp` with `mscratch`
/// (`csrrw sp, mscratch, sp`) before anything touches the interrupted stack.
fn audit_stack_swap_entries(
    elf: &oer_analysis_elf::Elf<'_>,
    symbols: &BTreeMap<&str, u64>,
    entries: &[Symbols],
) -> Result<()> {
    use oer_analysis_riscv_decode::{CsrOp, Extension, Extensions, Instruction, Operand};
    const SP: u8 = 2;
    const MSCRATCH: u16 = 0x340;
    for name in entries.iter().flat_map(Symbols::names) {
        let address = *symbols
            .get(name.as_str())
            .ok_or_else(|| format!("the ELF lacks `{name}`"))?;
        let section = elf
            .sections()
            .find(|section| section.executable && section.contains(address))
            .ok_or_else(|| format!("`{name}` is not in executable code"))?;
        let at = usize::try_from(address - section.address)?;
        let instruction = section
            .data
            .get(at..)
            .and_then(|bytes| oer_analysis_riscv_decode::decode(bytes, Extensions::ALL))
            .map(|(instruction, _)| instruction)
            .ok_or_else(|| format!("the code has no instruction for `{name}`"))?;
        let swaps = matches!(
            instruction,
            Instruction::Extension(Extension::Csr {
                op: CsrOp::Write,
                dest: SP,
                source: Operand::Register(SP),
                csr: MSCRATCH,
            })
        );
        if !swaps {
            return Err(format!(
                "`{name}` touches the interrupted stack before swapping to its own: `{instruction}`"
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
