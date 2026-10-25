//! The thin PAC renderer over a chip root and its ownership policy.
//!
//! [`render`] turns a loaded [`ChipModel`] and [`ChipPolicy`] into the Rust
//! of the exact hardware model, over `oer-mmio` and without `unsafe`:
//!
//! - one module per MMIO block type: a register module per template with
//!   its masks, offsets, value types and `R`/`W`/`Snapshot` images, and
//!   the owner of every part the policy places on an instance of the type;
//! - field accessors by class, in the one naming convention of the crate
//!   README ("Field accessors"); names lose the type's vendor prefix (the
//!   longest run of whole segments every named register and group of the
//!   type starts with);
//! - one owner type per part: a part alone on its block, or a member of
//!   the one division of an instance whose parts observe or split each
//!   other's words; the partitions, the one claim of the PAC root and the
//!   compile-time check that no two claimed blocks list one word;
//! - the interrupt sources of the root (`Interrupt`), each MMIO instance's
//!   base and bound sources (`instances`), and `InterruptSource` on an
//!   interrupt handler's part whose instance has exactly one bound source;
//! - the irreducible policy items ([`irreducible`]): whole-word images,
//!   proven write-one-to-clear acknowledgements, interrupt snapshots and
//!   writes with sourced whole-word bits; every other transaction is HAL
//!   code over the accessors;
//! - the analog-I2C dictionary ([`analog`]): block codes, routes, byte
//!   addresses, fields and register facts;
//! - the memory layouts of the root, by the layouts renderer.
//!
//! A field the policy reads as an address has no setter until the type
//! states its pointer facts. A narrow (8- or 16-bit) register has its
//! register module but no owner accessor: a part that reaches one is
//! refused. Analog-I2C types have no owners: their bytes are reached through
//! the HAL's analog host.

use std::{collections::BTreeMap, fmt::Write as _};

use crate::{
    chip_model::{ChipModel, Space},
    chip_policy::ChipPolicy,
};
use names::push_doc;

mod analog;
mod block;
mod irreducible;
mod linker;
mod names;
mod owners;
#[cfg(test)]
mod tests;
mod unique;

use block::{TypePlan, render_register_module};
pub use linker::dma_window;

/// Why the renderer refused the model or the policy: the entity and the
/// reason.
#[derive(Debug, thiserror::Error)]
#[error("{entity}: {reason}")]
pub struct Error {
    pub entity: String,
    pub reason: String,
}

impl Error {
    fn new(entity: &str, reason: impl Into<String>) -> Self {
        Self {
            entity: entity.to_owned(),
            reason: reason.into(),
        }
    }
}

/// Top-level module names the renderer itself uses.
const RESERVED_MODULES: &[&str] = &["instances", "ownership", "layouts", "analog_i2c"];

/// Render the PAC of `model` under `policy`. The output is deterministic:
/// it follows the root's, the types' and the policy's order.
pub fn render(model: &ChipModel, policy: &ChipPolicy) -> Result<String, Error> {
    let (mut output, layouts) = render_parts(model, policy)?;
    output.push('\n');
    output.push_str(&layouts);
    Ok(output)
}

