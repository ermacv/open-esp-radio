//! The placement check: where an image's linked symbols lie in the chip's
//! `[memory]` map, as the chip profile's `[placement]` contract names them.
//! Every rule is data: the flattened image's span and origin, the entry
//! point's text, symbol ranges and symbols with the region each lies in,
//! and the entries that must swap to their own stack first.

#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use oer_chip_profile::{Memory, Placement, Region, Symbols};
use std::{collections::BTreeMap, fs, path::Path};

/// Audit the ELF `elf` and, when the contract names a flattened image, its
/// flattened `binary` against `placement` in `memory`; returns the
/// placement report.
pub fn audit(
    memory: &Memory,
    placement: &Placement,
    elf: &Path,
    binary: Option<&Path>,
) -> Result<String> {
    let bytes = fs::read(elf)?;
    let elf = oer_elf::Elf::executable(&bytes)?;
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
        let start = symbol(start_name)?;
        let end = symbol(end_name)?;
        if !region(&rule.region)?.contains_range(start, end) {
            violations.push(format!(
                "`{start_name}`..`{end_name}` ({start:#010x}..{end:#010x}) is not in {}",
                rule.region
            ));
        }
        if let Some(bytes) = rule.bytes
            && end.saturating_sub(start) != bytes
        {
            violations.push(format!(
                "`{start_name}`..`{end_name}` holds {} bytes, not {bytes}",
                end.saturating_sub(start)
            ));
        }
        report.push_str(&format!(
            "{start_name}..{end_name}={start:#010x}..{end:#010x}\n"
        ));
    }
    for rule in &placement.symbol {
        let within = region(&rule.region)?;
        for name in rule.symbols().names() {
            let at = symbol(&name)?;
            if !within.contains_range(at, at + rule.bytes) {
                violations.push(format!("`{name}` ({at:#010x}) is not in {}", rule.region));
            }
        }
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

/// Each named entry's first instruction swaps `sp` with `mscratch`
/// (`csrrw sp, mscratch, sp`) before anything touches the interrupted stack.
fn audit_stack_swap_entries(
    elf: &oer_elf::Elf<'_>,
    symbols: &BTreeMap<&str, u64>,
    entries: &[Symbols],
) -> Result<()> {
    use oer_riscv_decode::{CsrOp, Extension, Extensions, Instruction, Operand};
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
            .and_then(|bytes| oer_riscv_decode::decode(bytes, Extensions::ALL))
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
