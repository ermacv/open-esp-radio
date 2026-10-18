//! The human form of an `mmio-diff` report.
use blobray_domain::{
    MmioAccess, MmioDiff, MmioLocation, MmioOperation, MmioStoredBits, MmioStoredSource,
    RegisterMaskKind,
};
use std::fmt::Write as _;

fn location(location: &MmioLocation) -> String {
    match location {
        MmioLocation::At { address } => format!("{address:#010x}"),
        MmioLocation::Indexed { progression } => format!(
            "{:#010x}{:+}*i{}",
            progression.base,
            progression.stride,
            progression
                .count
                .map_or(String::new(), |count| format!(" (i<{count})"))
        ),
    }
}

fn run(bits: &MmioStoredBits) -> String {
    let source = match &bits.source {
        MmioStoredSource::Constant {
            value,
            symbol: None,
        } => format!("{value:#x}"),
        MmioStoredSource::Constant {
            value,
            symbol: Some(symbol),
        } => format!("{value:#x}(&{}+{:#x})", symbol.name, symbol.offset),
        MmioStoredSource::EntryRegister { register, low } => format!("x{register}[{low}..]"),
        MmioStoredSource::Load {
            address,
            width,
            low,
            same_word,
        } => match (address, same_word) {
            (_, true) => "kept".to_owned(),
            (Some(address), false) => format!("load{width}({address:#010x})[{low}..]"),
            (None, false) => format!("load{width}(data)[{low}..]"),
        },
        MmioStoredSource::Unknown => "?".to_owned(),
    };
    format!("[{}..{}]={source}", bits.low, bits.low + bits.width)
}

fn operation(operation: MmioOperation) -> &'static str {
    match operation {
        MmioOperation::Load => "load",
        MmioOperation::Store => "store",
        MmioOperation::LoadReserved => "load-reserved",
        MmioOperation::StoreConditional => "store-conditional",
        MmioOperation::Atomic => "atomic",
        MmioOperation::FloatLoad => "float-load",
        MmioOperation::FloatStore => "float-store",
        MmioOperation::ReadSelection => "keeps",
    }
}

/// One access on one line: `0x20109c18 store w4 [0..2]=0x0 [2..32]=kept`, or
/// `0x20109c18 keeps 0xfffffffc` for the bits a loaded value keeps.
pub fn access(access: &MmioAccess) -> String {
    let mut out = format!(
        "{:<18} {}",
        location(&access.location),
        operation(access.operation)
    );
    if let Some(width) = access.width {
        let _ = write!(out, " w{width}");
    }
    if let Some(mask) = access.mask {
        let _ = match mask.kind {
            RegisterMaskKind::ReadSelection => write!(out, " {:#010x}", mask.bits),
            RegisterMaskKind::WriteReplacement => write!(out, " replaces {:#010x}", mask.bits),
        };
    }
    if access.operation != MmioOperation::ReadSelection && !access.stored.is_empty() {
        let runs: Vec<_> = access.stored.iter().map(run).collect();
        let _ = write!(out, " {}", runs.join(" "));
    }
    out
}

/// The whole report.
pub fn render(diff: &MmioDiff) -> String {
    let mut out = format!(
        "MMIO functions: {} old, {} new; {} changed; {} not shown unchanged; blocked {} old, {} new\n",
        diff.functions[0],
        diff.functions[1],
        diff.changes.len(),
        diff.uncertain.len(),
        diff.blocked[0],
        diff.blocked[1]
    );
    for (build, gaps) in ["old", "new"].iter().zip(&diff.gaps) {
        for gap in gaps {
            let object = match &gap.object {
                Some(object) => match object.location {
                    oer_riscv_model::ObjectLocation::ArchiveMember { ordinal } => {
                        format!(" member {ordinal}")
                    }
                    oer_riscv_model::ObjectLocation::Standalone => String::new(),
                },
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "{build} build input {}{object} not analyzed: {}",
                gap.input, gap.reason
            );
        }
    }
    for change in &diff.changes {
        use blobray_domain::MmioFunctionState::{Absent, Analyzed, Blocked};
        let presence = match change.state {
            [Analyzed, Absent] => " (old only)",
            [Absent, Analyzed] => " (new only)",
            [Blocked, Blocked] => " (blocked in both)",
            [Blocked, _] => " (blocked in old: its old accesses are unknown)",
            [_, Blocked] => " (blocked in new: its new accesses are unknown)",
            _ => "",
        };
        let partial = match change.complete {
            [true, true] => "",
            [false, true] => " (partial in old)",
            [true, false] => " (partial in new)",
            [false, false] => " (partial in both)",
        };
        let _ = writeln!(out, "{}{presence}{partial}", change.function);
        if change.unresolved[0] != change.unresolved[1] {
            let _ = writeln!(
                out,
                "  unresolved accesses {} -> {}",
                change.unresolved[0], change.unresolved[1]
            );
        }
        for removed in &change.removed {
            let _ = writeln!(out, "  - {}", access(removed));
        }
        for added in &change.added {
            let _ = writeln!(out, "  + {}", access(added));
        }
    }
    out
}
