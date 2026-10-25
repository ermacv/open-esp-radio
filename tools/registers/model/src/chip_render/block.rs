//! One block type: its register templates, the accessors each field's class
//! gives, the register modules of the type module and the accessor methods
//! of an owner that reaches a template's words.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::{Error, names};
use crate::{
    chip_model::{BlockType, Field, FieldClass, Register},
    pac_api_render::push_doc,
};

/// What a bit of a command register is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Role {
    /// Started by writing one; the hardware clears it when done.
    Command,
    /// Written with each start, by the start's argument.
    Qualifier,
    /// A self-clearing bit no start sets: read, zero in every start.
    Held,
}

/// The named values of a field.
pub(super) struct Values {
    pub(super) name: String,
    pub(super) variants: Vec<(String, u32, Option<String>)>,
    /// Every value of the field's width is named.
    pub(super) complete: bool,
}

/// The type a field's writer takes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Setter {
    /// `bool`, `u8`, `u16`, `u32` or `oer_mmio::Bits<WIDTH>`.
    Native,
    /// The field's value enumeration: its only writable values.
    Enum,
    /// The field's bounded type: a write range narrower than its width.
    Range,
}

pub(super) struct FieldPlan<'m> {
    pub(super) field: &'m Field,
    /// The snake-case stem: the model's name without the block prefix.
    pub(super) stem: String,
    pub(super) low: u32,
    pub(super) width: u32,
    pub(super) class: FieldClass,
    pub(super) role: Option<Role>,
    /// The hardware reads the field as an address: no integer setter.
    pub(super) pointer: bool,
    /// The one permitted value is zero: zero in every write, no accessor.
    pub(super) zero: bool,
    pub(super) values: Option<Values>,
    /// A write range narrower than the field's width.
    pub(super) range: Option<(u32, u32)>,
    pub(super) setter: Setter,
}

impl FieldPlan<'_> {
    pub(super) fn mask(&self) -> u32 {
        (u32::MAX >> (32 - self.width)) << self.low
    }

    fn ones(&self) -> u32 {
        u32::MAX >> (32 - self.width)
    }

    pub(super) fn method(&self) -> String {
        names::ident(&self.stem)
    }

    /// A reader returns the field.
    pub(super) fn getter(&self) -> bool {
        self.class.readable() && !self.zero
    }

    /// A writer of the whole register (`W`) sets the field.
    pub(super) fn setter(&self) -> bool {
        matches!(self.class, FieldClass::Plain | FieldClass::WriteOnly)
            && !self.pointer
            && !self.zero
            && self.role.is_none()
    }

    pub(super) fn native(&self) -> String {
        match self.width {
            1 => "bool".to_owned(),
            8 => "u8".to_owned(),
            16 => "u16".to_owned(),
            32 => "u32".to_owned(),
            width => format!("oer_mmio::Bits<{width}>"),
        }
    }

    fn native_bits(&self) -> &'static str {
        match self.width {
            32 => "value",
            1 | 8 | 16 => "u32::from(value)",
            _ => "value.get()",
        }
    }

    /// The parameter type a writer takes, as seen from the type module
    /// (`module` is the register module's name) or from the register module.
    pub(super) fn write_type(&self, module: Option<&str>) -> String {
        let qualify = |name: &str| match module {
            Some(module) => format!("{module}::{name}"),
            None => name.to_owned(),
        };
        match (self.setter, &self.values) {
            (Setter::Enum, Some(values)) => qualify(&values.name),
            (Setter::Range, _) => qualify(&self.range_name()),
            _ => self.native(),
        }
    }

    /// The field's bits of the writer's `value`, unshifted.
    pub(super) fn write_bits(&self) -> &'static str {
        match self.setter {
            Setter::Enum => "value.bits()",
            Setter::Range => "value.get()",
            Setter::Native => self.native_bits(),
        }
    }

    /// The writer's `value` in its register position.
    pub(super) fn positioned(&self) -> String {
        let bits = self.write_bits();
        if self.low == 0 {
            bits.to_owned()
        } else {
            format!("({bits} << {})", self.low)
        }
    }

    fn range_name(&self) -> String {
        format!("{}Range", names::camel(&self.stem))
    }

    /// The field's value in `bits`, the register image, as its reader type.
    pub(super) fn read_expression(&self, bits: &str) -> String {
        if self.width == 1 {
            return format!("({bits} & 0x{:08x}) != 0", self.mask());
        }
        let shifted = if self.low == 0 {
            bits.to_owned()
        } else {
            format!("({bits} >> {})", self.low)
        };
        match self.width {
            32 => shifted,
            8 => format!("{shifted} as u8"),
            16 => format!("{shifted} as u16"),
            _ => format!(
                "oer_mmio::Bits::truncate({})",
                shifted.trim_start_matches('(').trim_end_matches(')')
            ),
        }
    }
}

/// One element of a template: a register, a `dim` element or a group
/// element's register.
pub(super) struct Elem {
    /// The model's element name below the instance (`CH4_EVENT`, `T1.CONFIG`).
    pub(super) name: String,
    /// The snake-case stem of a per-element accessor.
    pub(super) stem: String,
    /// From the block's base.
    pub(super) offset: u64,
}

