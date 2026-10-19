//! Borrowed RV32 static ELF image. No loading, relocation or environment models.

#![forbid(unsafe_code)]
pub mod facts;
pub mod mapping;
pub mod tables;
use oer_analysis_elf::{ProgramHeaderKind, SectionKind, rv32};
use oer_analysis_riscv_model::*;

/// `PF_X` of an [`ImageSegment`]'s raw ELF permissions.
const PF_X: u32 = 1;

/// The memory of a static RV32 image as its sections lay it out: every
/// allocated section with file contents, as a firmware packer loads it. A
/// section's own flags say whether it is code or writable data, whatever the
/// flags of the load segment holding it; a reservation without contents
/// (`NOBITS`) is no memory, so it may overlap code.
pub struct ProgramView<'a> {
    bytes: &'a [u8],
    /// Loaded sections, by address; none overlaps another.
    regions: Vec<Region>,
    _capacity: MemoryReservation<'a>,
}
/// One loaded section of a [`ProgramView`].
#[derive(Clone, Copy)]
struct Region {
    address: u64,
    file_offset: u64,
    size: u64,
    executable: bool,
    writable: bool,
    /// Whether the section holds constants: neither executable nor
    /// writable, or writable inside a read-only window covering all of it.
    constant: bool,
}
impl Region {
    /// Whether `[address, address + length)` lies inside this section.
    fn holds(&self, address: u64, length: u64) -> bool {
        address >= self.address
            && address
                .checked_add(length)
                .is_some_and(|end| end <= self.address + self.size)
    }
}
fn bad(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::LinkBlocked, message)
}
/// The ELF-declared floating-point calling convention of an RV32 image.
pub fn abi(file: &oer_analysis_elf::Elf<'_>) -> Result<RiscvAbi> {
    let flags = rv32::header_flags(file.header_flags());
    if flags.rve {
        return Err(bad("RV32E is outside the RV32 integer-register profile"));
    }
    match flags.float_abi {
        rv32::FloatAbi::Soft => Ok(RiscvAbi::Ilp32),
        rv32::FloatAbi::Single => Ok(RiscvAbi::Ilp32f),
        rv32::FloatAbi::Double => Ok(RiscvAbi::Ilp32d),
        rv32::FloatAbi::Quad => Err(bad("unsupported ELF floating-point ABI")),
    }
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
impl<'a> ProgramView<'a> {
    pub fn executable_bytes(
        &self,
        address: u32,
        length: u8,
        c: &mut dyn RunControl,
    ) -> Result<&'a [u8]> {
        if !matches!(length, 2 | 4) || address & 1 != 0 {
            return Err(invalid("invalid instruction prefix"));
        }
        let region = self
            .region(u64::from(address), u64::from(length), c)?
            .filter(|region| region.executable)
            .ok_or_else(|| invalid("instruction outside executable captured mapping"))?;
        let start = (region.file_offset + u64::from(address) - region.address) as usize;
        self.bytes
            .get(start..start + usize::from(length))
            .ok_or_else(|| invalid("instruction outside captured image"))
    }

    /// The loaded section holding all of `[address, address + length)`.
    fn region(&self, address: u64, length: u64, c: &mut dyn RunControl) -> Result<Option<Region>> {
        c.checkpoint(1)?;
        let at = self
            .regions
            .partition_point(|region| region.address <= address);
        Ok(at
            .checked_sub(1)
            .map(|at| self.regions[at])
            .filter(|region| region.holds(address, length)))
    }

    /// The image of `bytes` that `file` parses, with the caller's
    /// `read_only` windows: a writable section all of whose bytes lie in one
    /// holds constants. Dynamic loading, runtime relocations and
    /// writable code are outside its profile; thread-local templates are
    /// ordinary data, thread-local storage itself is never memory.
    pub fn new(
        bytes: &'a [u8],
        file: &oer_analysis_elf::Elf<'_>,
        read_only: &[ReadOnlyWindow],
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        for header in file.program_headers() {
            c.checkpoint(1)?;
            if matches!(
                header.kind,
                ProgramHeaderKind::Dynamic | ProgramHeaderKind::Interpreter
            ) {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading is outside the static image profile",
                ));
            }
        }
        abi(file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
        let sections = file.sections().count();
        let capacity = memory.reserve(
            (sections as u64).saturating_mul(std::mem::size_of::<Region>() as u64),
            c.position(),
        )?;
        let mut regions: Vec<Region> = Vec::new();
        regions.try_reserve_exact(sections).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "image region allocation refused",
            )
        })?;
        for section in file.sections() {
            c.checkpoint(1)?;
            if section.kind == SectionKind::Dynamic {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading is outside the static image profile",
                ));
            }
            if section.allocated && matches!(section.kind, SectionKind::Rel | SectionKind::Rela) {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "runtime relocation sections are outside the static image profile",
                ));
            }
            if !section.allocated || section.nobits || section.size == 0 {
                continue;
            }
            if section
                .file_offset
                .checked_add(section.size)
                .is_none_or(|end| end > bytes.len() as u64)
                || section
                    .address
                    .checked_add(section.size)
                    .is_none_or(|end| end > 1u64 << 32)
            {
                return Err(invalid("invalid section bounds"));
            }
            if section.writable && section.executable {
                return Err(invalid("writable code"));
            }
            regions.push(Region {
                address: section.address,
                file_offset: section.file_offset,
                size: section.size,
                executable: section.executable,
                writable: section.writable,
                constant: !section.executable
                    && (!section.writable
                        || ReadOnlyWindow::cover(read_only, section.address, section.size)),
            });
        }
        if regions.is_empty() {
            return Err(invalid("image has no loaded sections"));
        }
        // A section is analyzed only as the bytes a load segment carries at
        // its address, and code only from an executable segment, whatever
        // other segments overlap it.
        let segments: Vec<_> = file
            .program_headers()
            .into_iter()
            .filter(|header| header.kind == ProgramHeaderKind::Load)
            .collect();
        for region in &regions {
            let loaded = segments.iter().any(|segment| {
                region.address >= segment.address
                    && region.address + region.size <= segment.address + segment.file_size
                    && segment
                        .file_offset
                        .checked_add(region.address - segment.address)
                        == Some(region.file_offset)
                    && (!region.executable || segment.flags & PF_X != 0)
            });
            c.checkpoint(segments.len() as u64)?;
            if !loaded {
                return Err(invalid("section is not the bytes a load segment carries"));
            }
        }
        regions.sort_unstable_by_key(|region| region.address);
        for pair in regions.windows(2) {
            c.checkpoint(1)?;
            if pair[1].address < pair[0].address + pair[0].size {
                return Err(invalid("overlapping loaded sections"));
            }
        }
        Ok(Self {
            bytes,
            regions,
            _capacity: capacity,
        })
    }
    /// Verify that a section's captured bytes are the file-backed bytes
    /// loaded at its VMA; whether they may change at run time.
    pub fn data_range(
        &self,
        address: u64,
        file_offset: u64,
        length: u64,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        let region = self
            .region(address, length.max(1), c)?
            .ok_or_else(|| invalid("data range has no loaded section"))?;
        if region.file_offset.checked_add(address - region.address) != Some(file_offset) {
            return Err(invalid("section data differs from its loaded bytes"));
        }
        Ok(!region.constant && region.writable)
    }
    pub fn code(&self, address: u64, code: &[u8], c: &mut dyn RunControl) -> Result<()> {
        let region = self
            .region(address, code.len() as u64, c)?
            .filter(|region| region.executable)
            .ok_or_else(|| invalid("function has no executable file-backed load range"))?;
        c.checkpoint(code.len() as u64)?;
        let start = (region.file_offset + address - region.address) as usize;
        if self.bytes[start..start + code.len()] == *code {
            return Ok(());
        }
        Err(invalid("section code differs from loaded section bytes"))
    }
}
impl ImageMemory for ProgramView<'_> {
    fn read_constant(
        &self,
        address: u32,
        bytes: &mut [u8],
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        let (address, length) = (u64::from(address), bytes.len() as u64);
        // Only data that is not written at run time holds constants: a
        // section neither writable nor executable, or a writable one a
        // window the caller asserts read-only covers whole. Code loaded into
        // RAM may carry tables the program rewrites, such as a vector table
        // in an `AX` section, whose file bytes are only the initial contents.
        let Some(region) = self
            .region(address, length, c)?
            .filter(|region| region.constant)
        else {
            return Ok(false);
        };
        let start = (region.file_offset + address - region.address) as usize;
        let length = bytes.len();
        for (to, from) in bytes
            .chunks_mut(4096)
            .zip(self.bytes[start..start + length].chunks(4096))
        {
            c.checkpoint(1)?;
            to.copy_from_slice(from);
        }
        Ok(true)
    }
}

