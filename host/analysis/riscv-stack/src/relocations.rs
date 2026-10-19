//! The addresses an image takes: what the link's relocations say about
//! which functions a pointer may reach.
use oer_analysis_elf::rv32::{self, Role};
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;

/// Every address an allocated section of `image` takes: a relocation's
/// target other than a call, a jump, a branch or a linker hint.
pub fn taken_addresses(
    image: &ImageFacts<'_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<std::collections::BTreeSet<u32>> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    let file = image.file();
    let mut taken = std::collections::BTreeSet::new();
    for section in file.sections().filter(|section| section.allocated) {
        control.checkpoint(1)?;
        let relocations = file
            .relocations(section.index)
            .map_err(|error| invalid(error.to_string()))?;
        for relocation in relocations {
            control.checkpoint(1)?;
            if rv32::transfers(relocation.r_type)
                || rv32::kind(relocation.r_type).role == Role::Hint
                || matches!(
                    relocation.target,
                    oer_analysis_elf::Target::Section(_) | oer_analysis_elf::Target::Other
                )
            {
                continue;
            }
            let target = file
                .target_address(&relocation)
                .map_err(|error| invalid(error.to_string()))?;
            if let Ok(address) = u32::try_from(target) {
                keep.add(&mut taken, address & !1, control)?;
            }
        }
    }
    Ok(taken)
}
