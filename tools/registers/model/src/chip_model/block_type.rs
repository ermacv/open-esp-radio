//! One block type file: loading, every check on its own, the elements it
//! expands to and its resolved review.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use oer_registers_contracts::NameOrigin;

use super::{
    Error, ErrorKind,
    source::{
        Command, Field, FieldClass, Group, Register, ReviewOverride, Space, TYPE_SCHEMA, TypeFile,
    },
};
use crate::{Position, ReviewAnnotation, opaque_position_name};

/// Where a check runs: the file and the evidence catalogs' source IDs.
#[derive(Clone, Copy)]
pub(super) struct Context<'a> {
    pub(super) path: &'a Path,
    pub(super) evidence: &'a BTreeSet<String>,
}

impl Context<'_> {
    pub(super) fn error(&self, entity: impl Into<String>, kind: ErrorKind) -> Error {
        Error::new(self.path, entity, kind)
    }

    /// Sources are nonempty, distinct, nonblank and defined in a catalog.
    pub(super) fn sources(&self, entity: &str, sources: &[String]) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        if sources.is_empty()
            || sources
                .iter()
                .any(|source| source.trim().is_empty() || !seen.insert(source))
        {
            return Err(self.error(entity, ErrorKind::InvalidSources));
        }
        if let Some(unknown) = sources
            .iter()
            .find(|source| !self.evidence.contains(source.as_str()))
        {
            return Err(self.error(entity, ErrorKind::UnknownSource(unknown.clone())));
        }
        Ok(())
    }
}

/// Bound both record overhead and owned text before materializing repetitions.
/// Separate budgets cover all included types and all expanded chip relations.
#[derive(Default)]
pub(super) struct ExpansionBudget {
    records: u64,
    text: u64,
}

impl ExpansionBudget {
    pub(super) fn include(
        &mut self,
        context: Context<'_>,
        entity: &str,
        count: u64,
        records: u64,
        text: u64,
    ) -> Result<(), Error> {
        self.records = self.records.saturating_add(count.saturating_mul(records));
        self.text = self.text.saturating_add(count.saturating_mul(text));
        let exceeded = if self.records > 65_536 {
            Some("65,536 expanded records")
        } else if self.text > 64 * 1024 * 1024 {
            Some("64 MiB of expanded text")
        } else {
            None
        };
        if let Some(limit) = exceeded {
            return Err(context.error(entity, ErrorKind::ExpansionLimit(limit)));
        }
        Ok(())
    }
}

/// A name: `UPPER_SNAKE` from a letter, with exactly one `%s` where
/// `indexed`, and none otherwise.
pub(super) fn check_name(name: &str, indexed: bool) -> Result<(), ErrorKind> {
    let placeholders = name.matches("%s").count();
    if placeholders != usize::from(indexed) {
        return Err(ErrorKind::InvalidName(if indexed {
            format!("{name:?} must hold exactly one %s")
        } else {
            format!("{name:?} holds %s but repeats nothing")
        }));
    }
    let plain = name.replace("%s", "0");
    let valid = plain.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && plain
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(ErrorKind::InvalidName(format!(
            "{name:?} is not UPPER_SNAKE starting with a letter"
        )))
    }
}

/// `"n"` or `"high:low"` as (low, width).
pub(super) fn parse_bits(bits: &str) -> Result<(u32, u32), ErrorKind> {
    let invalid = |reason: &str| ErrorKind::FieldBits {
        bits: bits.to_owned(),
        reason: reason.to_owned(),
    };
    let number = |text: &str| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| invalid("not a bit number"))
    };
    let (high, low) = match bits.split_once(':') {
        Some((high, low)) => (number(high)?, number(low)?),
        None => {
            let bit = number(bits)?;
            (bit, bit)
        }
    };
    if high < low {
        return Err(invalid("high is below low"));
    }
    if high > 31 {
        return Err(invalid("above bit 31"));
    }
    Ok((low, high - low + 1))
}

pub(super) fn mask(low: u32, width: u32) -> u64 {
    ((1u64 << width) - 1) << low
}

fn fits(value: u32, width: u32) -> bool {
    u64::from(value) >> width == 0
}