/// Lend validated static RV32 segments to an admitted session loader. No relocation.
pub fn execution_segments(
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: &mut impl FnMut(&ImageSegment, &[u8], &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = oer_analysis_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("execution requires static little-endian RV32 ELF"))?;
    let (segments, _capacity) = execution_profile(&bytes, &file, memory, control)?;
    let boot = boot_data(&file, &bytes, control)?;
    for segment in &segments {
        control.checkpoint(1)?;
        if segment.memory_size == 0 {
            continue;
        }
        let file_bytes = &bytes
            [segment.file_offset as usize..(segment.file_offset + segment.file_size) as usize];
        let zero_start = segment.address + segment.file_size;
        let zero_end = segment.address + segment.memory_size;
        let contained: Vec<_> = boot
            .iter()
            .filter(|&&(address, data)| {
                address >= zero_start && address + data.len() as u64 <= zero_end
            })
            .collect();
        if contained.is_empty() {
            consume(segment, file_bytes, control)?;
            continue;
        }
        // The initialized prefix ends at the last boot-data section.
        let end = contained
            .iter()
            .map(|&&(address, data)| address + data.len() as u64 - segment.address)
            .max()
            .unwrap() as usize;
        let mut image = memory.bytes(end, control.position())?;
        image[..file_bytes.len()].copy_from_slice(file_bytes);
        for &&(address, data) in &contained {
            control.checkpoint(1)?;
            let offset = (address - segment.address) as usize;
            image[offset..offset + data.len()].copy_from_slice(data);
        }
        consume(segment, &image, control)?;
    }
    Ok(())
}

