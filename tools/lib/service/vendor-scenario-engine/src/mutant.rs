//! Point mutants of a production probe image.
//!
//! A mutant names its target by address or by `symbol+offset`, may omit the
//! original bytes to take the instruction the probe ELF holds there, and
//! may replace them with `nop` or `ret` instead of spelled bytes. Every
//! mutant resolves against an executable segment of the ELF it patches, so
//! a failing run means a killed mutant, not a bad patch. After the run, each
//! mutant reports whether any comparison executed it: a mutant that
//! survives unexecuted says nothing about the comparisons.
use crate::harness::{Result, invalid};
use blobray_application::in_process::ImagePatch;
use std::collections::BTreeSet;

/// ELF program header flag of an executable segment.
/// Bytes of a compressed and of a full RV32 instruction.
const COMPRESSED: usize = 2;
const FULL: usize = 4;
/// `c.nop`, `addi x0, x0, 0`, `c.jr ra` and `jalr x0, 0(ra)`.
const C_NOP: u16 = 0x0001;
const NOP: u32 = 0x0000_0013;
const C_RET: u16 = 0x8082;
const RET: u32 = 0x0000_8067;

/// Where a mutant applies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Address(u32),
    Symbol { name: String, offset: u32 },
}

/// What replaces the original bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Replacement {
    Bytes(Vec<u8>),
    /// No-operation instructions over the original length.
    Nop,
    /// A return at the target, no-operations after it.
    Ret,
}

/// One mutant as written: `TARGET[:ORIGINAL]:REPLACEMENT`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spec {
    pub target: Target,
    pub original: Option<Vec<u8>>,
    pub replacement: Replacement,
}

fn hex_bytes(text: &str) -> std::result::Result<Vec<u8>, String> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return Err(format!(
            "`{text}` is not a whole number of hexadecimal bytes"
        ));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex_word(text: &str) -> std::result::Result<u32, String> {
    u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|e| format!("`{text}`: {e}"))
}

/// Parse `TARGET[:ORIGINAL]:REPLACEMENT`: TARGET a hexadecimal address or
/// `symbol+OFFSET`, ORIGINAL hexadecimal bytes in memory order, REPLACEMENT
/// hexadecimal bytes, `nop` or `ret`.
pub fn parse(text: &str) -> std::result::Result<Spec, String> {
    let parts: Vec<&str> = text.split(':').collect();
    let (target, original, replacement) = match parts[..] {
        [target, replacement] => (target, None, replacement),
        [target, original, replacement] => (target, Some(original), replacement),
        _ => return Err("expected TARGET[:ORIGINAL]:REPLACEMENT".into()),
    };
    let target = match target.split_once('+') {
        Some((name, offset)) if !name.is_empty() => Target::Symbol {
            name: name.into(),
            offset: hex_word(offset)?,
        },
        _ if target.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            Target::Address(hex_word(target)?)
        }
        _ => Target::Symbol {
            name: target.into(),
            offset: 0,
        },
    };
    let replacement = match replacement {
        "nop" => Replacement::Nop,
        "ret" => Replacement::Ret,
        bytes => Replacement::Bytes(hex_bytes(bytes)?),
    };
    Ok(Spec {
        target,
        original: original.map(hex_bytes).transpose()?,
        replacement,
    })
}

/// The bytes of `replacement` over `length` original bytes.
fn replacement_bytes(replacement: &Replacement, length: usize) -> Result<Vec<u8>> {
    let fill = |mut bytes: Vec<u8>| -> Result<Vec<u8>> {
        while bytes.len() < length {
            if (length - bytes.len()).is_multiple_of(FULL) {
                bytes.extend(NOP.to_le_bytes());
            } else {
                bytes.extend(C_NOP.to_le_bytes());
            }
        }
        if bytes.len() == length {
            Ok(bytes)
        } else {
            Err(invalid(format!(
                "{length} bytes hold no whole instructions"
            )))
        }
    };
    match replacement {
        Replacement::Bytes(bytes) => Ok(bytes.clone()),
        Replacement::Nop => fill(vec![]),
        Replacement::Ret if length.is_multiple_of(FULL) => fill(RET.to_le_bytes().to_vec()),
        Replacement::Ret => fill(C_RET.to_le_bytes().to_vec()),
    }
}