/// One element of a type: a register, one `dim` element or one register
/// of one group element.
#[derive(Clone, Debug)]
pub struct Element {
    /// The template key: `REGISTER` or `GROUP.REGISTER`, `%s` included.
    pub key: String,
    /// The expanded name below the instance: `REGISTER`, `REGCLK_1`, `T1.CONFIG`.
    pub name: String,
    /// From the block's base.
    pub offset: u64,
    /// Bits.
    pub width: u32,
    /// The group template the element repeats in, if any.
    pub group: Option<String>,
}

/// A validated block type.
#[derive(Clone, Debug)]
pub struct BlockType {
    pub path: PathBuf,
    pub file: TypeFile,
    /// Every register template by key.
    pub templates: BTreeMap<String, Register>,
    /// Every element, in offset order.
    pub elements: Vec<Element>,
    /// The resolved review of every template register (`KEY`) and field
    /// (`KEY.FIELD`).
    pub reviews: BTreeMap<String, ReviewAnnotation>,
}

impl BlockType {
    pub fn name(&self) -> &str {
        &self.file.header.name
    }

    pub fn space(&self) -> Space {
        self.file.header.space
    }

    /// Read and check one type file.
    pub(super) fn load(
        path: &Path,
        evidence: &BTreeSet<String>,
        budget: &mut ExpansionBudget,
    ) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::new(path, "", ErrorKind::Read(error)))?;
        let file: TypeFile = toml_edit::de::from_str(&text)
            .map_err(|error| Error::new(path, "", ErrorKind::Parse(error.to_string())))?;
        Self::check(Context { path, evidence }, path.to_owned(), file, budget)
    }

    fn check(
        context: Context<'_>,
        path: PathBuf,
        file: TypeFile,
        budget: &mut ExpansionBudget,
    ) -> Result<Self, Error> {
        let name = file.header.name.clone();
        if file.schema != TYPE_SCHEMA {
            return Err(context.error(
                &name,
                ErrorKind::Schema {
                    expected: TYPE_SCHEMA,
                    found: file.schema,
                },
            ));
        }
        check_name(&name, false).map_err(|kind| context.error(&name, kind))?;
        if file.header.description.trim().is_empty() {
            return Err(context.error(&name, ErrorKind::MissingDescription));
        }
        context.sources(&name, &file.header.sources)?;
        let space = file.header.space;

        let mut outputs = BTreeSet::new();
        for output in &file.interrupt_outputs {
            let entity = format!("{name}.{}", output.name);
            check_name(&output.name, false).map_err(|kind| context.error(&entity, kind))?;
            if !outputs.insert(output.name.as_str()) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Duplicate(format!("interrupt output {}", output.name)),
                ));
            }
            context.sources(&entity, &output.sources)?;
        }

        let mut templates = BTreeMap::new();
        let mut top = BTreeSet::new();
        for register in &file.registers {
            let entity = format!("{name}.{}", register.name);
            check_register(context, space, &entity, register)?;
            if !top.insert(register.name.as_str()) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Duplicate(format!("register {}", register.name)),
                ));
            }
            templates.insert(register.name.clone(), register.clone());
        }
        for group in &file.groups {
            let entity = format!("{name}.{}", group.name);
            check_group(context, space, &entity, group)?;
            if !top.insert(group.name.as_str()) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Duplicate(format!("register or group {}", group.name)),
                ));
            }
            for register in &group.registers {
                templates.insert(
                    format!("{}.{}", group.name, register.name),
                    register.clone(),
                );
            }
        }

        let count = preflight_elements(context, &file, budget)?;
        let elements = elements(context, &file, space, count)?;
        check_elements(context, space, &name, &elements)?;
        check_group_spans(context, &name, &file.groups, &elements)?;
        check_commands(context, &name, &file.commands, &templates)?;
        let reviews = resolve_reviews(context, &file, &templates)?;
        Ok(Self {
            path,
            file,
            templates,
            elements,
            reviews,
        })
    }
}

fn register_width(space: Space, register: &Register) -> u32 {
    register.width.unwrap_or_else(|| space.default_width())
}

