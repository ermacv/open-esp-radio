//! Consumer projections of the image's structural address-use index.
use oer_analysis_riscv_model::{Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::{ImageFacts, addresses::AddressUse};

/// Every numerical address taken through a symbol or absolute relocation.
/// Keep this consumer's existing policy: direct section targets do not enter
/// this set, and odd addresses have their instruction low bit cleared.
pub fn taken_addresses(
    image: &ImageFacts<'_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<std::collections::BTreeSet<u32>> {
    let mut taken = std::collections::BTreeSet::new();
    for address_use in image.address_uses() {
        control.checkpoint(1)?;
        if matches!(
            address_use.relocation.target,
            oer_analysis_elf::Target::Section(_) | oer_analysis_elf::Target::Other
        ) {
            continue;
        }
        if let Some(address) = address_use.value.canonical() {
            keep.add(&mut taken, address, control)?;
        }
    }
    Ok(taken)
}

/// The two all-uses audits intentionally admit different symbol identities.
#[derive(Clone, Copy)]
pub(super) enum FunctionUsePolicy {
    TaskPoll,
    TypedStorage,
}

pub(super) struct FunctionUse {
    pub symbol: Option<u32>,
    pub canonical: u32,
    pub containing: Option<u32>,
    pub function: Option<u32>,
    pub exact: bool,
}

/// Project onto the joint analysis's *numerical* function catalog. It can
/// include companion functions, while every section/symbol identity in the
/// observation continues to belong to its own ImageFacts. The index's local
/// function relations must not be interpreted as relations to another ELF.
/// This preserves the audits' existing nearest-entry policy even when the
/// structural index records overlapping extents or differing source/value
/// relations as ambiguous.
pub(super) fn function_use(
    address_use: &AddressUse<'_>,
    analysis: &crate::Analysis,
    policy: FunctionUsePolicy,
) -> Result<Option<FunctionUse>> {
    let Ok(raw) = u32::try_from(address_use.value.elf_value()?) else {
        return Ok(None);
    };
    let symbol = address_use.symbol.as_ref().filter(|symbol| {
        matches!(policy, FunctionUsePolicy::TypedStorage)
            || symbol.kind == oer_analysis_elf::SymbolKind::Text
    });
    let symbol = symbol
        .and_then(|symbol| u32::try_from(symbol.address).ok())
        .filter(|address| analysis.functions.contains_key(address));
    let canonical = address_use.value.canonical().ok_or_else(|| {
        oer_analysis_riscv_model::Error::new(
            oer_analysis_riscv_model::ErrorCode::Integrity,
            "address-use value exceeds RV32",
        )
    })?;
    let canonical = match policy {
        FunctionUsePolicy::TaskPoll => symbol.map_or(canonical, |symbol| symbol & !1),
        FunctionUsePolicy::TypedStorage => canonical,
    };
    let candidate = symbol.unwrap_or(canonical);
    let address = match policy {
        FunctionUsePolicy::TaskPoll => canonical,
        FunctionUsePolicy::TypedStorage => raw,
    };
    let containing = analysis
        .functions
        .range(..=address)
        .next_back()
        .filter(|(entry, facts)| {
            u64::from(address) < u64::from(**entry) + u64::from(facts.function.size)
        })
        .map(|(&entry, _)| entry);
    let function = if matches!(policy, FunctionUsePolicy::TypedStorage)
        && analysis.functions.contains_key(&candidate)
    {
        Some(candidate)
    } else {
        containing.or_else(|| {
            matches!(policy, FunctionUsePolicy::TaskPoll)
                .then_some(symbol.unwrap_or(raw))
                .filter(|target| analysis.functions.contains_key(target))
        })
    };
    Ok(Some(FunctionUse {
        symbol,
        canonical,
        containing,
        function,
        exact: function == Some(raw) && symbol.is_none_or(|symbol| symbol == raw),
    }))
}
