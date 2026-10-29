//! Waker calls. A call the analysis leaves unresolved where the DWARF says
//! `Waker::wake` (or `wake_by_ref`, `drop`, `clone`) is inlined reaches the
//! matching slot of the image's one `RawWakerVTable`.
//!
//! The census of vtables. Every `Waker` carries a `&'static RawWakerVTable`:
//! `Waker::from_raw` and `Waker::new` (both unsafe) take one, `Waker::noop`
//! carries `core`'s own and `Clone` copies the one a clone function returns.
//! A `RawWakerVTable` is four function pointers; the first, the clone
//! function, returns `core::task::wake::RawWaker`. The census finds the
//! image's vtables twice over:
//!
//! - By type. A function the DWARF says returns `RawWaker` (through
//!   `DW_AT_abstract_origin` and `DW_AT_specification` too) is a clone
//!   function. Every use of its address the link kept (`--emit-relocs`), in
//!   code or in data of any section, writable or not, must be the first word
//!   of four relocated function pointers in data whose other three the DWARF
//!   gives no return type: a vtable, wherever in data it lies.
//! - By layout. Four relocated function pointers in a row in data (an
//!   allocated section neither executable nor empty) whose first function's
//!   return the DWARF does not state ([`Returned::Unknown`]: a library built
//!   with line tables only, such as the precompiled `core`, or code without
//!   DWARF) are a vtable when that
//!   function's code takes the table's address, as a clone function of a
//!   static vtable does. Any other such row fails the census.
//!
//! Every static the DWARF types as or containing a `RawWakerVTable`, in
//! `.bss` too, must hold a vtable found so; a static the link discarded lies
//! in no allocated section and holds none. A clone function's address taken
//! anywhere else (code that could build a vtable at run time, a lone
//! pointer, an unrelocatable use), a row the census cannot classify, or a
//! typed vtable the layout does not show fails the analysis with
//! `Integrity`, and so does a second vtable: every waker carries the
//! executor's.
//!
//! The census rests on what it reads being whole:
//! - the DWARF: a function the DWARF gives no return type is found only
//!   where its address heads four function pointers in data, so a vtable
//!   built at run time from such a function (its address taken by code), or
//!   one inside an executable section (where trap vector tables lie), is not
//!   found;
//! - the relocations: `--emit-relocs` keeps every one the link resolved, each
//!   `HI20`/`LO12` pair the linker's relaxation rewrote included;
//! - memory safety: unsafe code, the image's and its dependencies', does not
//!   forge a `RawWakerVTable` or write the vtable pointer inside a waker by
//!   other means than the constructors above.
use crate::cha::TypeFacts;
use crate::dwarf::{Dwarf, Returned};
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_program::facts::addresses::TargetValue;
use std::collections::{BTreeMap, BTreeSet};

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

/// The vtables the census has counted: at most one.
#[derive(Default)]
struct Census {
    found: Option<(u32, [u32; 4])>,
}

