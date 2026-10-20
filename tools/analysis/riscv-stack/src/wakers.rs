//! Waker calls. A call the analysis leaves unresolved where the DWARF says
//! `Waker::wake` (or `wake_by_ref`, `drop`, `clone`) is inlined reaches the
//! matching slot of every `RawWakerVTable` of the image. A vtable is four
//! relocated function pointers in data neither writable nor executable whose
//! first function, by the DWARF, returns `core::task::wake::RawWaker`: a
//! `&'static RawWakerVTable` points at immutable data, so every waker the
//! image builds from a `const` or `static` vtable is among them. A vtable
//! built at run time in writable memory (a leaked allocation) is not.
use crate::dwarf::Dwarf;
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_analysis_riscv_model::{Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::BTreeSet;

const RAW_WAKER: &str = "core::task::wake::RawWaker";

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

/// Each `RawWakerVTable` of `image`: its four functions, by slot.
pub fn waker_vtables(
    image: &ImageFacts<'_>,
    dwarf: &Dwarf,
    analysis: &Analysis,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Vec<[u32; 4]>> {
    let function = |address: u32| {
        let target = image.word(address)? & !1;
        image.read_only_word(address)?;
        analysis.functions.contains_key(&target).then_some(target)
    };
    let mut vtables = Vec::new();
    for &(address, _) in image.words() {
        control.checkpoint(1)?;
        if address % 4 != 0 {
            continue;
        }
        let slots = [address, address + 4, address + 8, address + 12].map(function);
        let [Some(clone), Some(wake), Some(wake_by_ref), Some(drop)] = slots else {
            continue;
        };
        if dwarf.return_type(clone) == Some(RAW_WAKER) {
            keep.push(&mut vtables, [clone, wake, wake_by_ref, drop], control)?;
        }
    }
    Ok(vtables)
}

/// The targets of every unresolved call or jump inside a waker method:
/// that method's slot of each of `vtables`.
pub fn waker_resolutions(
    analysis: &Analysis,
    dwarf: &Dwarf,
    vtables: &[[u32; 4]],
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
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