fn check_register(
    context: Context<'_>,
    space: Space,
    entity: &str,
    register: &Register,
) -> Result<(), Error> {
    let fail = |kind| context.error(entity, kind);
    check_name(&register.name, register.dim.is_some()).map_err(fail)?;
    let width = register_width(space, register);
    let allowed: &[u32] = match space {
        Space::Mmio => &[8, 16, 32],
        Space::AnalogI2c => &[8],
    };
    if !allowed.contains(&width) {
        return Err(fail(ErrorKind::RegisterGeometry(format!(
            "a {} register is {allowed:?} bits wide, not {width}",
            space.label()
        ))));
    }
    let bytes = width / 8;
    if !register.offset.is_multiple_of(bytes) {
        return Err(fail(ErrorKind::RegisterGeometry(format!(
            "offset {:#x} is not aligned to the register's {bytes} bytes",
            register.offset
        ))));
    }
    if let Some(dim) = register.dim
        && (dim.count < 2 || dim.stride < bytes || !dim.stride.is_multiple_of(bytes))
    {
        return Err(fail(ErrorKind::Dim(format!(
            "count {} must be at least 2 and stride {:#x} a nonzero multiple of {bytes} bytes",
            dim.count, dim.stride
        ))));
    }

    let mut names = BTreeSet::new();
    let mut taken: Vec<(u64, &str)> = Vec::new();
    for field in &register.fields {
        let field_entity = format!("{entity}.{}", field.name);
        let fail = |kind| context.error(&field_entity, kind);
        check_name(&field.name, false).map_err(fail)?;
        if !names.insert(field.name.as_str()) {
            return Err(fail(ErrorKind::Duplicate(format!("field {}", field.name))));
        }
        let (low, field_width) = parse_bits(&field.bits).map_err(fail)?;
        if low + field_width > width {
            return Err(fail(ErrorKind::FieldBits {
                bits: field.bits.clone(),
                reason: format!("outside the {width}-bit register"),
            }));
        }
        let field_mask = mask(low, field_width);
        if let Some((_, other)) = taken.iter().find(|(other, _)| other & field_mask != 0) {
            return Err(fail(ErrorKind::FieldOverlap((*other).to_owned())));
        }
        taken.push((field_mask, field.name.as_str()));
        check_values(field, field_width).map_err(fail)?;
    }

    if let Some(reserved) = &register.reserved {
        let reserved_mask = u64::from(reserved.mask);
        if reserved.mask == 0 || !fits(reserved.mask, width) {
            return Err(fail(ErrorKind::Reserved(format!(
                "{:#x} is zero or wider than the register",
                reserved.mask
            ))));
        }
        if let Some((_, field)) = taken.iter().find(|(field, _)| field & reserved_mask != 0) {
            return Err(fail(ErrorKind::Reserved(format!("overlaps field {field}"))));
        }
        context.sources(entity, &reserved.sources)?;
    }

    if let Some(reset) = register.reset {
        let reset_mask = reset.mask.unwrap_or(u32::MAX >> (32 - width));
        if !fits(reset.value, width) || !fits(reset_mask, width) || reset.value & !reset_mask != 0 {
            return Err(fail(ErrorKind::Reset(format!(
                "value {:#x} and mask {reset_mask:#x} must fit the register, the value within the mask",
                reset.value
            ))));
        }
    }

    check_ports(context, entity, register)?;

    if let Some(read_safe) = &register.read_safe {
        if space != Space::AnalogI2c {
            return Err(fail(ErrorKind::AnalogOnly("read-safe")));
        }
        if read_safe.epoch.trim().is_empty()
            || read_safe
                .comparison_mask
                .is_some_and(|comparison| comparison == 0 || !fits(comparison, width))
        {
            return Err(fail(ErrorKind::ReadSafe(
                "needs an epoch and a nonzero comparison mask within the register".to_owned(),
            )));
        }
        if register.read_side_effect.is_some()
            || register.fifo.is_some()
            || register
                .fields
                .iter()
                .any(|field| field.class == FieldClass::ReadClears)
        {
            return Err(fail(ErrorKind::ReadSafe(
                "a read of this register changes the hardware".to_owned(),
            )));
        }
        let mut readable = 0u64;
        let mut described = 0u64;
        for field in &register.fields {
            let (low, width) = parse_bits(&field.bits).map_err(fail)?;
            let bits = mask(low, width);
            described |= bits;
            if field.class.readable() {
                readable |= bits;
            }
        }
        let comparison = read_safe.comparison_mask.map_or(described, u64::from);
        if readable == 0 || comparison & !readable != 0 {
            return Err(fail(ErrorKind::ReadSafe(
                "qualification needs readable fields and may compare only readable bits".to_owned(),
            )));
        }
        context.sources(entity, &read_safe.sources)?;
    }

    if !register.views.is_empty() && space != Space::AnalogI2c {
        return Err(fail(ErrorKind::AnalogOnly("a view")));
    }
    for view in &register.views {
        let view_entity = format!("{entity}.{}", view.name);
        let fail = |kind| context.error(&view_entity, kind);
        check_name(&view.name, false).map_err(fail)?;
        if !names.insert(view.name.as_str()) {
            return Err(fail(ErrorKind::Duplicate(format!(
                "field or view {}",
                view.name
            ))));
        }
        let Some(field) = register
            .fields
            .iter()
            .find(|field| field.name == view.field)
        else {
            return Err(fail(ErrorKind::View(format!(
                "field {} is not in the register",
                view.field
            ))));
        };
        let (field_low, field_width) = parse_bits(&field.bits).map_err(fail)?;
        let (low, view_width) = parse_bits(&view.bits).map_err(fail)?;
        let field_mask = mask(field_low, field_width);
        let view_mask = mask(low, view_width);
        if view_mask & !field_mask != 0 || view_mask == field_mask {
            return Err(fail(ErrorKind::View(format!(
                "bits {} are no strict subrange of {}",
                view.bits, field.name
            ))));
        }
        context.sources(&view_entity, &view.sources)?;
    }
    Ok(())
}

