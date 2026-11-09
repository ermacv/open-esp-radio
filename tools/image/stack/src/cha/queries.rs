//! Where the typed-location consumers can ask, from the instruction pass
//! alone, before the locations are read. Each rule covers one consumer:
//!
//! - `value_field` asks at an unresolved transfer's site;
//! - `fields` asks at a target load's site, at each witness of its base, and
//!   at each word store (and the instruction after it) in the straight-line
//!   lifetime of a register an address-use low materializes ([`lifetime`]);
//! - `waker_register` asks, from a PC of a function that has target loads up
//!   to a transfer's end, within that function.
//!
//! Each rule is a superset of the consumer's own conditions: a transfer,
//! load, witness or store the consumer skips still adds its PC.
use super::audit::lifetime;
use crate::Analysis;
use crate::dwarf::locations::LocationQueries;
use oer_tools_analysis_elf::rv32::Role;
use oer_tools_analysis_riscv_lift::RiscvDecoder;
use oer_tools_analysis_riscv_model::*;
use oer_tools_analysis_riscv_program::facts::ImageFacts;

/// The location queries of `analysis` of `image`, for
/// [`crate::read_locations`] of the same ELF. `analysis` may include
/// companions; their PCs ask nothing of this ELF's locations.
pub fn location_queries(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<LocationQueries> {
    let mut queries = LocationQueries::default();
    for facts in analysis.functions.values() {
        control.checkpoint(1)?;
        for transfer in &facts.transfers {
            if transfer.target.is_none() {
                queries.point(transfer.site, keep, control)?;
            }
        }
        for load in facts.target_loads.values().flatten() {
            queries.point(load.site, keep, control)?;
        }
        for base in facts.load_bases.values() {
            for witness in &base.witnesses {
                queries.point(witness.site, keep, control)?;
            }
        }
        // A Waker address proof asks first at a load's site, then walks back
        // over this function's own instructions; each asks up to the end of
        // a transfer inside it.
        let mut start = None;
        for load in facts
            .target_loads
            .values()
            .chain(facts.open_exit_target_loads.values())
            .flatten()
        {
            control.checkpoint(1)?;
            start = Some(start.map_or(load.site, |start: u32| start.min(load.site)));
        }
        if let Some(start) = start {
            let address = facts.function.address;
            let end = u64::from(address) + u64::from(facts.function.size);
            queries.window(start.min(address), end, keep, control)?;
        }
    }
    // A code use's low is an address-use low that adds an immediate to its
    // own destination, in an image function of its section that contains it.
    let functions = image.functions();
    let longest = functions.iter().map(|function| function.size).max();
    for address_use in image.address_uses() {
        control.checkpoint(1)?;
        if !matches!(address_use.role, Role::AbsoluteLow | Role::PcRelativeLow) {
            continue;
        }
        let site = address_use.relocation.at as u32;
        let before = functions.partition_point(|function| function.address <= site);
        for owner in functions.iter().take(before).rev() {
            control.checkpoint(1)?;
            if longest.is_none_or(|longest| {
                u64::from(owner.address) + u64::from(longest) <= u64::from(site)
            }) {
                break;
            }
            if owner.section != address_use.relocation.section
                || u64::from(site) >= u64::from(owner.address) + u64::from(owner.size)
            {
                continue;
            }
            let code = crate::image::function_bytes(image.file(), owner)?;
            let Some(bytes) = code.get(site.wrapping_sub(owner.address) as usize..) else {
                continue;
            };
            let Some(decoded) = RiscvDecoder.decode(bytes) else {
                continue;
            };
            let SemanticOp::Integer {
                op: IntegerOp::Add,
                dest,
                left: Operand::Register(base),
                right: Operand::Immediate(_),
            } = RiscvDecoder.lift(bytes)
            else {
                continue;
            };
            if dest == 0 || dest != base {
                continue;
            }
            lifetime(
                code,
                owner.address,
                &|_| false,
                site.wrapping_add(u32::from(decoded.length)),
                dest,
                &mut |store, control| {
                    queries.point(store.site, keep, control)?;
                    queries.point(store.next, keep, control)?;
                    Ok(true)
                },
                control,
            )?;
        }
    }
    queries.finish(control)?;
    Ok(queries)
}