/// One register template of a type and the accessors its fields' classes
/// give.
pub(super) struct Template<'m> {
    pub(super) key: String,
    /// The register module's snake-case stem.
    pub(super) stem: String,
    pub(super) register: &'m Register,
    pub(super) width: u32,
    pub(super) elements: Vec<Elem>,
    /// `(offset, stride, count)` when the elements are one evenly spaced
    /// array, indexed by `oer_mmio::Index<count>`.
    pub(super) array: Option<(u64, u64, u64)>,
    pub(super) fields: Vec<FieldPlan<'m>>,
    pub(super) fifo: bool,
    pub(super) port: bool,
    pub(super) command: bool,
    pub(super) reserved: u32,
    /// The reset value of the reserved bits: a write from zero stores it.
    pub(super) reserved_reset: u32,
}

impl Template<'_> {
    pub(super) fn module(&self) -> String {
        names::ident(&self.stem)
    }

    pub(super) fn templated(&self) -> bool {
        self.key.contains("%s")
    }

    fn has(&self, class: FieldClass) -> bool {
        self.fields.iter().any(|field| field.class == class)
    }

    fn mask_of(&self, keep: impl Fn(&FieldPlan<'_>) -> bool) -> u32 {
        self.fields
            .iter()
            .filter(|field| keep(field))
            .fold(0, |mask, field| mask | field.mask())
    }

    pub(super) fn declared(&self) -> u32 {
        self.mask_of(|_| true)
    }

    pub(super) fn triggers(&self) -> u32 {
        self.mask_of(|field| field.class == FieldClass::WriteTrigger)
    }

    fn commands(&self) -> u32 {
        self.mask_of(|field| field.role == Some(Role::Command))
    }

    fn busy(&self) -> u32 {
        self.mask_of(|field| field.role.is_some())
    }

    fn zeros(&self) -> u32 {
        self.mask_of(|field| field.zero)
    }

    /// The bits every write but a field's own writes zero to: triggers,
    /// command bits and zero-only fields.
    pub(super) fn write_zero(&self) -> u32 {
        self.triggers() | self.busy() | self.zeros()
    }

    /// A read changes nothing and the register has an API at all.
    pub(super) fn api(&self) -> bool {
        self.register.read_side_effect.is_none() && !self.has(FieldClass::ReadClears)
    }

    /// A read has a value and changes nothing.
    pub(super) fn readable(&self) -> bool {
        self.api() && !self.fifo && self.fields.iter().any(FieldPlan::getter)
    }

    /// Only write-one-to-clear fields: sampled and acknowledged.
    pub(super) fn w1c(&self) -> bool {
        !self.fields.is_empty()
            && self
                .fields
                .iter()
                .all(|field| field.class == FieldClass::WriteOneToClear)
    }

    /// Writing back what was read replays nothing: no command, port,
    /// unreadable write lane or bit the hardware sets or clears.
    pub(super) fn modifiable(&self) -> bool {
        self.readable()
            && !self.command
            && !self.port
            && !self.fields.iter().any(|field| {
                matches!(
                    field.class,
                    FieldClass::WriteOnly
                        | FieldClass::WriteOneToClear
                        | FieldClass::SelfClearing
                        | FieldClass::SetOnly
                        | FieldClass::HardwareWritten
                )
            })
    }

    pub(super) fn has_setters(&self) -> bool {
        self.fields.iter().any(FieldPlan::setter)
    }

    /// Written from zero through `W`: a register with no readable field.
    fn written_from_zero(&self) -> bool {
        self.api() && !self.fifo && !self.port && !self.readable() && self.has_setters()
    }

    /// Changed through `W` by a read-modify-write.
    fn modified(&self) -> bool {
        self.modifiable() && self.has_setters()
    }

    /// Each write-trigger field's explicit trigger: in a register written
    /// from zero, or one whose read-modify-write replays nothing.
    fn triggered(&self) -> bool {
        self.api() && !self.fifo && !self.port && (!self.readable() || self.modifiable())
    }

    fn has_w(&self) -> bool {
        self.modified() || self.written_from_zero()
    }

    /// The narrow (8- or 16-bit) word an owner cannot reach yet.
    pub(super) fn narrow(&self) -> bool {
        self.width != 32
    }
}

/// One block type of the chip and its register templates.
pub(super) struct TypePlan<'m> {
    pub(super) name: &'m str,
    pub(super) module: String,
    pub(super) block: &'m BlockType,
    pub(super) templates: Vec<Template<'m>>,
}