/// Named values and write constraints fit the field and name a writable
/// class.
fn check_values(field: &Field, width: u32) -> Result<(), ErrorKind> {
    let invalid = |reason: String| Err(ErrorKind::Values(reason));
    let mut names = BTreeSet::new();
    let mut values = BTreeSet::new();
    for value in &field.values {
        check_name(&value.name, false)?;
        if !names.insert(value.name.as_str()) || !values.insert(value.value) {
            return invalid(format!("value {} is named or numbered twice", value.name));
        }
        if !fits(value.value, width) {
            return invalid(format!(
                "value {} ({:#x}) is wider than the field",
                value.name, value.value
            ));
        }
    }
    let constrained = field.write_range.is_some() || field.write_values_only;
    if constrained && !field.class.written() {
        return invalid(format!(
            "a {} field takes no write constraint",
            field.class.label()
        ));
    }
    if field.write_values_only && (field.values.is_empty() || field.write_range.is_some()) {
        return invalid("write-values-only needs values and no write-range".to_owned());
    }
    if let Some(range) = field.write_range
        && (range.min > range.max || !fits(range.max, width))
    {
        return invalid(format!(
            "write-range {:#x}..={:#x} is reversed or wider than the field",
            range.min, range.max
        ));
    }
    Ok(())
}

/// A register is at most one of a FIFO port, a data port and a register
/// whose read changes the hardware, each with its shape.
fn check_ports(context: Context<'_>, entity: &str, register: &Register) -> Result<(), Error> {
    let fail = |reason: &str| context.error(entity, ErrorKind::Port(reason.to_owned()));
    let ports = [
        register.fifo.as_ref(),
        register.data_port.as_ref(),
        register.read_side_effect.as_ref(),
    ];
    if ports.iter().flatten().count() > 1 {
        return Err(fail(
            "is at most one of fifo, data-port and read-side-effect",
        ));
    }
    if register.fifo.is_some()
        && (register.dim.is_some()
            || !matches!(register.fields.as_slice(),
                [field] if !matches!(field.class, FieldClass::WriteOneToClear | FieldClass::Unknown)))
    {
        return Err(fail(
            "a FIFO port is one register, no dim, with one field that is neither write-one-to-clear nor unknown",
        ));
    }
    if register.data_port.is_some()
        && !matches!(register.fields.as_slice(),
            [field] if matches!(field.class, FieldClass::Plain | FieldClass::HardwareWritten))
    {
        return Err(fail("a data port has one plain or hardware-written field"));
    }
    for port in ports.into_iter().flatten() {
        context.sources(entity, &port.sources)?;
    }
    Ok(())
}

