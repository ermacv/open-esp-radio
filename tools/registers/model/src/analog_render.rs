//! Pure data and byte-value APIs; no transport transactions or CPU pointers.
use crate::{Error, RegisterModel, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

fn variant(profile: &str) -> String {
    profile
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            format!(
                "{}{}",
                chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or('_'),
                chars.as_str()
            )
        })
        .collect()
}

impl RegisterModel {
    /// Generated facts and field composition for the separate transport
    /// space. Access policy and hardware sequencing stay with the owner.
    pub fn render_analog_rust(&self) -> Result<String> {
        let Some(space) = self.analog_spaces().first() else {
            return Ok(String::new());
        };
        let mut output = String::from(
            "/// Native analog-I2C knowledge; these values perform no hardware access.\npub mod analog_i2c {\n pub use oer_espressif_analog::model::{BlockCode as Block, ByteAddress as Address, Host, KnownReset, RegisterFacts, Route, SampledByte};\n pub type Field = oer_espressif_analog::AnalogField<Address>;\npub mod blocks { use super::Block;\n",
        );
        for block in &space.blocks {
            writeln!(
                output,
                "/// SOURCE: {}\npub const {}: Block = Block::new({:#x});",
                block.sources.join(", "),
                block.name,
                block.code
            )
            .ok();
        }
        output.push_str("}\n");
        let mut profiles = BTreeMap::new();
        let mut variants = BTreeSet::new();
        for route in space.blocks.iter().flat_map(|b| &b.routes) {
            if profiles.contains_key(&route.profile) {
                continue;
            }
            let name = variant(&route.profile);
            if name.as_bytes().first().is_none_or(u8::is_ascii_digit)
                || !variants.insert(name.clone())
            {
                return Err(Error::message(
                    "route profiles have colliding or invalid Rust identifiers",
                ));
            }
            profiles.insert(route.profile.clone(), name);
        }
        writeln!(
            output,
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum Profile {{ {} }}",
            profiles
                .values()
                .map(|v| format!("{v},"))
                .collect::<String>()
        )
        .ok();
        output.push_str("/// Resolve the original request before any command-code alias.\npub const fn route(profile: Profile, request_code: u8) -> Option<Route> { match (profile, request_code) {\n");
        for block in &space.blocks {
            for route in &block.routes {
                let variant = profiles
                    .get(&route.profile)
                    .ok_or_else(|| Error::message("route profile omitted"))?;
                writeln!(output, "// SOURCE: {}\n(Profile::{variant}, {:#x}) => Some(Route {{ request_code: {:#x}, command_code: {:#x}, host: Host::Host{}, read_select: {:#x}, read_mask_bits: {} }}),", route.sources.join(", "), route.request_code, route.request_code, block.code, route.host, route.read_select, route.read_mask_bits).ok();
            }
        }
        output.push_str("_ => None, } }\n pub mod registers { use super::Address;\n");
        let mut register_names = BTreeSet::new();
        let mut field_names = BTreeSet::new();
        for block in &space.blocks {
            for register in &block.registers {
                let name = format!("{}_{}", block.name, register.name);
                if !register_names.insert(name.clone()) {
                    return Err(Error::message("analog register Rust identifier collision"));
                }
                writeln!(
                    output,
                    "/// SOURCE: {}\npub const {name}: Address = Address::new({:#x}, {:#x});",
                    block.sources.join(", "),
                    block.code,
                    register.index
                )
                .ok();
            }
        }
        output.push_str("}\n pub mod fields { use super::{Address, Field};\n");
        for block in &space.blocks {
            for register in &block.registers {
                for field in &register.fields {
                    let name = format!("{}_{}", block.name, field.name);
                    if !field_names.insert(name.clone()) {
                        return Err(Error::message("analog field Rust identifier collision"));
                    }
                    let entity = format!("{}.{}.{}", block.name, register.name, field.name);
                    let sources = space
                        .review
                        .iter()
                        .find(|r| r.entity == entity)
                        .map(|r| r.sources.join(", "))
                        .unwrap_or_default();
                    writeln!(output, "/// SOURCE: {sources}\npub const {name}: Field = match Field::new(Address::new({:#x}, {:#x}), {}, {}) {{ Some(field) => field, None => panic!(\"validated analog field\") }};", block.code, register.index, field.bit_offset() + field.bit_width() - 1, field.bit_offset()).ok();
                }
            }
        }
        output.push_str("}\n");
        let geometry = space.geometry(self.chip())?;
        writeln!(
            output,
            "pub const REGISTERS: [RegisterFacts; {}] = [",
            geometry.len()
        )
        .ok();
        for register in &geometry {
            let reset = match (register.reset_value, register.reset_known_mask) {
                (Some(value), Some(mask)) => format!("KnownReset::new({value:#x}, {mask:#x})"),
                _ => "None".into(),
            };
            writeln!(output, "RegisterFacts {{ address: Address::new({:#x}, {:#x}), described: {:#x}, opaque: {:#x}, reserved: {:#x}, reset: {reset}, read_safe_epoch: {:?} }},", register.address.block, register.address.index, register.states.described, register.states.opaque, register.states.reserved, register.read_safe_epoch).ok();
        }
        output.push_str("];\n/// Registers with an independently reviewed finite read-safe epoch.\npub const READ_SAFE: &[(Address, &str)] = &[\n");
        for register in &geometry {
            if let Some(epoch) = &register.read_safe_epoch {
                writeln!(
                    output,
                    "(Address::new({:#x}, {:#x}), {:?}),",
                    register.address.block, register.address.index, epoch
                )
                .ok();
            }
        }
        output.push_str("];\n}\n");
        Ok(output)
    }
}
