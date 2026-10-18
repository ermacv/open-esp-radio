//! Exact captured data of one object; no execution, inferred layout or
//! hardware lookup.
use crate::captured::visit_members;
use crate::in_process::{Executable, find};
use crate::*;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// The selected bytes of one object, in range order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataExport {
    /// Content identity of the object the bytes were read from.
    pub payload: ArtifactId,
    pub spans: Vec<DataSpan>,
    /// Every selected range's bytes, concatenated; a span's `export_offset`
    /// locates its bytes.
    pub bytes: Vec<u8>,
}

/// Read the ranges `request` selects from its object, which one of
/// `executables` contains.
pub fn export(
    request: &DataRequest,
    executables: &[Executable],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<DataExport> {
    if request.ranges.is_empty() || request.ranges.len() > 32 {
        return Err(invalid("data export requires 1..32 ranges"));
    }
    let executable = find(executables, &request.object.artifact)?;
    let mut export = None;
    visit_members(executable, memory, control, &mut |member, c| {
        if member.id != request.object {
            return Ok(());
        }
        let bytes = member.bytes.map_err(|d| {
            Error::new(
                ErrorCode::Unavailable,
                format!("object bytes unavailable: {}", d.message),
            )
        })?;
        let payload = hash_source(bytes, c)?;
        export = Some(read(request, bytes, payload, memory, c)?);
        Ok(())
    })?;
    export.ok_or_else(|| Error::new(ErrorCode::NotFound, "object absent from its executable"))
}

fn read(
    request: &DataRequest,
    source: &dyn ByteSource,
    payload: ArtifactId,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<DataExport> {
    blobray_artifacts::with_prepared_object(source, &payload, &[], memory, control, |object, c| {
        if let Some(symbol) = &request.symbol {
            object.validate_data_symbol(&request.object, symbol)?;
        }
        let mut spans = Vec::new();
        let mut bytes = Vec::new();
        let mut export_offset = 0u64;
        for selector in &request.ranges {
            object.with_data(&request.object, selector, c, |view, c| {
                c.bytes(view.bytes.len())?;
                bytes.extend_from_slice(view.bytes);
                let mut span = view.span;
                span.export_offset = export_offset;
                export_offset = export_offset
                    .checked_add(span.file_range.length)
                    .ok_or_else(|| invalid("export size overflow"))?;
                spans.push(span);
                Ok(())
            })?;
        }
        Ok(DataExport {
            payload: payload.clone(),
            spans,
            bytes,
        })
    })
}