/// The load segments of a static RV32 image under the execution profile:
/// no dynamic loading, thread-local storage, runtime relocations, writable
/// code or overlapping segments, since execution loads the segments
/// themselves.
fn execution_profile<'a>(
    bytes: &[u8],
    file: &oer_analysis_elf::Elf<'_>,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<(Vec<ImageSegment>, MemoryReservation<'a>)> {
    let headers = file.program_headers();
    for header in &headers {
        c.checkpoint(1)?;
        if matches!(
            header.kind,
            ProgramHeaderKind::Dynamic | ProgramHeaderKind::Interpreter | ProgramHeaderKind::Tls
        ) {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "dynamic loading and TLS are outside the execution profile",
            ));
        }
    }
    abi(file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
    let capacity = memory.reserve(
        1024 * std::mem::size_of::<ImageSegment>() as u64,
        c.position(),
    )?;
    let mut segments: Vec<ImageSegment> = Vec::new();
    segments.try_reserve_exact(1024).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "image segment allocation refused",
        )
    })?;
    for segment in headers
        .iter()
        .filter(|header| header.kind == ProgramHeaderKind::Load)
    {
        c.checkpoint(1)?;
        if segments.len() == 1024 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "image exceeds 1024 load segments",
            ));
        }
        let (offset, size) = (segment.file_offset, segment.file_size);
        let end = segment
            .address
            .checked_add(segment.memory_size)
            .filter(|v| *v <= 1u64 << 32)
            .ok_or_else(|| invalid("segment exceeds RV32 address space"))?;
        if size > segment.memory_size
            || offset
                .checked_add(size)
                .is_none_or(|v| v > bytes.len() as u64)
            || (segment.writable() && segment.executable())
        {
            return Err(invalid("invalid segment bounds or writable code"));
        }
        for previous in &segments {
            c.checkpoint(1)?;
            if segment.memory_size != 0
                && previous.memory_size != 0
                && segment.address < previous.address + previous.memory_size
                && previous.address < end
            {
                return Err(invalid("overlapping load segments"));
            }
        }
        segments.push(ImageSegment {
            address: segment.address,
            file_offset: offset,
            file_size: size,
            memory_size: segment.memory_size,
            flags: segment.flags,
        });
    }
    if segments.is_empty() {
        return Err(invalid("image has no load segments"));
    }
    for section in file.sections() {
        c.checkpoint(1)?;
        if section.tls || section.kind == SectionKind::Dynamic {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "dynamic loading and TLS are outside the execution profile",
            ));
        }
        if section.allocated && matches!(section.kind, SectionKind::Rel | SectionKind::Rela) {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "runtime relocation sections are outside the execution profile",
            ));
        }
    }
    Ok((segments, capacity))
}

