//! File-range archive cursor. Names are scoped; payloads are never read here.
use blobray_domain::*;
use oer_riscv_model::*;

fn malformed(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn number(bytes: &[u8]) -> Result<u64> {
    let bytes = bytes.split(|b| *b == b' ').next().unwrap_or_default();
    if bytes.is_empty() {
        return Err(malformed("empty archive number"));
    }
    bytes.iter().try_fold(0u64, |n, b| {
        if !b.is_ascii_digit() {
            return Err(malformed("invalid archive number"));
        }
        n.checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or_else(|| malformed("archive number overflow"))
    })
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| malformed("archive offset overflow"))
}

pub struct MemberRange<'a> {
    pub ordinal: u64,
    pub name: Option<ScratchBytes<'a>>,
    /// External thin payloads have no range in this container.
    pub payload: Option<(u64, u64)>,
}

pub struct MemberCursor<'a> {
    source: &'a dyn ByteSource,
    pub kind: ContainerKind,
    offset: u64,
    ordinal: u64,
    names: Option<(u64, u64)>,
    started: bool,
    prefix: u8,
    aix: Option<(u64, u64)>,
}
impl<'a> MemberCursor<'a> {
    pub fn new(source: &'a dyn ByteSource, control: &mut dyn RunControl) -> Result<Self> {
        control.phase(RunPhase::Members)?;
        let mut magic = [0; 8];
        let size = source.len().min(8) as usize;
        source.read_at(0, &mut magic[..size], control)?;
        let kind = match &magic {
            b"!<arch>\n" | b"<bigaf>\n" => ContainerKind::Archive,
            b"!<thin>\n" => ContainerKind::ThinArchive,
            _ if magic.starts_with(b"\x7fELF") => ContainerKind::Elf,
            _ => ContainerKind::Unsupported,
        };
        let mut result = Self {
            source,
            kind,
            offset: 8,
            ordinal: 0,
            names: None,
            started: false,
            prefix: 0,
            aix: None,
        };
        if &magic == b"<bigaf>\n" {
            let mut header = [0; 128];
            source.read_at(0, &mut header, control)?;
            // AIX big archives enumerate the explicit member index, never links.
            let symbol64 = number(&header[48..68])?;
            let symbol = if symbol64 != 0 {
                symbol64
            } else {
                number(&header[28..48])?
            };
            if symbol != 0 {
                result.aix_header(symbol, control)?;
            }
            let table = number(&header[8..28])?;
            let mut index = (0, 0);
            if table != 0 {
                let (payload, size, _) = result.aix_header(table, control)?;
                SourceRange::new(source, payload, size)?;
                let mut count = [0; 20];
                source.read_at(payload, &mut count, control)?;
                let count = number(&count)?;
                let extent = count
                    .checked_mul(20)
                    .and_then(|n| n.checked_add(20))
                    .ok_or_else(|| malformed("AIX member count overflow"))?;
                if extent > size {
                    return Err(malformed("AIX member index exceeds its payload"));
                }
                index = (add(payload, 20)?, count);
            }
            result.aix = Some(index);
        }
        Ok(result)
    }
    fn aix_header(
        &self,
        offset: u64,
        control: &mut dyn RunControl,
    ) -> Result<(u64, u64, (u64, u64))> {
        let mut header = [0; 112];
        self.source.read_at(offset, &mut header, control)?;
        let size = number(&header[..20])?;
        let name_length = number(&header[108..112])?;
        let name_offset = add(offset, 112)?;
        let end = add(name_offset, name_length)?;
        let terminator = add(end, end & 1)?;
        let mut bytes = [0; 2];
        self.source.read_at(terminator, &mut bytes, control)?;
        if &bytes != b"`\n" {
            return Err(malformed("invalid AIX archive terminator"));
        }
        Ok((add(terminator, 2)?, size, (name_offset, name_length)))
    }
    pub fn next<'m>(
        &mut self,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<MemberRange<'m>>> {
        control.phase(RunPhase::Members)?;
        loop {
            let mut position = control.position();
            position.member = Some(self.ordinal);
            position.table = None;
            position.entry = None;
            control.set_position(position);
            control.checkpoint(1)?;
            if !matches!(
                self.kind,
                ContainerKind::Archive | ContainerKind::ThinArchive
            ) {
                if self.started {
                    return Ok(None);
                }
                self.started = true;
                return Ok(Some(MemberRange {
                    ordinal: 0,
                    name: None,
                    payload: Some((0, self.source.len())),
                }));
            }
            if let Some((index, count)) = self.aix {
                if self.ordinal == count {
                    return Ok(None);
                }
                let mut entry = [0; 20];
                self.source.read_at(
                    add(
                        index,
                        self.ordinal
                            .checked_mul(20)
                            .ok_or_else(|| malformed("AIX index overflow"))?,
                    )?,
                    &mut entry,
                    control,
                )?;
                let (offset, length, (name_offset, name_length)) =
                    self.aix_header(number(&entry)?, control)?;
                let name = self.copy(name_offset, name_length, memory, control)?;
                let ordinal = self.ordinal;
                self.ordinal += 1;
                return Ok(Some(MemberRange {
                    ordinal,
                    name: Some(name),
                    payload: Some((offset, length)),
                }));
            }
            if self.offset >= self.source.len() {
                return Ok(None);
            }
            let mut header = [0; 60];
            self.source.read_at(self.offset, &mut header, control)?;
            if &header[58..] != b"`\n" {
                return Err(malformed("invalid archive terminator"));
            }
            let length = number(&header[48..58])?;
            let data_offset = add(self.offset, 60)?;
            let mut payload = (data_offset, length);
            let raw = &header[..16];
            let name = if raw.starts_with(b"#1/") && raw[3].is_ascii_digit() {
                let count = number(&raw[3..])?;
                payload = (
                    add(data_offset, count)?,
                    length
                        .checked_sub(count)
                        .ok_or_else(|| malformed("BSD name exceeds member"))?,
                );
                SourceRange::new(self.source, data_offset, count)?;
                let mut name_length = count;
                let mut scanned = 0;
                let mut buffer = [0; WORK_BLOCK];
                while scanned < count {
                    let size = (count - scanned).min(WORK_BLOCK as u64) as usize;
                    self.source.read_at(
                        add(data_offset, scanned)?,
                        &mut buffer[..size],
                        control,
                    )?;
                    control.bytes(size)?;
                    if let Some(end) = buffer[..size].iter().position(|b| *b == 0) {
                        name_length = scanned + end as u64;
                        break;
                    }
                    scanned += size as u64;
                }
                self.copy(data_offset, name_length, memory, control)?
            } else if raw[0] == b'/' && raw[1].is_ascii_digit() {
                let (base, size) = self
                    .names
                    .ok_or_else(|| malformed("archive name table missing"))?;
                let start = number(&raw[1..])?;
                if start >= size {
                    return Err(malformed("archive name offset outside table"));
                }
                let mut length = 0;
                let mut buffer = [0; WORK_BLOCK];
                let mut terminator = None;
                while start + length < size {
                    let count = (size - start - length).min(WORK_BLOCK as u64) as usize;
                    self.source.read_at(
                        add(base, start + length)?,
                        &mut buffer[..count],
                        control,
                    )?;
                    control.bytes(count)?;
                    if let Some(end) = buffer[..count].iter().position(|b| *b == b'\n' || *b == 0) {
                        length += end as u64;
                        terminator = Some(buffer[end]);
                        break;
                    }
                    length += count as u64;
                }
                if terminator.is_none() {
                    return Err(malformed("unterminated archive name"));
                }
                if terminator == Some(b'\n') {
                    if length == 0 {
                        return Err(malformed("invalid GNU archive name terminator"));
                    }
                    let mut last = [0];
                    self.source
                        .read_at(add(base, start + length - 1)?, &mut last, control)?;
                    if last[0] != b'/' {
                        return Err(malformed("invalid GNU archive name terminator"));
                    }
                    length -= 1;
                }
                self.copy(add(base, start)?, length, memory, control)?
            } else {
                let end = if raw[0] == b'/' {
                    raw.iter().position(|b| *b == b' ')
                } else {
                    raw.iter()
                        .position(|b| *b == b'/')
                        .or_else(|| raw.iter().position(|b| *b == b' '))
                }
                .unwrap_or(raw.len());
                let mut name = memory.bytes(end, control.position())?;
                name.copy_from_slice(&raw[..end]);
                name
            };
            let special = matches!(&*name, b"/" | b"//" | b"/SYM64/");
            let external = self.kind == ContainerKind::ThinArchive && !special;
            self.offset = if external {
                data_offset
            } else {
                add(add(data_offset, length)?, length & 1)?
            };
            let next_prefix = match (self.prefix, &*name) {
                (0, b"/") => Some(1),
                (1, b"/") => Some(2),
                (0, b"/SYM64/") => Some(4),
                (0 | 1 | 4, b"//") => Some(255),
                (2, b"//") => Some(3),
                (3, b"/<ECSYMBOLS>/") => Some(255),
                (
                    0,
                    b"__.SYMDEF" | b"__.SYMDEF SORTED" | b"__.SYMDEF_64" | b"__.SYMDEF_64 SORTED",
                ) => Some(255),
                _ => None,
            };
            if !self.started
                && let Some(next_prefix) = next_prefix
            {
                self.prefix = next_prefix;
                if &*name == b"//" {
                    SourceRange::new(self.source, data_offset, length)?;
                    self.names = Some((data_offset, length));
                }
                continue;
            }
            self.started = true;
            let ordinal = self.ordinal;
            self.ordinal = self
                .ordinal
                .checked_add(1)
                .ok_or_else(|| malformed("archive ordinal overflow"))?;
            return Ok(Some(MemberRange {
                ordinal,
                name: Some(name),
                payload: (!external).then_some(payload),
            }));
        }
    }
    fn copy<'m>(
        &self,
        offset: u64,
        length: u64,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<ScratchBytes<'m>> {
        read_scratch(
            &SourceRange::new(self.source, offset, length)?,
            memory,
            control,
        )
    }
}

