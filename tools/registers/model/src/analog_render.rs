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

fn field_name(block: &str, register: &str, field: &str) -> String {
    if field.ends_with(crate::OPAQUE_SUFFIX) {
        format!("{block}_{register}_{field}")
    } else {
        format!(
            "{block}_{}",
            field.strip_prefix(&format!("{block}_")).unwrap_or(field)
        )
    }
}

impl RegisterModel {
    /// Generated facts and field composition for the separate transport
    /// space. Access policy and hardware sequencing stay with the owner.
    pub fn render_analog_rust(&self) -> Result<String> {
        let Some(space) = self.analog_spaces().first() else {
            return Ok(String::new());
        };
        let mut output = String::from(
            "/// Native analog-I2C knowledge; these values perform no hardware access.\npub mod analog_i2c {\n pub use oer_espressif_analog::model::{BlockCode as Block, ByteAddress as Address, CaptureRegister, Host, KnownReset, RegisterFacts, Route, SampledByte};\n pub type Field = oer_espressif_analog::AnalogField<Address>;\npub mod blocks { use super::Block;\n",
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
        output.push_str("_ => None, } }\n");
        output.push_str(r#"
/// A validated transport request. Its original code is retained independently
/// of the physical byte identity, including aliases with another host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request { profile: Profile, code: u8, index: u8 }
impl Request {
    pub const fn new(profile: Profile, code: u8, index: u8) -> Option<Self> {
        match route(profile, code) {
            Some(_) => Some(Self { profile, code, index }),
            None => None,
        }
    }
    pub const fn from_address(profile: Profile, address: Address) -> Option<Self> {
        match route(profile, address.block()) {
            Some(r) if r.command_code == address.block() => Self::new(profile, address.block(), address.index()),
            _ => None,
        }
    }
    pub const fn route(self) -> Route {
        match route(self.profile, self.code) {
            Some(route) => route,
            None => panic!("validated analog request"),
        }
    }
    pub const fn address(self) -> Address { self.route().address(self.index) }
    pub const fn index(self) -> u8 { self.index }
}
"#);
        output.push_str("pub mod registers { use super::Address;\n");
        let mut register_names = BTreeSet::new();
        let mut field_names = BTreeSet::new();
        for block in &space.blocks {
            for register in &block.registers {
                let name = format!(
                    "{}_{}",
                    block.name,
                    register
                        .name
                        .strip_prefix(&format!("{}_", block.name))
                        .unwrap_or(&register.name)
                );
                if !register_names.insert(name.clone()) {
                    return Err(Error::message("analog register Rust identifier collision"));
                }
                let entity = format!("{}.{}", block.name, register.name);
                let sources = space
                    .review
                    .iter()
                    .find(|review| review.entity == entity)
                    .ok_or_else(|| Error::message("analog register review omitted"))?
                    .sources
                    .join(", ");
                writeln!(
                    output,
                    "/// SOURCE: {}\npub const {name}: Address = Address::new({:#x}, {:#x});",
                    sources, block.code, register.index
                )
                .ok();
            }
        }
        output.push_str("}\n pub mod fields { use super::{Address, Field};\n");
        for block in &space.blocks {
            for register in &block.registers {
                for field in &register.fields {
                    let name = field_name(&block.name, &register.name, &field.name);
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
                for view in &register.views {
                    let name = field_name(&block.name, &register.name, &view.name);
                    if !field_names.insert(name.clone()) {
                        return Err(Error::message(
                            "analog field view Rust identifier collision",
                        ));
                    }
                    writeln!(output, "/// Subrange of {}; SOURCE: {}\npub const {name}: Field = match Field::new(Address::new({:#x}, {:#x}), {}, {}) {{ Some(field) => field, None => panic!(\"validated analog field view\") }};", view.field, view.sources.join(", "), block.code, register.index, view.bit_offset + view.bit_width - 1, view.bit_offset).ok();
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
        output.push_str("];\n/// The qualified comparison image, ordered by physical transport identity.\npub const CAPTURE: &[CaptureRegister] = &[\n");
        for register in &geometry {
            if let (Some(epoch), Some(mask)) = (&register.read_safe_epoch, register.comparison_mask)
            {
                writeln!(output, "CaptureRegister {{ address: Address::new({:#x}, {:#x}), mask: {mask:#x}, epoch: {epoch:?} }},", register.address.block, register.address.index).ok();
            }
        }
        output.push_str("];\n}\n");
        Ok(output)
    }
}