/// Address of the defined code symbol at `index` of the static symbol table
/// in section `table_section` of a static RV32 executable: a text or untyped
/// symbol at an even address inside an executable section.
pub fn code_symbol_at(
    source: &dyn ByteSource,
    table_section: u32,
    index: u64,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<u32> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = oer_analysis_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("code symbols require a static RV32 executable"))?;
    let physical = file
        .symbol_table_section()
        .ok_or_else(|| invalid("executable has no static symbol table"))?;
    if physical != table_section as usize {
        return Err(invalid("symbol belongs to another physical table"));
    }
    // The entry is selected by index; its name is never read.
    let symbol = file
        .raw_symbol_at(usize::try_from(index).map_err(|_| invalid("symbol index overflow"))?)
        .map_err(|_| invalid("symbol index outside the table"))?;
    let section = symbol
        .section
        .and_then(|i| file.section(i).ok())
        .ok_or_else(|| invalid("execution boundary is undefined/absolute"))?;
    let address = symbol.address;
    if !symbol.defined
        || !matches!(
            symbol.kind,
            oer_analysis_elf::SymbolKind::Text | oer_analysis_elf::SymbolKind::Unknown
        )
        || !section.executable
        || address < section.address
        || address - section.address >= section.size
        || address & 1 != 0
    {
        return Err(invalid("execution symbol is outside executable bytes"));
    }
    u32::try_from(address).map_err(|_| invalid("symbol address exceeds RV32"))
}

/// Defined, named code symbols of a static RV32 executable as (address, name),
/// ascending by address and then name. Section and mapping symbols are omitted.
pub fn code_symbols(
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<(u32, String)>> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = oer_analysis_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("code symbols require a static RV32 executable"))?;
    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        control.checkpoint(1)?;
        if !symbol.defined
            || symbol.kind != oer_analysis_elf::SymbolKind::Text
            || symbol.name.is_empty()
            || symbol.name.starts_with('$')
        {
            continue;
        }
        let address =
            u32::try_from(symbol.address).map_err(|_| invalid("symbol address exceeds RV32"))?;
        symbols
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "symbol allocation refused"))?;
        symbols.push((address, symbol.name.to_owned()));
    }
    symbols.sort_unstable();
    symbols.dedup();
    Ok(symbols)
}

