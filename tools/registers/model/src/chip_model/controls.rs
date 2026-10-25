//! Fixed cross-instance hardware links, resolved against expanded MMIO fields.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    ExpandedRegister, FieldClass, Location, RootFile,
    block_type::{Context, ExpansionBudget, check_name},
    error::{Error, ErrorKind},
};

/// The kind of a fixed instance control.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlKind {
    Clock,
    Reset,
    Mux,
}

impl ControlKind {
    fn label(self) -> &'static str {
        match self {
            Self::Clock => "clock gate",
            Self::Reset => "reset",
            Self::Mux => "mux",
        }
    }
}

/// A checked field reference with its absolute register address and bit range.
#[derive(Clone, Debug)]
pub struct ResolvedField {
    pub identity: String,
    pub address: u64,
    pub low: u32,
    pub width: u32,
    pub class: FieldClass,
}

/// One instance's fixed link; mux inputs use checked numeric encodings.
#[derive(Clone, Debug)]
pub struct ExpandedControl {
    pub instance: String,
    pub role: String,
    pub kind: ControlKind,
    pub field: ResolvedField,
    pub inputs: BTreeMap<u32, String>,
    pub sources: Vec<String>,
}

/// A vendor's named gate group at absolute field identities and addresses.
#[derive(Clone, Debug)]
pub struct ExpandedGateGroup {
    pub name: String,
    pub fields: Vec<ResolvedField>,
    pub sources: Vec<String>,
}

pub(super) struct Controls {
    pub links: Vec<ExpandedControl>,
    pub groups: Vec<ExpandedGateGroup>,
}

