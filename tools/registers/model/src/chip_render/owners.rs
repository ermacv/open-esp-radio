//! Ownership: the owner type of every part, the divisions that let parts
//! of one instance observe or split each other's words, the partitions and
//! the one claim of the PAC root.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::{
    Error,
    block::{Call, Mode, Template, TypePlan, render_methods, render_split_triggers},
    irreducible, names,
};
use crate::{
    chip_model::{ChipModel, Location},
    chip_policy::{ChipPolicy, Claim, Holder, Part, PartId},
    chip_render::names::push_doc,
};

/// The words one part reaches, as byte offsets from its instance's base.
pub(super) struct PartWords {
    pub(super) owned: BTreeSet<u64>,
    pub(super) observed: BTreeSet<u64>,
    /// Words of write-trigger fields split by field: the part's fields.
    pub(super) triggers: BTreeMap<u64, BTreeSet<String>>,
}

/// One part and where it lives.
pub(super) struct Owner<'p> {
    pub(super) id: PartId,
    pub(super) part: &'p Part,
    pub(super) partition: &'p str,
    pub(super) type_name: String,
    pub(super) base: u64,
    pub(super) words: PartWords,
    /// The division the part is a member of, if its instance's parts
    /// observe or split each other's words.
    pub(super) division: Option<usize>,
    /// The interrupt sources the policy names for the part.
    pub(super) sources: Vec<u16>,
}

impl Owner<'_> {
    pub(super) fn name(&self) -> &str {
        &self.part.name
    }

    fn mode(&self) -> Mode {
        if self.part.shared {
            Mode::Shared
        } else if self.part.read_only {
            Mode::Read
        } else {
            Mode::Owned
        }
    }

    /// The bits the part writes of a split trigger word at `offset`.
    pub(super) fn trigger_mask(&self, template: &Template<'_>, offset: u64) -> u32 {
        self.words.triggers.get(&offset).map_or(0, |fields| {
            template
                .fields
                .iter()
                .filter(|field| fields.contains(&field.field.name))
                .fold(0, |mask, field| mask | field.mask())
        })
    }

    /// The part's own words as its block lists them: held words and, for a
    /// part alone on its block, the split trigger words it is the only
    /// part to hold fields of.
    fn listed(&self) -> BTreeSet<u64> {
        let mut words = self.words.owned.clone();
        if self.division.is_none() {
            words.extend(self.words.triggers.keys());
        }
        words
    }
}

/// Parts of one instance that one block divides.
pub(super) struct Division {
    pub(super) instance: String,
    pub(super) type_name: String,
    pub(super) base: u64,
    /// Owner indices, in part order.
    pub(super) members: Vec<usize>,
}

impl Division {
    fn words(&self, owners: &[Owner<'_>]) -> BTreeSet<u64> {
        let mut words = BTreeSet::new();
        for owner in self.members.iter().filter_map(|member| owners.get(*member)) {
            words.extend(&owner.words.owned);
            words.extend(owner.words.triggers.keys());
        }
        words
    }

    fn camel(&self) -> String {
        format!("{}Division", names::camel(&self.instance))
    }

    fn snake(&self) -> String {
        names::snake(&self.instance)
    }
}

/// Every part's words and divisions.
pub(super) struct Ownership<'p> {
    pub(super) owners: Vec<Owner<'p>>,
    pub(super) divisions: Vec<Division>,
}

