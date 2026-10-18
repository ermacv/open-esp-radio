//! ELF records are emitted synchronously; no symbol or relocation accumulator.
use super::*;
use crate::meter::{Bytes, Meter};

/// Unknown payloads only need a prefix and streaming hash. Supported ELF payloads
/// require one admitted contiguous input window for the object crate's views.
pub fn inspect_source(
    source: &dyn ByteSource,
    object: &ObjectId,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn ElfSink,
) -> Result<(ArtifactId, Option<ElfInventory>)> {
    control.phase(RunPhase::ReadCaptured)?;
    let content = hash_source(source, control)?;
    let mut prefix = [0; 16];
    let count = source.len().min(prefix.len() as u64) as usize;
    source.read_at(0, &mut prefix[..count], control)?;
    let elf = if prefix.starts_with(b"\x7fELF") {
        let bytes = read_scratch(source, memory, control)?;
        inspect_payload(&bytes, object, memory, control, sink)?
    } else {
        inspect_payload(&prefix[..count], object, memory, control, sink)?
    };
    Ok((content, elf))
}

/// Inspect a single admitted ELF payload. Returned header has empty record lists;
/// every record and diagnostic is delivered through the sink before returning.
pub fn inspect_payload(
    bytes: &[u8],
    object: &ObjectId,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn ElfSink,
) -> Result<Option<ElfInventory>> {
    control.phase(RunPhase::Elf)?;
    // Two simultaneous copied names plus fixed diagnostic/identity records. Input
    // storage has a separate reservation; no record-count-dependent allocation.
    let _names = memory.reserve(
        (bytes.len() as u64)
            .checked_mul(2)
            .and_then(|n| n.checked_add(16384))
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "ELF scratch size overflow"))?,
        control.position(),
    )?;
    let meter = Meter::new(control);
    let parsed = match FileKind::parse(bytes) {
        Ok(FileKind::Elf32) => {
            inspect_elf_stream::<elf::FileHeader32<Endianness>>(bytes, object, sink, &meter)
        }
        Ok(FileKind::Elf64) => {
            inspect_elf_stream::<elf::FileHeader64<Endianness>>(bytes, object, sink, &meter)
        }
        _ => {
            emit(
                sink,
                &meter,
                &diagnostic(
                    if bytes.starts_with(b"\x7fELF") {
                        DiagnosticCode::MalformedObject
                    } else {
                        DiagnosticCode::UnsupportedFormat
                    },
                    "object",
                    "payload is not a readable ELF32/ELF64 object; original bytes retained",
                ),
            )?;
            return meter.finish(Ok(None));
        }
    };
    if meter.failed() {
        return meter.finish(parsed).map(Some);
    }
    match meter.finish(parsed) {
        Ok(header) => Ok(Some(header)),
        Err(error) if error.code == ErrorCode::Integrity => {
            emit(
                sink,
                &meter,
                &diagnostic(
                    DiagnosticCode::MalformedObject,
                    "ELF header/section table",
                    error,
                ),
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
fn emit(sink: &mut dyn ElfSink, meter: &Meter<'_>, value: &Diagnostic) -> Result<()> {
    meter.with_control(|control| sink.diagnostic(value, control))
}
fn emit_section(sink: &mut dyn ElfSink, meter: &Meter<'_>, value: &SectionRecord) -> Result<()> {
    meter.with_control(|control| sink.section(value, control))
}
fn emit_symbol(sink: &mut dyn ElfSink, meter: &Meter<'_>, value: &SymbolRecord) -> Result<()> {
    meter.with_control(|control| sink.symbol(value, control))
}
fn relocation<'data, Elf: FileHeader>(
    sink: &mut dyn ElfSink,
    meter: &Meter<'_>,
    sections: &object::read::elf::SectionTable<'data, Elf, Bytes<'data, '_, '_>>,
    endian: Elf::Endian,
    data: Bytes<'data, '_, '_>,
    value: &RelocationRecord,
) -> Result<()> {
    let valid = sections
        .section(object::SectionIndex(value.symbol_table_section as usize))
        .ok()
        .filter(|s| matches!(s.sh_type(endian), elf::SHT_SYMTAB | elf::SHT_DYNSYM))
        .and_then(|s| s.data_as_array::<Elf::Sym, _>(endian, data).ok())
        .is_some_and(|entries| (value.symbol_index as usize) < entries.len());
    if !valid {
        emit(
            sink,
            meter,
            &diagnostic(
                DiagnosticCode::MalformedTable,
                format!("section {} relocation {}", value.section, value.index),
                "relocation references an unavailable symbol-table entry",
            ),
        )?;
    }
    meter.with_control(|control| sink.relocation(value, control))
}
pub(super) fn inspect_elf_stream<Elf: FileHeader>(
    bytes: &[u8],
    object: &ObjectId,
    sink: &mut dyn ElfSink,
    meter: &Meter<'_>,
) -> Result<ElfInventory> {
    let data = Bytes { bytes, meter };
    let malformed = |error: object::Error| Error::new(ErrorCode::Integrity, error.to_string());
    let header = Elf::parse(data).map_err(malformed)?;
    let endian = header.endian().map_err(malformed)?;
    let sections = header.sections(endian, data).map_err(malformed)?;
    let result = ElfInventory {
        bits: if header.is_type_64() { 64 } else { 32 },
        little_endian: endian.is_little_endian(),
        machine: header.e_machine(endian),
        object_type: header.e_type(endian),
        entry: header.e_entry(endian).into(),
        sections: Vec::new(),
        symbols: Vec::new(),
        relocations: Vec::new(),
    };
    // Validate program-header framing even though this slice does not build images.
    if let Err(error) = header.program_headers(endian, data) {
        emit(
            sink,
            meter,
            &diagnostic(DiagnosticCode::MalformedTable, "program headers", error),
        )?;
    }
    for (index, section) in sections.enumerate() {
        let mut position = meter.position();
        position.table = Some(index.0 as u64);
        position.entry = None;
        meter.set_position(position);
        meter.checkpoint(1)?;
        let context = format!("section {}", index.0);
        let name = match sections.section_name(endian, section) {
            Ok(name) => Some(meter.copy(name)?),
            Err(error) => {
                emit(
                    sink,
                    meter,
                    &diagnostic(DiagnosticCode::MalformedName, &context, error),
                )?;
                None
            }
        };
        emit_section(
            sink,
            meter,
            &SectionRecord {
                index: index.0 as u32,
                name,
                section_type: section.sh_type(endian),
                flags: section.sh_flags(endian).into(),
                address: section.sh_addr(endian).into(),
                file_offset: section.sh_offset(endian).into(),
                size: section.sh_size(endian).into(),
                link: section.sh_link(endian),
                info: section.sh_info(endian),
                alignment: section.sh_addralign(endian).into(),
                entry_size: section.sh_entsize(endian).into(),
            },
        )?;
        if section.sh_type(endian) != elf::SHT_NULL
            && let Err(error) = section.data(endian, data)
        {
            emit(
                sink,
                meter,
                &diagnostic(DiagnosticCode::MalformedTable, &context, error),
            )?;
        }
        match section.sh_type(endian) {
            elf::SHT_SYMTAB | elf::SHT_DYNSYM => {
                check_stride::<Elf::Sym>(
                    section.sh_size(endian).into(),
                    section.sh_entsize(endian).into(),
                    &context,
                    sink,
                    meter,
                )?;
                let entries = match section.data_as_array::<Elf::Sym, _>(endian, data) {
                    Ok(entries) => entries,
                    Err(error) => {
                        emit(
                            sink,
                            meter,
                            &diagnostic(DiagnosticCode::MalformedTable, &context, error),
                        )?;
                        continue;
                    }
                };
                let table = match section.symbols(endian, data, &sections, index) {
                    Ok(table) => table,
                    Err(error) => {
                        emit(
                            sink,
                            meter,
                            &diagnostic(DiagnosticCode::MalformedTable, &context, error),
                        )?;
                        None
                    }
                };
                let table_kind = if section.sh_type(endian) == elf::SHT_SYMTAB {
                    SymbolTableKind::Static
                } else {
                    SymbolTableKind::Dynamic
                };
                for (symbol_index, symbol) in entries.iter().enumerate() {
                    let mut position = meter.position();
                    position.entry = Some(symbol_index as u64);
                    meter.set_position(position);
                    meter.checkpoint(1)?;
                    let symbol_context = format!("{context} symbol {symbol_index}");
                    let name = if let Some(table) = table.as_ref() {
                        match symbol.name(endian, table.strings()) {
                            Ok(name) => Some(meter.copy(name)?),
                            Err(error) => {
                                meter.checkpoint(0)?;
                                emit(
                                    sink,
                                    meter,
                                    &diagnostic(
                                        DiagnosticCode::MalformedName,
                                        &symbol_context,
                                        error,
                                    ),
                                )?;
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let extended_section = if symbol.st_shndx(endian) == elf::SHN_XINDEX {
                        let value = table
                            .as_ref()
                            .and_then(|t| t.shndx(endian, object::SymbolIndex(symbol_index)));
                        if value.is_none() {
                            emit(
                                sink,
                                meter,
                                &diagnostic(
                                    DiagnosticCode::MalformedTable,
                                    &symbol_context,
                                    "missing extended symbol section index",
                                ),
                            )?;
                        }
                        value
                    } else {
                        None
                    };
                    let raw_section = symbol.st_shndx(endian);
                    let resolved_section = extended_section.or_else(|| {
                        (raw_section != elf::SHN_UNDEF && raw_section < elf::SHN_LORESERVE)
                            .then_some(u32::from(raw_section))
                    });
                    if resolved_section.is_some_and(|section| section as usize >= sections.len()) {
                        emit(
                            sink,
                            meter,
                            &diagnostic(
                                DiagnosticCode::MalformedTable,
                                &symbol_context,
                                "symbol references an unavailable section",
                            ),
                        )?;
                    }
                    emit_symbol(
                        sink,
                        meter,
                        &SymbolRecord {
                            id: SymbolId {
                                object: object.clone(),
                                table: table_kind,
                                table_section: index.0 as u32,
                                index: symbol_index as u64,
                            },
                            name,
                            name_offset: symbol.st_name(endian),
                            value: symbol.st_value(endian).into(),
                            size: symbol.st_size(endian).into(),
                            binding: symbol.st_bind(),
                            symbol_type: symbol.st_type(),
                            other: symbol.st_other(),
                            raw_section,
                            extended_section,
                        },
                    )?;
                }
            }
            elf::SHT_REL => {
                check_stride::<Elf::Rel>(
                    section.sh_size(endian).into(),
                    section.sh_entsize(endian).into(),
                    &context,
                    sink,
                    meter,
                )?;
                match section.rel(endian, data) {
                    Ok(Some((entries, table))) => {
                        for (ordinal, entry) in entries.iter().enumerate() {
                            let mut position = meter.position();
                            position.entry = Some(ordinal as u64);
                            meter.set_position(position);
                            meter.checkpoint(1)?;
                            relocation::<Elf>(
                                sink,
                                meter,
                                &sections,
                                endian,
                                data,
                                &RelocationRecord {
                                    section: index.0 as u32,
                                    index: ordinal as u64,
                                    offset: entry.r_offset(endian).into(),
                                    relocation_type: entry.r_type(endian),
                                    symbol_table_section: table.0 as u32,
                                    symbol_index: entry.r_sym(endian),
                                    addend: None,
                                },
                            )?;
                        }
                    }
                    Err(error) => emit(
                        sink,
                        meter,
                        &diagnostic(DiagnosticCode::MalformedTable, &context, error),
                    )?,
                    Ok(None) => unreachable!("section type checked"),
                }
            }
            elf::SHT_RELA => {
                check_stride::<Elf::Rela>(
                    section.sh_size(endian).into(),
                    section.sh_entsize(endian).into(),
                    &context,
                    sink,
                    meter,
                )?;
                let mips64el = header.e_machine(endian) == elf::EM_MIPS
                    && header.is_type_64()
                    && endian.is_little_endian();
                match section.rela(endian, data) {
                    Ok(Some((entries, table))) => {
                        for (ordinal, entry) in entries.iter().enumerate() {
                            let mut position = meter.position();
                            position.entry = Some(ordinal as u64);
                            meter.set_position(position);
                            meter.checkpoint(1)?;
                            relocation::<Elf>(
                                sink,
                                meter,
                                &sections,
                                endian,
                                data,
                                &RelocationRecord {
                                    section: index.0 as u32,
                                    index: ordinal as u64,
                                    offset: entry.r_offset(endian).into(),
                                    relocation_type: entry.r_type(endian, mips64el),
                                    symbol_table_section: table.0 as u32,
                                    symbol_index: entry.r_sym(endian, mips64el),
                                    addend: Some(entry.r_addend(endian).into()),
                                },
                            )?;
                        }
                    }
                    Err(error) => emit(
                        sink,
                        meter,
                        &diagnostic(DiagnosticCode::MalformedTable, &context, error),
                    )?,
                    Ok(None) => unreachable!("section type checked"),
                }
            }
            elf::SHT_RELR => emit(
                sink,
                meter,
                &diagnostic(
                    DiagnosticCode::UnsupportedFormat,
                    &context,
                    "packed RELR relocation decoding is not implemented; section bytes retained",
                ),
            )?,
            _ => {}
        }
    }
    Ok(result)
}

fn check_stride<T>(
    size: u64,
    stride: u64,
    context: &str,
    sink: &mut dyn ElfSink,
    meter: &Meter<'_>,
) -> Result<()> {
    let expected = std::mem::size_of::<T>() as u64;
    if stride != expected || !size.is_multiple_of(expected) {
        emit(
            sink,
            meter,
            &diagnostic(
                DiagnosticCode::MalformedTable,
                context,
                "invalid table entry size or trailing bytes",
            ),
        )?;
    }
    Ok(())
}
