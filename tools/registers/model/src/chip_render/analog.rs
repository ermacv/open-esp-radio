//! The analog-I2C dictionary of the chip root: each block's code and its
//! routes, every register's byte address and facts, every field's bits.
//! The values perform no hardware access; the HAL's analog host does.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::{Error, names};
use crate::chip_model::{ChipModel, Location, parse_bits};

/// The `analog_i2c` module, or nothing when the root has no analog block.
pub(super) fn render(model: &ChipModel) -> Result<String, Error> {
    let instances = model.analog_instances();
    if instances.is_empty() {
        return Ok(String::new());
    }
    let mut output = String::from(
        "/// The analog-I2C blocks of the chip: block codes, transport routes,\n\
         /// register bytes and their fields. These values perform no hardware\n\
         /// access.\n\
         pub mod analog_i2c {\n\
         pub use oer_espressif_analog::model::{BlockCode as Block, ByteAddress as Address, CaptureRegister, Host, KnownReset, RegisterFacts, Route, SampledByte};\n\
         /// One field of an analog-I2C byte.\n\
         pub type Field = oer_espressif_analog::AnalogField<Address>;\n\n\
         /// The block codes.\npub mod blocks {\nuse super::Block;\n",
    );
    for instance in instances {
        let _ = writeln!(
            output,
            "/// SOURCE: {}.\npub const {}: Block = Block::new(0x{:x});",
            instance.sources.join(", "),
            instance.name,
            instance.code
        );
    }
    output.push_str("}\n\n");
    render_routes(&mut output, model)?;
    let mut facts = Vec::new();
    for instance in instances {
        let block = model
            .types()
            .get(&instance.block_type)
            .ok_or_else(|| Error::new(&instance.name, "its type is no analog type of the root"))?;
        let prefix = names::block_prefix(block);
        let module = names::ident(&names::snake(&instance.name));
        let _ = writeln!(
            output,
            "/// The `{}` block (code 0x{:x}), of type `{}`: a module per register\n\
             /// with its byte's `ADDRESS` and its fields.\npub mod {module} {{",
            instance.name, instance.code, instance.block_type
        );
        let mut modules = BTreeMap::new();
        for register in model
            .registers()
            .iter()
            .filter(|register| register.instance == instance.name)
        {
            let Location::AnalogI2c(address) = register.location else {
                continue;
            };
            let entity = register.identity.as_str();
            let template = block
                .templates
                .get(&register.key)
                .ok_or_else(|| Error::new(entity, "its template is not in its type"))?;
            let register_module = names::ident(&names::snake(names::strip(&register.key, &prefix)));
            if let Some(other) = modules.insert(register_module.clone(), entity.to_owned()) {
                return Err(Error::new(
                    entity,
                    format!("its module `{register_module}` is also {other}'s"),
                ));
            }
            let _ = writeln!(
                output,
                "/// `{}`.\npub mod {register_module} {{\nuse super::super::{{Address, Field}};\n\n\
                 /// The register's byte.\n///\n/// SOURCE: {}.\n\
                 pub const ADDRESS: Address = Address::new(0x{:x}, 0x{:x});\n",
                register.key,
                register.review.sources.join(", "),
                address.block,
                address.index
            );
            let mut field_names = BTreeMap::from([("ADDRESS".to_owned(), entity.to_owned())]);
            let mut described = 0u32;
            let mut opaque = 0u32;
            for field in &register.fields {
                let mask = (u32::MAX >> (32 - field.width)) << field.low;
                if field.name.ends_with(crate::OPAQUE_SUFFIX) {
                    opaque |= mask;
                } else {
                    described |= mask;
                }
                let field_constant = constant(names::strip(&field.name, &prefix));
                let maximum = u32::MAX >> (32 - field.width);
                let _ = writeln!(
                    output,
                    "/// Width of `{}`.\npub const {field_constant}_WIDTH: u32 = {};\n/// Greatest field image.\npub const {field_constant}_MAX: u8 = {maximum};",
                    field.name, field.width
                );
                if let Some(source) = template
                    .fields
                    .iter()
                    .find(|source| source.name == field.name)
                {
                    for value in &source.values {
                        let sources = if value.sources.is_empty() {
                            &field.review.sources
                        } else {
                            &value.sources
                        };
                        let _ = writeln!(
                            output,
                            "/// Named image of `{}`. SOURCE: {}.\npub const {field_constant}_{}: u8 = {};",
                            field.name,
                            sources.join(", "),
                            value.name,
                            value.value
                        );
                    }
                }
                push_field(
                    &mut output,
                    &mut field_names,
                    &format!("{entity}.{}", field.name),
                    &constant(names::strip(&field.name, &prefix)),
                    &format!("SOURCE: {}.", field.review.sources.join(", ")),
                    (address.block, address.index),
                    (field.low, field.width),
                )?;
            }
            for view in &template.views {
                let (low, width) = parse_bits(&view.bits)
                    .map_err(|error| Error::new(entity, format!("{error:?}")))?;
                push_field(
                    &mut output,
                    &mut field_names,
                    &format!("{entity}.{}", view.name),
                    &constant(names::strip(&view.name, &prefix)),
                    &format!(
                        "A subrange of `{}`; SOURCE: {}.",
                        view.field,
                        view.sources.join(", ")
                    ),
                    (address.block, address.index),
                    (low, width),
                )?;
            }
            output.push_str("}\n\n");
            let byte = |bits: u32| u8::try_from(bits & 0xff).unwrap_or(u8::MAX);
            facts.push(Facts {
                address: (address.block, address.index),
                described: byte(described),
                opaque: byte(opaque),
                reserved: byte(register.reserved),
                reset: register.reset.map(|reset| {
                    (
                        byte(reset.value & reset.mask.unwrap_or(0xff)),
                        byte(reset.mask.unwrap_or(0xff)),
                    )
                }),
                read_safe: template
                    .read_safe
                    .as_ref()
                    .map(|safe| (safe.epoch.clone(), safe.comparison_mask)),
            });
        }
        output.push_str("}\n\n");
    }
    render_facts(&mut output, &facts);
    output.push_str("}\n\n");
    Ok(output)
}

