//! Register and field names of the chip's published register bindings.
//!
//! The triage report names the addresses and bit masks it resolves from the
//! published model, so a reviewer reads `MAC.TX_CONFIG` and its fields rather
//! than an upper immediate and a shift. The index is read through the
//! register publication's own reader ([`BindingIndex`]).
use oer_register_bindings::BindingIndex;
use std::collections::BTreeMap;
use std::path::Path;

/// Bytes of one register word.
const WORD: u32 = 4;
/// Bits of one byte.
const BYTE_BITS: u32 = 8;

/// One published register.
#[derive(Clone, Debug)]
pub struct Register {
    pub identity: String,
    /// Fields as (name, first bit, width).
    pub fields: Vec<(String, u32, u32)>,
}

/// Published registers by word address.
#[derive(Default)]
pub struct Registers {
    by_address: BTreeMap<u32, Register>,
}

impl Registers {
    /// The registers of the bindings file at `path`.
    pub fn load(path: &Path) -> crate::harness::Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    /// The registers of bindings `text`. A word two published registers
    /// share names both.
    pub fn parse(text: &str) -> crate::harness::Result<Self> {
        let bindings = BindingIndex::parse(text)?;
        let mut by_address: BTreeMap<u32, Register> = BTreeMap::new();
        for bound in bindings.registers {
            let fields = bound
                .fields
                .into_iter()
                .map(|f| (f.svd_name, f.bit_offset, f.bit_width))
                .collect();
            match by_address.get_mut(&bound.address) {
                Some(register) => {
                    register.identity = format!("{}|{}", register.identity, bound.identity);
                }
                None => {
                    by_address.insert(
                        bound.address,
                        Register {
                            identity: bound.identity,
                            fields,
                        },
                    );
                }
            }
        }
        Ok(Self { by_address })
    }

    /// The register holding byte `address`, and the bit its byte starts at.
    pub fn at(&self, address: u32) -> Option<(&Register, u32)> {
        let word = address & !(WORD - 1);
        self.by_address
            .get(&word)
            .map(|register| (register, (address - word) * BYTE_BITS))
    }

    /// `address` with its register's name when one is published.
    pub fn name(&self, address: u32) -> String {
        match self.at(address) {
            Some((register, 0)) => format!("{address:#010x} {}", register.identity),
            Some((register, bit)) => {
                format!("{address:#010x} {} from bit {bit}", register.identity)
            }
            None => format!("{address:#010x}"),
        }
    }

    /// The bits of `mask` in the register at word `address`, named by the
    /// fields they touch; a bit no field covers is listed by number.
    pub fn bits(&self, address: u32, mask: u32) -> String {
        let named = self.at(address).map(|(register, _)| register);
        let mut parts = vec![];
        let mut rest = mask;
        if let Some(register) = named {
            for (name, first, width) in &register.fields {
                let field = field_mask(*first, *width);
                if mask & field == 0 {
                    continue;
                }
                rest &= !field;
                parts.push(if mask & field == field {
                    name.clone()
                } else {
                    format!("{name}[{}]", bit_list(mask & field, *first))
                });
            }
        }
        if rest != 0 {
            parts.push(format!("bits {}", bit_list(rest, 0)));
        }
        let owner = named.map_or_else(|| format!("{address:#010x}"), |r| r.identity.clone());
        format!("{owner}: {}", parts.join(", "))
    }
}

fn field_mask(first: u32, width: u32) -> u32 {
    let ones = if width >= u32::BITS {
        u32::MAX
    } else {
        (1 << width) - 1
    };
    ones.checked_shl(first).unwrap_or(0)
}

/// The set bits of `mask`, numbered from `base`.
fn bit_list(mask: u32, base: u32) -> String {
    (0..u32::BITS)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| (bit - base.min(bit)).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINDINGS: &str = r#"
schema = 2
crate-name = "oer_chip_pac_raw"
[[registers]]
address = 0x20104020
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
[[registers.fields]]
svd-name = "COLOR"
method = "color"
bit-offset = 16
bit-width = 6
access = "read-write"
"#;

    #[test]
    fn masks_are_named_by_the_fields_they_touch() {
        let registers = Registers::parse(BINDINGS).unwrap();
        assert_eq!(registers.name(0x2010_4020), "0x20104020 MAC.TX_CONFIG");
        assert_eq!(
            registers.name(0x2010_4022),
            "0x20104022 MAC.TX_CONFIG from bit 16"
        );
        assert_eq!(registers.bits(0x2010_4020, 1), "MAC.TX_CONFIG: ENABLE");
        assert_eq!(
            registers.bits(0x2010_4020, 1 << 19 | 1 << 30),
            "MAC.TX_CONFIG: COLOR[3], bits 30"
        );
        assert_eq!(registers.bits(0x3000_0000, 0x80), "0x30000000: bits 7");
    }
}
