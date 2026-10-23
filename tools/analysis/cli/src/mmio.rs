//! The human form of an `mmio-diff` report.
use oer_analysis_blobray_domain::{
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
    }
}

/// One access on one line: `0x20109c18 store w4 [0..2]=0x0 [2..32]=kept`, or
/// `0x20109c18 load w4 reads[used=... preserved=... unknown=... unused=...]`.
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
            RegisterMaskKind::WriteReplacement => write!(out, " replaces {:#010x}", mask.bits),
        };
    }
    if let Some(reads) = access.reads {
        let _ = write!(
            out,
            " reads[used={:#010x} preserved={:#010x} unknown={:#010x} unused={:#010x}]",
            reads.used, reads.preserved, reads.unknown, reads.unused
        );
    }
    if !access.stored.is_empty() {
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
                    oer_analysis_riscv_model::ObjectLocation::ArchiveMember { ordinal } => {
                        format!(" member {ordinal}")
                    }
                    oer_analysis_riscv_model::ObjectLocation::Standalone => String::new(),
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
        use oer_analysis_blobray_domain::MmioFunctionState::{Absent, Analyzed, Blocked};
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

/// Bit classes of one load, with every sink and stopping reason retained.
pub fn reads(reads: &oer_analysis_blobray_domain::ReadUses) -> String {
    use oer_analysis_blobray_domain::{ReadLimitKind, ReadPrecision, ReadSink};
    let mut parts = Vec::new();
    for usage in &reads.uses {
        let sink = match &usage.sink {
            ReadSink::Branch { site } => format!("branch@+{site:x}"),
            ReadSink::Dispatch { site } => format!("dispatch@+{site:x}"),
            ReadSink::IndirectTarget { site } => format!("indirect-target@+{site:x}"),
            ReadSink::Address { site } => format!("address@+{site:x}"),
            ReadSink::Return { register } => format!("return-x{register}"),
            ReadSink::CallArgument {
                site,
                register,
                callee,
                shift,
            } => {
                let target = callee
                    .as_ref()
                    .map(|target| {
                        let address = target.address.map(|address| format!("{address:#010x}"));
                        match (&target.name, address) {
                            (Some(name), Some(address)) => format!("{name}@{address}"),
                            (Some(name), None) => name.clone(),
                            (None, Some(address)) => address,
                            (None, None) => "?".to_owned(),
                        }
                    })
                    .unwrap_or_else(|| "?".to_owned());
                let wiring = shift
                    .map(|shift| format!(",shift={shift}"))
                    .unwrap_or_default();
                format!("call@+{site:x}-x{register}({target}{wiring})")
            }
            ReadSink::Store {
                site,
                address,
                same_word,
            } => format!(
                "store@+{site:x}({}{})",
                address
                    .map(|address| format!("{address:#010x}"))
                    .unwrap_or_else(|| "unresolved".to_owned()),
                if *same_word { ",same-word" } else { "" }
            ),
        };
        let precision = match usage.precision {
            ReadPrecision::Exact => "exact",
            ReadPrecision::May => "may",
        };
        parts.push(format!("used={:#010x}:{sink}:{precision}", usage.bits));
    }
    for limit in &reads.unknown {
        let reason = match limit.limit {
            ReadLimitKind::PartialFunction => "partial-function",
            ReadLimitKind::StackSpill => "stack-spill",
            ReadLimitKind::UnsupportedOperation => "unsupported-operation",
            ReadLimitKind::WorkLimit => "work-limit",
        };
        parts.push(format!("unknown={:#010x}:{reason}", limit.bits));
    }
    parts.push(format!("preserved={:#010x}", reads.preserved));
    parts.push(format!("unused={:#010x}", reads.unused));
    format!("reads[{}]", parts.join(" "))
}