impl<'m> TypePlan<'m> {
    /// Plan the type `block`. `pointers` holds the `(template key, field)`
    /// pairs some instance's policy reads as an address.
    pub(super) fn new(
        block: &'m BlockType,
        pointers: &BTreeSet<(String, String)>,
    ) -> Result<Self, Error> {
        let name = block.name();
        let prefix = names::block_prefix(block);
        let mut commands = BTreeMap::new();
        for command in &block.file.commands {
            for (fields, role) in [
                (&command.commands, Role::Command),
                (&command.qualifiers, Role::Qualifier),
                (&command.held, Role::Held),
            ] {
                for field in fields {
                    commands.insert((command.register.as_str(), field.as_str()), role);
                }
            }
        }
        // A template whose stem without `%s` is a single register's keeps
        // its index place as `_n`: `STATUS%s` beside `STATUS` is `status_n`.
        let singles = block
            .templates
            .keys()
            .filter(|key| !key.contains("%s"))
            .map(|key| key_stem(key, &prefix))
            .collect::<BTreeSet<_>>();
        let mut templates = Vec::new();
        for (key, register) in &block.templates {
            let entity = format!("{name}.{key}");
            let mut stem = key_stem(key, &prefix);
            if key.contains("%s") && singles.contains(&stem) {
                stem = key_stem(&key.replace("%s", "_N"), &prefix);
            }
            if !names::valid(&stem) {
                return Err(Error::new(
                    &entity,
                    format!("its name stem {stem:?} is no identifier"),
                ));
            }
            let mut elements = block
                .elements
                .iter()
                .filter(|element| element.key == *key)
                .map(|element| Elem {
                    name: element.name.clone(),
                    stem: key_stem(&element.name, &prefix),
                    offset: element.offset,
                })
                .collect::<Vec<_>>();
            elements.sort_by_key(|element| element.offset);
            let width = block
                .elements
                .iter()
                .find(|element| element.key == *key)
                .map_or(32, |element| element.width);
            let array = if key.contains("%s") {
                arithmetic(&elements)
            } else {
                None
            };
            let mut fields = Vec::new();
            for field in &register.fields {
                fields.push(field_plan(
                    &entity,
                    key,
                    field,
                    &prefix,
                    commands.get(&(key.as_str(), field.name.as_str())).copied(),
                    pointers.contains(&(key.clone(), field.name.clone())),
                )?);
            }
            let reserved = register
                .reserved
                .as_ref()
                .map_or(0, |reserved| reserved.mask);
            let reserved_reset = register.reset.map_or(0, |reset| {
                reset.value & reset.mask.unwrap_or(u32::MAX) & reserved
            });
            templates.push(Template {
                key: key.clone(),
                stem,
                register,
                width,
                elements,
                array,
                fields,
                fifo: register.fifo.is_some(),
                port: register.data_port.is_some(),
                command: block
                    .file
                    .commands
                    .iter()
                    .any(|command| command.register == *key),
                reserved,
                reserved_reset,
            });
        }
        templates.sort_by_key(|template| {
            (
                template
                    .elements
                    .first()
                    .map_or(u64::MAX, |element| element.offset),
                template.key.clone(),
            )
        });
        let plan = Self {
            name,
            module: names::ident(&names::snake(name)),
            block,
            templates,
        };
        plan.check_names()?;
        Ok(plan)
    }