/// The register blocks and ownership, and the `layouts` module apart.
fn render_parts(model: &ChipModel, policy: &ChipPolicy) -> Result<(String, String), Error> {
    let mut plans = BTreeMap::new();
    for (name, block) in model.types() {
        if block.space() != Space::Mmio {
            continue;
        }
        let plan = TypePlan::new(block, model.ram_regions())?;
        if RESERVED_MODULES.contains(&plan.module.as_str()) {
            return Err(Error::new(name, "its module name is one the renderer uses"));
        }
        if let Some(other) = plans
            .values()
            .find(|other: &&TypePlan<'_>| other.module == plan.module)
        {
            return Err(Error::new(
                name,
                format!("its module `{}` is also type {}'s", plan.module, other.name),
            ));
        }
        plans.insert(name.as_str(), plan);
    }
    let ownership = owners::plan(model, policy)?;
    let mut output = String::from(
        "// The thin PAC of the chip root and its ownership policy, rendered by\n\
         // `cargo registers` over `oer-mmio`; no `unsafe`, no raw write.\n\n",
    );
    render_interrupts(&mut output, model);
    render_instances(&mut output, model, &plans);
    for plan in plans.values() {
        let _ = writeln!(output, "/// The `{}` block type.", plan.name);
        output.push_str("///\n");
        push_doc(
            &mut output,
            &block::clean(&plan.block.file.header.description),
            "",
        );
        let _ = writeln!(output, "pub mod {} {{", plan.module);
        for value in &plan.block.file.geometry {
            if let Some(description) = &value.description {
                push_doc(&mut output, description, "");
            }
            let _ = writeln!(
                output,
                "/// Hardware geometry. SOURCE: {}.\npub const {}: u32 = {};",
                value.sources.join(", "),
                value.name,
                value.value
            );
        }
        for template in &plan.templates {
            render_register_module(&mut output, template);
        }
        owners::render_type_owners(&mut output, plan, &ownership, model, policy)?;
        output.push_str("}\n\n");
    }
    owners::render_root(&mut output, &ownership, policy, &plans)?;
    output.push_str(&analog::render(model)?);
    let regions = model
        .ram_regions()
        .iter()
        .map(|region| {
            (
                region.name.clone(),
                (region.base, region.base.saturating_add(region.size)),
            )
        })
        .collect();
    let layouts = crate::layouts::render(model.layouts(), &regions)
        .map_err(|error| Error::new("layouts", error.to_string()))?;
    unique::check(&output)?;
    unique::check(&layouts)?;
    Ok((output, layouts))
}

fn render_interrupts(output: &mut String, model: &ChipModel) {
    let mut sources = model
        .interrupts()
        .iter()
        .map(|interrupt| {
            (
                interrupt.source,
                interrupt.name.as_str(),
                &interrupt.sources,
            )
        })
        .chain(model.unbound_interrupts().iter().map(|interrupt| {
            (
                interrupt.source,
                interrupt.name.as_str(),
                &interrupt.sources,
            )
        }))
        .collect::<Vec<_>>();
    sources.sort_by_key(|(source, ..)| *source);
    if sources.is_empty() {
        return;
    }
    output.push_str(
        "/// The chip's interrupt-matrix sources.\n\
         #[allow(non_camel_case_types, clippy::upper_case_acronyms, reason = \"the vendor's source names\")]\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         #[repr(u16)]\npub enum Interrupt {\n",
    );
    for (source, name, evidence) in &sources {
        let _ = writeln!(
            output,
            "/// SOURCE: {}.\n{name} = {source},",
            evidence.join(", ")
        );
    }
    output.push_str(
        "}\n\nimpl Interrupt {\n/// The source's number in the interrupt matrix.\n\
         #[inline]\npub const fn source(self) -> u16 {\nself as u16\n}\n\n\
         /// The source numbered `source`, if the chip has one.\n\
         pub const fn from_source(source: u16) -> Option<Self> {\nmatch source {\n",
    );
    for (source, name, _) in &sources {
        let _ = writeln!(output, "{source} => Some(Self::{name}),");
    }
    output.push_str("_ => None,\n}\n}\n}\n\n");
}

fn render_instances(output: &mut String, model: &ChipModel, plans: &BTreeMap<&str, TypePlan<'_>>) {
    output.push_str("/// The MMIO instances of the root: their bases and interrupt sources.\npub mod instances {\n");
    for instance in model.instances() {
        let Some(plan) = plans.get(instance.block_type.as_str()) else {
            continue;
        };
        let _ = writeln!(
            output,
            "/// The `{}` instance of [`{}`](super::{}).\n///\n/// SOURCE: {}.\n\
             pub mod {} {{\n/// The instance's base address.\npub const BASE: usize = 0x{:x};\n\
             /// The bytes of the instance's window.\npub const SIZE: usize = 0x{:x};\n",
            instance.name,
            plan.name,
            plan.module,
            instance.sources.join(", "),
            names::ident(&names::snake(&instance.name)),
            instance.base,
            instance.extent.size,
        );
        for interrupt in model
            .interrupts()
            .iter()
            .filter(|interrupt| interrupt.instance == instance.name)
        {
            let _ = writeln!(
                output,
                "/// The interrupt source of {}.\n\
                 #[allow(non_upper_case_globals, reason = \"the vendor's source name\")]\n\
                 pub const {}: super::super::Interrupt = super::super::Interrupt::{};\n",
                interrupt.output.as_ref().map_or_else(
                    || "the instance".to_owned(),
                    |output| format!("the output `{output}`")
                ),
                interrupt.name,
                interrupt.name
            );
        }
        output.push_str("}\n\n");
    }
    output.push_str("}\n\n");
}
