//! Human rendering of register-access groups and stored values.
use oer_analysis_blobray_application::access_groups::{AccessGroups, GroupFunction};
use oer_analysis_blobray_domain::{IndexedAddress, RegisterMaskKind, StoredBits, StoredBitsSource};
use oer_analysis_riscv_model::ObjectLocation;

/// `name (input 0, member 3)`; `<unnamed>` for a function without a name
/// and no member for a standalone ELF.
fn function_human(function: &GroupFunction) -> String {
    let name = if function.name.is_empty() {
        "<unnamed>"
    } else {
        &function.name
    };
    match function.symbol.object.location {
        ObjectLocation::ArchiveMember { ordinal } => {
            format!("{name} (input {}, member {ordinal})", function.input)
        }
        _ => format!("{name} (input {})", function.input),
    }
}
/// One header line per group and one indented line per entry.
pub fn human(groups: &AccessGroups) -> Vec<String> {
    let mut lines = Vec::new();
    for group in groups.groups() {
        lines.push(match (&group.function, group.word) {
            (Some(function), _) => function_human(function),
            (None, Some(word)) => format!("{word:#010x}"),
            (None, None) => String::from("unresolved"),
        });
        for entry in &group.entries {
            let subject = match (&entry.function, entry.word, entry.indexed) {
                (Some(function), _, _) => function_human(function),
                (None, Some(word), _) => format!("{word:#010x}"),
                (None, None, Some(indexed)) => indexed_human(&indexed),
                (None, None, None) => String::from("unresolved"),
            };
            let indexed = match (&entry.function, entry.indexed) {
                (Some(_), Some(indexed)) => format!(" {}", indexed_human(&indexed)),
                _ => String::new(),
            };
            let width = entry
                .width
                .map(|width| format!(" {width}B"))
                .unwrap_or_default();
            let mask = entry
                .mask
                .map(|mask| match mask.kind {
                    RegisterMaskKind::WriteReplacement => {
                        format!(" replace {:#010x}", mask.bits)
                    }
                })
                .unwrap_or_default();
            let stored = entry
                .stored
                .as_deref()
                .map(|runs| format!(" {}", stored_human(runs)))
                .unwrap_or_default();
            let reads = entry
                .reads
                .as_ref()
                .map(|reads| format!(" {}", crate::mmio::reads(reads)))
                .unwrap_or_default();
            lines.push(format!(
                "  {subject}{indexed} {}{width}{mask}{stored}{reads} x{}",
                entry.access, entry.count
            ));
        }
    }
    lines
}
/// `indexed 0x2010f400+0x10*i` for an indexed progression, with `(i<6)` when
/// the index is bounded; a negative stride prints as `-0x7c*i`.
pub fn indexed_human(indexed: &IndexedAddress) -> String {
    let sign = if indexed.stride < 0 { '-' } else { '+' };
    let bound = indexed
        .count
        .map(|count| format!(" (i<{count})"))
        .unwrap_or_default();
    format!(
        "indexed {:#010x}{sign}{:#x}*i{bound}",
        indexed.base,
        indexed.stride.unsigned_abs()
    )
}

/// The runs of a stored value, high bits first: `[24:18]=arg0[6:0]` for an
/// argument's bits (`entry x8[5:4]` for another entry register),
/// `[17:0]=kept` for bits a read-modify-write keeps in place,
/// `=same word[9:2]` for the stored word's bits moved elsewhere, `=0x1b` for
/// fixed bits, `=load 4B 0x20109004[7:0]` for another loaded value with its
/// load width and `=?` for an unknown source.
pub fn stored_human(runs: &[StoredBits]) -> String {
    let range = |low: u8, width: u8| {
        if width == 1 {
            format!("[{low}]")
        } else {
            format!("[{}:{low}]", low + width - 1)
        }
    };
    runs.iter()
        .rev()
        .map(|run| {
            let source = match &run.source {
                StoredBitsSource::Constant { value } => format!("{value:#x}"),
                StoredBitsSource::EntryRegister { register, low }
                    if (10..18).contains(register) =>
                {
                    format!("arg{}{}", register - 10, range(*low, run.width))
                }
                StoredBitsSource::EntryRegister { register, low } => {
                    format!("entry x{register}{}", range(*low, run.width))
                }
                StoredBitsSource::Load {
                    same_word: true,
                    low,
                    ..
                } if *low == run.low => String::from("kept"),
                StoredBitsSource::Load {
                    address,
                    width,
                    low,
                    same_word,
                } => {
                    let word = match (same_word, address) {
                        (true, _) => String::from("same word"),
                        (false, Some(address)) => format!("load {width}B {address:#x}"),
                        (false, None) => format!("load {width}B"),
                    };
                    format!("{word}{}", range(*low, run.width))
                }
                StoredBitsSource::Unknown => String::from("?"),
            };
            format!("{}={source}", range(run.low, run.width))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
