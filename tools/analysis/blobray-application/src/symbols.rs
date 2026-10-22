//! Symbol tables and read-only strings of captured inputs.
//!
//! These views replace extracting archive members and running `nm` or
//! `strings` on vendor objects: the bytes stay inside the process.
use crate::in_process::Executable;
use oer_analysis_blobray_domain::{
    ArtifactInventory, ElfInventory, LibraryFunction, SectionRecord,
};
use oer_analysis_elf::classify::{self, ObjectKind, SectionFlags, SectionKind, SymbolKind};
use oer_analysis_riscv_model::{FunctionRecord, ObjectId, Result, RunControl, WorkingMemory};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
            let kind = match classify::symbol_kind(symbol.symbol_type) {
                SymbolKind::Function => "function",
                SymbolKind::Object => "object",
                _ => continue,
            };
            let Some(name) = symbol.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            if !classify::symbol_section(symbol.raw_section, symbol.extended_section).is_defined() {
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
            let section = classify::symbol_section(symbol.raw_section, symbol.extended_section)
                .index()
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

/// The sections of `elf` that hold read-only, allocated, non-executable data.
pub fn read_only_data(elf: &ElfInventory) -> impl Iterator<Item = &SectionRecord> {
    elf.sections.iter().filter(|section| {
        classify::section_kind(section.section_type) == SectionKind::Progbits
            && SectionFlags(section.flags).allocated()
            && !SectionFlags(section.flags).writable()
            && !SectionFlags(section.flags).executable()
            && section.size != 0
    })
}

/// Whether section `index` of `elf` is read-only data the toolchain marked
/// as NUL-terminated strings (`SHF_STRINGS`), such as `.rodata.str1.4`.
pub fn is_string_section(elf: &ElfInventory, index: u32) -> bool {
    read_only_data(elf)
        .any(|section| section.index == index && SectionFlags(section.flags).strings())
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

/// One string, by input, archive member, section and section offset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringRow {
    pub input: u64,
    pub member: Option<String>,
    pub section: Option<String>,
    pub offset: u64,
    /// Escaped text: printable ASCII with `\\n`, `\\t`, `\\r`, `\\\\`, `\\"` and `\\xNN`.
    pub text: String,
}

/// Captured string-section bytes keyed by physical object and section.
pub type StringSections = BTreeMap<(ObjectId, u32), Vec<u8>>;

/// The bytes of every read-only string section (`SHF_STRINGS`) of the
/// objects that define a function `names` names, by object and section.
pub fn string_sections(
    executables: &[Executable],
    names: &[String],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<StringSections> {
    let mut sections = std::collections::BTreeMap::new();
    for executable in executables {
        let inventory = crate::captured::inventory(executable, memory, control)?;
        for object in &inventory.objects {
            let Some(elf) = &object.elf else { continue };
            let defines = elf.symbols.iter().any(|symbol| {
                classify::symbol_section(symbol.raw_section, symbol.extended_section).is_defined()
                    && symbol
                        .name
                        .as_deref()
                        .is_some_and(|name| names.iter().any(|wanted| wanted.as_bytes() == name))
            });
            if !defines {
                continue;
            }
            for section in read_only_data(elf) {
                if is_string_section(elf, section.index) {
                    let bytes = section_bytes(&object.id, section, executables, memory, control)?;
                    sections.insert((object.id.clone(), section.index), bytes);
                }
            }
        }
    }
    Ok(sections)
}

/// Read-only strings named by this function's local-label relocations, by
/// instruction offset. PCREL low references retain their high label/addend.
pub fn referenced_strings(
    function: &LibraryFunction,
    records: &[FunctionRecord],
    sections: &StringSections,
) -> BTreeMap<u64, Vec<String>> {
    let mut texts: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for record in records {
        let oer_analysis_riscv_model::FunctionRecord::Reference {
            raw,
            target,
            addend,
            ..
        } = record
        else {
            continue;
        };
        let (Some(section), true) = (target.section, target.name.starts_with(b".L")) else {
            continue;
        };
        let Some(bytes) = sections.get(&(function.symbol.object.clone(), section)) else {
            continue;
        };
        let Some(offset) = target.offset.checked_add_signed(addend.unwrap_or(0)) else {
            continue;
        };
        if let Some(text) = string_at(bytes, offset).filter(|text| !text.is_empty()) {
            texts.entry(raw.offset).or_default().push(text);
        }
    }
    texts
}

/// The complete bytes of `section` of `object`, as its inventory record
/// sizes it, without another inventory pass.
pub fn section_bytes(
    object: &ObjectId,
    section: &oer_analysis_blobray_domain::SectionRecord,
    executables: &[Executable],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<u8>> {
    let request = oer_analysis_blobray_domain::DataRequest {
        object: object.clone(),
        symbol: None,
        ranges: vec![oer_analysis_blobray_domain::DataSelector::Section {
            section: section.index,
            offset: 0,
            length: section.size,
        }],
    };
    Ok(crate::data::export(&request, executables, memory, control)?.bytes)
}

/// Sized, named function entries for a listing renderer.
pub type ImageFunctions = Vec<(u64, u64, String)>;

/// The sized function symbols of `executable` when it is one little-endian
/// executable image (ET_EXEC), whose transfers carry no relocations to name
/// them. Other inputs are recognized from their first bytes and inventoried
/// no further.
pub fn image_symbols(
    executable: &Executable,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Option<ImageFunctions>> {
    // Decide from the ELF header alone, so an archive or relocatable object
    // costs no second inventory pass against the analysis' shared budget.
    if !classify::is_executable_image(executable.bytes()) {
        return Ok(None);
    }
    let inventory = crate::captured::inventory(executable, memory, control)?;
    let [object] = inventory.objects.as_slice() else {
        return Ok(None);
    };
    let Some(elf) = object
        .elf
        .as_ref()
        .filter(|elf| classify::object_kind(elf.object_type) == ObjectKind::Executable)
    else {
        return Ok(None);
    };
    Ok(Some(
        elf.symbols
            .iter()
            .filter(|symbol| classify::symbol_kind(symbol.symbol_type) == SymbolKind::Function)
            .filter_map(|symbol| {
                let name = symbol.name.as_deref()?;
                Some((
                    symbol.value,
                    symbol.size,
                    String::from_utf8_lossy(name).into_owned(),
                ))
            })
            .filter(|(_, size, name)| *size != 0 && !name.is_empty())
            .collect(),
    ))
}

/// Strings in the requested read-only sections, in input/member/section order.
pub fn string_rows(
    executables: &[Executable],
    member: Option<&str>,
    minimum: usize,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<StringRow>> {
    let mut rows = Vec::new();
    for (index, executable) in executables.iter().enumerate() {
        let inventory = crate::captured::inventory(executable, memory, control)?;
        for object in &inventory.objects {
            let name = object
                .name
                .as_deref()
                .map(|name| String::from_utf8_lossy(name).into_owned());
            if member
                .as_ref()
                .is_some_and(|wanted| name.as_deref() != Some(*wanted))
            {
                continue;
            }
            let Some(elf) = &object.elf else { continue };
            for section in read_only_data(elf) {
                let bytes = section_bytes(&object.id, section, executables, memory, control)?;
                for (offset, text) in strings(&bytes, minimum) {
                    rows.push(StringRow {
                        input: index as u64,
                        member: name.clone(),
                        section: section
                            .name
                            .as_deref()
                            .map(|name| String::from_utf8_lossy(name).into_owned()),
                        offset,
                        text,
                    });
                }
            }
        }
    }
    Ok(rows)
}