/// Boot-initialized data: writable `PROGBITS` sections whose bytes the file
/// carries although their load segment is zero-filled at that address. A ROM
/// copies them there at start-up; execution begins from that state. Sections
/// that overlap with content, or exceed the file, are rejected.
fn boot_data<'b>(
    file: &oer_analysis_elf::Elf<'_>,
    bytes: &'b [u8],
    control: &mut dyn RunControl,
) -> Result<Vec<(u64, &'b [u8])>> {
    let mut result: Vec<(u64, &'b [u8])> = Vec::new();
    for section in file.sections() {
        control.checkpoint(1)?;
        let size = section.size;
        if section.kind != SectionKind::Program
            || !section.writable
            || section.executable
            || section.tls
            || size == 0
        {
            continue;
        }
        let offset = section.file_offset;
        let address = section.address;
        let data = offset
            .checked_add(size)
            .filter(|end| *end <= bytes.len() as u64)
            .and_then(|end| bytes.get(offset as usize..end as usize))
            .ok_or_else(|| invalid("boot data exceeds the file"))?;
        if address.checked_add(size).is_none_or(|end| end > 1u64 << 32) {
            return Err(invalid("boot data exceeds RV32"));
        }
        if result
            .iter()
            .any(|&(start, other)| start < address + size && address < start + other.len() as u64)
        {
            return Err(invalid("boot data sections overlap"));
        }
        result.push((address, data));
    }
    Ok(result)
}

/// Visit every executable section, including code without function symbols.
/// Section-less files fail rather than reporting a vacuous clean result.
pub fn executable_sections(
    bytes: &[u8],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: &mut impl FnMut(ExecutableSectionView<'_>, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let file = oer_analysis_elf::Elf::executable(bytes)
        .map_err(|_| invalid("audit requires static little-endian RV32 ELF"))?;
    let _view = ProgramView::new(bytes, &file, &[], memory, control)?;
    let mut found = false;
    for section in file.sections() {
        control.checkpoint(1)?;
        if !section.executable {
            continue;
        }
        let data = section.data;
        // A section whose file range leaves the file reads as empty; that is
        // a malformed image, never an empty section to skip.
        if !section.nobits
            && section
                .file_offset
                .checked_add(section.size)
                .is_none_or(|end| end > bytes.len() as u64)
        {
            return Err(invalid("invalid executable section bytes"));
        }
        if data.is_empty() {
            continue;
        }
        let address =
            u32::try_from(section.address).map_err(|_| invalid("section address exceeds RV32"))?;
        if section
            .address
            .checked_add(data.len() as u64)
            .is_none_or(|v| v > 1u64 << 32)
            || address & 1 != 0
        {
            return Err(invalid("invalid executable section extent"));
        }
        found = true;
        _view.code(u64::from(address), data, control)?;
        let mappings = crate::mapping::data_ranges(&file, section.index, memory, control)?;
        consume(
            ExecutableSectionView {
                section: section.index as u32,
                address,
                bytes: data,
                data_ranges: &mappings.ranges,
            },
            control,
        )?;
    }
    if !found {
        return Err(invalid("no executable sections: target audit unavailable"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    struct Source(Vec<u8>);
    impl ByteSource for Source {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, bytes: &mut [u8], c: &mut dyn RunControl) -> Result<()> {
            c.bytes(bytes.len())?;
            bytes.copy_from_slice(&self.0[offset as usize..offset as usize + bytes.len()]);
            Ok(())
        }
    }
    fn put16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    /// ELF32 RV32 executable: one RW load at 0x2000 with 16 zero-filled bytes
    /// and a writable PROGBITS section at `section_address` whose four bytes
    /// the file carries at offset 84.
    fn elf(section_address: u32) -> Vec<u8> {
        segment_elf(section_address, 0, 6)
    }
    /// `elf`, its load segment carrying `file_size` bytes from offset 84 and
    /// the ELF permissions `flags`.
    fn segment_elf(section_address: u32, file_size: u32, flags: u32) -> Vec<u8> {
        let mut b = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0];
        b.resize(16, 0);
        put16(&mut b, 2); // ET_EXEC
        put16(&mut b, 243); // EM_RISCV
        put32(&mut b, 1);
        put32(&mut b, 0x2000); // entry
        put32(&mut b, 52); // phoff
        put32(&mut b, 100); // shoff
        put32(&mut b, 0); // flags: soft-float RV32
        put16(&mut b, 52);
        put16(&mut b, 32);
        put16(&mut b, 1);
        put16(&mut b, 40);
        put16(&mut b, 3);
        put16(&mut b, 2);
        // PT_LOAD, 16 bytes of memory.
        for v in [1, 84, 0x2000, 0x2000, file_size, 16, flags, 4] {
            put32(&mut b, v);
        }
        b.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // offset 84
        b.extend_from_slice(b"\0.dat\0.shs\0\0"); // offset 88, 12 bytes
        b.resize(100, 0);
        b.extend_from_slice(&[0; 40]);
        // .dat: PROGBITS, SHF_WRITE | SHF_ALLOC.
        for v in [1, 1, 3, section_address, 84, 4, 0, 0, 4, 0] {
            put32(&mut b, v);
        }
        // .shs: STRTAB.
        for v in [6, 3, 0, 0, 88, 12, 0, 0, 1, 0] {
            put32(&mut b, v);
        }
        b
    }
    fn load(bytes: Vec<u8>) -> Result<Vec<(u64, Vec<u8>)>> {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut result = Vec::new();
        execution_segments(&Source(bytes), &memory, &mut || Ok(()), &mut |s, b, _| {
            result.push((s.address, b.to_vec()));
            Ok(())
        })?;
        Ok(result)
    }
    #[test]
    fn boot_data_initializes_its_zero_filled_load_address() {
        assert_eq!(
            load(elf(0x2008)).unwrap(),
            [(0x2000, vec![0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44])]
        );
        // Outside every zero-filled range, boot data is not mapped at all.
        assert_eq!(load(elf(0x3000)).unwrap(), [(0x2000, vec![])]);
    }
    #[test]
    fn an_executable_section_beyond_the_file_fails_instead_of_being_skipped() {
        // `.dat` becomes `AX` (SHF_ALLOC | SHF_EXECINSTR) at the executable
        // segment's address, its size reaching past the end of the file.
        let mut bytes = segment_elf(0x2000, 4, 5);
        let header = 140;
        bytes[header + 8..header + 12].copy_from_slice(&6u32.to_le_bytes());
        bytes[header + 20..header + 24].copy_from_slice(&0x1000u32.to_le_bytes());
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let error = executable_sections(&bytes, &memory, &mut || Ok(()), &mut |_, _| {
            panic!("a truncated section is never visited")
        })
        .unwrap_err();
        assert_eq!(error.message, "invalid section bounds");
    }
    #[test]
    fn a_code_symbol_resolves_whatever_bytes_its_name_holds() {
        use object::write::{Object, StandardSection, Symbol, SymbolSection};
        use object::{
            Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope,
        };
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0; 8], 4);
        object.add_symbol(Symbol {
            name: b"f\xff".to_vec(),
            value: 4,
            size: 4,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        let mut bytes = object.write().unwrap();
        // The same file as an executable (`e_type` ET_EXEC).
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        let elf = oer_analysis_elf::Elf::executable(&bytes).unwrap();
        let table = elf.symbol_table_section().unwrap() as u32;
        let index = elf
            .raw_symbols()
            .find(|s| s.name == Some(b"f\xff".as_slice()))
            .unwrap()
            .index as u64;
        drop(elf);
        let memory = WorkingMemory::new(1 << 20).unwrap();
        assert_eq!(
            code_symbol_at(&Source(bytes), table, index, &memory, &mut || Ok(())).unwrap(),
            4
        );
    }
    /// `PF_W` and `PF_R` of a segment's raw ELF permissions.
    pub(crate) const PF_W: u32 = 2;
    pub(crate) const PF_R: u32 = 4;
    /// `SHF_WRITE`, `SHF_ALLOC`, `SHF_EXECINSTR` and `SHF_TLS`.
    pub(crate) const W: u32 = 1;
    pub(crate) const A: u32 = 2;
    pub(crate) const X: u32 = 4;
    pub(crate) const T: u32 = 0x400;
    /// One load segment: address, file offset, file size, memory size and
    /// flags.
    pub(crate) type Load = (u32, u32, u32, u32, u32);
    /// One section: type, flags, address, file offset and size.
    pub(crate) type Part = (u32, u32, u32, u32, u32);
    /// An ELF32 RV32 executable of `payload` from file offset 0x200, with
    /// `loads` and a `PT_TLS` header when `tls`, and `parts` (offsets
    /// relative to the payload) after a null section and before a string
    /// table.
    pub(crate) fn image(
        loads: &[Load],
        tls: Option<Load>,
        parts: &[Part],
        payload: &[u8],
    ) -> Vec<u8> {
        const BASE: u32 = 0x200;
        let headers = loads.len() + usize::from(tls.is_some());
        let mut b = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0];
        b.resize(16, 0);
        put16(&mut b, 2); // ET_EXEC
        put16(&mut b, 243); // EM_RISCV
        put32(&mut b, 1);
        put32(&mut b, loads[0].0); // entry
        put32(&mut b, 52); // phoff
        let shoff = BASE + payload.len() as u32;
        put32(&mut b, shoff);
        put32(&mut b, 0); // flags: soft-float RV32
        put16(&mut b, 52);
        put16(&mut b, 32);
        put16(&mut b, headers as u16);
        put16(&mut b, 40);
        put16(&mut b, parts.len() as u16 + 2);
        put16(&mut b, parts.len() as u16 + 1);
        let segments = loads
            .iter()
            .map(|&load| (1, load))
            .chain(tls.map(|load| (7, load)));
        for (kind, (address, offset, file, memory, flags)) in segments {
            for v in [
                kind,
                BASE + offset,
                address,
                address,
                file,
                memory,
                flags,
                4,
            ] {
                put32(&mut b, v);
            }
        }
        b.resize(BASE as usize, 0);
        b.extend_from_slice(payload);
        b.extend_from_slice(&[0; 40]);
        for &(kind, flags, address, offset, size) in parts {
            for v in [0, kind, flags, address, BASE + offset, size, 0, 0, 4, 0] {
                put32(&mut b, v);
            }
        }
        let names = b.len() as u32 + 40;
        for v in [0, 3, 0, 0, names, 1, 0, 0, 1, 0] {
            put32(&mut b, v);
        }
        b.push(0);
        b
    }
    fn view(bytes: &[u8], read_only: &[ReadOnlyWindow]) -> Result<Option<[u8; 4]>> {
        let file = oer_analysis_elf::Elf::parse(bytes).unwrap();
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let view = ProgramView::new(bytes, &file, read_only, &memory, &mut || Ok(()))?;
        let mut word = [0; 4];
        let known = view.read_constant(0x3000, &mut word, &mut || Ok(()))?;
        Ok(known.then_some(word))
    }
    const WORD: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
    #[test]
    fn constants_come_only_from_sections_neither_writable_nor_executable() {
        let constant = |flags, segment, read_only: &[ReadOnlyWindow]| {
            let bytes = image(
                &[(0x3000, 0, 4, 4, segment)],
                None,
                &[(1, flags, 0x3000, 0, 4)],
                &WORD,
            );
            view(&bytes, read_only).unwrap()
        };
        let (r, w, x) = (PF_R, PF_W, PF_X);
        assert_eq!(constant(A, r, &[]), Some(WORD));
        // The section's flags decide, whatever its segment's flags say.
        assert_eq!(constant(A, r | w, &[]), Some(WORD));
        assert_eq!(constant(A | W, r, &[]), None);
        // A vector table in RAM code: the program rewrites what the file
        // carries as its initial contents.
        assert_eq!(constant(A | X, r | x, &[]), None);
        // A writable section inside a window the caller asserts read-only.
        let window = ReadOnlyWindow {
            start: 0x3000,
            length: 0x1000,
        };
        assert_eq!(constant(A | W, r | w, &[window]), Some(WORD));
        let short = ReadOnlyWindow {
            start: 0x3000,
            length: 2,
        };
        assert_eq!(constant(A | W, r | w, &[short]), None);
        assert_eq!(constant(A | X, r | x, &[window]), None);
        // A window covering only the word read, not the whole section,
        // leaves the section writable, as `ImageFacts` reads it.
        let wide = image(
            &[(0x3000, 0, 8, 8, r | w)],
            None,
            &[(1, A | W, 0x3000, 0, 8)],
            &[0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0],
        );
        let word = ReadOnlyWindow {
            start: 0x3000,
            length: 4,
        };
        assert_eq!(view(&wide, &[word]).unwrap(), None);
    }
    #[test]
    fn an_esp_idf_layout_loads_by_its_sections() {
        // Code at 0x2000 and data at 0x3000 in one RWE segment; flash code
        // at 0x4000 and a rodata segment from the same address, whose
        // reservation (NOBITS) covers the code; a thread-local template and
        // its zero-initialized part.
        let mut payload = vec![0; 0x1110];
        let mut put = |at: usize, word: [u8; 4]| payload[at..at + 4].copy_from_slice(&word);
        put(0, [0x13, 0, 0, 0]); // nop at 0x2000
        put(0x1000, WORD); // .data at 0x3000
        put(0x1004, [0x13, 0, 0, 0]); // flash nop at 0x4000
        put(0x1108, WORD); // .rodata (WA) at 0x4100
        put(0x110c, [1, 0, 0, 0]); // .tdata at 0x4104
        let bytes = image(
            &[
                (0x2000, 0, 0x1004, 0x1004, PF_R | PF_W | PF_X),
                (0x4000, 0x1004, 4, 4, PF_R | PF_X),
                (0x4000, 0x1008, 0x108, 0x10c, PF_R | PF_W),
            ],
            Some((0x4104, 0x110c, 4, 8, PF_R)),
            &[
                (1, A | X, 0x2000, 0, 4),
                (1, A | W, 0x3000, 0x1000, 4),
                (1, A | X, 0x4000, 0x1004, 4),
                (8, A | W, 0x4000, 0x1008, 0x100),
                (1, A | W, 0x4100, 0x1108, 4),
                (1, A | W | T, 0x4104, 0x110c, 4),
                (8, A | W | T, 0x4108, 0x1110, 4),
            ],
            &payload,
        );
        // The RWE segment's data loads, but stays writable.
        assert_eq!(view(&bytes, &[]).unwrap(), None);
        let file = oer_analysis_elf::Elf::parse(bytes.as_slice()).unwrap();
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let window = [ReadOnlyWindow {
            start: 0x4000,
            length: 0x1000,
        }];
        let view = ProgramView::new(&bytes, &file, &window, &memory, &mut || Ok(())).unwrap();
        let fetch = |at| {
            view.executable_bytes(at, 4, &mut || Ok(()))
                .map(<[u8]>::to_vec)
        };
        assert_eq!(fetch(0x2000).unwrap(), [0x13, 0, 0, 0]);
        assert_eq!(fetch(0x4000).unwrap(), [0x13, 0, 0, 0]);
        let mut word = [0; 4];
        assert!(
            view.read_constant(0x4100, &mut word, &mut || Ok(()))
                .unwrap()
        );
        assert_eq!(word, WORD);
        // Thread-local storage is never memory.
        assert!(
            !view
                .read_constant(0x4108, &mut word, &mut || Ok(()))
                .unwrap()
        );
    }
    #[test]
    fn a_section_must_be_the_bytes_a_load_segment_carries() {
        // Data whose segment carries other file bytes.
        let moved = image(
            &[(0x3000, 4, 4, 4, PF_R)],
            None,
            &[(1, A, 0x3000, 0, 4)],
            &[0; 8],
        );
        assert_eq!(
            view(&moved, &[]).unwrap_err().message,
            "section is not the bytes a load segment carries"
        );
        // Code in a segment that does not execute.
        let code = image(
            &[(0x3000, 0, 4, 4, PF_R)],
            None,
            &[(1, A | X, 0x3000, 0, 4)],
            &WORD,
        );
        assert_eq!(
            view(&code, &[]).unwrap_err().message,
            "section is not the bytes a load segment carries"
        );
        // Two loaded sections at one address.
        let overlap = image(
            &[(0x3000, 0, 8, 8, PF_R)],
            None,
            &[(1, A, 0x3000, 0, 8), (1, A, 0x3004, 4, 4)],
            &[0; 8],
        );
        assert_eq!(
            view(&overlap, &[]).unwrap_err().message,
            "overlapping loaded sections"
        );
        let writable_code = image(
            &[(0x3000, 0, 4, 4, PF_R | PF_W | PF_X)],
            None,
            &[(1, A | W | X, 0x3000, 0, 4)],
            &WORD,
        );
        assert_eq!(
            view(&writable_code, &[]).unwrap_err().message,
            "writable code"
        );
    }
    #[test]
    fn execution_keeps_refusing_thread_local_storage() {
        let bytes = image(
            &[(0x2000, 0, 4, 4, PF_R | PF_X)],
            Some((0x3000, 4, 4, 4, PF_R)),
            &[(1, A | X, 0x2000, 0, 4)],
            &[0x13, 0, 0, 0, 1, 0, 0, 0],
        );
        assert_eq!(
            load(bytes).unwrap_err().message,
            "dynamic loading and TLS are outside the execution profile"
        );
    }
}