/// Resolve `specs` against the executable segments and symbols of `elf`.
pub fn resolve(elf: &[u8], specs: &[Spec]) -> Result<Vec<ImagePatch>> {
    if specs.is_empty() {
        return Ok(vec![]);
    }
    let file = oer_elf::Elf::parse(elf)?;
    specs
        .iter()
        .map(|spec| {
            let address = match &spec.target {
                Target::Address(address) => *address,
                Target::Symbol { name, offset } => {
                    let symbol = file
                        .symbols()
                        .find(|s| s.name == name.as_str() && s.size > 0)
                        .ok_or_else(|| {
                            invalid(format!("mutant symbol {name} is not in the probe"))
                        })?;
                    if u64::from(*offset) >= symbol.size {
                        return Err(invalid(format!(
                            "mutant offset {offset:#x} lies outside {name}"
                        )));
                    }
                    u32::try_from(symbol.address)? + offset
                }
            };
            let segment = file
                .segments()
                .find(|segment| {
                    segment.executable
                        && segment.address <= u64::from(address)
                        && u64::from(address) < segment.address + segment.size
                })
                .ok_or_else(|| {
                    invalid(format!("mutant at {address:#x} is outside executable code"))
                })?;
            let at = |length: usize| -> Result<Vec<u8>> {
                segment
                    .bytes(u64::from(address), length)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| invalid(format!("mutant at {address:#x} runs past its segment")))
            };
            let original = match &spec.original {
                Some(original) => {
                    if at(original.len())? != *original {
                        return Err(invalid(format!(
                            "mutant at {address:#x} does not match the probe bytes"
                        )));
                    }
                    original.clone()
                }
                None => {
                    let first = at(COMPRESSED)?;
                    at(if first[0] & 3 == 3 { FULL } else { COMPRESSED })?
                }
            };
            let replacement = replacement_bytes(&spec.replacement, original.len())?;
            if replacement.len() != original.len() {
                return Err(invalid(format!(
                    "mutant at {address:#x} replaces {} bytes with {}",
                    original.len(),
                    replacement.len()
                )));
            }
            Ok(ImagePatch {
                address,
                original,
                replacement,
            })
        })
        .collect()
}

/// One line per mutant: whether a comparison executed it.
pub fn report(patches: &[ImagePatch], executed: &BTreeSet<u32>) -> Vec<String> {
    patches
        .iter()
        .map(|patch| {
            let end = patch.address + patch.original.len() as u32;
            if executed.range(patch.address..end).next().is_some() {
                format!(
                    "mutant at {:#x} executed and survived: no comparison observes its change",
                    patch.address
                )
            } else {
                format!(
                    "mutant at {:#x} NOT REACHED: no comparison executed it, so its survival says nothing",
                    patch.address
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutants_parse_by_address_or_symbol_with_named_replacements() {
        assert_eq!(
            parse("100019ca:130101cd:67800000").unwrap(),
            Spec {
                target: Target::Address(0x1000_19ca),
                original: Some(vec![0x13, 0x01, 0x01, 0xcd]),
                replacement: Replacement::Bytes(vec![0x67, 0x80, 0, 0]),
            }
        );
        assert_eq!(
            parse("open_x+0x12:nop").unwrap(),
            Spec {
                target: Target::Symbol {
                    name: "open_x".into(),
                    offset: 0x12
                },
                original: None,
                replacement: Replacement::Nop,
            }
        );
        assert_eq!(
            parse("open_x:ret").unwrap().target,
            Target::Symbol {
                name: "open_x".into(),
                offset: 0
            }
        );
        assert!(parse("open_x").is_err());
        assert!(parse("10:abc:00").is_err());
    }

    #[test]
    fn named_replacements_fill_the_original_length_with_whole_instructions() {
        assert_eq!(
            replacement_bytes(&Replacement::Nop, 2).unwrap(),
            [0x01, 0x00]
        );
        assert_eq!(
            replacement_bytes(&Replacement::Nop, 4).unwrap(),
            [0x13, 0, 0, 0]
        );
        assert_eq!(
            replacement_bytes(&Replacement::Ret, 4).unwrap(),
            [0x67, 0x80, 0, 0]
        );
        assert_eq!(
            replacement_bytes(&Replacement::Ret, 6).unwrap(),
            [0x82, 0x80, 0x13, 0, 0, 0]
        );
        assert!(replacement_bytes(&Replacement::Nop, 3).is_err());
    }

    #[test]
    fn an_unexecuted_mutant_is_reported_unreached() {
        let patch = ImagePatch {
            address: 0x100,
            original: vec![0; 4],
            replacement: vec![0; 4],
        };
        let lines = report(std::slice::from_ref(&patch), &BTreeSet::from([0x102]));
        assert!(lines[0].contains("executed and survived"), "{lines:?}");
        let lines = report(&[patch], &BTreeSet::from([0x104]));
        assert!(lines[0].contains("NOT REACHED"), "{lines:?}");
    }
}