pub(super) fn expand(
    context: Context<'_>,
    root: &RootFile,
    registers: &[ExpandedRegister],
    budget: &mut ExpansionBudget,
) -> Result<Controls, Error> {
    let count = preflight(context, root, budget)?;
    let fields = registers
        .iter()
        .filter_map(|register| match register.location {
            Location::Mmio { address } => Some(
                register
                    .fields
                    .iter()
                    .map(move |field| (field.identity.as_str(), (field, address))),
            ),
            Location::AnalogI2c(_) => None,
        })
        .flatten()
        .collect::<BTreeMap<_, _>>();
    let resolve = |entity: &str, identity: &str, kind: ControlKind| {
        let (field, address) = fields
            .get(identity)
            .ok_or_else(|| context.error(entity, ErrorKind::UnknownField(identity.to_owned())))?;
        if field.class != FieldClass::Plain || (kind != ControlKind::Mux && field.width != 1) {
            return Err(context.error(
                entity,
                ErrorKind::ControlField {
                    field: identity.to_owned(),
                    kind: kind.label(),
                    class: field.class.label(),
                    width: field.width,
                },
            ));
        }
        Ok(ResolvedField {
            identity: field.identity.clone(),
            address: *address,
            low: field.low,
            width: field.width,
            class: field.class,
        })
    };
    let clocks = check_clock_sources(context, root)?;
    let mut links = Vec::new();
    links
        .try_reserve_exact(count)
        .map_err(|error| context.error(&root.chip, ErrorKind::Allocation(error)))?;
    let mut users: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut mux_inputs = BTreeMap::new();
    for instance in &root.instances {
        let mut roles = BTreeSet::new();
        let controls = instance
            .clocks
            .iter()
            .map(|control| {
                (
                    ControlKind::Clock,
                    &control.role,
                    &control.field,
                    &control.sources,
                    None,
                )
            })
            .chain(instance.resets.iter().map(|control| {
                (
                    ControlKind::Reset,
                    &control.role,
                    &control.field,
                    &control.sources,
                    None,
                )
            }))
            .chain(instance.muxes.iter().map(|mux| {
                (
                    ControlKind::Mux,
                    &mux.role,
                    &mux.field,
                    &mux.sources,
                    Some(&mux.inputs),
                )
            }));
        for (kind, role, identity, sources, inputs) in controls {
            let entity = format!("{}.{kind:?}.{role}", instance.name);
            if role.is_empty()
                || !role
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err(context.error(
                    &entity,
                    ErrorKind::Control("role must be lowercase-kebab".to_owned()),
                ));
            }
            if !roles.insert((kind, role)) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Control("each kind/role occurs once per instance".to_owned()),
                ));
            }
            context.sources(&entity, sources)?;
            let field = resolve(&entity, identity, kind)?;
            let mut decoded = BTreeMap::new();
            if let Some(inputs) = inputs {
                if inputs.is_empty() {
                    return Err(context.error(
                        &entity,
                        ErrorKind::Control("mux needs at least one sourced input".to_owned()),
                    ));
                }
                let limit = 1u64.checked_shl(field.width).ok_or_else(|| {
                    context.error(
                        &entity,
                        ErrorKind::Control("mux field width exceeds the encoding space".to_owned()),
                    )
                })?;
                for (encoding, clock) in inputs {
                    let number = encoding.parse::<u32>().ok().filter(|number| {
                        number.to_string() == *encoding && u64::from(*number) < limit
                    });
                    let number = number.ok_or_else(|| {
                        context.error(
                            &entity,
                            ErrorKind::Control(format!(
                                "mux input {encoding:?} is not a canonical decimal encoding within the field"
                            )),
                        )
                    })?;
                    if !clocks.contains_key(clock.as_str()) {
                        return Err(context.error(
                            &entity,
                            ErrorKind::ClockSource(format!("unknown mux input {clock}")),
                        ));
                    }
                    decoded.insert(number, clock.clone());
                }
                if let Some(previous) = mux_inputs.insert(identity, inputs)
                    && previous != inputs
                {
                    return Err(context.error(
                        &entity,
                        ErrorKind::Control(
                            "users of one mux must state the same input map".to_owned(),
                        ),
                    ));
                }
            }
            users.entry(identity).or_default().insert(&instance.name);
            links.push(ExpandedControl {
                instance: instance.name.clone(),
                role: role.clone(),
                kind,
                field,
                inputs: decoded,
                sources: sources.clone(),
            });
        }
    }
    check_shared(context, root, &users)?;
    let mut names = BTreeSet::new();
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(root.gate_groups.len())
        .map_err(|error| context.error(&root.chip, ErrorKind::Allocation(error)))?;
    for group in &root.gate_groups {
        check_name(&group.name, false).map_err(|kind| context.error(&group.name, kind))?;
        if !names.insert(&group.name) {
            return Err(context.error(
                &group.name,
                ErrorKind::Duplicate(format!("gate group {}", group.name)),
            ));
        }
        context.sources(&group.name, &group.sources)?;
        let distinct = group.fields.iter().collect::<BTreeSet<_>>();
        if group.fields.is_empty() || distinct.len() != group.fields.len() {
            return Err(context.error(
                &group.name,
                ErrorKind::GateGroup("needs nonempty, distinct gate fields".to_owned()),
            ));
        }
        let mut resolved = Vec::new();
        resolved
            .try_reserve_exact(group.fields.len())
            .map_err(|error| context.error(&group.name, ErrorKind::Allocation(error)))?;
        for field in &group.fields {
            resolved.push(resolve(&group.name, field, ControlKind::Clock)?);
        }
        groups.push(ExpandedGateGroup {
            name: group.name.clone(),
            fields: resolved,
            sources: group.sources.clone(),
        });
    }
    Ok(Controls { links, groups })
}