fn check_group(
    context: Context<'_>,
    space: Space,
    entity: &str,
    group: &Group,
) -> Result<(), Error> {
    let fail = |kind| context.error(entity, kind);
    check_name(&group.name, true).map_err(fail)?;
    if group.count < 2 || group.registers.is_empty() {
        return Err(fail(ErrorKind::Group(
            "needs a count of at least 2 and registers".to_owned(),
        )));
    }
    let mut names = BTreeSet::new();
    let mut span = 0u64;
    for register in &group.registers {
        let register_entity = format!("{entity}.{}", register.name);
        check_register(context, space, &register_entity, register)?;
        let bytes = register_width(space, register) / 8;
        if !group.offset.is_multiple_of(bytes) || !group.stride.is_multiple_of(bytes) {
            return Err(context.error(
                &register_entity,
                ErrorKind::Group(format!(
                    "offset and stride must preserve the member's {bytes}-byte alignment"
                )),
            ));
        }
        if !names.insert(register.name.as_str()) {
            return Err(context.error(
                &register_entity,
                ErrorKind::Duplicate(format!("register {}", register.name)),
            ));
        }
        let repeat = register
            .dim
            .map_or(0, |dim| u64::from(dim.count - 1) * u64::from(dim.stride));
        let end =
            u64::from(register.offset) + repeat + u64::from(register_width(space, register) / 8);
        span = span.max(end);
    }
    if u64::from(group.stride) < span {
        return Err(fail(ErrorKind::Group(format!(
            "stride {:#x} is shorter than its registers' {span:#x} bytes",
            group.stride
        ))));
    }
    Ok(())
}

/// Check the last byte and total expansion cost without iterating repetitions.
fn preflight_elements(
    context: Context<'_>,
    file: &TypeFile,
    budget: &mut ExpansionBudget,
) -> Result<usize, Error> {
    let space = file.header.space;
    let mut count = 0usize;
    let mut include = |register: &Register, group: Option<&Group>| {
        let key = match group {
            Some(group) => format!("{}.{}", group.name, register.name),
            None => register.name.clone(),
        };
        let entity = format!("{}.{key}", file.header.name);
        let fail = || {
            context.error(
                &entity,
                ErrorKind::RegisterGeometry("repeated span overflows".to_owned()),
            )
        };
        let (base, group_repeat, groups) = group.map_or((0, 0, 1), |group| {
            (
                u64::from(group.offset),
                u64::from(group.count - 1) * u64::from(group.stride),
                u64::from(group.count),
            )
        });
        let (repeat, registers) = register.dim.map_or((0, 1), |dim| {
            (
                u64::from(dim.count - 1) * u64::from(dim.stride),
                u64::from(dim.count),
            )
        });
        let end = base
            .checked_add(group_repeat)
            .and_then(|offset| offset.checked_add(u64::from(register.offset)))
            .and_then(|offset| offset.checked_add(repeat))
            .and_then(|offset| offset.checked_add(u64::from(register_width(space, register) / 8)))
            .ok_or_else(fail)?;
        if space == Space::AnalogI2c && end > 256 {
            return Err(context.error(
                &entity,
                ErrorKind::RegisterGeometry("repeated analog byte index is above 0xff".to_owned()),
            ));
        }
        let elements = groups.checked_mul(registers).ok_or_else(fail)?;
        // key, expanded name (up to two ten-digit indices), optional group.
        let text = (key.len() as u64)
            .saturating_mul(2)
            .saturating_add(group.map_or(0, |group| group.name.len() as u64))
            .saturating_add(22);
        budget.include(
            context,
            &entity,
            elements,
            1 + register.fields.len() as u64,
            text,
        )?;
        count += elements as usize; // The record budget fits usize on host targets.
        Ok(())
    };
    for register in &file.registers {
        include(register, None)?;
    }
    for group in &file.groups {
        for register in &group.registers {
            include(register, Some(group))?;
        }
    }
    Ok(count)
}