    /// No two register modules, accessor stems or field methods of one
    /// register collide after the prefix is cut.
    fn check_names(&self) -> Result<(), Error> {
        let mut modules = BTreeMap::new();
        for template in &self.templates {
            if let Some(other) = modules.insert(template.stem.clone(), &template.key) {
                return Err(Error::new(
                    &format!("{}.{}", self.name, template.key),
                    format!("its module `{}` is also {other}'s", template.stem),
                ));
            }
            let mut fields = BTreeSet::new();
            for field in &template.fields {
                if !fields.insert(field.stem.as_str()) {
                    return Err(Error::new(
                        &format!("{}.{}.{}", self.name, template.key, field.field.name),
                        format!("its method `{}` is another field's", field.stem),
                    ));
                }
            }
        }
        // A per-element stem names the element's own accessors; it may not
        // be another template's module stem.
        for template in &self.templates {
            for element in &template.elements {
                if element.stem != template.stem
                    && let Some(other) = modules.get(&element.stem)
                {
                    return Err(Error::new(
                        &format!("{}.{}", self.name, element.name),
                        format!("its accessor stem `{}` is {other}'s module", element.stem),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The template and element at `offset` from the block's base.
    pub(super) fn element(&self, offset: u64) -> Option<(&Template<'m>, &Elem)> {
        self.templates.iter().find_map(|template| {
            template
                .elements
                .iter()
                .find(|element| element.offset == offset)
                .map(|element| (template, element))
        })
    }
}

/// `CH%s_EVENT` → `ch_event`, `T1.CONFIG` → `t1_config`: each segment of
/// a key or element name without the block prefix, in snake case.
fn key_stem(key: &str, prefix: &str) -> String {
    let joined = key
        .split('.')
        .map(|part| names::strip(part, prefix))
        .collect::<Vec<_>>()
        .join("_");
    names::snake(&joined)
}

fn arithmetic(elements: &[Elem]) -> Option<(u64, u64, u64)> {
    let [first, second, ..] = elements else {
        return None;
    };
    let stride = second.offset.checked_sub(first.offset)?;
    let even = elements
        .windows(2)
        .all(|pair| matches!(pair, [a, b] if b.offset.checked_sub(a.offset) == Some(stride)));
    (even && stride > 0).then_some((first.offset, stride, elements.len() as u64))
}

fn field_plan<'m>(
    register: &str,
    key: &str,
    field: &'m Field,
    prefix: &str,
    role: Option<Role>,
    pointer: bool,
) -> Result<FieldPlan<'m>, Error> {
    let entity = format!("{register}.{}", field.name);
    let (low, width) = crate::chip_model::parse_bits(&field.bits)
        .map_err(|error| Error::new(&entity, format!("{error:?}")))?;
    let stem = names::snake(names::strip(&field.name, prefix));
    if !names::valid(&stem) {
        return Err(Error::new(
            &entity,
            format!("its stem {stem:?} is no identifier"),
        ));
    }
    let ones = u32::MAX >> (32 - width);
    let range = field
        .write_range
        .map(|range| (range.min, range.max))
        .filter(|(min, max)| *min != 0 || *max != ones);
    let zero = range == Some((0, 0));
    if let Some((min, max)) = range
        && (min > max || max > ones)
    {
        return Err(Error::new(
            &entity,
            "its write range lies outside the field",
        ));
    }
    let values = if field.values.is_empty() {
        None
    } else {
        let mut seen = BTreeSet::new();
        let mut variants = Vec::new();
        for value in &field.values {
            let mut variant = names::camel(&value.name);
            if variant.starts_with(|character: char| character.is_ascii_digit()) {
                variant.insert(0, 'V');
            }
            if value.value > ones || !seen.insert(value.value) || !names::valid(&variant) {
                return Err(Error::new(
                    &entity,
                    format!(
                        "value {} lies outside the field, repeats or names no variant",
                        value.name
                    ),
                ));
            }
            variants.push((variant, value.value, value.description.clone()));
        }
        if variants
            .iter()
            .map(|(variant, ..)| variant)
            .collect::<BTreeSet<_>>()
            .len()
            != variants.len()
        {
            return Err(Error::new(&entity, "two values have one variant name"));
        }
        let complete = width < 32 && variants.len() as u64 == 1_u64 << width;
        Some(Values {
            name: names::camel(&stem),
            variants,
            complete,
        })
    };
    let only_values = values.is_some()
        && (field.write_values_only || values.as_ref().is_some_and(|v| v.complete));
    let setter = if only_values {
        Setter::Enum
    } else if range.is_some() && !zero {
        Setter::Range
    } else {
        Setter::Native
    };
    if role.is_some() && (width != 1 || field.class != FieldClass::SelfClearing) {
        return Err(Error::new(
            &entity,
            "a command register's bit is one self-clearing bit",
        ));
    }
    let _ = key;
    Ok(FieldPlan {
        field,
        stem,
        low,
        width,
        class: field.class,
        role,
        pointer,
        zero,
        values,
        range,
        setter,
    })
}

/// The register modules of one type: masks, offsets, value types and the
/// `R`, `W` and `Snapshot` images.
pub(super) fn render_register_module(output: &mut String, template: &Template<'_>) {
    let module = template.module();
    let _ = writeln!(output, "/// The `{}` register.", template.key);
    if let Some(description) = &template.register.description {
        output.push_str("///\n");
        push_doc(output, &clean(description), "");
    }
    let _ = writeln!(
        output,
        "pub mod {module} {{\n\
         /// The bits of the register's declared fields.\n\
         pub const FIELDS: u32 = 0x{:08x};\n\
         /// The register's width in bits.\n\
         pub const WIDTH: u32 = {};\n",
        template.declared(),
        template.width
    );
    match (template.array, template.elements.as_slice()) {
        (Some((offset, stride, count)), _) => {
            let _ = writeln!(
                output,
                "/// The first element's byte offset in its block.\n\
                 pub const OFFSET: usize = 0x{offset:x};\n\
                 /// The byte distance between two elements.\n\
                 pub const STRIDE: usize = 0x{stride:x};\n\
                 /// The element count.\n\
                 pub const COUNT: usize = {count};\n"
            );
        }
        (None, [element]) => {
            let _ = writeln!(
                output,
                "/// The register's byte offset in its block.\n\
                 pub const OFFSET: usize = 0x{:x};\n",
                element.offset
            );
        }
        (None, _) => {}
    }
    for (mask, name, doc) in [
        (
            template.reserved,
            "RESERVED",
            "The reserved bits: never a field, kept by every read-modify-write and\n/// stored at their reset value by a write from zero.",
        ),
        (
            template.triggers(),
            "TRIGGERS",
            "The bits of the write-trigger fields: zero in every write but a\n/// field's explicit trigger.",
        ),
        (
            template.commands(),
            "COMMANDS",
            "The bits of the commands: each set only by its start.",
        ),
        (
            template.busy(),
            "BUSY",
            "The self-clearing bits: a start refuses while any reads one.",
        ),
        (
            template.zeros(),
            "ZERO",
            "The bits of the fields whose one permitted value is zero.",
        ),
    ] {
        if mask != 0 {
            let _ = writeln!(output, "/// {doc}\npub const {name}: u32 = 0x{mask:08x};\n");
        }
    }
    for field in &template.fields {
        if let Some(values) = &field.values {
            render_values(output, field, values);
        }
        if field.setter == Setter::Range
            && let Some((min, max)) = field.range
        {
            render_range(output, field, min, max);
        }
    }
    if template.readable() {
        output.push_str(
            "/// One observation of the register.\n\
             #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
             pub struct R(pub(super) u32);\n\nimpl R {\n",
        );
        for field in template.fields.iter().filter(|field| field.getter()) {
            render_getter(output, field, "self");
        }
        output.push_str("}\n\n");
    }
    if template.w1c() && template.readable() {
        output.push_str(
            "/// One sample of the register's asserted fields, to acknowledge once.\n\
             #[must_use = \"a sampled write-one-to-clear set must be acknowledged\"]\n\
             #[derive(Debug, Eq, PartialEq)]\n\
             pub struct Snapshot(pub(super) u32);\n\nimpl Snapshot {\n\
             /// Whether no declared field was asserted.\n\
             pub const fn is_clear(&self) -> bool {\nself.0 == 0\n}\n\n",
        );
        for field in &template.fields {
            render_getter(output, field, "&self");
            let _ = writeln!(
                output,
                "/// The snapshot without `{}` unless `keep`: it is then not acknowledged.\n\
                 pub const fn retain_{}(self, keep: bool) -> Self {{\n\
                 if keep {{ self }} else {{ Self({}) }}\n}}\n",
                field.field.name,
                field.stem,
                without("self.0", field.mask())
            );
        }
        output.push_str("}\n\n");
    }
    if template.has_w() {
        output.push_str(if template.readable() {
            "/// The register's value while it is changed.\n\
             pub struct W(pub(super) u32);\n\nimpl W {\n"
        } else {
            "/// The value written to the register.\n\
             pub struct W(pub(super) u32);\n\nimpl W {\n"
        });
        if template.readable() {
            for field in template.fields.iter().filter(|field| field.getter()) {
                render_getter(output, field, "&self");
            }
        }
        for field in template.fields.iter().filter(|field| field.setter()) {
            let assignment = if field.width == 32 {
                format!("self.0 = {};", field.positioned())
            } else {
                format!(
                    "self.0 = (self.0 & 0x{:08x}) | {};",
                    !field.mask(),
                    field.positioned()
                )
            };
            let _ = writeln!(
                output,
                "/// Set `{}`.\n#[inline]\npub fn set_{}(&mut self, value: {}) -> &mut Self {{\n{assignment}\nself\n}}\n",
                field.field.name,
                field.stem,
                field.write_type(None),
            );
        }
        output.push_str("}\n\n");
    }
    output.push_str("}\n\n");
}

fn render_getter(output: &mut String, field: &FieldPlan<'_>, receiver: &str) {
    match &field.field.description {
        Some(description) => push_doc(output, &clean(description), ""),
        None => {
            let _ = writeln!(output, "/// The `{}` field.", field.field.name);
        }
    }
    let _ = writeln!(
        output,
        "#[inline]\npub const fn {}({receiver}) -> {} {{\n{}\n}}\n",
        field.method(),
        field.native(),
        field.read_expression("self.0")
    );
    if let Some(values) = &field.values {
        let shifted = if field.low == 0 {
            "self.0".to_owned()
        } else {
            format!("(self.0 >> {})", field.low)
        };
        let (result, body) = if values.complete {
            (
                values.name.clone(),
                format!(
                    "{}::from_field({})",
                    values.name,
                    shifted.trim_start_matches('(').trim_end_matches(')')
                ),
            )
        } else if field.width == 32 {
            (
                format!("Option<{}>", values.name),
                format!("{}::from_bits(self.0)", values.name),
            )
        } else {
            (
                format!("Option<{}>", values.name),
                format!(
                    "{}::from_bits({shifted} & 0x{:x})",
                    values.name,
                    field.ones()
                ),
            )
        };
        let _ = writeln!(
            output,
            "/// The `{}` field as its declared value{}.\n#[inline]\npub const fn {}_variant({receiver}) -> {result} {{\n{body}\n}}\n",
            field.field.name,
            if values.complete {
                ""
            } else {
                ", if it is one"
            },
            field.stem,
        );
    }
}

fn render_values(output: &mut String, field: &FieldPlan<'_>, values: &Values) {
    let name = &values.name;
    let repr = if values
        .variants
        .iter()
        .any(|(_, value, _)| *value > i32::MAX.unsigned_abs())
    {
        "#[repr(u32)]\n"
    } else {
        ""
    };
    let _ = writeln!(
        output,
        "/// The declared values of the `{}` field.\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
         #[allow(clippy::enum_variant_names, reason = \"the variants are the model's value names\")]\n\
         {repr}pub enum {name} {{",
        field.field.name
    );
    for (variant, value, description) in &values.variants {
        if let Some(description) = description {
            push_doc(output, &clean(description), "");
        }
        let _ = writeln!(output, "{variant} = 0x{value:x},");
    }
    let _ = writeln!(
        output,
        "}}\n\nimpl {name} {{\n/// The value's field bits.\n#[inline]\npub const fn bits(self) -> u32 {{\nself as u32\n}}\n"
    );
    if values.complete
        && let Some((last, rest)) = values.variants.split_last()
    {
        let arms = rest
            .iter()
            .map(|(variant, value, _)| format!("0x{value:x} => Self::{variant},"))
            .collect::<String>();
        let _ = writeln!(
            output,
            "/// The value of the field in the low bits of `bits`; the field has\n\
             /// exactly the declared values.\n\
             #[inline]\npub const fn from_field(bits: u32) -> Self {{\n\
             match bits & 0x{:x} {{\n{arms}_ => Self::{},\n}}\n}}\n",
            field.ones(),
            last.0
        );
    } else {
        let arms = values
            .variants
            .iter()
            .map(|(variant, value, _)| format!("0x{value:x} => Some(Self::{variant}),"))
            .collect::<String>();
        let _ = writeln!(
            output,
            "/// The declared value `bits`, if it is one.\n\
             #[inline]\npub const fn from_bits(bits: u32) -> Option<Self> {{\n\
             match bits {{\n{arms}_ => None,\n}}\n}}\n"
        );
    }
    output.push_str("}\n\n");
}

fn render_range(output: &mut String, field: &FieldPlan<'_>, min: u32, max: u32) {
    let name = field.range_name();
    let check = if min == 0 {
        format!("value <= 0x{max:x}")
    } else {
        format!("value >= 0x{min:x} && value <= 0x{max:x}")
    };
    let _ = writeln!(
        output,
        "/// A value the `{0}` field may be written: `0x{min:x}..=0x{max:x}`.\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
         pub struct {name}(u32);\n\n\
         impl {name} {{\n\
         /// The least writable value.\n\
         pub const MIN: u32 = 0x{min:x};\n\
         /// The greatest writable value.\n\
         pub const MAX: u32 = 0x{max:x};\n\n\
         /// `value`, if the field may be written it.\n\
         #[inline]\npub const fn new(value: u32) -> Option<Self> {{\n\
         if {check} {{ Some(Self(value)) }} else {{ None }}\n}}\n\n\
         /// The constant `V`, checked at compile time.\n\
         #[inline]\npub const fn constant<const V: u32>() -> Self {{\n\
         const {{ assert!(V >= 0x{min:x} && V <= 0x{max:x}, \"the value lies in the field's write range\") }};\n\
         Self(V)\n}}\n\n\
         /// The value.\n\
         #[inline]\npub const fn get(self) -> u32 {{\nself.0\n}}\n}}\n",
        field.field.name
    );
}

/// How an owner's method names a word: one element, or every element of
/// an array by an `oer_mmio::Index`.
#[derive(Clone, Copy)]
pub(super) enum Call {
    Fixed(u64),
    Indexed {
        offset: u64,
        stride: u64,
        count: u64,
    },
}

impl Call {
    fn parameter(self) -> String {
        match self {
            Self::Fixed(_) => String::new(),
            Self::Indexed { count, .. } => format!(", index: oer_mmio::Index<{count}>"),
        }
    }

    pub(super) fn read(self) -> String {
        match self {
            Self::Fixed(offset) => format!("read::<0x{offset:x}>()"),
            Self::Indexed {
                offset,
                stride,
                count,
            } => format!("read_indexed::<0x{offset:x}, 0x{stride:x}, {count}>(index)"),
        }
    }

    pub(super) fn write(self, value: &str) -> String {
        match self {
            Self::Fixed(offset) => format!("write::<0x{offset:x}>({value})"),
            Self::Indexed {
                offset,
                stride,
                count,
            } => format!("write_indexed::<0x{offset:x}, 0x{stride:x}, {count}>(index, {value})"),
        }
    }
}

/// What an owner may do with a word it reaches.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Mode {
    /// It holds the word: every accessor of the word's classes.
    Owned,
    /// It observes the word or gave up its writes: reads only.
    Read,
    /// It shares the word with every context through `&self`: reads,
    /// strobes and whole-word stores.
    Shared,
}

/// The accessor methods of one word or array an owner reaches. `stem`
/// names them; the register module is `template`'s.
pub(super) fn render_methods(
    body: &mut String,
    template: &Template<'_>,
    stem: &str,
    call: Call,
    mode: Mode,
) {
    let module = template.module();
    let index = call.parameter();
    let name = &template.key;
    if template.fifo {
        if mode == Mode::Owned {
            render_fifo(body, template, stem, call);
        }
        return;
    }
    if !template.api() {
        return;
    }
    if template.readable() {
        let _ = writeln!(
            body,
            "/// Read `{name}`.\n#[inline]\npub fn read_{stem}(&self{index}) -> {module}::R {{\n{module}::R(self.0.{})\n}}\n",
            call.read()
        );
        for field in template
            .fields
            .iter()
            .filter(|field| field.role == Some(Role::Command))
        {
            let _ = writeln!(
                body,
                "/// Whether the command `{}` of `{name}` still runs: the hardware\n\
                 /// clears the field when the command is done.\n\
                 #[inline]\npub fn {stem}_{}_busy(&self{index}) -> bool {{\n\
                 (self.0.{} & 0x{:08x}) != 0\n}}\n",
                field.field.name,
                field.stem,
                call.read(),
                field.mask()
            );
        }
    }
    match mode {
        Mode::Read => {}
        Mode::Shared => render_shared(body, template, stem, call),
        Mode::Owned => render_owned(body, template, stem, call),
    }
}

fn render_owned(body: &mut String, template: &Template<'_>, stem: &str, call: Call) {
    let module = template.module();
    let index = call.parameter();
    let name = &template.key;
    if template.w1c() && template.readable() {
        let _ = writeln!(
            body,
            "/// Sample `{name}` once, keeping its declared fields, to acknowledge.\n\
             #[inline]\npub fn sample_{stem}(&self{index}) -> {module}::Snapshot {{\n\
             {module}::Snapshot(self.0.{} & {module}::FIELDS)\n}}\n\n\
             /// Acknowledge exactly the sampled set: one to each bit the snapshot\n\
             /// holds, zero to every other bit.\n\
             #[inline]\npub fn acknowledge_{stem}(&mut self{index}, snapshot: {module}::Snapshot) {{\n\
             self.0.{};\n}}\n",
            call.read(),
            call.write("snapshot.0")
        );
        return;
    }
    let zero = template.write_zero();
    let modify_read = if zero == 0 {
        format!("self.0.{}", call.read())
    } else {
        format!("self.0.{} & 0x{:08x}", call.read(), !zero)
    };
    if template.command {
        let qualifiers = template
            .fields
            .iter()
            .filter(|field| field.role == Some(Role::Qualifier))
            .collect::<Vec<_>>();
        let parameters = qualifiers
            .iter()
            .map(|field| format!(", {}: bool", field.method()))
            .collect::<String>();
        let qualified = qualifiers
            .iter()
            .map(|field| {
                format!(
                    " | if {} {{ 0x{:08x} }} else {{ 0 }}",
                    field.method(),
                    field.mask()
                )
            })
            .collect::<String>();
        for field in template
            .fields
            .iter()
            .filter(|field| field.role == Some(Role::Command))
        {
            let _ = writeln!(
                body,
                "/// Start the command `{}` of `{name}`: one write sets it and each\n\
                 /// qualifier as given; every other field keeps the value read, every\n\
                 /// other self-clearing bit is zero. It refuses, writing nothing, while\n\
                 /// any self-clearing bit of the register reads one.\n\
                 #[inline]\npub fn start_{stem}_{}(&mut self{index}{parameters}) -> Result<(), oer_mmio::CommandBusy> {{\n\
                 let value = self.0.{};\n\
                 if value & 0x{busy:08x} != 0 {{\n\
                 return Err(oer_mmio::CommandBusy {{ running: value & 0x{busy:08x} }});\n}}\n\
                 self.0.{};\nOk(())\n}}\n",
                field.field.name,
                field.stem,
                call.read(),
                call.write(&format!(
                    "(value & 0x{:08x}) | 0x{:08x}{qualified}",
                    !zero,
                    field.mask()
                )),
                busy = template.busy(),
            );
        }
        return;
    }
    if template.port {
        if let [field] = template.fields.as_slice() {
            let _ = writeln!(
                body,
                "/// Store the data word `{name}` whole: its one field `{}`, zero in every\n\
                 /// other bit; the hardware also writes it, so it is never modified.\n\
                 #[inline]\npub fn write_{stem}(&mut self{index}, value: {}) {{\n\
                 self.0.{};\n}}\n",
                field.field.name,
                field.write_type(Some(&module)),
                call.write(&field.positioned())
            );
        }
        return;
    }
    if template.triggered() {
        for field in template
            .fields
            .iter()
            .filter(|field| field.class == FieldClass::WriteTrigger)
        {
            let (parameter, bits) = if field.width == 1 {
                (String::new(), format!("0x{:08x}", field.mask()))
            } else {
                (
                    format!(", value: {}", field.write_type(Some(&module))),
                    field.positioned(),
                )
            };
            let image = if template.readable() {
                format!("{modify_read} | {bits}")
            } else if template.reserved_reset == 0 {
                bits.strip_prefix('(')
                    .and_then(|bits| bits.strip_suffix(')'))
                    .map_or_else(|| bits.clone(), str::to_owned)
            } else {
                format!("0x{:08x} | {bits}", template.reserved_reset)
            };
            let _ = writeln!(
                body,
                "/// Write to the write-trigger field `{}` of `{name}`, starting its\n\
                 /// hardware action once; {}.\n\
                 #[inline]\npub fn trigger_{stem}_{}(&mut self{index}{parameter}) {{\n\
                 self.0.{};\n}}\n",
                field.field.name,
                if template.readable() {
                    "every other field keeps the value read, every other trigger is zero"
                } else {
                    "every other bit is zero"
                },
                field.stem,
                call.write(&image)
            );
        }
    }
    if template.modified() {
        let _ = writeln!(
            body,
            "/// Read `{name}`, change fields and write it back; every other bit keeps\n\
             /// the value read, every write-trigger field is zero.\n\
             #[inline]\npub fn modify_{stem}(&mut self{index}, change: impl FnOnce(&mut {module}::W)) {{\n\
             let mut value = {module}::W({modify_read});\n\
             change(&mut value);\n\
             self.0.{};\n}}\n",
            call.write("value.0")
        );
    } else if template.written_from_zero() {
        let _ = writeln!(
            body,
            "/// Write `{name}` from zero: the fields set, the reserved bits at their\n\
             /// reset value, zero in every other bit.\n\
             #[inline]\npub fn write_{stem}(&mut self{index}, change: impl FnOnce(&mut {module}::W)) {{\n\
             let mut value = {module}::W(0x{:08x});\n\
             change(&mut value);\n\
             self.0.{};\n}}\n",
            template.reserved_reset,
            call.write("value.0")
        );
    }
}

/// A shared word's writes through `&self`: each strobe alone, or the one
/// field stored whole; neither reads the word back.
fn render_shared(body: &mut String, template: &Template<'_>, stem: &str, call: Call) {
    let Call::Fixed(offset) = call else {
        return;
    };
    let name = &template.key;
    let module = template.module();
    for field in template
        .fields
        .iter()
        .filter(|field| field.class == FieldClass::WriteTrigger && field.width == 1)
    {
        let _ = writeln!(
            body,
            "/// Strobe the write-trigger field `{}` of `{name}` alone: one there,\n\
             /// zero in every other bit, nothing read back.\n\
             #[inline]\npub fn trigger_{stem}_{}(&self) {{\n\
             self.0.write_shared::<0x{offset:x}>(0x{:08x});\n}}\n",
            field.field.name,
            field.stem,
            field.mask()
        );
    }
    if let [field] = template.fields.as_slice()
        && field.setter()
    {
        let _ = writeln!(
            body,
            "/// Store `{name}` whole: its one field `{}`, zero in every other bit,\n\
             /// nothing read back.\n\
             #[inline]\npub fn store_{stem}(&self, value: {}) {{\n\
             self.0.write_shared::<0x{offset:x}>({});\n}}\n",
            field.field.name,
            field.write_type(Some(&module)),
            field.positioned()
        );
    }
}

fn render_fifo(body: &mut String, template: &Template<'_>, stem: &str, call: Call) {
    let module = template.module();
    let name = &template.key;
    let [field] = template.fields.as_slice() else {
        return;
    };
    if field.getter() {
        let read = format!("self.0.{}", call.read());
        let element = match field.read_expression("value") {
            whole if whole == "value" => read,
            extracted => format!("let value = {read};\n{extracted}"),
        };
        let _ = writeln!(
            body,
            "/// Take the next element out of the FIFO port `{name}`.\n\
             #[inline]\npub fn pop_{stem}(&mut self) -> {} {{\n{element}\n}}\n",
            field.native(),
        );
    }
    if field.setter() {
        let _ = writeln!(
            body,
            "/// Put one element into the FIFO port `{name}`; every other bit is zero.\n\
             #[inline]\npub fn push_{stem}(&mut self, value: {}) {{\n\
             self.0.{};\n}}\n",
            field.write_type(Some(&module)),
            call.write(&field.positioned())
        );
    }
}

/// The trigger of each of an owner's fields of a word several owners
/// split by field: its bits alone, zero in every other.
pub(super) fn render_split_triggers(
    body: &mut String,
    template: &Template<'_>,
    stem: &str,
    offset: u64,
    fields: &BTreeSet<String>,
    divided: bool,
) {
    let name = &template.key;
    for field in template
        .fields
        .iter()
        .filter(|field| fields.contains(&field.field.name))
    {
        let write = if divided {
            format!("write_trigger::<0x{offset:x}>(0x{:08x})", field.mask())
        } else {
            format!("write::<0x{offset:x}>(0x{:08x})", field.mask())
        };
        let _ = writeln!(
            body,
            "/// Write one to the write-trigger field `{}` of the split word `{name}`,\n\
             /// starting its hardware action once; zero in every other bit, which\n\
             /// leaves the other holders' fields alone.\n\
             #[inline]\npub fn trigger_{stem}_{}(&mut self) {{\n\
             self.0.{write};\n}}\n",
            field.field.name, field.stem,
        );
    }
}

/// `expression` without the bits of `mask`, as an expression.
pub(super) fn without(expression: &str, mask: u32) -> String {
    if mask == u32::MAX {
        "0".to_owned()
    } else {
        format!("{expression} & 0x{:08x}", !mask)
    }
}

/// Description text as doc lines: each line trimmed, blank ones dropped.
pub(super) fn clean(description: &str) -> String {
    description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
