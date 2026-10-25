//! The generated controls of the root's links: a setter for each clock gate
//! and reset the root links to an instance, on the part that holds the
//! field's word.
//!
//! A link is a fact (which field gates or resets which instance), so its
//! setter is generated beside the field accessors; when and in which order
//! a gate opens or a reset is asserted is HAL logic. A control field
//! several instances share (`[[shared-controls]]`) gets one setter named
//! after the shared control, never one per user, so no user's name hides
//! that the bit is also another's. Mux links get none.

use std::{collections::BTreeSet, fmt::Write as _};

use super::{
    Error,
    block::{Mode, Setter, TypePlan},
    names,
    owners::Owner,
};
use crate::chip_model::{ChipModel, ControlKind, ExpandedControl, FieldClass};

/// One generated control: its method, the field it sets and its sources.
struct Control<'m> {
    method: String,
    what: String,
    kind: ControlKind,
    field: &'m str,
    address: u64,
    low: u32,
    sources: Vec<&'m str>,
}

/// The controls of the links whose field lies in a word `owner` holds, in
/// `body`: `set_<instance>_<role>_clock(on)` and
/// `set_<instance>_<role>_reset(asserted)`, or
/// `set_<shared control>_clock`/`_reset` for a shared field.
///
/// A control is generated for a one-bit `plain` field of a word the owner
/// holds with a read-modify-write accessor (`modify_<register>`) that sets
/// the field as a `bool` and needs no index; any other link (a mux, a
/// write-only or write-trigger reset, a field set through its value enum,
/// a field in an indexed array, an observed or unclaimed word) has none.
pub(super) fn render(
    body: &mut String,
    plan: &TypePlan<'_>,
    owner: &Owner<'_>,
    mode: Mode,
    model: &ChipModel,
    acknowledged: &BTreeSet<u64>,
) -> Result<(), Error> {
    if mode != Mode::Owned {
        return Ok(());
    }
    for control in controls(model) {
        let Some(offset) = control.address.checked_sub(owner.base) else {
            continue;
        };
        if !owner.words.owned.contains(&offset) || acknowledged.contains(&offset) {
            continue;
        }
        let Some((template, element)) = plan.element(offset) else {
            continue;
        };
        let held = template
            .elements
            .iter()
            .filter(|element| owner.words.owned.contains(&element.offset))
            .count();
        let whole = held == template.elements.len();
        let stem = match (template.array, whole, template.templated()) {
            // An indexed accessor: the control would need an index.
            (Some(_), true, _) => continue,
            (_, true, false) => &template.stem,
            _ => &element.stem,
        };
        let Some(field) = template
            .fields
            .iter()
            .find(|field| field.low == control.low && field.width == 1)
        else {
            continue;
        };
        if !template.modified() || !field.setter() || field.setter != Setter::Native {
            continue;
        }
        let (parameter, verb) = match control.kind {
            ControlKind::Clock => ("on", "Open (`true`) or close the"),
            ControlKind::Reset => ("asserted", "Assert (`true`) or release the"),
            ControlKind::Mux => continue,
        };
        let _ = writeln!(
            body,
            "/// {verb} {} (`{}`); every other bit keeps the value read.\n\
             ///\n/// SOURCE: {}.\n\
             #[inline]\npub fn {}(&mut self, {parameter}: bool) {{\n\
             self.modify_{stem}(|w| {{\nw.set_{}({parameter});\n}});\n}}\n",
            control.what,
            control.field,
            control.sources.join(", "),
            control.method,
            field.method(),
        );
    }
    Ok(())
}

/// Every gate and reset control of the root: one per unshared link of a
/// `plain` one-bit field, one per shared control.
fn controls(model: &ChipModel) -> Vec<Control<'_>> {
    let shared = model
        .shared_controls()
        .iter()
        .map(|control| control.field.as_str())
        .collect::<BTreeSet<_>>();
    let gate = |link: &ExpandedControl| {
        link.kind != ControlKind::Mux
            && link.field.class == FieldClass::Plain
            && link.field.width == 1
    };
    let suffix = |kind: ControlKind| match kind {
        ControlKind::Clock => "clock",
        ControlKind::Reset => "reset",
        ControlKind::Mux => "mux",
    };
    let mut controls = model
        .controls()
        .iter()
        .filter(|link| gate(link) && !shared.contains(link.field.identity.as_str()))
        .map(|link| Control {
            method: format!(
                "set_{}_{}_{}",
                names::snake(&link.instance),
                names::snake(&link.role),
                suffix(link.kind)
            ),
            what: format!(
                "`{}` {} of `{}`",
                link.role,
                match link.kind {
                    ControlKind::Clock => "clock",
                    _ => "reset",
                },
                link.instance
            ),
            kind: link.kind,
            field: &link.field.identity,
            address: link.field.address,
            low: link.field.low,
            sources: link.sources.iter().map(String::as_str).collect(),
        })
        .collect::<Vec<_>>();
    for shared in model.shared_controls() {
        let Some(link) = model
            .controls()
            .iter()
            .find(|link| link.field.identity == shared.field && gate(link))
        else {
            continue;
        };
        controls.push(Control {
            method: format!("set_{}_{}", names::snake(&shared.name), suffix(link.kind)),
            what: format!(
                "shared {} `{}` of {}",
                suffix(link.kind),
                shared.name,
                shared
                    .instances
                    .iter()
                    .map(|instance| format!("`{instance}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            kind: link.kind,
            field: &link.field.identity,
            address: link.field.address,
            low: link.field.low,
            sources: shared.sources.iter().map(String::as_str).collect(),
        });
    }
    controls
}