/// Every element of `file`, in offset order, after preflight bounded its cost.
fn elements(
    context: Context<'_>,
    file: &TypeFile,
    space: Space,
    count: usize,
) -> Result<Vec<Element>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(count)
        .map_err(|error| context.error(&file.header.name, ErrorKind::Allocation(error)))?;
    let mut push = |key: String,
                    name: &str,
                    base: u64,
                    prefix: &str,
                    group: Option<&str>,
                    register: &Register| {
        let group = group.map(str::to_owned);
        let width = register_width(space, register);
        let offset = base + u64::from(register.offset);
        match register.dim {
            None => out.push(Element {
                key,
                name: format!("{prefix}{name}"),
                offset,
                width,
                group,
            }),
            Some(dim) => {
                for index in 0..dim.count {
                    out.push(Element {
                        key: key.clone(),
                        name: format!("{prefix}{}", name.replace("%s", &index.to_string())),
                        offset: offset + u64::from(index) * u64::from(dim.stride),
                        width,
                        group: group.clone(),
                    });
                }
            }
        }
    };
    for register in &file.registers {
        push(register.name.clone(), &register.name, 0, "", None, register);
    }
    for group in &file.groups {
        for index in 0..group.count {
            let base = u64::from(group.offset) + u64::from(index) * u64::from(group.stride);
            let prefix = format!("{}.", group.name.replace("%s", &index.to_string()));
            for register in &group.registers {
                push(
                    format!("{}.{}", group.name, register.name),
                    &register.name,
                    base,
                    &prefix,
                    Some(&group.name),
                    register,
                );
            }
        }
    }
    out.sort_by_key(|element| element.offset);
    Ok(out)
}

/// Expanded names are unique, no two words overlap and an analog byte's
/// index is below 256.
fn check_elements(
    context: Context<'_>,
    space: Space,
    name: &str,
    elements: &[Element],
) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    for element in elements {
        if !names.insert(element.name.as_str()) {
            return Err(context.error(
                format!("{name}.{}", element.name),
                ErrorKind::Duplicate(format!("expanded register {}", element.name)),
            ));
        }
    }
    for element in elements {
        let entity = format!("{name}.{}", element.name);
        if !element.offset.is_multiple_of(u64::from(element.width / 8)) {
            return Err(context.error(
                &entity,
                ErrorKind::RegisterGeometry("expanded offset is not naturally aligned".to_owned()),
            ));
        }
        // A group element's name and a register's name share one scope.
        if let Some((group, _)) = element.name.split_once('.')
            && names.contains(group)
        {
            return Err(context.error(
                &entity,
                ErrorKind::Duplicate(format!("register or group element {group}")),
            ));
        }
        if space == Space::AnalogI2c && element.offset + u64::from(element.width / 8) > 256 {
            return Err(context.error(
                &entity,
                ErrorKind::RegisterGeometry(format!(
                    "analog byte index {:#x} is above 0xff",
                    element.offset
                )),
            ));
        }
    }
    for pair in elements.windows(2) {
        if let [first, second] = pair
            && first.offset + u64::from(first.width / 8) > second.offset
        {
            return Err(context.error(
                format!("{name}.{}", second.name),
                ErrorKind::WordOverlap {
                    offset: second.offset,
                    other: first.name.clone(),
                },
            ));
        }
    }
    Ok(())
}

/// A group occupies its whole span, from its first element's start to its
/// last element's end: no other register or group lies inside it.
fn check_group_spans(
    context: Context<'_>,
    name: &str,
    groups: &[Group],
    elements: &[Element],
) -> Result<(), Error> {
    let span = |group: &Group| {
        let members = elements
            .iter()
            .filter(|element| element.group.as_deref() == Some(group.name.as_str()));
        let start = u64::from(group.offset);
        let end = members
            .map(|element| element.offset + u64::from(element.width / 8))
            .max()
            .unwrap_or(start);
        (start, end)
    };
    for group in groups {
        let (start, end) = span(group);
        if let Some(element) = elements.iter().find(|element| {
            element.group.as_deref() != Some(group.name.as_str())
                && element.offset < end
                && element.offset + u64::from(element.width / 8) > start
        }) {
            return Err(context.error(
                format!("{name}.{}", element.name),
                ErrorKind::WordOverlap {
                    offset: element.offset,
                    other: format!("group {}", group.name),
                },
            ));
        }
    }
    Ok(())
}

