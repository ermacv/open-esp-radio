//! Waker calls. A call the analysis leaves unresolved where the DWARF says
//! `Waker::wake` (or `wake_by_ref`, `drop`, `clone`) is inlined reaches the
//! matching slot of the image's one `RawWakerVTable`.
//!
//! The census of vtables is complete. Safe Rust builds a `Waker` only from a
//! `&'static RawWakerVTable`, and a `RawWakerVTable` only through
//! `RawWakerVTable::new`, whose first argument is a function returning
//! `core::task::wake::RawWaker`. Every use of such a function's address the
//! link kept (`--emit-relocs`), in code or in data of any section, writable
//! or not, must be the first word of four relocated function pointers in
//! data: a vtable, wherever it lies. Every static the DWARF types as or
//! containing a `RawWakerVTable`, in `.bss` too, must hold one of those;
//! a static the link discarded lies in no allocated section and holds none.
//! A clone function's address taken anywhere else (code that could build a
//! vtable at run time, a lone pointer, an unrelocatable use) or a typed
//! vtable the layout does not show fails the analysis with `Integrity`, and
//! so does a second vtable: every waker carries the executor's.
use crate::cha::TypeFacts;
use crate::dwarf::Dwarf;
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_program::facts::addresses::TargetValue;
use std::collections::BTreeSet;

const RAW_WAKER: &str = "core::task::wake::RawWaker";
const RAW_WAKER_VTABLE: &str = "RawWakerVTable";

/// The slot of a waker method in `RawWakerVTable`: clone, wake,
/// wake_by_ref, drop.
fn slot(function: &str) -> Option<usize> {
    match function {
        "<core::task::wake::Waker as core::clone::Clone>::clone" => Some(0),
        // An inherent method demangles as `<Type>::method`.
        "<core::task::wake::Waker>::wake" | "core::task::wake::Waker::wake" => Some(1),
        "<core::task::wake::Waker>::wake_by_ref" | "core::task::wake::Waker::wake_by_ref" => {
            Some(2)
        }
        "<core::task::wake::Waker as core::ops::drop::Drop>::drop" => Some(3),
        _ => None,
    }
}

fn unclassified(message: String) -> Error {
    Error::new(
        ErrorCode::Integrity,
        format!("{message}: the waker vtable census cannot classify it"),
    )
}

/// The one `RawWakerVTable` of `image` by the complete census, its four
/// functions by slot, or `None` when the image has none. Anything the census
/// cannot classify and a second vtable are [`ErrorCode::Integrity`] errors.
pub fn waker_vtable(
    image: &ImageFacts<'_>,
    dwarf: &Dwarf,
    types: &TypeFacts,
    analysis: &Analysis,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<[u32; 4]>> {
    let clone = |address: u32| {
        analysis.functions.contains_key(&address) && dwarf.return_type(address) == Some(RAW_WAKER)
    };
    let mut found: Option<(u32, [u32; 4])> = None;
    for address_use in image.address_uses() {
        control.checkpoint(1)?;
        // A PC-relative low names its `auipc`'s label, which a clone
        // function may begin with; its high names the target.
        if address_use.role == Role::PcRelativeLow {
            continue;
        }
        let site = address_use.relocation.at;
        let target = match address_use.value {
            TargetValue::Unsupported => {
                return Err(unclassified(format!(
                    "the relocation at {site:#010x} names no address, perhaps a waker clone \
                     function's"
                )));
            }
            value => value.canonical(),
        };
        let Some(target) = target.filter(|&target| clone(target)) else {
            continue;
        };
        let vtable = (address_use.role == Role::Word)
            .then(|| u32::try_from(site).ok())
            .flatten()
            .filter(|address| address % 4 == 0)
            .and_then(|address| {
                let function = |slot: u32| {
                    let target = image.word(address.checked_add(4 * slot)?)? & !1;
                    analysis.functions.contains_key(&target).then_some(target)
                };
                let slots = [function(0)?, function(1)?, function(2)?, function(3)?];
                // The other three return nothing.
                slots[1..]
                    .iter()
                    .all(|&slot| dwarf.return_type(slot).is_none())
                    .then_some((address, slots))
            });
        let Some((address, slots)) = vtable else {
            return Err(unclassified(format!(
                "the address of the waker clone function {target:#010x} is taken at \
                 {site:#010x} outside a waker vtable"
            )));
        };
        match found {
            Some((first, _)) if first != address => {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    format!(
                        "the image has a second waker vtable at {address:#010x} (clone \
                         {target:#010x}) beside the one at {first:#010x}: every waker must \
                         carry the executor's vtable"
                    ),
                ));
            }
            _ => found = Some((address, slots)),
        }
    }
    let mut scratch = Ledger::new(keep.memory(), control)?;
    for address in types.static_objects(RAW_WAKER_VTABLE, &mut scratch, control)? {
        control.checkpoint(1)?;
        // The link discarded a static outside every allocated section (its
        // DWARF location is a tombstone): it has no storage a waker reaches.
        let stored = image
            .file()
            .sections()
            .any(|section| section.allocated && section.contains(u64::from(address)));
        if stored && found.is_none_or(|(vtable, _)| vtable != address) {
            return Err(unclassified(format!(
                "a static `RawWakerVTable` at {address:#010x} holds no waker vtable the \
                 relocations show"
            )));
        }
    }
    Ok(found.map(|(_, vtable)| vtable))
}

/// The targets of every unresolved call or jump inside a waker method:
/// that method's slot of `vtable`. Without a vtable each such site gets an
/// empty set, a hole.
pub fn waker_resolutions(
    analysis: &Analysis,
    dwarf: &Dwarf,
    vtable: Option<&[u32; 4]>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let vtables = vtable.as_slice();
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            let chain = dwarf.inline_chain(transfer.site)?;
            let mut open_exit = false;
            let mut scratch = Ledger::new(keep.memory(), control)?;
            let mut slots = BTreeSet::new();
            if let Some(slot) = chain.first().and_then(|function| slot(function)) {
                scratch.add(&mut slots, slot, control)?;
            } else {
                // A source scope can end before the call. Use every reaching
                // target load in the complete local graph, including merges
                // of wake and wake_by_ref. A single unknown path leaves a hole.
                let loads = facts.target_loads.get(&transfer.site).or_else(|| {
                    open_exit = true;
                    facts.open_exit_target_loads.get(&transfer.site)
                });
                let Some(loads) = loads else {
                    continue;
                };
                let mut known = true;
                for load in loads {
                    control.checkpoint(1)?;
                    let chain = dwarf.inline_chain(load.site)?;
                    let Some(slot) = chain
                        .first()
                        .and_then(|function| slot(function))
                        .filter(|slot| load.offset == 4 * *slot as i32)
                    else {
                        known = false;
                        break;
                    };
                    scratch.add(&mut slots, slot, control)?;
                }
                if !known || slots.is_empty() {
                    continue;
                }
            }
            if open_exit {
                let start = u64::from(facts.function.address);
                let end = start + u64::from(facts.function.size);
                let mut local_target = false;
                for vtable in vtables {
                    for slot in &slots {
                        control.checkpoint(1)?;
                        if (start..end).contains(&u64::from(vtable[*slot])) {
                            local_target = true;
                        }
                    }
                }
                if local_target {
                    continue;
                }
            }
            keep.admit(Resolutions::admission(vtables.len() * slots.len()), control)?;
            let targets = vtables
                .iter()
                .flat_map(|vtable| slots.iter().map(|slot| vtable[*slot]));
            resolutions.add(transfer.site, Fact::WakerVtables, targets);
        }
    }
    Ok(resolutions)
}