/// Charge copied text and relation records before building any lookup or output.
/// Saturation is deliberate: an overflow must exceed the existing chip budget.
fn preflight(
    context: Context<'_>,
    root: &RootFile,
    budget: &mut ExpansionBudget,
) -> Result<usize, Error> {
    let strings = |values: &[String]| {
        values.iter().fold(0u64, |sum, value| {
            sum.saturating_add(value.len() as u64)
                .saturating_add(std::mem::size_of::<String>() as u64)
        })
    };
    let mut count = 0usize;
    for instance in &root.instances {
        let mut link = |role: &str,
                        field: &str,
                        sources: &[String],
                        inputs: u64,
                        text: u64|
         -> Result<(), Error> {
            let text = (instance.name.len() as u64)
                .saturating_add(role.len() as u64)
                .saturating_add(field.len() as u64)
                .saturating_add(strings(sources))
                .saturating_add(text);
            let records = 1u64
                .saturating_add(sources.len() as u64)
                .saturating_add(inputs);
            budget.include(context, &instance.name, 1, records, text)?;
            // Each link was charged above; the record limit fits host usize.
            count = count.saturating_add(1);
            Ok(())
        };
        for control in instance.clocks.iter().chain(&instance.resets) {
            link(&control.role, &control.field, &control.sources, 0, 0)?;
        }
        for mux in &instance.muxes {
            let text = mux.inputs.values().fold(0u64, |sum, clock| {
                sum.saturating_add(clock.len() as u64)
                    .saturating_add(std::mem::size_of::<String>() as u64)
            });
            link(
                &mux.role,
                &mux.field,
                &mux.sources,
                mux.inputs.len() as u64,
                text,
            )?;
        }
    }
    for group in &root.gate_groups {
        budget.include(
            context,
            &group.name,
            1,
            1u64.saturating_add(group.fields.len() as u64)
                .saturating_add(group.sources.len() as u64),
            (group.name.len() as u64)
                .saturating_add(strings(&group.fields))
                .saturating_add(strings(&group.sources)),
        )?;
    }
    for shared in &root.shared_controls {
        budget.include(
            context,
            &shared.name,
            1,
            1u64.saturating_add(shared.instances.len() as u64),
            0,
        )?;
    }
    for clock in &root.clock_sources {
        budget.include(context, &clock.name, 1, 1, 0)?;
    }
    Ok(count)
}

fn check_shared(
    context: Context<'_>,
    root: &RootFile,
    users: &BTreeMap<&str, BTreeSet<&str>>,
) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    for shared in &root.shared_controls {
        let fail = |reason: String| context.error(&shared.name, ErrorKind::SharedControl(reason));
        check_name(&shared.name, false).map_err(|kind| context.error(&shared.name, kind))?;
        context.sources(&shared.name, &shared.sources)?;
        if !names.insert(&shared.name) || !fields.insert(shared.field.as_str()) {
            return Err(fail("name and field must be unique".to_owned()));
        }
        let listed = shared
            .instances
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if listed.len() < 2 || listed.len() != shared.instances.len() {
            return Err(fail(
                "needs at least two distinct instance users".to_owned(),
            ));
        }
        for instance in &listed {
            if !root.instances.iter().any(|known| known.name == *instance) {
                return Err(context.error(
                    &shared.name,
                    ErrorKind::UnknownInstance((*instance).to_owned()),
                ));
            }
        }
        if users.get(shared.field.as_str()) != Some(&listed) {
            return Err(fail(format!(
                "{} must list exactly the instances that reference the field",
                shared.field
            )));
        }
    }
    for (field, instances) in users {
        if instances.len() >= 2 && !fields.contains(field) {
            return Err(context.error(
                *field,
                ErrorKind::SharedControl(
                    "field has multiple instance users but no shared-controls entry".to_owned(),
                ),
            ));
        }
    }
    Ok(())
}

fn check_clock_sources<'a>(
    context: Context<'_>,
    root: &'a RootFile,
) -> Result<BTreeMap<&'a str, &'a super::ClockSource>, Error> {
    let mut clocks = BTreeMap::new();
    for clock in &root.clock_sources {
        check_name(&clock.name, false).map_err(|kind| context.error(&clock.name, kind))?;
        context.sources(&clock.name, &clock.sources)?;
        if clocks.insert(clock.name.as_str(), clock).is_some() {
            return Err(context.error(
                &clock.name,
                ErrorKind::Duplicate(format!("clock source {}", clock.name)),
            ));
        }
        if clock.frequency_hz == Some(0) {
            return Err(context.error(
                &clock.name,
                ErrorKind::ClockSource("frequency must be positive when known".to_owned()),
            ));
        }
    }
    let mut checked = BTreeSet::new();
    for clock in &root.clock_sources {
        let mut seen = BTreeSet::new();
        let mut parent = Some(clock.name.as_str());
        while let Some(name) = parent {
            if checked.contains(name) {
                break;
            }
            let ancestor = clocks.get(name).ok_or_else(|| {
                context.error(
                    &clock.name,
                    ErrorKind::ClockSource(format!("unknown parent {name}")),
                )
            })?;
            if !seen.insert(name) {
                return Err(context.error(
                    &clock.name,
                    ErrorKind::ClockSource("parent cycle".to_owned()),
                ));
            }
            parent = ancestor.parent.as_deref();
        }
        checked.extend(seen);
    }
    Ok(clocks)
}
