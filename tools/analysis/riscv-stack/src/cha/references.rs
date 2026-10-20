//! Narrow adapter from prepared image observations to the shared ISA normalizer.
//! No ELF/symbol walk and no private PC-relative pairing algorithm live here.
use oer_analysis_elf::Target;
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_analysis::PreparedReferences;
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::sync::Arc;

type Visitor<'a> = dyn FnMut(
        &FunctionRelocation,
        Option<&FunctionRelocation>,
        &NormalizedReference,
        &mut dyn RunControl,
    ) -> Result<()>
    + 'a;

/// Visit each PC-relative low using PreparedReferences::{normalized,pair}.
/// Original observation indexes serve as physical IDs in this image-only
/// adapter. Section/offset and symbol identity are retained; names are unused.
pub(super) fn visit(
    image: &ImageFacts<'_>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visitor: &mut Visitor<'_>,
) -> Result<()> {
    let uses = image.address_uses();
    control.checkpoint(uses.len() as u64)?;
    if !uses.iter().any(|use_| use_.role == Role::PcRelativeLow) {
        return Ok(());
    }
    // The model's symbol identity includes the source object. Hash captured
    // bytes under RunControl; this neither parses ELF nor constructs an index.
    let object = ObjectId {
        artifact: ArtifactId::of_bytes_controlled(image.bytes(), control)?,
        location: ObjectLocation::Standalone,
    };
    let table_section = image.file().symbol_table_section().unwrap_or(0) as u32;
    let mut at = 0;
    while at < uses.len() {
        let section = uses[at].relocation.section;
        let count = uses[at..].partition_point(|use_| use_.relocation.section == section);
        let end = at + count;
        control.checkpoint(count as u64)?;
        if uses[at..end]
            .iter()
            .any(|use_| use_.role == Role::PcRelativeLow)
        {
            let mut scratch = Ledger::new(memory, control)?;
            let mut raw = Vec::new();
            for (index, use_) in uses.iter().enumerate().take(end).skip(at) {
                control.checkpoint(1)?;
                // Only these roles can supply or consume a PC-relative pair.
                // The common decoder itself checks ambiguity among all HI20s.
                if !matches!(use_.role, Role::PcRelativeHigh | Role::PcRelativeLow) {
                    continue;
                }
                let (symbol, definition, target_section, offset, binding) = match &use_.symbol {
                    Some(symbol) => (
                        symbol.index as u64,
                        if symbol.index == 0 {
                            SymbolDefinition::Null
                        } else if !symbol.defined {
                            SymbolDefinition::Undefined
                        } else if symbol.section.is_some() {
                            SymbolDefinition::Section
                        } else {
                            SymbolDefinition::Absolute
                        },
                        symbol.section.map(|section| section as u32),
                        symbol.address,
                        u8::from(symbol.global),
                    ),
                    None => {
                        let target_section = match use_.relocation.target {
                            Target::Section(section) => Some(section as u32),
                            _ => None,
                        };
                        let value = use_.value.elf_value().unwrap_or(0);
                        (
                            u64::MAX - target_section.map_or(0, |section| u64::from(section) + 1),
                            if target_section.is_some() {
                                SymbolDefinition::Section
                            } else if use_.relocation.target == Target::Absolute {
                                SymbolDefinition::Absolute
                            } else {
                                SymbolDefinition::Null
                            },
                            target_section,
                            value.wrapping_add_signed(use_.relocation.addend.wrapping_neg()),
                            0,
                        )
                    }
                };
                scratch.admit(
                    size_of::<ReferenceTarget>() + 2 * size_of::<usize>() + 64,
                    control,
                )?;
                let target = Arc::new(ReferenceTarget {
                    symbol: SymbolId {
                        object: object.clone(),
                        table: SymbolTableKind::Static,
                        table_section,
                        index: symbol,
                    },
                    // Diagnostics retain the original borrowed symbol/name.
                    // No decision in the shared normalizer depends on names.
                    name: Vec::new(),
                    section: target_section,
                    offset,
                    binding,
                    definition,
                    symbol_type: 0,
                });
                scratch.push(
                    &mut raw,
                    FunctionRelocation {
                        section: section as u32,
                        index: index as u64,
                        offset: use_.relocation.at,
                        relocation_type: use_.relocation.r_type,
                        addend: (!use_.relocation.implicit_addend)
                            .then_some(use_.relocation.addend),
                        target,
                    },
                    control,
                )?;
            }
            control.checkpoint(
                (raw.len() as u64)
                    .saturating_mul(u64::from(usize::BITS - raw.len().leading_zeros())),
            )?;
            raw.sort_unstable_by_key(|raw| (raw.offset, raw.index));
            let references =
                PreparedReferences::new(&raw, section as u32, &RiscvDecoder, memory, control)?;
            for (index, raw) in references.iter().enumerate() {
                control.checkpoint(1)?;
                if oer_analysis_elf::rv32::kind(raw.relocation_type).role == Role::PcRelativeLow {
                    let normalized = references.normalized(index);
                    let high = references
                        .pair(index)
                        .filter(|_| normalized.known)
                        .and_then(|high| references.get(high));
                    visitor(raw, high, normalized, control)?;
                }
            }
        }
        at = end;
    }
    Ok(())
}
