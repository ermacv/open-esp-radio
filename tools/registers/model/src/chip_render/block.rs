//! One block type: its register templates, the accessors each field's class
//! gives, the register modules of the type module and the accessor methods
//! of an owner that reaches a template's words.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::names::push_doc;
use super::{Error, names};
use crate::chip_model::{BlockType, Field, FieldClass, RamRegion, Register};

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
    /// A typed address of the field's placement: `oer_mmio::DmaRef` of its
    /// target layout, or the window's `DmaWindowAddress`.
    Pointer,
}

/// What the hardware reads in an address field (`pointer` facts): the
/// layout it points to, the member offset and the encoding.
pub(super) struct PointerPlan {
    /// The target layout's Rust type, if the pointer has one.
    pub(super) target: Option<String>,
    pub(super) member_offset: u32,
    pub(super) shift: u32,
}

/// What a read-modify-write or a command start writes into a field it does
/// not set itself.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Replay {
    /// The value read: writing it back changes nothing.
    Keep,
    /// Zero: no action (a trigger, a write-one-to-clear or set-only lane).
    Zero,
    /// Neither is safe: the write lane is unreadable, the hardware also
    /// writes it, or its class is unknown.
    Refuse,
}

pub(super) struct FieldPlan<'m> {
    pub(super) field: &'m Field,
    /// The snake-case stem: the model's name without the block prefix.
    pub(super) stem: String,
    pub(super) low: u32,
    pub(super) width: u32,
    pub(super) class: FieldClass,
    pub(super) role: Option<Role>,
    /// The hardware reads the field as an address: a typed setter.
    pub(super) pointer: Option<PointerPlan>,
    /// The field supplies the fixed high bits of a pointer's address: its
    /// one value is the placement's prefix.
    pub(super) prefix: Option<u32>,
    /// A doorbell: set by its trigger alone, written back as read.
    pub(super) doorbell: bool,
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
            && !self.zero
            && self.role.is_none()
            && self.prefix.is_none()
    }

    /// A data port's whole-word store sets the field: a `plain` or
    /// `write-only` field, or the `hardware-written` field the hardware also
    /// fills, which no other writer sets.
    pub(super) fn port_setter(&self) -> bool {
        matches!(
            self.class,
            FieldClass::Plain | FieldClass::WriteOnly | FieldClass::HardwareWritten
        ) && !self.zero
            && self.role.is_none()
            && self.prefix.is_none()
    }

    /// Zero is a value a write may give the field: no write range above
    /// it, no value list without it, and no address it would forge.
    pub(super) fn zero_admitted(&self) -> bool {
        self.pointer.is_none()
            && self.prefix.is_none_or(|prefix| prefix == 0)
            && self.range.is_none_or(|(min, _)| min == 0)
            && !(self.field.write_values_only
                && self
                    .values
                    .as_ref()
                    .is_some_and(|values| values.variants.iter().all(|(_, value, _)| *value != 0)))
    }

    /// What a write of the word that does not set this field gives it.
    pub(super) fn replay(&self) -> Replay {
        match self.class {
            FieldClass::Plain | FieldClass::ReadOnly | FieldClass::DebugOnly => Replay::Keep,
            FieldClass::SelfClearing if self.doorbell => Replay::Keep,
            FieldClass::WriteTrigger | FieldClass::WriteOneToClear | FieldClass::SetOnly
                if self.zero_admitted() =>
            {
                Replay::Zero
            }
            _ => Replay::Refuse,
        }
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

    fn native_bits(&self, var: &str) -> String {
        match self.width {
            32 => var.to_owned(),
            1 | 8 | 16 => format!("u32::from({var})"),
            _ => format!("{var}.get()"),
        }
    }

    /// The parameter type a writer takes, as seen from the type module
    /// (`module` is the register module's name) or from the register module.
    pub(super) fn write_type(&self, module: Option<&str>) -> String {
        let qualify = |name: &str| match module {
            Some(module) => format!("{module}::{name}"),
            None => name.to_owned(),
        };
        let layouts = match module {
            Some(_) => "super::layouts",
            None => "super::super::layouts",
        };
        match (self.setter, &self.values, &self.pointer) {
            (Setter::Pointer, _, Some(pointer)) => match &pointer.target {
                Some(target) => format!("oer_mmio::DmaRef<{layouts}::{target}>"),
                None => format!("{layouts}::DmaWindowAddress"),
            },
            (Setter::Enum, Some(values), _) => qualify(&values.name),
            (Setter::Range, ..) => qualify(&self.range_name()),
            _ => self.native(),
        }
    }

    /// The field's bits of the writer's argument `var`, unshifted.
    pub(super) fn write_bits_of(&self, var: &str) -> String {
        match (self.setter, &self.pointer) {
            (Setter::Pointer, Some(pointer)) => {
                let mut address = match pointer.target {
                    Some(_) => format!("{var}.bits()"),
                    None => format!("{var}.get()"),
                };
                if pointer.member_offset != 0 {
                    address = format!("{address}.wrapping_add(0x{:x})", pointer.member_offset);
                }
                if pointer.shift != 0 {
                    address = format!("({address} >> {})", pointer.shift);
                }
                if self.width == 32 {
                    address
                } else {
                    format!("({address} & 0x{:x})", self.ones())
                }
            }
            (Setter::Enum, _) => format!("{var}.bits()"),
            (Setter::Range, _) => format!("{var}.get()"),
            _ => self.native_bits(var),
        }
    }

    /// The writer's `value` in its register position.
    pub(super) fn positioned(&self) -> String {
        self.positioned_of("value")
    }

    /// The writer's argument `var` in its register position.
    pub(super) fn positioned_of(&self, var: &str) -> String {
        let bits = self.write_bits_of(var);
        if self.low == 0 {
            bits
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
    /// unreadable write lane or bit the hardware sets or clears but a
    /// doorbell, which the vendor writes back as read.
    pub(super) fn modifiable(&self) -> bool {
        self.readable()
            && !self.command
            && !self.port
            && !self.fields.iter().any(|field| {
                matches!(
                    field.class,
                    FieldClass::WriteOnly
                        | FieldClass::WriteOneToClear
                        | FieldClass::SetOnly
                        | FieldClass::HardwareWritten
                ) || (field.class == FieldClass::SelfClearing && !field.doorbell)
            })
    }

    /// A `W` has something to set: a field's value or a high field's prefix.
    pub(super) fn has_setters(&self) -> bool {
        self.fields
            .iter()
            .any(|field| field.setter() || field.prefix.is_some())
    }

    /// Written from zero through `W`: a register with no readable field, or
    /// a readable one whose write-only lanes no read-modify-write can
    /// replay and whose other fields a write from zero may leave zero
    /// (plain, read-only, trigger and write-one-to-clear ones). A word with
    /// an `unknown` or `debug-only` lane has no writer from zero.
    pub(super) fn written_from_zero(&self) -> bool {
        let unwritable = self
            .fields
            .iter()
            .any(|field| matches!(field.class, FieldClass::Unknown | FieldClass::DebugOnly));
        let lanes = self.fields.iter().all(|field| {
            matches!(
                field.class,
                FieldClass::Plain
                    | FieldClass::WriteOnly
                    | FieldClass::ReadOnly
                    | FieldClass::WriteTrigger
                    | FieldClass::WriteOneToClear
            )
        });
        let write_only = self
            .fields
            .iter()
            .any(|field| field.class == FieldClass::WriteOnly);
        self.api()
            && !self.fifo
            && !self.port
            && !self.command
            && !unwritable
            && self.has_setters()
            && (!self.readable() || (write_only && lanes && !self.modifiable()))
    }

    /// The fields a write from zero takes as arguments: setters whose zero
    /// no write may give them.
    pub(super) fn required(&self) -> Vec<&FieldPlan<'_>> {
        self.fields
            .iter()
            .filter(|field| field.setter() && !field.zero_admitted())
            .collect()
    }

    /// The image a write from zero starts from: the reserved bits at their
    /// reset value and each high field's prefix.
    pub(super) fn zero_image(&self) -> u32 {
        self.fields
            .iter()
            .filter_map(|field| field.prefix.map(|prefix| prefix << field.low))
            .fold(self.reserved_reset, |image, bits| image | bits)
    }

    /// The write-trigger fields a write may set: not zero-only.
    pub(super) fn settable_triggers(&self) -> impl Iterator<Item = &FieldPlan<'_>> {
        self.fields
            .iter()
            .filter(|field| field.class == FieldClass::WriteTrigger && !field.zero)
    }

    /// A word whose every writable field is a write trigger, at least two of
    /// them settable: one write may set several together (`T`).
    pub(super) fn trigger_set(&self) -> bool {
        self.api()
            && !self.fifo
            && !self.port
            && !self.command
            && self.settable_triggers().count() >= 2
            && self
                .fields
                .iter()
                .all(|field| matches!(field.class, FieldClass::WriteTrigger | FieldClass::ReadOnly))
    }

    /// The bits of the doorbells.
    pub(super) fn doorbells(&self) -> u32 {
        self.mask_of(|field| field.doorbell)
    }

    /// Changed through `W` by a read-modify-write.
    pub(super) fn modified(&self) -> bool {
        self.modifiable() && self.has_setters()
    }

    /// A readable mixed word's trigger may keep ordinary fields and zero
    /// every write-one-to-clear/trigger lane without replaying an action.
    fn mixed_trigger(&self) -> bool {
        self.readable()
            && !self.command
            && !self.port
            && self.has(FieldClass::WriteOneToClear)
            && self.replayed().1.is_empty()
    }

    /// Each write-trigger field's explicit trigger: in a register written
    /// from zero, one whose read-modify-write replays nothing, or a mixed
    /// word whose write-one-to-clear lanes can all be zeroed.
    fn triggered(&self) -> bool {
        self.api()
            && !self.fifo
            && !self.port
            && (!self.readable() || self.modifiable() || self.mixed_trigger())
    }

    fn has_w(&self) -> bool {
        self.modified() || self.written_from_zero()
    }

    /// The bits a write that sets none of them gives zero, and the fields
    /// whose value it can neither keep nor zero: a read-modify-write's or a
    /// command start's view of the word.
    pub(super) fn replayed(&self) -> (u32, Vec<&FieldPlan<'_>>) {
        let mut zero = self.write_zero();
        let mut refused = Vec::new();
        for field in &self.fields {
            if field.role.is_some() {
                continue;
            }
            match field.replay() {
                Replay::Keep => {}
                Replay::Zero => zero |= field.mask(),
                Replay::Refuse => refused.push(field),
            }
        }
        (zero, refused)
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
    /// Plan the type `block`; `regions` are the root's RAM regions, which
    /// give a pointer's high field its prefix.
    pub(super) fn new(block: &'m BlockType, regions: &[RamRegion]) -> Result<Self, Error> {
        let name = block.name();
        let prefix = names::block_prefix(block);
        // The fixed value of each high field: its pointers' placement
        // prefix (the model's loader checked they agree).
        let mut prefixes = BTreeMap::new();
        for (key, register) in &block.templates {
            for field in &register.fields {
                let Some(pointer) = &field.pointer else {
                    continue;
                };
                let Some(high) = &pointer.encoding.high_field else {
                    continue;
                };
                let region = regions
                    .iter()
                    .find(|region| region.name == pointer.placement)
                    .ok_or_else(|| {
                        Error::new(
                            &format!("{name}.{key}.{}", field.name),
                            "its placement is no RAM region of the root",
                        )
                    })?;
                let value = u32::try_from(region.base >> pointer.encoding.width)
                    .map_err(|_| Error::new(high, "its prefix exceeds 32 bits"))?;
                prefixes.insert(high.clone(), value);
            }
        }
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
                let mut plan = field_plan(
                    &entity,
                    field,
                    &prefix,
                    commands.get(&(key.as_str(), field.name.as_str())).copied(),
                )?;
                plan.prefix = prefixes.get(&format!("{key}.{}", field.name)).copied();
                plan.doorbell = block
                    .file
                    .doorbells
                    .iter()
                    .any(|doorbell| doorbell.register == *key && doorbell.field == field.name);
                fields.push(plan);
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
            // Every method a field gives the register's images, by name:
            // getters, `_variant` getters, setters and `retain_` methods.
            let mut methods: BTreeMap<String, &str> = BTreeMap::new();
            for field in &template.fields {
                let mut names = vec![field.method()];
                if field.values.is_some() {
                    names.push(format!("{}_variant", field.stem));
                }
                if field.setter() {
                    names.push(format!("set_{}", field.stem));
                }
                if field.prefix.is_some() {
                    names.push(format!("set_{}_prefix", field.stem));
                }
                if template.w1c() {
                    names.push(format!("retain_{}", field.stem));
                }
                for method in names {
                    if let Some(other) = methods.insert(method.clone(), &field.field.name) {
                        return Err(Error::new(
                            &format!("{}.{}.{}", self.name, template.key, field.field.name),
                            format!("its method `{method}` is also {other}'s"),
                        ));
                    }
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
    field: &'m Field,
    prefix: &str,
    role: Option<Role>,
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
            seen.insert(value.value);
            if value.value > ones || !names::valid(&variant) {
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
        let complete = width < 32 && seen.len() as u64 == 1_u64 << width;
        Some(Values {
            name: names::camel(&stem),
            variants,
            complete,
        })
    };
    // A narrower write range wins over a complete enumeration: the
    // enumeration still reads the field.
    let only_values = values.is_some()
        && range.is_none()
        && (field.write_values_only || values.as_ref().is_some_and(|v| v.complete));
    if field
        .pointer
        .as_ref()
        .is_some_and(|pointer| pointer.placement != crate::layouts::DMA_REGION)
    {
        return Err(Error::new(
            &entity,
            "a pointer outside the DMA window has no typed address",
        ));
    }
    let pointer = field.pointer.as_ref().map(|pointer| PointerPlan {
        target: pointer.points_to.as_deref().map(crate::layouts::camel),
        member_offset: pointer.member_offset,
        shift: pointer.encoding.shift,
    });
    let setter = if pointer.is_some() {
        Setter::Pointer
    } else if only_values {
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
    Ok(FieldPlan {
        field,
        stem,
        low,
        width,
        class: field.class,
        role,
        pointer,
        prefix: None,
        doorbell: false,
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
            template.doorbells(),
            "DOORBELLS",
            "The doorbells: each set only by its trigger, written back as read.",
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
        let constant = field.stem.to_ascii_uppercase();
        let _ = writeln!(
            output,
            "/// Width of `{}` in bits.\npub const {constant}_WIDTH: u32 = {};\n\
             /// Greatest unsigned image of `{}`.\npub const {constant}_MAX: u32 = 0x{:x};\n\
             /// Positioned bits of `{}`.\npub const {constant}_MASK: u32 = 0x{:x};\n",
            field.field.name,
            field.width,
            field.field.name,
            field.ones(),
            field.field.name,
            field.mask()
        );
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
        output.push_str(if template.modified() {
            "/// The register's value while it is changed.\n\
             pub struct W(pub(super) u32);\n\nimpl W {\n"
        } else {
            "/// The value written to the register.\n\
             pub struct W(pub(super) u32);\n\nimpl W {\n"
        });
        if template.modified() {
            for field in template.fields.iter().filter(|field| field.getter()) {
                render_getter(output, field, "&self");
            }
        }
        for field in &template.fields {
            if let Some(prefix) = field.prefix {
                let _ = writeln!(
                    output,
                    "/// Set `{}` to the fixed high address bits of its pointers'\n\
                     /// placement, `0x{prefix:x}`: its one value.\n\
                     #[inline]\npub fn set_{}_prefix(&mut self) -> &mut Self {{\n\
                     self.0 = (self.0 & 0x{:08x}) | 0x{:08x};\nself\n}}\n",
                    field.field.name,
                    field.stem,
                    !field.mask(),
                    prefix << field.low,
                );
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
            // A pointer takes a typed reference or the zero image, the null
            // reference; never an integer.
            if field.setter == Setter::Pointer {
                let _ = writeln!(
                    output,
                    "/// Set `{}` to the zero image: no address.\n#[inline]\npub fn clear_{}(&mut self) -> &mut Self {{\nself.0 &= 0x{:08x};\nself\n}}\n",
                    field.field.name,
                    field.stem,
                    !field.mask(),
                );
            }
        }
        output.push_str("}\n\n");
    }
    if template.trigger_set() {
        output.push_str(
            "/// The write-trigger fields one write of the register sets together;\n\
             /// every other bit of the write is zero or its fixed value.\n\
             pub struct T(pub(super) u32);\n\nimpl T {\n",
        );
        for field in template.settable_triggers() {
            if field.width == 1 {
                let _ = writeln!(
                    output,
                    "/// Set the write-trigger field `{}`.\n#[inline]\npub fn {}(&mut self) -> &mut Self {{\nself.0 |= 0x{:08x};\nself\n}}\n",
                    field.field.name,
                    field.method(),
                    field.mask(),
                );
            } else {
                let _ = writeln!(
                    output,
                    "/// Set the write-trigger field `{}` to `value`.\n#[inline]\npub fn {}(&mut self, value: {}) -> &mut Self {{\nself.0 = (self.0 & 0x{:08x}) | {};\nself\n}}\n",
                    field.field.name,
                    field.method(),
                    field.write_type(None),
                    !field.mask(),
                    field.positioned(),
                );
            }
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
    let mut unique = BTreeMap::new();
    for (variant, value, description) in &values.variants {
        if unique.insert(*value, variant).is_some() {
            continue;
        }
        if let Some(description) = description {
            push_doc(output, &clean(description), "");
        }
        let _ = writeln!(output, "{variant} = 0x{value:x},");
    }
    let _ = writeln!(
        output,
        "}}\n\nimpl {name} {{\n/// The value's field bits.\n#[inline]\npub const fn bits(self) -> u32 {{\nself as u32\n}}\n"
    );
    let _ = writeln!(
        output,
        "/// The value positioned in its register.\n#[inline]\npub const fn register_bits(self) -> u32 {{ self.bits() << {} }}\n",
        field.low
    );
    let mut canonical = BTreeMap::new();
    let mut variants = Vec::new();
    for (variant, value, description) in &values.variants {
        if let Some(first) = canonical.get(value) {
            if let Some(description) = description {
                push_doc(output, &clean(description), "");
            }
            let _ = writeln!(
                output,
                "/// Context-specific name for the same field image.\n#[allow(non_upper_case_globals)]\npub const {variant}: Self = Self::{first};"
            );
        } else {
            canonical.insert(*value, variant);
            variants.push((variant, value, description));
        }
    }
    if values.complete
        && let Some((last, rest)) = variants.split_last()
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
        let arms = variants
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
) -> Result<(), Error> {
    let module = template.module();
    let index = call.parameter();
    let name = &template.key;
    if template.fifo {
        if mode == Mode::Owned {
            render_fifo(body, template, stem, call);
        }
        return Ok(());
    }
    if !template.api() {
        return Ok(());
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
        for field in template.fields.iter().filter(|field| field.doorbell) {
            let _ = writeln!(
                body,
                "/// Whether the doorbell `{}` of `{name}` still waits: the hardware\n\
                 /// clears it when it accepts the request.\n\
                 #[inline]\npub fn {stem}_{}_pending(&self{index}) -> bool {{\n\
                 (self.0.{} & 0x{:08x}) != 0\n}}\n",
                field.field.name,
                field.stem,
                call.read(),
                field.mask()
            );
        }
    }
    match mode {
        Mode::Read => Ok(()),
        Mode::Shared => render_shared(body, template, stem, call),
        Mode::Owned => render_owned(body, template, stem, call),
    }
}

/// The entity of a template's field, for an error.
fn entity(template: &Template<'_>, field: &FieldPlan<'_>) -> String {
    format!("{}.{}", template.key, field.field.name)
}

fn render_owned(
    body: &mut String,
    template: &Template<'_>,
    stem: &str,
    call: Call,
) -> Result<(), Error> {
    let module = template.module();
    let index = call.parameter();
    let name = &template.key;
    if template.w1c() && template.readable() {
        // Each unsampled lane takes zero: no field may forbid it.
        if let Some(field) = template.fields.iter().find(|field| !field.zero_admitted()) {
            return Err(Error::new(
                &entity(template, field),
                "an acknowledgement writes zero to it, which its write constraint forbids",
            ));
        }
        // The bits of no field keep the value read now; a word whose every
        // bit is a field has none to keep.
        let acknowledgement = if template.declared() == u32::MAX {
            format!("self.0.{};", call.write("snapshot.0"))
        } else {
            format!(
                "let kept = self.0.{} & !{module}::FIELDS;\nself.0.{};",
                call.read(),
                call.write("kept | snapshot.0")
            )
        };
        let _ = writeln!(
            body,
            "/// Sample `{name}` once, keeping its declared fields, to acknowledge.\n\
             #[inline]\npub fn sample_{stem}(&self{index}) -> {module}::Snapshot {{\n\
             {module}::Snapshot(self.0.{} & {module}::FIELDS)\n}}\n\n\
             /// Acknowledge exactly the sampled set: one to each bit the snapshot\n\
             /// holds, zero to every other field; the bits of no field keep the\n\
             /// value read now.\n\
             #[inline]\npub fn acknowledge_{stem}(&mut self{index}, snapshot: {module}::Snapshot) {{\n\
             {acknowledgement}\n}}\n",
            call.read(),
        );
        return Ok(());
    }
    let zero = template.write_zero();
    let modify_read = if zero == 0 {
        format!("self.0.{}", call.read())
    } else {
        format!("self.0.{} & 0x{:08x}", call.read(), !zero)
    };
    if template.command {
        render_command(body, template, stem, call)?;
        return Ok(());
    }
    if template.port {
        if let [field] = template.fields.as_slice()
            && field.port_setter()
        {
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
        return Ok(());
    }
    let from_zero = template.written_from_zero();
    let required = template.required();
    if template.triggered() || from_zero {
        for field in template
            .fields
            .iter()
            .filter(|field| field.class == FieldClass::WriteTrigger && !field.zero)
        {
            let (parameter, bits) = if field.width == 1 {
                (String::new(), format!("0x{:08x}", field.mask()))
            } else {
                (
                    format!(", value: {}", field.write_type(Some(&module))),
                    field.positioned(),
                )
            };
            let image = if template.mixed_trigger() {
                let (zero, _) = template.replayed();
                format!("(self.0.{} & 0x{:08x}) | {bits}", call.read(), !zero)
            } else if template.modifiable() {
                format!("{modify_read} | {bits}")
            } else if let Some(other) = required.first() {
                return Err(Error::new(
                    &entity(template, field),
                    format!(
                        "its trigger writes zero to {}, which no write may give it",
                        other.field.name
                    ),
                ));
            } else if template.zero_image() == 0 {
                unparenthesized(&bits)
            } else {
                format!("0x{:08x} | {bits}", template.zero_image())
            };
            let _ = writeln!(
                body,
                "/// Write to the write-trigger field `{}` of `{name}`, starting its\n\
                 /// hardware action once; {}.\n\
                 #[inline]\npub fn trigger_{stem}_{}(&mut self{index}{parameter}) {{\n\
                 self.0.{};\n}}\n",
                field.field.name,
                if template.mixed_trigger() {
                    "ordinary fields and unassigned bits keep the value read, write-one-to-clear and other trigger lanes are zero"
                } else if template.modifiable() {
                    "every other field keeps the value read, every other trigger is zero"
                } else {
                    "every other bit is zero or its fixed value"
                },
                field.stem,
                call.write(&image)
            );
        }
    }
    if template.trigger_set() && template.triggered() {
        let image = if template.zero_image() == 0 {
            "triggers.0".to_owned()
        } else {
            format!("0x{:08x} | triggers.0", template.zero_image())
        };
        let _ = writeln!(
            body,
            "/// Write to the write-trigger fields of `{name}` the change sets, in one\n\
             /// write, starting each one's hardware action once; every other bit is\n\
             /// zero or its fixed value.\n\
             #[inline]\npub fn trigger_{stem}(&mut self{index}, change: impl FnOnce(&mut {module}::T)) {{\n\
             let mut triggers = {module}::T(0);\n\
             change(&mut triggers);\n\
             self.0.{};\n}}\n",
            call.write(&image)
        );
    }
    if template.modifiable() {
        for field in template.fields.iter().filter(|field| field.doorbell) {
            let _ = writeln!(
                body,
                "/// Ring the doorbell `{}` of `{name}`: one there, every other field\n\
                 /// keeps the value read, every trigger is zero. The hardware clears it\n\
                 /// when it accepts the request.\n\
                 #[inline]\npub fn trigger_{stem}_{}(&mut self{index}) {{\n\
                 self.0.{};\n}}\n",
                field.field.name,
                field.stem,
                call.write(&format!("{modify_read} | 0x{:08x}", field.mask()))
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
    } else if from_zero {
        let mut parameters = String::new();
        let mut terms = Vec::new();
        if template.zero_image() != 0 || required.is_empty() {
            terms.push(format!("0x{:08x}", template.zero_image()));
        }
        for field in &required {
            let parameter = field.method();
            let _ = write!(
                parameters,
                ", {parameter}: {}",
                field.write_type(Some(&module))
            );
            terms.push(field.positioned_of(&parameter));
        }
        let image = match terms.as_slice() {
            [single] => unparenthesized(single),
            _ => terms.join(" | "),
        };
        let _ = writeln!(
            body,
            "/// Write `{name}` from zero: {}the fields set, the reserved bits at\n\
             /// their reset value, every high field at its prefix, zero in every\n\
             /// other bit.\n\
             #[inline]\npub fn write_{stem}(&mut self{index}{parameters}, change: impl FnOnce(&mut {module}::W)) {{\n\
             let mut value = {module}::W({image});\n\
             change(&mut value);\n\
             self.0.{};\n}}\n",
            if required.is_empty() {
                String::new()
            } else {
                format!(
                    "the fields no write may leave zero ({}) as given, ",
                    required
                        .iter()
                        .map(|field| format!("`{}`", field.field.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            call.write("value.0")
        );
    }
    Ok(())
}

/// A command register's starts: one write sets the command and each
/// qualifier as given; the other fields keep the value read, triggers,
/// write-one-to-clear and set-only lanes are zero; a lane neither is safe
/// for is refused.
fn render_command(
    body: &mut String,
    template: &Template<'_>,
    stem: &str,
    call: Call,
) -> Result<(), Error> {
    let index = call.parameter();
    let name = &template.key;
    let (zero, refused) = template.replayed();
    if let Some(field) = refused.first() {
        return Err(Error::new(
            &entity(template, field),
            format!(
                "a start of its command register can neither keep nor zero this {} field",
                field.class.label()
            ),
        ));
    }
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
             /// other self-clearing bit, trigger and write-one-to-clear lane is\n\
             /// zero. It refuses, writing nothing, while any self-clearing bit of\n\
             /// the register reads one.\n\
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
    Ok(())
}

/// A shared word's writes through `&self`: each strobe alone, or the one
/// field stored whole; neither reads the word back. An array's elements
/// each have their own.
fn render_shared(
    body: &mut String,
    template: &Template<'_>,
    stem: &str,
    call: Call,
) -> Result<(), Error> {
    let offsets = match call {
        Call::Fixed(offset) => vec![(stem.to_owned(), offset)],
        Call::Indexed {
            offset,
            stride,
            count,
        } => (0..count)
            .map(|element| {
                let name = template
                    .elements
                    .iter()
                    .find(|candidate| candidate.offset == offset + element * stride)
                    .map_or_else(|| format!("{stem}_{element}"), |found| found.stem.clone());
                (name, offset + element * stride)
            })
            .collect(),
    };
    let name = &template.key;
    let module = template.module();
    for (stem, offset) in offsets {
        for field in template
            .fields
            .iter()
            .filter(|field| field.class == FieldClass::WriteTrigger && !field.zero)
        {
            let (parameter, bits) = if field.width == 1 {
                (String::new(), format!("0x{:08x}", field.mask()))
            } else {
                (
                    format!(", value: {}", field.write_type(Some(&module))),
                    field.positioned(),
                )
            };
            let _ = writeln!(
                body,
                "/// Strobe the write-trigger field `{}` of `{name}` alone: its value\n\
                 /// there, zero in every other bit, nothing read back.\n\
                 #[inline]\npub fn trigger_{stem}_{}(&self{parameter}) {{\n\
                 self.0.write_shared::<0x{offset:x}>({bits});\n}}\n",
                field.field.name, field.stem,
            );
        }
        if let [field] = template.fields.as_slice()
            && field.setter()
        {
            if !field.zero_admitted() && field.pointer.is_none() && field.range.is_none() {
                return Err(Error::new(
                    &entity(template, field),
                    "a shared store of its word has no admitted value type",
                ));
            }
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
    Ok(())
}

fn render_fifo(body: &mut String, template: &Template<'_>, stem: &str, call: Call) {
    let module = template.module();
    let name = &template.key;
    let index = call.parameter();
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
             #[inline]\npub fn pop_{stem}(&mut self{index}) -> {} {{\n{element}\n}}\n",
            field.native(),
        );
    }
    if field.setter() {
        let _ = writeln!(
            body,
            "/// Put one element into the FIFO port `{name}`; every other bit is zero.\n\
             #[inline]\npub fn push_{stem}(&mut self{index}, value: {}) {{\n\
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
    let module = template.module();
    for field in template
        .fields
        .iter()
        .filter(|field| fields.contains(&field.field.name) && !field.zero)
    {
        let (parameter, bits) = if field.width == 1 {
            (String::new(), format!("0x{:08x}", field.mask()))
        } else {
            (
                format!(", value: {}", field.write_type(Some(&module))),
                field.positioned(),
            )
        };
        let write = if divided {
            format!("write_trigger::<0x{offset:x}>({bits})")
        } else {
            format!("write::<0x{offset:x}>({bits})")
        };
        let _ = writeln!(
            body,
            "/// Write to the write-trigger field `{}` of the split word `{name}`,\n\
             /// starting its hardware action once; zero in every other bit, which\n\
             /// leaves the other holders' fields alone.\n\
             #[inline]\npub fn trigger_{stem}_{}(&mut self{parameter}) {{\n\
             self.0.{write};\n}}\n",
            field.field.name, field.stem,
        );
    }
}

/// `expression` without the parentheses around all of it, if it has them.
fn unparenthesized(expression: &str) -> String {
    let Some(inner) = expression
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
    else {
        return expression.to_owned();
    };
    let mut depth = 0_i32;
    for character in inner.chars() {
        depth += match character {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        if depth < 0 {
            return expression.to_owned();
        }
    }
    inner.to_owned()
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
