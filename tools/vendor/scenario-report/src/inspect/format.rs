//! Presentation of typed values and stored-bit runs; no instruction semantics.
use super::*;
use oer_analysis_blobray_domain::{StoredBits, StoredBitsSource};
use oer_analysis_riscv_model::{AbstractValue, Expression, ValueAlternative};

pub(super) fn bit_list(mask: u32) -> String {
    let bits: Vec<_> = (0..u32::BITS)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| bit.to_string())
        .collect();
    format!("bits {}", bits.join(","))
}

pub(super) fn effect(
    corpus: &Corpus,
    address: Option<u32>,
    width: u8,
    stored: Option<&[StoredBits]>,
) -> String {
    let mask = u32::MAX
        .checked_shr(32u32.saturating_sub(u32::from(width) * 8))
        .unwrap_or(0);
    let Some(stored) = stored else {
        return format!("a value {}", bits(corpus, address, mask));
    };
    let mut fixed = 0;
    let mut ones = 0;
    let mut kept = 0;
    for run in stored {
        let run_mask = u32::MAX
            .checked_shr(32u32.saturating_sub(u32::from(run.width)))
            .unwrap_or(0);
        let shifted = run_mask.checked_shl(u32::from(run.low)).unwrap_or(0);
        match run.source {
            StoredBitsSource::Constant { value } => {
                fixed |= shifted;
                ones |= (value & run_mask)
                    .checked_shl(u32::from(run.low))
                    .unwrap_or(0);
            }
            StoredBitsSource::Load {
                same_word: true,
                low,
                ..
            } if low == run.low => kept |= shifted,
            _ => {}
        }
    }
    if fixed == mask {
        return format!("= {ones:#x}");
    }
    let value = mask & !(fixed | kept);
    if fixed == 0 && kept == 0 {
        return format!("a value {}", bits(corpus, address, value));
    }
    let mut parts = vec![];
    for (label, mask) in [
        ("clears", fixed & !ones),
        ("sets", ones),
        ("from a value", value),
    ] {
        if mask != 0 {
            parts.push(format!("{label} {}", bits(corpus, address, mask)));
        }
    }
    if parts.is_empty() {
        "rewrites the value read".into()
    } else {
        parts.join("; ")
    }
}

fn bits(corpus: &Corpus, address: Option<u32>, mask: u32) -> String {
    address.map_or_else(
        || bit_list(mask),
        |address| corpus.registers.bits(address & !3, mask),
    )
}

pub(super) fn read_bits(
    corpus: &Corpus,
    address: Option<u32>,
    reads: &oer_analysis_blobray_domain::ReadUses,
) -> String {
    let used = reads.uses.iter().fold(0, |mask, usage| mask | usage.bits);
    let unknown = reads
        .unknown
        .iter()
        .fold(0, |mask, limit| mask | limit.bits);
    [
        ("uses", used),
        ("preserves", reads.preserved),
        ("unused", reads.unused),
        ("unknown uses", unknown),
    ]
    .into_iter()
    .filter(|(_, mask)| *mask != 0)
    .map(|(label, mask)| format!("{label} {}", bits(corpus, address, mask)))
    .collect::<Vec<_>>()
    .join("; ")
}

pub(super) fn value(value: &AbstractValue, metadata: &Metadata) -> String {
    match value {
        AbstractValue::Unknown => "?".into(),
        AbstractValue::Expression { id } => format!("expr#{id}"),
        AbstractValue::Constant { value } => format!("{value:#x}"),
        AbstractValue::ImageAddress { address } => format!("{address:#x}"),
        AbstractValue::Section { section, offset } => format!("section{section}{offset:+#x}"),
        AbstractValue::Symbol { symbol, addend } => symbol_value(symbol, *addend, metadata),
        AbstractValue::EntryStack { offset } => format!("sp{offset:+#x}"),
        AbstractValue::Alternatives { values } => format!(
            "one-of{{{}}}",
            values
                .values()
                .iter()
                .map(|v| alternative(v, metadata))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    }
}

fn alternative(value_: &ValueAlternative, metadata: &Metadata) -> String {
    match value_ {
        ValueAlternative::Constant { value } => format!("{value:#x}"),
        ValueAlternative::ImageAddress { address } => format!("{address:#x}"),
        ValueAlternative::Section { section, offset } => format!("section{section}{offset:+#x}"),
        ValueAlternative::Symbol { symbol, addend } => symbol_value(symbol, *addend, metadata),
        ValueAlternative::EntryStack { offset } => format!("sp{offset:+#x}"),
    }
}

fn symbol_value(
    symbol: &oer_analysis_riscv_model::SymbolId,
    addend: i64,
    metadata: &Metadata,
) -> String {
    let name = metadata
        .symbols
        .get(symbol)
        .and_then(|s| s.name.as_deref())
        .map(String::from_utf8_lossy)
        .map(|s| s.into_owned())
        .unwrap_or_else(|| format!("symbol{}", symbol.index));
    if addend == 0 {
        format!("&{name}")
    } else {
        format!("&{name}{addend:+#x}")
    }
}

pub(super) fn expression(expression: &Expression, metadata: &Metadata) -> String {
    match expression {
        Expression::EntryRegister { register } => {
            oer_analysis_riscv_lift::register_name(*register).to_owned()
        }
        Expression::CallResult { callsite, register } => format!(
            "{} from call {callsite:#x}",
            oer_analysis_riscv_lift::register_name(*register)
        ),
        Expression::Load { address, width, .. } => {
            format!("load{width}({})", value(address, metadata))
        }
        Expression::Integer { op, left, right } => format!(
            "{op:?}({}, {})",
            value(left, metadata),
            value(right, metadata)
        ),
        Expression::CallWord { .. } => format!("{expression:?}"),
    }
}

/// Print just the definitions referenced by displayed values. This follows
/// dictionary references for presentation; it never evaluates an expression.
pub(super) fn dictionary<'a>(
    records: &'a [FunctionRecord],
    values: impl IntoIterator<Item = &'a AbstractValue>,
    metadata: &Metadata,
) -> String {
    let definitions: BTreeMap<_, _> = records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Expression { id, expression, .. } => Some((*id, expression)),
            _ => None,
        })
        .collect();
    let mut needed = BTreeSet::new();
    let mut pending = Vec::new();
    let mut reference = |value: &AbstractValue| {
        if let AbstractValue::Expression { id } = value
            && needed.insert(*id)
        {
            pending.push(*id);
        }
    };
    for value in values {
        reference(value);
    }
    while let Some(id) = pending.pop() {
        let inputs = match definitions.get(&id) {
            Some(Expression::Integer { left, right, .. }) => [Some(left), Some(right)],
            Some(Expression::Load { address, .. }) => [Some(address), None],
            Some(Expression::CallWord { source, .. }) => [Some(source), None],
            _ => [None, None],
        };
        for value in inputs.into_iter().flatten() {
            if let AbstractValue::Expression { id } = value
                && needed.insert(*id)
            {
                pending.push(*id);
            }
        }
    }
    let mut text = String::new();
    for id in needed {
        if let Some(definition) = definitions.get(&id) {
            let _ = writeln!(text, "    expr#{id} = {}", expression(definition, metadata));
        }
    }
    text
}