/// A command register's commands, qualifiers and held bits are each one
/// self-clearing bit of it, together every one; it holds no write trigger,
/// no field a read clears and no field of unknown class, and is no port.
fn check_commands(
    context: Context<'_>,
    name: &str,
    commands: &[Command],
    templates: &BTreeMap<String, Register>,
) -> Result<(), Error> {
    let mut registers = BTreeSet::new();
    for command in commands {
        let entity = format!("{name}.{}", command.register);
        let fail = |reason: String| context.error(&entity, ErrorKind::Command(reason));
        let Some(register) = templates.get(&command.register) else {
            return Err(fail(format!(
                "register {} is not in the type",
                command.register
            )));
        };
        if !registers.insert(command.register.as_str()) {
            return Err(context.error(
                &entity,
                ErrorKind::Duplicate(format!("command register {}", command.register)),
            ));
        }
        let named = command
            .commands
            .iter()
            .chain(&command.qualifiers)
            .chain(&command.held)
            .collect::<Vec<_>>();
        let distinct = named
            .iter()
            .map(|field| field.as_str())
            .collect::<BTreeSet<_>>();
        if command.commands.is_empty() || distinct.len() != named.len() {
            return Err(fail(
                "needs one or more commands and names each field once".to_owned(),
            ));
        }
        for field_name in &named {
            let field = register
                .fields
                .iter()
                .find(|field| &field.name == *field_name);
            let one_bit = field.is_some_and(|field| {
                field.class == FieldClass::SelfClearing
                    && parse_bits(&field.bits).is_ok_and(|(_, width)| width == 1)
            });
            if !one_bit {
                return Err(fail(format!(
                    "{field_name} is not one self-clearing bit of the register"
                )));
            }
        }
        for field in &register.fields {
            let one_bit = parse_bits(&field.bits).is_ok_and(|(_, width)| width == 1);
            if field.class == FieldClass::SelfClearing
                && one_bit
                && !distinct.contains(field.name.as_str())
            {
                return Err(fail(format!(
                    "self-clearing bit {} is not named a command, qualifier or held bit",
                    field.name
                )));
            }
            if matches!(
                field.class,
                FieldClass::WriteTrigger | FieldClass::ReadClears | FieldClass::Unknown
            ) {
                return Err(fail(format!(
                    "holds the {} field {}",
                    field.class.label(),
                    field.name
                )));
            }
        }
        if register.fifo.is_some()
            || register.data_port.is_some()
            || register.read_side_effect.is_some()
        {
            return Err(fail(
                "is also a FIFO port, a data port or has a read side effect".to_owned(),
            ));
        }
        context.sources(&entity, &command.sources)?;
    }
    Ok(())
}

/// One reviewable entity of the type: its key, whether its name is opaque
/// and the positional name an opaque one must carry.
struct ReviewEntity {
    key: String,
    opaque: bool,
    position: String,
}

fn review_entities(
    context: Context<'_>,
    file: &TypeFile,
    templates: &BTreeMap<String, Register>,
) -> Result<Vec<ReviewEntity>, Error> {
    let space = file.header.space;
    let group_offsets =
        file.groups
            .iter()
            .flat_map(|group| {
                group.registers.iter().map(move |register| {
                    (format!("{}.{}", group.name, register.name), group.offset)
                })
            })
            .collect::<BTreeMap<_, _>>();
    let mut out = Vec::new();
    for (key, register) in templates {
        let offset = group_offsets
            .get(key)
            .copied()
            .unwrap_or(0)
            .checked_add(register.offset)
            .ok_or_else(|| {
                context.error(
                    format!("{}.{key}", file.header.name),
                    ErrorKind::RegisterGeometry(
                        "the first group member's offset exceeds u32".to_owned(),
                    ),
                )
            })?;
        let position = match space {
            Space::Mmio => {
                opaque_position_name(Position::Register { offset }, register.dim.is_some())
            }
            Space::AnalogI2c => format!("BYTE_{offset:02X}{}", crate::OPAQUE_SUFFIX),
        };
        out.push(ReviewEntity {
            key: key.clone(),
            opaque: register.name.ends_with(crate::OPAQUE_SUFFIX),
            position,
        });
        for field in &register.fields {
            let (low, width) = parse_bits(&field.bits).unwrap_or((0, 0));
            out.push(ReviewEntity {
                key: format!("{key}.{}", field.name),
                opaque: field.name.ends_with(crate::OPAQUE_SUFFIX),
                position: opaque_position_name(Position::Field { offset: low, width }, false),
            });
        }
    }
    Ok(out)
}