impl Census {
    fn count(&mut self, address: u32, slots: [u32; 4]) -> Result<()> {
        match self.found {
            Some((first, _)) if first != address => Err(Error::new(
                ErrorCode::Integrity,
                format!(
                    "the image has two waker vtables, at {first:#010x} and at {address:#010x} \
                     (clone {:#010x}): every waker must carry the executor's vtable",
                    slots[0]
                ),
            )),
            _ => {
                self.found = Some((address, slots));
                Ok(())
            }
        }
    }
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
    let mut scratch = Ledger::new(keep.memory(), control)?;
    // Every relocated word of allocated data that names a function's entry:
    // its address and the function.
    let mut rows = BTreeMap::new();
    for &(site, target) in image.words() {
        control.checkpoint(1)?;
        if analysis.functions.contains_key(&(target & !1)) {
            scratch.insert(&mut rows, site, target & !1, control)?;
        }
    }
    // The four functions of the row of relocated words at `address`.
    let row = |address: u32| -> Option<[u32; 4]> {
        if !address.is_multiple_of(4) {
            return None;
        }
        let function = |slot: u32| rows.get(&address.checked_add(4 * slot)?).copied();
        Some([function(0)?, function(1)?, function(2)?, function(3)?])
    };
    let mut census = Census::default();
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
        let Some(target) = target.filter(|&target| {
            analysis.functions.contains_key(&target)
                && dwarf.returned(target) == Returned::Type(RAW_WAKER)
        }) else {
            continue;
        };
        let vtable = (address_use.role == Role::Word)
            .then(|| u32::try_from(site).ok())
            .flatten()
            .and_then(|address| Some((address, row(address)?)))
            // The other three return nothing.
            .filter(|(_, slots)| {
                slots[1..]
                    .iter()
                    .all(|&slot| !matches!(dwarf.returned(slot), Returned::Type(_)))
            });
        let Some((address, slots)) = vtable else {
            return Err(unclassified(format!(
                "the address of the waker clone function {target:#010x} is taken at \
                 {site:#010x} outside a waker vtable"
            )));
        };
        census.count(address, slots)?;
    }
    // Rows whose first function the DWARF does not type, by the address of
    // the row and its first function.
    let mut untyped = BTreeMap::new();
    for &address in rows.keys() {
        control.checkpoint(1)?;
        if let Some(slots) = row(address)
            && dwarf.returned(slots[0]) == Returned::Unknown
        {
            scratch.insert(&mut untyped, address, slots, control)?;
        }
    }
    // Those whose first function takes the row's address.
    let mut referenced = BTreeSet::new();
    if !untyped.is_empty() {
        for address_use in image.address_uses() {
            control.checkpoint(1)?;
            if address_use.role == Role::PcRelativeLow {
                continue;
            }
            let Some(target) = address_use.value.canonical() else {
                continue;
            };
            let Some(slots) = untyped.get(&target) else {
                continue;
            };
            // Every row holds functions of the analysis.
            let Some(facts) = analysis.functions.get(&slots[0]) else {
                continue;
            };
            let function = &facts.function;
            let start = u64::from(function.address);
            if (start..start + u64::from(function.size)).contains(&address_use.relocation.at) {
                scratch.add(&mut referenced, target, control)?;
            }
        }
    }
    for (&address, &slots) in &untyped {
        control.checkpoint(1)?;
        if !referenced.contains(&address) {
            return Err(unclassified(format!(
                "four function pointers at {address:#010x} begin with {:#010x}, whose return \
                 the DWARF does not state and whose code does not take their address",
                slots[0]
            )));
        }
        census.count(address, slots)?;
    }
    for address in types.static_objects(RAW_WAKER_VTABLE, &mut scratch, control)? {
        control.checkpoint(1)?;
        // The link discarded a static outside every allocated section (its
        // DWARF location is a tombstone): it has no storage a waker reaches.
        let stored = image
            .file()
            .sections()
            .any(|section| section.allocated && section.contains(u64::from(address)));
        if stored && census.found.is_none_or(|(vtable, _)| vtable != address) {
            return Err(unclassified(format!(
                "a static `RawWakerVTable` at {address:#010x} holds no waker vtable the \
                 relocations show"
            )));
        }
    }
    Ok(census.found.map(|(_, vtable)| vtable))
}

/// Targets proved by Waker method scopes or typed vtable locations with
/// constant local address arithmetic. `analysis`, `dwarf` and `types` must
/// describe the same ELF (companions contribute no typed locations).
/// Without a vtable each such site gets an empty set, a hole.
pub fn waker_resolutions(
    analysis: &Analysis,
    dwarf: &Dwarf,
    types: &TypeFacts,
    vtable: Option<&[u32; 4]>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let file = types
        .source
        .as_ref()
        .map(|source| {
            oer_analysis_elf::Elf::parse(source)
                .map_err(|_| crate::dwarf::invalid("invalid typed-location ELF"))
        })
        .transpose()?;
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
                    if let Some(slot) = chain.first().and_then(|function| slot(function)) {
                        if load.offset != 4 * slot as i32 {
                            known = false;
                            break;
                        }
                        scratch.add(&mut slots, slot, control)?;
                    } else if let Some(file) = &file
                        && let Some(proven) = crate::waker_locations::slots(
                            file,
                            facts,
                            types,
                            load,
                            transfer.site,
                            &mut scratch,
                            control,
                        )?
                    {
                        for slot in proven {
                            scratch.add(&mut slots, slot, control)?;
                        }
                    } else {
                        known = false;
                        break;
                    }
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
