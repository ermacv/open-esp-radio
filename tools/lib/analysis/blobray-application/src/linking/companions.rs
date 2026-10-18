//! Explicit companion definitions are captured occurrences, never fabricated
//! implementations.
use super::*;

/// The companion records of one static executable, matched by symbol.
struct Scan<'a> {
    companions: &'a [SymbolId],
    matches: Vec<Option<SymbolRecord>>,
}
impl ElfSink for Scan<'_> {
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        for (i, symbol) in self.companions.iter().enumerate() {
            if symbol == &r.id {
                self.matches[i] = Some(r.clone());
            }
        }
        Ok(())
    }
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}

/// Fail when a link input's symbol `r` defines a companion name. The link's
/// own pass over its inputs applies this, so resolving the companions reads
/// only the companions' executables.
pub(super) fn check_link_symbol(names: &[(String, u32)], r: &SymbolRecord) -> Result<()> {
    if r.raw_section != 0
        && r.binding != 0
        && r.name
            .as_ref()
            .is_some_and(|n| names.iter().any(|(name, _)| name.as_bytes() == n))
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "companion name also defined by a selected link input",
        ));
    }
    Ok(())
}

/// The record of every companion, read once per executable that defines one.
fn records(
    companions: &[SymbolId],
    executables: &[Executable],
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<SymbolRecord>> {
    let mut records = vec![None; companions.len()];
    for (i, companion) in companions.iter().enumerate() {
        if records[i].is_some() || companions[..i].iter().any(|s| s.object == companion.object) {
            continue;
        }
        if companion.object.location != ObjectLocation::Standalone {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "a companion is defined by a static executable, not an archive member",
            ));
        }
        let executable = find(executables, &companion.object.artifact)?;
        let mut scan = Scan {
            companions,
            matches: vec![None; companions.len()],
        };
        let bytes: &[u8] = executable.bytes();
        let (_, header) = inspect_source(&bytes, &companion.object, memory, c, &mut scan)?;
        let executable = header.is_some_and(|e| {
            e.object_type == 2 && e.bits == 32 && e.machine == 243 && e.little_endian
        });
        for (slot, record) in records.iter_mut().zip(scan.matches) {
            let Some(record) = record else {
                continue;
            };
            // STT_OBJECT (1) or STT_FUNC (2); the carrier validates the extent.
            if !executable || !matches!(record.symbol_type, 1 | 2) || record.raw_section == 0 {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "companion requires a defined function or data object in a captured static RV32 ELF",
                ));
            }
            *slot = Some(record);
        }
    }
    records
        .into_iter()
        .map(|r| r.ok_or_else(|| Error::new(ErrorCode::NotFound, "companion occurrence absent")))
        .collect()
}

/// Linker definitions of the request's companions, in selection order,
/// then of its absent names. A link input must not define a companion name;
/// see [`check_link_symbol`].
pub(super) fn resolve(
    request: &LinkRequest,
    executables: &[Executable],
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<(String, u32)>> {
    let absent = request
        .absent
        .iter()
        .map(|name| (name.clone(), ABSENT_SYMBOL_ADDRESS));
    if request.companions.is_empty() {
        return Ok(absent.collect());
    }
    if request.companions.len() > 64 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "at most 64 exact companion definitions",
        ));
    }
    let _capacity = memory.reserve(1024 * 1024, c.position())?;
    let records = records(&request.companions, executables, memory, c)?;
    let mut definitions = Vec::new();
    for (symbol, record) in request.companions.iter().zip(&records) {
        let name = String::from_utf8(record.name.clone().unwrap_or_default())
            .map_err(|_| Error::new(ErrorCode::InvalidRequest, "non-UTF8 companion linker name"))?;
        if name.is_empty()
            || name.len() > 512
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'))
            || definitions.iter().any(|(old, _)| old == &name)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid or duplicate companion name",
            ));
        }
        let bytes: &[u8] = find(executables, &symbol.object.artifact)?.bytes();
        let address = blobray_artifacts::inspect_link_definition(&bytes, symbol, memory, c)?;
        if request.layout.code.contains(u64::from(address), 1)
            || request.layout.data.contains(u64::from(address), 1)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "companion address overlaps synthetic placement",
            ));
        }
        definitions.push((name, address));
    }
    for (name, address) in absent {
        if definitions.iter().any(|(old, _)| *old == name) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "an absent name is also a companion",
            ));
        }
        definitions.push((name, address));
    }
    Ok(definitions)
}