/// Every register's and field's review: its default (`[review]`, or
/// `[review.opaque]` with naming `opaque` for an `*_OPAQUE` name) with its
/// override's keys, each of which differs from the default; complete,
/// cited and named by the naming rules.
fn resolve_reviews(
    context: Context<'_>,
    file: &TypeFile,
    templates: &BTreeMap<String, Register>,
) -> Result<BTreeMap<String, ReviewAnnotation>, Error> {
    let type_name = &file.header.name;
    let review = &file.review;
    if let Some(sources) = &review.sources {
        context.sources(type_name, sources)?;
    }
    if let Some(sources) = review
        .opaque
        .as_ref()
        .and_then(|opaque| opaque.sources.as_ref())
    {
        context.sources(&format!("{type_name}.review.opaque"), sources)?;
    }
    for (key, annotation) in &review.entity {
        if let Some(sources) = &annotation.sources {
            context.sources(&format!("{type_name}.{key}"), sources)?;
        }
    }
    let entities = review_entities(context, file, templates)?;
    if let Some(key) = review
        .entity
        .keys()
        .find(|key| !entities.iter().any(|entity| entity.key == **key))
    {
        return Err(context.error(format!("{type_name}.{key}"), ErrorKind::ReviewUnknownEntity));
    }
    let mut resolved = BTreeMap::new();
    for entity in entities {
        let path = format!("{type_name}.{}", entity.key);
        let fail = |kind| context.error(&path, kind);
        let default = if entity.opaque {
            let opaque = review.opaque.clone().unwrap_or_default();
            ReviewOverride {
                sources: opaque.sources,
                provenance: opaque.provenance,
                accuracy: opaque.accuracy,
                completeness: opaque.completeness,
                naming: Some(NameOrigin::Opaque),
            }
        } else {
            ReviewOverride {
                sources: review.sources.clone(),
                provenance: review.provenance,
                accuracy: review.accuracy,
                completeness: review.completeness,
                naming: review.naming,
            }
        };
        let merged = match review.entity.get(&entity.key) {
            None => default,
            Some(stated) => merge(default, stated).map_err(fail)?,
        };
        let annotation = ReviewAnnotation {
            entity: path.clone(),
            sources: merged
                .sources
                .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("sources")))?,
            provenance: Some(
                merged
                    .provenance
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("provenance")))?,
            ),
            accuracy: Some(
                merged
                    .accuracy
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("accuracy")))?,
            ),
            completeness: Some(
                merged
                    .completeness
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("completeness")))?,
            ),
            naming: Some(
                merged
                    .naming
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("naming")))?,
            ),
        };
        context.sources(&path, &annotation.sources)?;
        if !crate::review_annotation_is_assertion(&annotation) {
            return Err(fail(ErrorKind::ReviewHint));
        }
        crate::validate_naming(&path, Some(&annotation), &entity.position)
            .map_err(|error| fail(ErrorKind::Naming(error.to_string())))?;
        resolved.insert(entity.key, annotation);
    }
    Ok(resolved)
}

/// `default` with the keys `stated` sets, each of which must differ.
fn merge(default: ReviewOverride, stated: &ReviewOverride) -> Result<ReviewOverride, ErrorKind> {
    fn pick<T: Clone + PartialEq>(
        key: &'static str,
        default: Option<T>,
        stated: &Option<T>,
    ) -> Result<Option<T>, ErrorKind> {
        match stated {
            None => Ok(default),
            Some(value) if default.as_ref() == Some(value) => {
                Err(ErrorKind::ReviewRepeatsDefault(key))
            }
            Some(value) => Ok(Some(value.clone())),
        }
    }
    Ok(ReviewOverride {
        sources: pick("sources", default.sources, &stated.sources)?,
        provenance: pick("provenance", default.provenance, &stated.provenance)?,
        accuracy: pick("accuracy", default.accuracy, &stated.accuracy)?,
        completeness: pick("completeness", default.completeness, &stated.completeness)?,
        naming: pick("naming", default.naming, &stated.naming)?,
    })
}