/// One container's ephemeral ordinal index; thin members have no container range.
pub struct MemberIndex<'a> {
    ranges: AdmittedVec<'a, Option<(u64, u64)>>,
    terminal_error: Option<Error>,
}
impl<'a> MemberIndex<'a> {
    pub fn new(
        source: &dyn ByteSource,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut cursor = MemberCursor::new(source, c)?;
        let mut ranges = AdmittedVec::new(memory);
        let mut terminal_error = None;
        loop {
            let member = match cursor.next(memory, c) {
                Ok(Some(member)) => member,
                Ok(None) => break,
                Err(error) if error.code == ErrorCode::Integrity => {
                    terminal_error = Some(error);
                    break;
                }
                Err(error) => return Err(error),
            };
            if member.ordinal != ranges.len() as u64 {
                return Err(malformed("archive ordinal discontinuity"));
            }
            ranges.push(member.payload, c.position())?;
        }
        Ok(Self {
            ranges,
            terminal_error,
        })
    }
    /// A malformed suffix does not erase valid captured occurrences. This index
    /// cannot assert archive completeness: inventory owns that explicit coverage.
    pub fn terminal_error(&self) -> Option<&Error> {
        self.terminal_error.as_ref()
    }
    pub fn get(&self, ordinal: u64) -> Result<Option<(u64, u64)>> {
        self.ranges
            .get(usize::try_from(ordinal).map_err(|_| malformed("member ordinal overflow"))?)
            .copied()
            .ok_or_else(|| {
                self.terminal_error
                    .clone()
                    .unwrap_or_else(|| malformed("captured archive member missing"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Source(Vec<u8>);
    impl ByteSource for Source {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(
            &self,
            offset: u64,
            bytes: &mut [u8],
            control: &mut dyn RunControl,
        ) -> Result<()> {
            control.bytes(bytes.len())?;
            let start = usize::try_from(offset).map_err(|_| malformed("offset overflow"))?;
            let end = start
                .checked_add(bytes.len())
                .ok_or_else(|| malformed("range overflow"))?;
            bytes.copy_from_slice(
                self.0
                    .get(start..end)
                    .ok_or_else(|| malformed("short source"))?,
            );
            Ok(())
        }
    }
    fn member(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut header = [b' '; 60];
        header[..name.len()].copy_from_slice(name);
        let size = format!("{:<10}", data.len());
        header[48..58].copy_from_slice(size.as_bytes());
        header[58..].copy_from_slice(b"`\n");
        let mut result = header.to_vec();
        result.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            result.push(b'\n');
        }
        result
    }
    #[test]
    fn names_and_physical_ranges_match_upstream_for_gnu_coff_and_bsd() {
        let cases = [
            vec![member(b"//", b"long name.o/\n"), member(b"/0", b"data")],
            vec![
                member(b"/", b""),
                member(b"/", b""),
                member(b"//", b"path/\0"),
                member(b"/0", b"data"),
            ],
            vec![member(b"#1/10", b"name\0junk\0payload")],
            vec![member(b"same.o/", b"odd"), member(b"same.o/", b"even")],
        ];
        for pieces in cases {
            let mut data = b"!<arch>\n".to_vec();
            for piece in pieces {
                data.extend(piece);
            }
            let source = Source(data);
            let archive = object::read::archive::ArchiveFile::parse(source.0.as_slice()).unwrap();
            let expected: Vec<_> = archive
                .members()
                .map(|m| {
                    let m = m.unwrap();
                    (m.name().to_vec(), m.file_range())
                })
                .collect();
            let memory = WorkingMemory::new(1024).unwrap();
            let mut control = || Ok(());
            let mut cursor = MemberCursor::new(&source, &mut control).unwrap();
            let mut actual = Vec::new();
            while let Some(member) = cursor.next(&memory, &mut control).unwrap() {
                actual.push((
                    member.name.as_deref().unwrap().to_vec(),
                    member.payload.unwrap(),
                ));
            }
            assert_eq!(actual, expected);
            assert_eq!(memory.used(), 0);
        }
    }
    #[test]
    fn aix_uses_finite_index_and_preserves_upstream_payload_ranges() {
        fn aix_member(name: &[u8], data: &[u8]) -> Vec<u8> {
            let mut header = [b' '; 112];
            header[..20].copy_from_slice(format!("{:<20}", data.len()).as_bytes());
            header[108..112].copy_from_slice(format!("{:<4}", name.len()).as_bytes());
            let mut bytes = header.to_vec();
            bytes.extend(name);
            if !bytes.len().is_multiple_of(2) {
                bytes.push(0);
            }
            bytes.extend(b"`\n");
            bytes.extend(data);
            bytes
        }
        let mut header = [b' '; 128];
        header[..8].copy_from_slice(b"<bigaf>\n");
        for field in header[8..].chunks_mut(20) {
            field.copy_from_slice(format!("{:<20}", 0).as_bytes());
        }
        header[8..28].copy_from_slice(format!("{:<20}", 128).as_bytes());
        let mut index = format!("{:<20}{:<20}", 1, 284).into_bytes();
        index.extend(b"x\0");
        let mut data = header.to_vec();
        data.extend(aix_member(b"", &index));
        data.extend(aix_member(b"x.o", b"data"));
        let source = Source(data);
        let archive = object::read::archive::ArchiveFile::parse(source.0.as_slice()).unwrap();
        let expected = archive.members().next().unwrap().unwrap();
        let memory = WorkingMemory::new(1024).unwrap();
        let mut control = || Ok(());
        let mut cursor = MemberCursor::new(&source, &mut control).unwrap();
        {
            let actual = cursor.next(&memory, &mut control).unwrap().unwrap();
            assert_eq!(actual.name.as_deref().unwrap(), expected.name());
            assert_eq!(actual.payload.unwrap(), expected.file_range());
        }
        assert!(cursor.next(&memory, &mut control).unwrap().is_none());
        assert_eq!(memory.used(), 0);
        let mut damaged = source.0;
        damaged[242..262].copy_from_slice(format!("{:<20}", u64::MAX).as_bytes());
        assert!(matches!(
            MemberCursor::new(&Source(damaged), &mut control),
            Err(Error {
                code: ErrorCode::Integrity,
                ..
            })
        ));
    }
    #[test]
    fn malformed_gnu_name_and_exhausted_name_capacity_are_distinct() {
        for (names, limit, code) in [
            (b"missing slash\n".as_slice(), 100, ErrorCode::Integrity),
            (b"long name/\n".as_slice(), 3, ErrorCode::ResourceLimited),
        ] {
            let mut data = b"!<arch>\n".to_vec();
            data.extend(member(b"//", names));
            data.extend(member(b"/0", b"payload"));
            let source = Source(data);
            let mut control = || Ok(());
            let memory = WorkingMemory::new(limit).unwrap();
            let mut cursor = MemberCursor::new(&source, &mut control).unwrap();
            let error = match cursor.next(&memory, &mut control) {
                Err(error) => error,
                _ => panic!("expected failure"),
            };
            assert_eq!(error.code, code);
            assert_eq!(memory.used(), 0);
        }
    }
}
