//! Strict RV32 link-input and prepared-image checks over admitted captured bytes.
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;

#[derive(Clone, Debug)]
pub struct LinkRootFacts {
    pub symbol: SymbolId,
    pub name: Vec<u8>,
    pub section: Vec<u8>,
    pub section_size: u64,
    pub offset: u64,
    pub size: u64,
}
fn bad(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::LinkBlocked, message)
}
pub struct ValidatedImage {
    pub segments: Vec<ImageSegment>,
    pub abi: RiscvAbi,
}
fn load<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ScratchBytes<'a>> {
    let size = usize::try_from(source.len()).map_err(|_| bad("ELF exceeds host address space"))?;
    let mut bytes = memory.bytes(size, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    Ok(bytes)
}
fn check(bytes: &[u8], file: &object::File<'_>, kind: object::ObjectKind) -> Result<RiscvAbi> {
    if file.is_64()
        || !file.is_little_endian()
        || file.architecture() != object::Architecture::Riscv32
        || file.kind() != kind
    {
        return Err(bad("requires little-endian RV32 ELF of the declared kind"));
    }
    oer_analysis_riscv_program::abi(
        &oer_analysis_elf::Elf::parse(bytes).map_err(|e| bad(e.to_string()))?,
    )
}
pub fn inspect_link_input(
    source: &dyn ByteSource,
    expected: &ArtifactId,
    roots: &[SymbolId],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<LinkRootFacts>> {
    let bytes = load(source, memory, control)?;
    if ArtifactId::of_bytes_controlled(&bytes, control)? != *expected {
        return Err(Error::new(
            ErrorCode::Integrity,
            "member payload differs from captured inventory",
        ));
    }
    let file = object::File::parse(&*bytes).map_err(|e| bad(e.to_string()))?;
    check(&bytes, &file, object::ObjectKind::Relocatable)?;
    for section in file.sections() {
        control.checkpoint(1)?;
        if section
            .name_bytes()
            .map_err(|e| bad(e.to_string()))?
            .iter()
            .any(|b| matches!(b, b'\n' | b'\r'))
        {
            return Err(bad(
                "section name cannot be represented safely in linker map evidence",
            ));
        }
        if matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags & u64::from(object::elf::SHF_TLS)!=0)
        {
            return Err(bad("TLS is outside the synthetic static-image profile"));
        }
    }
    for symbol in file.symbols() {
        control.checkpoint(1)?;
        if symbol
            .name_bytes()
            .map_err(|e| bad(e.to_string()))?
            .iter()
            .any(|b| matches!(b, b'\n' | b'\r'))
        {
            return Err(bad(
                "symbol name cannot be represented safely in linker map evidence",
            ));
        }
    }
    let mut result = Vec::new();
    for selected in roots {
        control.checkpoint(1)?;
        if selected.table != SymbolTableKind::Static {
            return Err(bad("root must reference the static symbol table"));
        }
        let table = file
            .section_by_index(object::SectionIndex(selected.table_section as usize))
            .map_err(|e| bad(e.to_string()))?;
        // Object's index API uses the regular static table; reject alternative tables.
        let mut tables = 0;
        for section in file.sections() {
            control.checkpoint(1)?;
            if section.name_bytes().ok() == Some(b".symtab".as_slice()) {
                tables += 1;
            }
        }
        if table.name_bytes().map_err(|e| bad(e.to_string()))? != b".symtab" || tables != 1 {
            return Err(bad("root needs one unambiguous static .symtab"));
        }
        let symbol = file
            .symbol_by_index(object::SymbolIndex(
                usize::try_from(selected.index).map_err(|_| bad("symbol index overflow"))?,
            ))
            .map_err(|e| bad(e.to_string()))?;
        let name = symbol.name_bytes().map_err(|e| bad(e.to_string()))?;
        if name.is_empty()
            || name.len() > 4096
            || name.contains(&0)
            || !symbol.is_global()
            || symbol.is_undefined()
        {
            return Err(bad(
                "root must be a named defined global/weak symbol (at most 4096 bytes)",
            ));
        }
        let section = file
            .section_by_index(
                symbol
                    .section_index()
                    .ok_or_else(|| bad("root has no code section"))?,
            )
            .map_err(|e| bad(e.to_string()))?;
        if !matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags & u64::from(object::elf::SHF_EXECINSTR)!=0)
            || symbol.address() >= section.size()
            || symbol
                .address()
                .checked_add(symbol.size())
                .is_none_or(|end| end > section.size())
        {
            return Err(bad("root is outside its executable section"));
        }
        let section_name = section.name_bytes().map_err(|e| bad(e.to_string()))?;
        let mut same_name = 0;
        for other in file.sections() {
            control.checkpoint(1)?;
            if other.name_bytes().ok() == Some(section_name) {
                same_name += 1;
            }
        }
        if section_name.len() > 4096
            || section_name
                .iter()
                .any(|b| matches!(b, b'\n' | b'\r' | b'\t' | b'(' | b')'))
            || same_name != 1
        {
            return Err(bad(
                "root section cannot be identified unambiguously in the linker map",
            ));
        }
        result.push(LinkRootFacts {
            symbol: selected.clone(),
            name: name.into(),
            section: section_name.into(),
            section_size: section.size(),
            offset: symbol.address(),
            size: symbol.size(),
        });
    }
    Ok(result)
}
pub fn validate_image(
    source: &dyn ByteSource,
    layout: &ImageLayout,
    roots: &[ResolvedRoot],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ValidatedImage> {
    control.phase(RunPhase::ValidateImage)?;
    layout.validate()?;
    let bytes = load(source, memory, control)?;
    let file = object::File::parse(&*bytes).map_err(|e| bad(e.to_string()))?;
    let abi = check(&bytes, &file, object::ObjectKind::Executable)?;
    let mut segments = Vec::new();
    for segment in file.segments() {
        control.checkpoint(1)?;
        if segments.len() == 1024 {
            return Err(bad("image exceeds 1024 load segments"));
        }
        let (offset, size) = segment.file_range();
        let object::SegmentFlags::Elf { p_flags } = segment.flags() else {
            return Err(bad("non-ELF segment flags"));
        };
        let region = if p_flags & object::elf::PF_W != 0 {
            if p_flags & object::elf::PF_X != 0 {
                return Err(bad("writable executable segment"));
            }
            layout.data
        } else {
            layout.code
        };
        // GNU retains empty PHDRS at address zero when a layout region has no
        // sections. Preserve the record; an empty extent occupies no memory.
        if (segment.size() != 0 && !region.contains(segment.address(), segment.size()))
            || size > segment.size()
            || offset
                .checked_add(size)
                .is_none_or(|end| end > source.len())
        {
            return Err(bad("load segment exceeds declared memory/file bounds"));
        }
        if segments.iter().any(|s: &ImageSegment| {
            segment.size() != 0
                && s.memory_size != 0
                && segment.address() < s.address + s.memory_size
                && s.address < segment.address() + segment.size()
        }) {
            return Err(bad("load segments overlap"));
        }
        segments.push(ImageSegment {
            address: segment.address(),
            file_offset: offset,
            file_size: size,
            memory_size: segment.size(),
            flags: p_flags,
        });
    }
    for section in file.sections() {
        control.checkpoint(1)?;
        let allocated = matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags & u64::from(object::elf::SHF_ALLOC)!=0);
        if !allocated {
            continue;
        }
        if section.size() > 0
            && !segments.iter().any(|s| {
                section.address() >= s.address
                    && section
                        .address()
                        .checked_add(section.size())
                        .is_some_and(|end| end <= s.address + s.memory_size)
            })
        {
            return Err(bad("allocated section has no declared load segment"));
        }
        for (_, relocation) in section.relocations() {
            control.checkpoint(1)?;
            if let object::RelocationTarget::Symbol(index) = relocation.target() {
                let symbol = file
                    .symbol_by_index(index)
                    .map_err(|e| bad(e.to_string()))?;
                if symbol.is_undefined() && index.0 != 0 {
                    return Err(bad("unresolved relocation remains in an allocated section"));
                }
            }
        }
    }
    for (index, root) in roots.iter().enumerate() {
        control.checkpoint(1)?;
        if index == 0 && file.entry() != root.address {
            return Err(bad("ELF entry differs from selected source occurrence"));
        }
        if !segments.iter().any(|s| {
            s.flags & object::elf::PF_X != 0
                && root.address >= s.address
                && root
                    .address
                    .checked_add(root.size.max(1))
                    .is_some_and(|end| end <= s.address + s.file_size)
        }) {
            return Err(bad("root has no executable file-backed bytes"));
        }
        // LLD localizes hidden definitions. The map proves the occurrence;
        // unrelated local names at other addresses are not competing definitions.
        let mut found = 0;
        for symbol in file.symbols() {
            control.checkpoint(1)?;
            if symbol.name_bytes().ok() == Some(root.name.as_slice()) && !symbol.is_undefined() {
                if symbol.address() == root.address {
                    found += 1;
                } else if symbol.is_global() {
                    return Err(bad(
                        "linker selected a different definition of the requested root",
                    ));
                }
            }
        }
        if found != 1 {
            return Err(bad("root symbol is missing or ambiguous in prepared ELF"));
        }
    }
    if roots.is_empty() {
        return Err(bad("image lacks a verified entry"));
    }
    Ok(ValidatedImage { segments, abi })
}