pub(super) fn plan<'p>(model: &ChipModel, policy: &'p ChipPolicy) -> Result<Ownership<'p>, Error> {
    let instances = model
        .instances()
        .iter()
        .map(|instance| (instance.name.as_str(), instance))
        .collect::<BTreeMap<_, _>>();
    // Each MMIO word's instance and offset.
    let mut words = BTreeMap::new();
    for register in model.registers() {
        let Location::Mmio { address } = register.location else {
            continue;
        };
        let Some(instance) = instances.get(register.instance.as_str()) else {
            continue;
        };
        words.insert(
            register.identity.as_str(),
            (register, address.wrapping_sub(instance.base)),
        );
    }
    let mut owners = Vec::new();
    let mut index = BTreeMap::new();
    for (partition_position, partition) in policy.partitions().iter().enumerate() {
        for (part_position, part) in partition.parts.iter().enumerate() {
            let id = PartId {
                partition: partition_position,
                part: part_position,
            };
            let entity = format!("{}.{}", partition.name, part.name);
            let instance = instances.get(part.instance.as_str()).ok_or_else(|| {
                Error::new(
                    &entity,
                    format!("instance {} is no MMIO instance", part.instance),
                )
            })?;
            // The policy names each part's interrupts; its loader checked
            // each source is bound to the part's instance.
            let sources = part
                .interrupt
                .iter()
                .filter_map(|name| {
                    model
                        .interrupts()
                        .iter()
                        .find(|interrupt| interrupt.name == *name)
                        .map(|interrupt| interrupt.source)
                })
                .collect();
            index.insert(id, owners.len());
            owners.push(Owner {
                id,
                part,
                partition: &partition.name,
                type_name: instance.block_type.clone(),
                base: instance.base,
                words: PartWords {
                    owned: BTreeSet::new(),
                    observed: BTreeSet::new(),
                    triggers: BTreeMap::new(),
                },
                division: None,
                sources,
            });
        }
    }
    // The words each part holds whole or by field.
    let mut splits: BTreeMap<(String, u64), BTreeSet<usize>> = BTreeMap::new();
    for (identity, claim) in policy.claims() {
        let Some((register, offset)) = words.get(identity.as_str()) else {
            continue;
        };
        match claim {
            Claim::Whole(Holder::Part(id)) => {
                if let Some(owner) = index.get(id).and_then(|position| owners.get_mut(*position)) {
                    owner.words.owned.insert(*offset);
                }
            }
            Claim::Whole(Holder::Unclaimed(_)) => {}
            Claim::Fields(fields) => {
                for (field, holder) in fields {
                    let Holder::Part(id) = holder else {
                        continue;
                    };
                    if let Some(position) = index.get(id)
                        && let Some(owner) = owners.get_mut(*position)
                    {
                        owner
                            .words
                            .triggers
                            .entry(*offset)
                            .or_default()
                            .insert(field.clone());
                        splits
                            .entry((register.instance.clone(), *offset))
                            .or_default()
                            .insert(*position);
                    }
                }
            }
        }
    }
    // What each part observes, and which parts must share one division.
    let holders = owners
        .iter()
        .enumerate()
        .flat_map(|(position, owner)| {
            owner
                .words
                .owned
                .iter()
                .map(move |offset| ((owner.part.instance.clone(), *offset), position))
        })
        .collect::<BTreeMap<_, _>>();
    let mut links = Vec::new();
    for (position, owner) in owners.iter_mut().enumerate() {
        let entity = format!("{}.{}", owner.partition, owner.part.name);
        for observed in &owner.part.observes {
            let mut found = false;
            for register in model.registers().iter().filter(|register| {
                register.instance == owner.part.instance
                    && (register.key == *observed
                        || register.identity == format!("{}.{observed}", owner.part.instance))
            }) {
                let Some((_, offset)) = words.get(register.identity.as_str()) else {
                    continue;
                };
                found = true;
                owner.words.observed.insert(*offset);
                let holder = holders
                    .get(&(owner.part.instance.clone(), *offset))
                    .ok_or_else(|| {
                        Error::new(
                            &entity,
                            format!("observes {}, which no part holds", register.identity),
                        )
                    })?;
                links.push((position, *holder));
            }
            if !found {
                return Err(Error::new(
                    &entity,
                    format!("observes {observed}, which is no word"),
                ));
            }
        }
    }
    for members in splits.values() {
        let mut members = members.iter();
        if let Some(first) = members.next() {
            for other in members {
                links.push((*first, *other));
            }
        }
    }
    // Connected parts form one division.
    let mut root = (0..owners.len()).collect::<Vec<_>>();
    fn find(root: &mut [usize], mut node: usize) -> usize {
        while let Some(&parent) = root.get(node) {
            if parent == node {
                break;
            }
            node = parent;
        }
        node
    }
    for (a, b) in links {
        let (a, b) = (find(&mut root, a), find(&mut root, b));
        if a != b
            && let Some(slot) = root.get_mut(a.max(b))
        {
            *slot = a.min(b);
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for position in 0..owners.len() {
        groups
            .entry(find(&mut root, position))
            .or_default()
            .push(position);
    }
    let mut divisions = Vec::new();
    for members in groups.into_values().filter(|members| members.len() > 1) {
        let Some(first) = members.first().and_then(|first| owners.get(*first)) else {
            continue;
        };
        let instance = first.part.instance.clone();
        if members.len() > 4 {
            return Err(Error::new(
                &instance,
                format!(
                    "{} parts observe or split each other's words; one block divides into at most four",
                    members.len()
                ),
            ));
        }
        if let Some(shared) = members
            .iter()
            .filter_map(|member| owners.get(*member))
            .find(|owner| owner.part.shared)
        {
            return Err(Error::new(
                &format!("{}.{}", shared.partition, shared.part.name),
                "a shared part is alone on its block: no part observes or splits its words",
            ));
        }
        let division = Division {
            instance,
            type_name: first.type_name.clone(),
            base: first.base,
            members: members.clone(),
        };
        for member in &members {
            if let Some(owner) = owners.get_mut(*member) {
                owner.division = Some(divisions.len());
            }
        }
        divisions.push(division);
    }
    Ok(Ownership { owners, divisions })
}

/// The owner items of the parts on instances of `plan`'s type, inside its
/// type module.
pub(super) fn render_type_owners(
    output: &mut String,
    plan: &TypePlan<'_>,
    ownership: &Ownership<'_>,
    model: &ChipModel,
    policy: &ChipPolicy,
) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    let mut claim = |name: String, entity: &str| -> Result<(), Error> {
        if names.insert(name.clone()) {
            Ok(())
        } else {
            Err(Error::new(
                entity,
                format!("its item `{name}` is another part's or division's"),
            ))
        }
    };
    for owner in ownership
        .owners
        .iter()
        .filter(|owner| owner.type_name == plan.name)
    {
        let entity = format!("{}.{}", owner.partition, owner.name());
        claim(owner.name().to_owned(), &entity)?;
        claim(format!("{}Words", owner.name()), &entity)?;
        claim(format!("{}Claim", owner.name()), &entity)?;
        render_owner(output, plan, ownership, owner, model, policy)?;
    }
    for division in ownership
        .divisions
        .iter()
        .filter(|division| division.type_name == plan.name)
    {
        claim(division.camel(), &division.instance)?;
        claim(format!("{}Claim", division.camel()), &division.instance)?;
        render_division(output, division, &ownership.owners);
    }
    Ok(())
}

