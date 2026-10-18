//! Symbol tables and read-only strings of captured inputs.
//!
//! These views replace extracting archive members and running `nm` or
//! `strings` on vendor objects: the bytes stay inside the process.
use blobray_domain::{ArtifactInventory, ElfInventory, SectionRecord};
use serde::{Deserialize, Serialize};

const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const SHN_UNDEF: u16 = 0;
const SHT_PROGBITS: u32 = 1;
const SHF_WRITE: u64 = 0x1;
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const SHF_STRINGS: u64 = 0x20;
/// The shortest run `strings` reports, as binutils `strings` does by default.
pub const MIN_STRING: usize = 4;

/// One defined function or data symbol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolRow {
    /// Position of the input among the requested inputs.
    pub input: u64,
    /// The archive member, or `None` for a standalone ELF.
    pub member: Option<String>,
    /// The defining section's name, when the section table names it.
    pub section: Option<String>,
    pub address: u64,
    pub size: u64,
    /// `function` or `object`.
    pub kind: String,
    /// `local`, `global` or `weak`.
    pub binding: String,
    pub name: String,
    /// `ROLE[member]::name`, the form vendor provenance checks print when the
    /// input's role is the pinned artifact's id.
    pub citation: String,
}

/// Whether `name` matches `pattern`: a pattern with `*` matches the whole name
/// with `*` standing for any run of characters; one without `*` matches any
/// name containing it.
pub fn matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return name.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Every defined, named function and data symbol of `inventory`, in container
/// and table order, that `pattern` selects (all without one).
pub fn rows(
    input: u64,
    role: &str,
    inventory: &ArtifactInventory,
    pattern: Option<&str>,
) -> Vec<SymbolRow> {
    let mut rows = Vec::new();
    for object in &inventory.objects {
        let Some(elf) = &object.elf else { continue };
        let member = object
            .name
            .as_deref()
            .map(|name| String::from_utf8_lossy(name).into_owned());
        for symbol in &elf.symbols {
            let kind = match symbol.symbol_type & 0xf {
                STT_FUNC => "function",
                STT_OBJECT => "object",
                _ => continue,
            };
            let Some(name) = symbol.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            if symbol.raw_section == SHN_UNDEF {
                continue;
            }
            let name = String::from_utf8_lossy(name).into_owned();
            if pattern.is_some_and(|pattern| !matches(pattern, &name)) {
                continue;
            }
            let binding = match symbol.binding {
                0 => "local",
                1 => "global",
                2 => "weak",
                _ => "other",
            };
            let section = section_index(symbol.raw_section, symbol.extended_section)
                .and_then(|index| elf.sections.iter().find(|section| section.index == index))
                .and_then(|section| section.name.as_deref())
                .map(|name| String::from_utf8_lossy(name).into_owned());
            rows.push(SymbolRow {
                input,
                citation: format!("{role}[{}]::{name}", member.as_deref().unwrap_or("")),
                member: member.clone(),
                section,
                address: symbol.value,
                size: symbol.size,
                kind: kind.into(),
                binding: binding.into(),
                name,
            });
        }
    }
    rows
}

fn section_index(raw: u16, extended: Option<u32>) -> Option<u32> {
    match raw {
        0 | 0xfff1..=0xfff2 => None,
        0xffff => extended,
        raw => Some(u32::from(raw)),
    }
}

/// The sections of `elf` that hold read-only, allocated, non-executable data.
pub fn read_only_data(elf: &ElfInventory) -> impl Iterator<Item = &SectionRecord> {
    elf.sections.iter().filter(|section| {
        section.section_type == SHT_PROGBITS
            && section.flags & SHF_ALLOC != 0
            && section.flags & (SHF_WRITE | SHF_EXECINSTR) == 0
            && section.size != 0
    })
}

/// Whether section `index` of `elf` is read-only data the toolchain marked
/// as NUL-terminated strings (`SHF_STRINGS`), such as `.rodata.str1.4`.
pub fn is_string_section(elf: &ElfInventory, index: u32) -> bool {
    read_only_data(elf).any(|section| section.index == index && section.flags & SHF_STRINGS != 0)
}

/// Every NUL-terminated run of at least `minimum` text bytes in `bytes`, with
/// its offset. Text is printable ASCII, tab, newline, carriage return and any
/// byte above 0x7f; the rendering escapes everything but printable ASCII, so
/// a non-UTF-8 string stays exact. A run without its terminating NUL at the end
/// of `bytes` is not reported.
pub fn strings(bytes: &[u8], minimum: usize) -> Vec<(u64, String)> {
    let text = |byte: u8| matches!(byte, 0x20..=0x7e | b'\t' | b'\n' | b'\r' | 0x80..=0xff);
    let mut found = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == 0 {
            let run = &bytes[start..index];
            if run.len() >= minimum && run.iter().all(|byte| text(*byte)) {
                found.push((start as u64, escape(run)));
            }
            start = index + 1;
        }
    }
    found
}

/// `bytes` as printable ASCII with `\n`, `\t`, `\r`, `\\`, `\"` and `\xNN`
/// escapes.
pub fn escape(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for byte in bytes {
        match byte {
            b'\n' => text.push_str("\\n"),
            b'\t' => text.push_str("\\t"),
            b'\r' => text.push_str("\\r"),
            b'\\' => text.push_str("\\\\"),
            b'"' => text.push_str("\\\""),
            0x20..=0x7e => text.push(*byte as char),
            other => text.push_str(&format!("\\x{other:02x}")),
        }
    }
    text
}

/// The NUL-terminated text starting at `offset` of `bytes`, escaped, or `None`
/// when it is not text or has no terminator.
pub fn string_at(bytes: &[u8], offset: u64) -> Option<String> {
    let tail = bytes.get(usize::try_from(offset).ok()?..)?;
    let end = tail.iter().position(|byte| *byte == 0)?;
    let run = &tail[..end];
    run.iter()
        .all(|byte| matches!(byte, 0x20..=0x7e | b'\t' | b'\n' | b'\r' | 0x80..=0xff))
        .then(|| escape(run))
}

#[cfg(test)]
mod tests;