/// An upper snake constant name of a model name; one starting with a
/// digit takes a leading `_`.
fn constant(name: &str) -> String {
    let constant = names::snake(name).to_ascii_uppercase();
    if constant.starts_with(|character: char| character.is_ascii_digit()) {
        format!("_{constant}")
    } else {
        constant
    }
}

fn push_field(
    output: &mut String,
    seen: &mut BTreeMap<String, String>,
    entity: &str,
    name: &str,
    doc: &str,
    (block, index): (u8, u8),
    (low, width): (u32, u32),
) -> Result<(), Error> {
    if let Some(other) = seen.insert(name.to_owned(), entity.to_owned()) {
        return Err(Error::new(
            entity,
            format!("its constant `{name}` is also {other}'s"),
        ));
    }
    let _ = writeln!(
        output,
        "/// {doc}\npub const {name}: Field = match Field::new(Address::new(0x{block:x}, 0x{index:x}), {}, {low}) {{\n\
         Some(field) => field,\nNone => panic!(\"the model's field lies in its byte\"),\n}};",
        low + width - 1
    );
    Ok(())
}

/// The route profiles and the route of each request code.
fn render_routes(output: &mut String, model: &ChipModel) -> Result<(), Error> {
    let mut profiles = BTreeMap::new();
    let mut variants = BTreeSet::new();
    for route in model
        .analog_instances()
        .iter()
        .flat_map(|instance| &instance.routes)
    {
        if profiles.contains_key(&route.profile) {
            continue;
        }
        let variant = names::camel(&route.profile);
        if !names::valid(&variant)
            || variant.starts_with(|character: char| character.is_ascii_digit())
            || !variants.insert(variant.clone())
        {
            return Err(Error::new(
                &route.profile,
                "the route profile names no distinct Rust variant",
            ));
        }
        profiles.insert(route.profile.clone(), variant);
    }
    let _ = writeln!(
        output,
        "/// A transport's route profile.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum Profile {{\n{}}}\n\n\
         /// The route of `request_code` in `profile`: the original request is\n\
         /// resolved before any command-code alias.\n\
         pub const fn route(profile: Profile, request_code: u8) -> Option<Route> {{\n\
         match (profile, request_code) {{",
        profiles
            .values()
            .map(|variant| format!("{variant},\n"))
            .collect::<String>()
    );
    for instance in model.analog_instances() {
        for route in &instance.routes {
            let variant = profiles
                .get(&route.profile)
                .ok_or_else(|| Error::new(&route.profile, "its profile is unknown"))?;
            let host = match route.host {
                0 => "Host0",
                1 => "Host1",
                _ => {
                    return Err(Error::new(
                        &instance.name,
                        format!("route host {} is no analog-I2C host", route.host),
                    ));
                }
            };
            let _ = writeln!(
                output,
                "// SOURCE: {}.\n(Profile::{variant}, 0x{:x}) => Some(Route {{ request_code: 0x{:x}, command_code: 0x{:x}, host: Host::{host}, read_select: 0x{:x}, read_mask_bits: {} }}),",
                route.sources.join(", "),
                route.request_code,
                route.request_code,
                instance.code,
                route.read_select,
                route.read_mask_bits
            );
        }
    }
    output.push_str("_ => None,\n}\n}\n\n");
    output.push_str(
        "/// A request the profile routes: its original code is kept apart from\n\
         /// the byte it reaches, including an alias on another host.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub struct Request {\nroute: Route,\nindex: u8,\n}\n\n\
         impl Request {\n\
         /// The request `code` for byte `index`, if the profile routes it.\n\
         pub const fn new(profile: Profile, code: u8, index: u8) -> Option<Self> {\n\
         match route(profile, code) {\nSome(route) => Some(Self { route, index }),\nNone => None,\n}\n}\n\n\
         /// The request reaching `address` under its own command code.\n\
         pub const fn from_address(profile: Profile, address: Address) -> Option<Self> {\n\
         match route(profile, address.block()) {\n\
         Some(route) if route.command_code == address.block() => Some(Self { route, index: address.index() }),\n\
         _ => None,\n}\n}\n\n\
         /// The request's route, with its original request code.\n\
         pub const fn route(self) -> Route {\nself.route\n}\n\n\
         /// The byte the request reaches.\n\
         pub const fn address(self) -> Address {\nself.route().address(self.index)\n}\n\n\
         /// The byte's index in its block.\n\
         pub const fn index(self) -> u8 {\nself.index\n}\n}\n\n",
    );
    Ok(())
}

