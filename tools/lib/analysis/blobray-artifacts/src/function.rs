//! Borrowed ELF function views; no linker or ISA semantics.
use object::{Object, ObjectSection, ObjectSymbol, ObjectSymbolTable};
use oer_tool_analysis_blobray_domain::*;
use oer_tool_analysis_riscv_model::*;

pub struct FunctionView<'a> {
    pub abi: RiscvAbi,
    pub address_space: CodeAddressSpace,
    pub image: Option<&'a dyn ImageMemory>,
    pub section: u32,
    pub extent: CodeRange,
    pub code: &'a [u8],
    pub relocations: &'a [FunctionRelocation],
    pub data_ranges: &'a [CodeRange],
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn parse(error: object::Error) -> Error {
    Error::new(ErrorCode::Integrity, error.to_string())
}
pub struct PreparedObject<'a, 'm> {
    /// The verified bytes the views below read.
    bytes: &'a [u8],
    file: object::File<'a>,
    /// The same bytes through the repository's ELF view, for the static
    /// image and mapping-symbol readers of `oer-tool-analysis-riscv-program`.
    elf: oer_tool_analysis_elf::Elf<'a>,
    occurrence: Option<ObjectId>,
    image: Option<oer_tool_analysis_riscv_program::ProgramView<'a>>,
    abi: RiscvAbi,
    memory: &'m WorkingMemory,
    targets: Vec<Option<std::sync::Arc<ReferenceTarget>>>,
    target_capacity: Vec<MemoryReservation<'m>>,
    sections: Vec<Option<PreparedSection<'m>>>,
    _capacity: MemoryReservation<'m>,
}
struct PreparedSection<'a> {
    relocations: Vec<FunctionRelocation>,
    mappings: oer_tool_analysis_riscv_program::mapping::DataRanges<'a>,
    _capacity: MemoryReservation<'a>,
}
fn reserved<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "object metadata allocation refused",
        )
    })?;
    Ok(v)
}
/// Own captured bytes in this callback scope; parsed views cannot escape it.
/// A linked image's sections inside `read_only` hold constants whatever
/// their flags say ([`ReadOnlyWindow`]).
pub fn with_prepared_object<T>(
    source: &dyn ByteSource,
    payload: &ArtifactId,
    read_only: &[ReadOnlyWindow],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: impl FnOnce(&mut PreparedObject<'_, '_>, &mut dyn RunControl) -> Result<T>,
) -> Result<T> {
    control.phase(RunPhase::PrepareObject)?;
    let size =
        usize::try_from(source.len()).map_err(|_| invalid("object exceeds host address space"))?;
    let mut bytes = memory.bytes(size, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    let actual = ArtifactId::of_bytes_controlled(&bytes, control)?;
    if actual != *payload {
        return Err(Error::new(
            ErrorCode::Integrity,
            "function payload differs from captured inventory",
        ));
    }
    let file = object::File::parse(&*bytes).map_err(parse)?;
    let view = oer_tool_analysis_elf::Elf::parse(&bytes)
        .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?;
    if file.is_64()
        || !file.is_little_endian()
        || file.architecture() != object::Architecture::Riscv32
        || !matches!(
            file.kind(),
            object::ObjectKind::Relocatable | object::ObjectKind::Executable
        )
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "function analysis requires little-endian RV32 ET_REL or ET_EXEC",
        ));
    }
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    use object::read::elf::SectionHeader;
    let mut static_tables = 0;
    let mut dynamic_tables = 0;
    for header in elf.elf_section_table().iter() {
        control.checkpoint(1)?;
        match header.sh_type(elf.endian()) {
            object::elf::SHT_SYMTAB => static_tables += 1,
            object::elf::SHT_DYNSYM => dynamic_tables += 1,
            _ => {}
        }
    }
    if static_tables > 1 || dynamic_tables > 1 {
        return Err(invalid("ambiguous physical symbol tables"));
    }
    let image = if file.kind() == object::ObjectKind::Executable {
        Some(oer_tool_analysis_riscv_program::ProgramView::new(
            &bytes, &view, read_only, memory, control,
        )?)
    } else {
        None
    };
    let abi = oer_tool_analysis_riscv_program::abi(&view)
        .map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    let symbols = elf.elf_symbol_table().symbols().len();
    let section_count = elf.elf_section_table().len();
    let _capacity = memory.reserve(
        (symbols as u64)
            .checked_mul(
                (std::mem::size_of::<Option<std::sync::Arc<ReferenceTarget>>>()
                    + 2 * std::mem::size_of::<MemoryReservation<'_>>()) as u64,
            )
            .and_then(|n| {
                n.checked_add(
                    (section_count as u64)
                        .checked_mul(std::mem::size_of::<Option<PreparedSection<'_>>>() as u64)?,
                )
            })
            .ok_or_else(|| invalid("object metadata overflow"))?,
        control.position(),
    )?;
    let mut targets = reserved(symbols)?;
    targets.resize_with(symbols, || None);
    let mut sections = reserved(section_count)?;
    sections.resize_with(section_count, || None);
    let target_capacity = reserved(
        symbols
            .checked_mul(2)
            .ok_or_else(|| invalid("symbol capacity overflow"))?,
    )?;
    let mut object = PreparedObject {
        bytes: &bytes,
        file,
        elf: view,
        occurrence: None,
        image,
        abi,
        memory,
        targets,
        target_capacity,
        sections,
        _capacity,
    };
    consume(&mut object, control)
}
impl<'data> PreparedObject<'data, '_> {
    /// Exact executable bytes for an ISA consumer; no instruction semantics here.
    pub fn executable_bytes(
        &self,
        address: u32,
        length: u8,
        c: &mut dyn RunControl,
    ) -> Result<&[u8]> {
        self.image
            .as_ref()
            .ok_or_else(|| invalid("executable bytes require a linked image"))?
            .executable_bytes(address, length, c)
    }

    /// Validate physical table kind, section and index without a name lookup or
    /// interpreting one table's index in another table.
    fn selected_symbol(
        &self,
        occurrence: &ObjectId,
        symbol: &SymbolId,
    ) -> Result<object::Symbol<'data, '_>> {
        let object::File::Elf32(elf) = &self.file else {
            unreachable!()
        };
        let (physical, table) = match symbol.table {
            SymbolTableKind::Static => (elf.elf_symbol_table().section(), self.file.symbol_table()),
            SymbolTableKind::Dynamic => (
                elf.elf_dynamic_symbol_table().section(),
                self.file.dynamic_symbol_table(),
            ),
        };
        if symbol.object != *occurrence
            || physical.0 == 0
            || symbol.table_section as usize != physical.0
        {
            return Err(invalid(
                "symbol belongs to another object or physical table",
            ));
        }
        table
            .ok_or_else(|| invalid("missing selected symbol table"))?
            .symbol_by_index(object::SymbolIndex(
                usize::try_from(symbol.index).map_err(|_| invalid("symbol index overflow"))?,
            ))
            .map_err(parse)
    }

    fn prepare_section(
        &mut self,
        section_index: object::SectionIndex,
        occurrence: &ObjectId,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let file = &self.file;
        let memory = self.memory;
        let section = file.section_by_index(section_index).map_err(parse)?;
        let object::File::Elf32(elf) = file else {
            unreachable!()
        };
        let table_section = elf.elf_symbol_table().section().0 as u32;
        if self.sections[section_index.0].is_none() {
            control.phase(RunPhase::PrepareSection)?;
            let mut count = 0usize;
            for _ in section.relocations() {
                control.checkpoint(1)?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("relocation count overflow"))?;
            }
            let amount = (count as u64)
                .checked_mul(std::mem::size_of::<FunctionRelocation>() as u64)
                .ok_or_else(|| invalid("metadata capacity overflow"))?;
            let _metadata = memory.reserve(amount, control.position())?;
            let mut relocations = Vec::new();
            relocations.try_reserve_exact(count).map_err(|_| {
                Error::new(ErrorCode::ResourceLimited, "relocation allocation refused")
            })?;
            // Preserve the physical relocation section and record index, not a virtual address.
            let mut relocation_section = None;
            for s in file.sections() {
                control.checkpoint(1)?;
                if let object::SectionFlags::Elf { .. } = s.flags() {
                    // Raw ELF32 sh_info/sh_link preserve table identity without ISA interpretation.
                    let object::File::Elf32(elf) = &file else {
                        unreachable!()
                    };
                    use object::read::elf::SectionHeader;
                    let header = elf.elf_section_table().section(s.index()).map_err(parse)?;
                    let hdr = (
                        header.sh_type(elf.endian()),
                        header.sh_link(elf.endian()),
                        header.sh_info(elf.endian()),
                    );
                    if matches!(hdr.0, object::elf::SHT_RELA | object::elf::SHT_REL)
                        && hdr.2 == section_index.0 as u32
                    {
                        if hdr.1 != table_section {
                            return Err(Error::new(
                                ErrorCode::Incompatible,
                                "relocations using a non-static symbol table require a dynamic relocation profile",
                            ));
                        }
                        if relocation_section
                            .replace((s.index().0 as u32, hdr.0))
                            .is_some()
                        {
                            return Err(Error::new(
                                ErrorCode::Incompatible,
                                "multiple relocation sections for code section",
                            ));
                        }
                    }
                }
            }
            for (index, (offset, relocation)) in section.relocations().enumerate() {
                control.checkpoint(1)?;
                // ELF relocations with r_sym=0 (notably RELAX/ALIGN) are exposed
                // by object as Absolute. Keep the physical null-symbol index.
                let target = match relocation.target() {
                    object::RelocationTarget::Symbol(index) => index,
                    object::RelocationTarget::Absolute => object::SymbolIndex(0),
                    _ => {
                        return Err(Error::new(
                            ErrorCode::Incompatible,
                            "non-symbol ELF relocation target",
                        ));
                    }
                };
                if target.0 >= self.targets.len() {
                    return Err(invalid("relocation target outside symbol table"));
                }
                let target_info = if let Some(existing) = &self.targets[target.0] {
                    existing.clone()
                } else {
                    let charge = memory.reserve(
                        (std::mem::size_of::<ReferenceTarget>()
                            + 2 * std::mem::size_of::<usize>()
                            + 64) as u64,
                        control.position(),
                    )?;
                    let symbol_id = SymbolId {
                        object: occurrence.clone(),
                        table: SymbolTableKind::Static,
                        table_section,
                        index: target.0 as u64,
                    };
                    let target_info = if target.0 == 0 {
                        use object::read::elf::Sym;
                        let zero = elf
                            .elf_symbol_table()
                            .symbols()
                            .first()
                            .ok_or_else(|| invalid("missing ELF null symbol"))?;
                        if zero.st_name(elf.endian()) != 0
                            || zero.st_value(elf.endian()) != 0
                            || zero.st_size(elf.endian()) != 0
                            || zero.st_info() != 0
                            || zero.st_other() != 0
                            || zero.st_shndx(elf.endian()) != 0
                        {
                            return Err(invalid("invalid ELF null symbol"));
                        }
                        ReferenceTarget {
                            symbol: symbol_id,
                            name: Vec::new(),
                            section: None,
                            offset: 0,
                            binding: 0,
                            definition: SymbolDefinition::Null,
                            symbol_type: 0,
                        }
                    } else {
                        let symbol = file.symbol_by_index(target).map_err(parse)?;
                        let name = symbol.name_bytes().map_err(parse)?;
                        if name.len() > 4096 {
                            return Err(Error::new(
                                ErrorCode::ResourceLimited,
                                "reference name exceeds 4096 bytes",
                            ));
                        }
                        let charge = memory.reserve(name.len() as u64, control.position())?;
                        self.target_capacity.push(charge);
                        let mut owned_name = reserved(name.len())?;
                        owned_name.extend_from_slice(name);
                        ReferenceTarget {
                            symbol: symbol_id,
                            name: owned_name,
                            section: symbol.section_index().map(|s| s.0 as u32),
                            offset: symbol.address(),
                            binding: match symbol.flags() {
                                object::SymbolFlags::Elf { st_info, .. } => st_info >> 4,
                                _ => 0,
                            },
                            definition: match symbol.section() {
                                object::SymbolSection::Undefined => SymbolDefinition::Undefined,
                                object::SymbolSection::Common => SymbolDefinition::Common,
                                object::SymbolSection::Absolute => SymbolDefinition::Absolute,
                                object::SymbolSection::Section(_) => SymbolDefinition::Section,
                                _ => SymbolDefinition::Other,
                            },
                            symbol_type: match symbol.flags() {
                                object::SymbolFlags::Elf { st_info, .. } => st_info & 15,
                                _ => 0,
                            },
                        }
                    };
                    let shared = std::sync::Arc::new(target_info);
                    self.targets[target.0] = Some(shared.clone());
                    self.target_capacity.push(charge);
                    shared
                };
                let object::RelocationFlags::Elf { r_type } = relocation.flags() else {
                    return Err(invalid("non-ELF relocation"));
                };
                let (rel_section, kind) =
                    relocation_section.ok_or_else(|| invalid("relocation section missing"))?;
                relocations.push(FunctionRelocation {
                    section: rel_section,
                    index: index as u64,
                    offset,
                    relocation_type: r_type,
                    addend: if kind == object::elf::SHT_RELA {
                        Some(relocation.addend())
                    } else {
                        None
                    },
                    target: target_info,
                });
            }
            control.checkpoint(
                (count as u64).saturating_mul(u64::from(usize::BITS - count.leading_zeros())),
            )?;
            relocations.sort_unstable_by_key(|r| (r.offset, r.section, r.index));
            let mappings = oer_tool_analysis_riscv_program::mapping::data_ranges(
                &self.elf,
                section_index.0,
                memory,
                control,
            )?;
            self.sections[section_index.0] = Some(PreparedSection {
                relocations,
                mappings,
                _capacity: _metadata,
            });
        }
        Ok(())
    }

    /// The object's bytes, verified against its payload identity, for
    /// readers of further facts of the same file.
    pub fn bytes(&self) -> &'data [u8] {
        self.bytes
    }

    /// The function `symbol` defines, over its declared size.
    pub fn with_function<T>(
        &mut self,
        symbol: &SymbolId,
        control: &mut dyn RunControl,
        consume: impl FnOnce(FunctionView<'_>, &mut dyn RunControl) -> Result<T>,
    ) -> Result<T> {
        let occurrence = &symbol.object;
        if self.occurrence.as_ref().is_some_and(|id| id != occurrence) {
            return Err(invalid("prepared object belongs to another occurrence"));
        }
        self.occurrence = Some(occurrence.clone());
        let file = &self.file;
        let symbol = self.selected_symbol(occurrence, symbol)?;
        let section_index = symbol
            .section_index()
            .ok_or_else(|| invalid("function symbol has no defined section"))?;
        if symbol.size() == 0 {
            return Err(Error::new(
                ErrorCode::NeedsExtent,
                "symbol size is unknown, so the function has no extent",
            ));
        }
        let extent = CodeRange {
            start: symbol.address(),
            length: symbol.size(),
        };
        let section = file.section_by_index(section_index).map_err(parse)?;
        if !matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags&u64::from(object::elf::SHF_EXECINSTR)!=0)
        {
            return Err(invalid("function selection is not in executable code"));
        }
        let end = extent
            .start
            .checked_add(extent.length)
            .ok_or_else(|| invalid("function extent overflow"))?;
        if !extent.start.is_multiple_of(2)
            || extent.length == 0
            || extent.start < section.address()
            || section
                .address()
                .checked_add(section.size())
                .is_none_or(|limit| end > limit)
        {
            return Err(invalid(
                "function extent must be aligned and stay in its section",
            ));
        }
        self.prepare_section(section_index, occurrence, control)?;
        let section = self.file.section_by_index(section_index).map_err(parse)?;
        let image = &self.image;
        let data = section.data().map_err(parse)?;
        let code = data
            .get(
                usize::try_from(extent.start - section.address())
                    .map_err(|_| invalid("extent overflow"))?
                    ..usize::try_from(end - section.address())
                        .map_err(|_| invalid("extent overflow"))?,
            )
            .ok_or_else(|| invalid("function has no file-backed bytes"))?;
        if let Some(image) = &image {
            image.code(extent.start, code, control)?;
        }
        let prepared = self.sections[section_index.0].as_ref().unwrap();
        control.phase(RunPhase::AnalyzeFunction)?;
        consume(
            FunctionView {
                abi: self.abi,
                address_space: if image.is_some() {
                    CodeAddressSpace::Image
                } else {
                    CodeAddressSpace::Section
                },
                image: image.as_ref().map(|v| v as &dyn ImageMemory),
                section: section_index.0 as u32,
                extent,
                code,
                relocations: &prepared.relocations,
                data_ranges: &prepared.mappings.ranges,
            },
            control,
        )
    }
}

#[path = "data.rs"]
mod data;
pub use data::DataView;