fn list(words: impl IntoIterator<Item = u64>) -> String {
    words
        .into_iter()
        .map(|word| format!("0x{word:x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_owner(
    output: &mut String,
    plan: &TypePlan<'_>,
    ownership: &Ownership<'_>,
    owner: &Owner<'_>,
    model: &ChipModel,
    policy: &ChipPolicy,
) -> Result<(), Error> {
    let name = owner.name();
    let entity = format!("{}.{}", owner.partition, name);
    let base = owner.base;
    let words = format!("{name}Words");
    let copy = owner.part.read_only && owner.part.copy;
    let registers = match owner.division {
        Some(division) => ownership
            .divisions
            .get(division)
            .map_or_else(String::new, Division::camel),
        None => words.clone(),
    };
    let inner = match (owner.division, copy) {
        (_, true) => format!("oer_mmio::Reader<P, {registers}, {words}>"),
        (None, false) => format!("oer_mmio::Block<P, {words}>"),
        (Some(_), false) => format!("oer_mmio::Part<P, {registers}, {words}>"),
    };
    // Every word the part reaches is a 32-bit word its block lists.
    for offset in owner
        .listed()
        .iter()
        .chain(&owner.words.observed)
        .chain(owner.words.triggers.keys())
    {
        let Some((template, element)) = plan.element(*offset) else {
            return Err(Error::new(
                &entity,
                format!("no register lies at 0x{offset:x}"),
            ));
        };
        if template.narrow() {
            return Err(Error::new(
                &entity,
                format!(
                    "it reaches the {}-bit word {}; an owner reaches only 32-bit words",
                    template.width, element.name
                ),
            ));
        }
    }
    let _ = writeln!(
        output,
        "/// The `{}` part of `{}`.",
        name, owner.part.instance
    );
    if let Some(description) = &owner.part.description {
        output.push_str("///\n");
        push_doc(output, &super::block::clean(description), "");
    }
    if !owner.part.sources.is_empty() {
        let _ = writeln!(
            output,
            "///\n/// SOURCE: {}.",
            owner.part.sources.join(", ")
        );
    }
    let must_use = if copy {
        ""
    } else {
        "#[must_use = \"dropping a register owner permanently loses its register authority\"]\n"
    };
    let _ = writeln!(
        output,
        "{must_use}pub struct {name}<P: oer_mmio::Placement = oer_mmio::Fixed<0x{base:x}>>({inner});\n"
    );
    if copy {
        let _ = writeln!(
            output,
            "impl<P: oer_mmio::Placement> Clone for {name}<P> {{\nfn clone(&self) -> Self {{\n*self\n}}\n}}\n\n\
             impl<P: oer_mmio::Placement> Copy for {name}<P> {{}}\n"
        );
    }
    // The word lists.
    let listed = owner.listed();
    match owner.division {
        None => {
            let _ = writeln!(
                output,
                "/// The words [`{name}`] holds, as byte offsets from its instance's base.\n\
                 pub struct {words};\n\n\
                 impl oer_mmio::Registers for {words} {{\nconst WORDS: &'static [usize] = &[{}];\n}}\n\n\
                 impl oer_mmio::Words for {words} {{\nconst OWNED: &'static [usize] = &[{}];\nconst OBSERVED: &'static [usize] = &[];\n}}\n",
                list(listed.iter().copied()),
                list(listed.iter().copied()),
            );
            if owner.part.shared {
                let _ = writeln!(
                    output,
                    "// SOURCE: {}. The shared part's words are side-effect-free reads,\n\
                     // write-trigger strobes and one-field words stored whole.\n\
                     impl oer_mmio::SharedRegisters for {words} {{}}\n",
                    owner.part.sources.join(", ")
                );
            }
            let _ = writeln!(
                output,
                "/// The part's device registers, as the PAC root claims them.\n\
                 pub type {name}Claim = oer_mmio::Block<oer_mmio::Fixed<0x{base:x}>, {words}>;\n"
            );
            let (wrap, wrap_host) = if copy {
                (
                    "Self(block.into_reader())".to_owned(),
                    "oer_mmio::Block::host(words).map(|block| Self(block.into_reader()))",
                )
            } else {
                (
                    "Self(block)".to_owned(),
                    "oer_mmio::Block::host(words).map(Self)",
                )
            };
            let _ = writeln!(
                output,
                "impl {name} {{\n/// The owner of the part's device registers.\n\
                 #[inline]\npub fn new(block: {name}Claim) -> Self {{\n{wrap}\n}}\n}}\n\n\
                 impl<'a> {name}<oer_mmio::Host<'a>> {{\n\
                 /// The part over host memory, for tests of its accessors.\n\
                 pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {{\n{wrap_host}\n}}\n}}\n"
            );
        }
        Some(_) => {
            let triggers = owner
                .words
                .triggers
                .keys()
                .map(|offset| {
                    let mask = plan
                        .element(*offset)
                        .map_or(0, |(template, _)| owner.trigger_mask(template, *offset));
                    format!("(0x{offset:x}, 0x{mask:08x})")
                })
                .collect::<Vec<_>>();
            let triggers = if triggers.is_empty() {
                String::new()
            } else {
                format!(
                    "const TRIGGERS: &'static [(usize, u32)] = &[{}];\n",
                    triggers.join(", ")
                )
            };
            let _ = writeln!(
                output,
                "/// The words [`{name}`] holds, observes and shares by field.\n\
                 pub struct {words};\n\n\
                 impl oer_mmio::Words for {words} {{\n\
                 const OWNED: &'static [usize] = &[{}];\n\
                 const OBSERVED: &'static [usize] = &[{}];\n{triggers}}}\n",
                list(owner.words.owned.iter().copied()),
                list(owner.words.observed.iter().copied()),
            );
        }
    }
    // One source names the part's `InterruptSource`; several, only its
    // `InterruptSources`, so the part never stands where one is expected.
    match owner.sources.as_slice() {
        [] => {}
        [source] => {
            let _ = writeln!(
                output,
                "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for {name}<P> {{\nconst SOURCE: u16 = {source};\n}}\n"
            );
        }
        sources => {
            let _ = writeln!(
                output,
                "impl<P: oer_mmio::Placement> oer_mmio::InterruptSources for {name}<P> {{\nconst SOURCES: &'static [u16] = &[{}];\n}}\n",
                sources
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    // The accessors.
    let mut body = String::new();
    let mode = owner.mode();
    // A snapshot's clear word is written only by its acknowledgement.
    let mut acknowledged = BTreeSet::new();
    for snapshot in &policy.file().interrupt_snapshots {
        if policy.owner(&snapshot.name) == Some(owner.id)
            && let Some(register) = model.register(&snapshot.clear)
            && let Location::Mmio { address } = register.location
        {
            acknowledged.insert(address.wrapping_sub(owner.base));
        }
    }
    for template in &plan.templates {
        let offsets = template
            .elements
            .iter()
            .map(|element| element.offset)
            .collect::<BTreeSet<_>>();
        for (reached, mode) in [
            (&owner.words.owned, mode),
            (&owner.words.observed, Mode::Read),
        ] {
            let held = offsets
                .iter()
                .filter(|offset| reached.contains(offset))
                .collect::<Vec<_>>();
            if held.is_empty() {
                continue;
            }
            let whole = held.len() == offsets.len()
                && !held.iter().any(|offset| acknowledged.contains(*offset));
            match (template.array, whole, template.templated()) {
                (Some((offset, stride, count)), true, _) => render_methods(
                    &mut body,
                    template,
                    &template.stem,
                    Call::Indexed {
                        offset,
                        stride,
                        count,
                    },
                    mode,
                )?,
                (_, true, false) => {
                    if let Some(offset) = held.first() {
                        render_methods(
                            &mut body,
                            template,
                            &template.stem,
                            Call::Fixed(**offset),
                            mode,
                        )?;
                    }
                }
                _ => {
                    for element in template
                        .elements
                        .iter()
                        .filter(|element| reached.contains(&element.offset))
                    {
                        render_methods(
                            &mut body,
                            template,
                            &element.stem,
                            Call::Fixed(element.offset),
                            if acknowledged.contains(&element.offset) {
                                Mode::Read
                            } else {
                                mode
                            },
                        )?;
                    }
                }
            }
        }
        for element in &template.elements {
            if let Some(fields) = owner.words.triggers.get(&element.offset)
                && !acknowledged.contains(&element.offset)
            {
                let stem = if template.templated() {
                    &element.stem
                } else {
                    &template.stem
                };
                if mode == Mode::Owned {
                    render_split_triggers(
                        &mut body,
                        template,
                        stem,
                        element.offset,
                        fields,
                        owner.division.is_some(),
                    );
                }
            }
        }
    }
    irreducible::render(&mut body, output, plan, owner, model, policy)?;
    if !body.is_empty() {
        let _ = writeln!(
            output,
            "impl<P: oer_mmio::Placement> {name}<P> {{\n{body}}}\n"
        );
    }
    Ok(())
}

fn render_division(output: &mut String, division: &Division, owners: &[Owner<'_>]) {
    let name = division.camel();
    let snake = division.snake();
    let base = division.base;
    let members = division
        .members
        .iter()
        .filter_map(|member| owners.get(*member))
        .collect::<Vec<_>>();
    let parts = members
        .iter()
        .map(|owner| format!("{}<P>", owner.name()))
        .collect::<Vec<_>>()
        .join(", ");
    let words = members
        .iter()
        .map(|owner| format!("{}Words", owner.name()))
        .collect::<Vec<_>>()
        .join(", ");
    let bindings = members
        .iter()
        .map(|owner| names::camel_snake(owner.name()))
        .collect::<Vec<_>>();
    let wrapped = members
        .iter()
        .zip(&bindings)
        .map(|(owner, binding)| {
            if owner.part.read_only && owner.part.copy {
                format!("{}({binding}.into_reader())", owner.name())
            } else {
                format!("{}({binding})", owner.name())
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        output,
        "/// The words of `{}` its parts {} hold, which one block divides:\n\
         /// the parts observe or split each other's words.\n\
         pub struct {name};\n\n\
         impl oer_mmio::Registers for {name} {{\nconst WORDS: &'static [usize] = &[{}];\n}}\n\n\
         /// The divided words' device registers, as the PAC root claims them.\n\
         pub type {name}Claim = oer_mmio::Block<oer_mmio::Fixed<0x{base:x}>, {name}>;\n\n\
         /// Divide the block into its parts.\n\
         #[inline]\npub fn {snake}_divide<P: oer_mmio::Placement>(block: oer_mmio::Block<P, {name}>) -> ({parts},) {{\n\
         let ({},) = block.divide::<({words},)>();\n\
         ({wrapped},)\n}}\n\n\
         /// The parts over host memory, for tests of their accessors.\n\
         #[allow(clippy::type_complexity, reason = \"one owner per part\")]\n\
         pub fn {snake}_host(words: &mut [u32]) -> Result<({},), oer_mmio::HostTooSmall> {{\n\
         oer_mmio::Block::host(words).map({snake}_divide)\n}}\n",
        division.instance,
        members
            .iter()
            .map(|owner| format!("[`{}`]", owner.name()))
            .collect::<Vec<_>>()
            .join(", "),
        list(division.words(owners)),
        bindings.join(", "),
        members
            .iter()
            .map(|owner| format!("{}<oer_mmio::Host<'_>>", owner.name()))
            .collect::<Vec<_>>()
            .join(", "),
    );
}

/// A nested tuple of at most eight elements per level.
fn nest(items: &[String]) -> String {
    if items.len() <= 8 {
        return format!("({},)", items.join(", "));
    }
    let chunk = items.len().div_ceil(8);
    let chunks = items.chunks(chunk).map(nest).collect::<Vec<_>>();
    format!("({},)", chunks.join(", "))
}

/// The partitions, the one claim of every owned word and the partition of
/// that claim, with the compile-time check that no two claimed blocks list
/// one word.
pub(super) fn render_root(
    output: &mut String,
    ownership: &Ownership<'_>,
    policy: &ChipPolicy,
    plans: &BTreeMap<&str, TypePlan<'_>>,
) -> Result<(), Error> {
    let module = |type_name: &str| {
        plans
            .get(type_name)
            .map_or_else(|| names::snake(type_name), |plan| plan.module.clone())
    };
    output.push_str(
        "/// The ownership partitions of the typed register blocks and the one\n\
         /// claim the PAC root takes.\n\
         pub mod ownership {\n",
    );
    let mut members = BTreeSet::new();
    for partition in policy.partitions() {
        let member = names::ident(&partition.member);
        if !members.insert(member.clone()) {
            return Err(Error::new(
                &partition.name,
                "its member is another partition's",
            ));
        }
        push_doc(output, &super::block::clean(&partition.description), "");
        let _ = writeln!(output, "pub struct {} {{", partition.name);
        let mut fields = BTreeSet::new();
        for owner in ownership
            .owners
            .iter()
            .filter(|owner| owner.partition == partition.name)
        {
            let field = names::ident(&names::camel_snake(owner.name()));
            if !fields.insert(field.clone()) {
                return Err(Error::new(
                    &format!("{}.{}", partition.name, owner.name()),
                    "its member name is another part's",
                ));
            }
            let _ = writeln!(
                output,
                "/// The `{}` part.\npub {field}: super::{}::{},",
                owner.name(),
                module(&owner.type_name),
                owner.name()
            );
        }
        output.push_str("}\n\n");
    }
    output.push_str("/// Every partition.\npub struct Partitions {\n");
    for partition in policy.partitions() {
        let _ = writeln!(
            output,
            "/// [`{}`].\npub {}: {},",
            partition.name,
            names::ident(&partition.member),
            partition.name
        );
    }
    output.push_str("}\n\n");
    // One claim per standalone part and per division.
    let mut claims = Vec::new();
    let mut bindings = Vec::new();
    let mut blocks = Vec::new();
    let mut statements = String::new();
    for (position, owner) in ownership.owners.iter().enumerate() {
        if owner.division.is_some() {
            continue;
        }
        let path = format!("super::{}::{}", module(&owner.type_name), owner.name());
        claims.push(format!("{path}Claim"));
        let binding = format!("claim_{position}");
        let _ = writeln!(statements, "let owner_{position} = {path}::new({binding});");
        bindings.push(binding);
        blocks.push(format!(
            "(0x{:x}, <{path}Words as oer_mmio::Registers>::WORDS)",
            owner.base
        ));
    }
    for (position, division) in ownership.divisions.iter().enumerate() {
        let path = format!(
            "super::{}::{}",
            module(&division.type_name),
            division.camel()
        );
        claims.push(format!("{path}Claim"));
        let binding = format!("division_{position}");
        let owners = division
            .members
            .iter()
            .map(|member| format!("owner_{member}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            statements,
            "let ({owners},) = super::{}::{}_divide({binding});",
            module(&division.type_name),
            division.snake()
        );
        bindings.push(binding);
        blocks.push(format!(
            "(0x{:x}, <{path} as oer_mmio::Registers>::WORDS)",
            division.base
        ));
    }
    let _ = writeln!(
        output,
        "/// The device registers of every part, which the PAC root takes with\n\
         /// one `oer_mmio::assume_owned`.\n\
         pub type Claims = {};\n",
        nest(&claims)
    );
    let mut built = String::from("Partitions {\n");
    for partition in policy.partitions() {
        let _ = writeln!(
            built,
            "{}: {} {{",
            names::ident(&partition.member),
            partition.name
        );
        for (position, owner) in ownership
            .owners
            .iter()
            .enumerate()
            .filter(|(_, owner)| owner.partition == partition.name)
        {
            let _ = writeln!(
                built,
                "{}: owner_{position},",
                names::ident(&names::camel_snake(owner.name()))
            );
        }
        built.push_str("},\n");
    }
    built.push_str("}\n");
    let _ = writeln!(
        output,
        "/// Give every claimed part to its owner in its partition.\n\
         #[inline]\npub fn partition(claims: Claims) -> Partitions {{\n\
         let {} = claims;\n{statements}{built}}}\n\n\
         const _: () = assert!(\n\
         oer_mmio::check::disjoint_blocks(&[{}]),\n\
         \"no two claimed blocks list the same word\"\n);\n}}\n",
        nest(&bindings),
        blocks.join(", ")
    );
    Ok(())
}