/// One register byte's facts.
struct Facts {
    address: (u8, u8),
    described: u8,
    opaque: u8,
    reserved: u8,
    reset: Option<(u8, u8)>,
    read_safe: Option<(String, Option<u32>)>,
}

fn render_facts(output: &mut String, facts: &[Facts]) {
    let _ = writeln!(
        output,
        "/// The facts of every register byte.\npub const REGISTERS: [RegisterFacts; {}] = [",
        facts.len()
    );
    for fact in facts {
        let (block, index) = fact.address;
        let reset = fact.reset.map_or_else(
            || "None".to_owned(),
            |(value, mask)| format!("KnownReset::new(0x{value:x}, 0x{mask:x})"),
        );
        let _ = writeln!(
            output,
            "RegisterFacts {{ address: Address::new(0x{block:x}, 0x{index:x}), described: 0x{:x}, opaque: 0x{:x}, reserved: 0x{:x}, reset: {reset}, read_safe_epoch: {:?} }},",
            fact.described,
            fact.opaque,
            fact.reserved,
            fact.read_safe.as_ref().map(|(epoch, _)| epoch.as_str())
        );
    }
    output.push_str(
        "];\n\n/// The bytes a finite reviewed epoch admits to a read.\n\
         pub const READ_SAFE: &[(Address, &str)] = &[\n",
    );
    for fact in facts {
        if let Some((epoch, _)) = &fact.read_safe {
            let (block, index) = fact.address;
            let _ = writeln!(
                output,
                "(Address::new(0x{block:x}, 0x{index:x}), {epoch:?}),"
            );
        }
    }
    output.push_str(
        "];\n\n/// The qualified comparison image, in transport order.\n\
         pub const CAPTURE: &[CaptureRegister] = &[\n",
    );
    for fact in facts {
        if let Some((epoch, Some(mask))) = &fact.read_safe {
            let (block, index) = fact.address;
            let _ = writeln!(
                output,
                "CaptureRegister {{ address: Address::new(0x{block:x}, 0x{index:x}), mask: 0x{mask:x}, epoch: {epoch:?} }},"
            );
        }
    }
    output.push_str("];\n");
}
