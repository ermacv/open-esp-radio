//! The published binding index (`chips/<chip>/registers/published/*.bindings.toml`):
//! every register address of a publication with its fields and the svd2rust
//! path that reaches it. A publication that generates no PAC, such as a
//! chip's platform registers read by the host, omits the crate name; the
//! paths then name what svd2rust would generate. `cargo registers generate` writes it from the release
//! SVD; tools that name addresses and bits read it through [`BindingIndex`]
//! instead of declaring its shape again.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// The binding index format.
pub const SCHEMA: u32 = 2;

/// One publication's binding index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BindingIndex {
    pub schema: u32,
    /// The Rust crate identifier of the raw PAC; absent when the publication
    /// generates none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crate_name: Option<String>,
    #[serde(default)]
    pub registers: Vec<RegisterBinding>,
}

/// One register at one address; a word two registers share appears twice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterBinding {
    pub address: u32,
    /// Width in bits.
    pub width: u32,
    pub access: String,
    /// `PERIPHERAL.REGISTER`, the SVD names.
    pub identity: String,
    pub peripheral: String,
    pub peripheral_type: String,
    pub peripheral_module: String,
    #[serde(default)]
    pub scope: Vec<ScopeBinding>,
    pub register_method: String,
    pub register_index: Option<u32>,
    pub alternate_register: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldBinding>,
}

/// One cluster level between the peripheral and the register.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeBinding {
    pub method: String,
    pub index: Option<u32>,
}

/// One field of a register.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldBinding {
    pub svd_name: String,
    pub method: String,
    pub index: Option<u32>,
    pub bit_offset: u32,
    pub bit_width: u32,
    pub access: String,
}

impl BindingIndex {
    /// The index of `text`; another schema is an error.
    pub fn parse(text: &str) -> Result<Self, String> {
        let index: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        if index.schema != SCHEMA {
            return Err(format!(
                "binding index schema {} is not {SCHEMA}",
                index.schema
            ));
        }
        Ok(index)
    }

    /// The index of the file at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One register of a published index, as the generator writes it.
    pub const ONE_REGISTER: &str = r#"
schema = 2
crate-name = "oer_chip_pac_raw"

[[registers]]
address = 537935904
width = 32
access = "read-write"
identity = "MAC.TX_CONFIG"
peripheral = "MAC"
peripheral-type = "Mac"
peripheral-module = "mac"
register-method = "tx_config"

[[registers.fields]]
svd-name = "ENABLE"
method = "enable"
bit-offset = 0
bit-width = 1
access = "read-write"
"#;

    #[test]
    fn a_published_index_parses_and_another_schema_does_not() {
        let index = BindingIndex::parse(ONE_REGISTER).unwrap();
        assert_eq!(index.registers[0].address, 0x2010_4020);
        assert_eq!(index.registers[0].fields[0].svd_name, "ENABLE");
        assert!(index.registers[0].scope.is_empty());
        let other = ONE_REGISTER.replace("schema = 2", "schema = 1");
        assert!(
            BindingIndex::parse(&other)
                .unwrap_err()
                .contains("schema 1")
        );
        let unknown = ONE_REGISTER.replace("width = 32", "width = 32\ncolor = 1");
        assert!(BindingIndex::parse(&unknown).is_err());
    }
}
