//! Checks on the ROM entry points exported by a chip's linker script.

use alloc::{collections::BTreeSet, format};

use crate::{Result, Rom};

impl Rom {
    pub(crate) fn validate_functions(&self) -> Result<()> {
        let mut names = BTreeSet::new();
        for function in &self.functions {
            let name = &function.name;
            let valid_name = name
                .as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
                && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
            if !valid_name || !names.insert(name) {
                return Err(format!("invalid or duplicate ROM function name: {name:?}").into());
            }
            let source = &function.source;
            if !source.starts_with("SOURCE: ")
                || source.trim() == "SOURCE:"
                || source.contains(['\n', '\r'])
                || source.contains("*/")
            {
                return Err(
                    format!("ROM function `{name}` needs a single-line SOURCE: citation").into(),
                );
            }
            // Radio ROM is forbidden by source-policy.md; libc and compiler
            // intrinsics come from compiler_builtins, never vendor scripts.
            for script in [
                "net80211",
                "pp",
                "phy",
                "coexist",
                "libc",
                "newlib",
                "newlib-nano",
                "libgcc",
                "rvfp",
            ] {
                if source.contains(&format!(".rom.{script}.ld")) {
                    return Err(
                        format!("ROM function `{name}` cites excluded script `{script}`").into(),
                    );
                }
            }
            if function.address == 0 || !function.address.is_multiple_of(2) {
                return Err(
                    format!("ROM function `{name}` needs a nonzero aligned entry point").into(),
                );
            }
            if self
                .forbidden
                .iter()
                .any(|range| (range.start..range.end).contains(&function.address))
            {
                return Err(format!("ROM function `{name}` is in forbidden ROM code").into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RomFunction, RomRange, generate};
    use alloc::{string::ToString, vec};

    fn rom() -> Rom {
        Rom {
            revision: "rev0".to_string(),
            elf: "rom".to_string(),
            summaries: None,
            forbidden: vec![RomRange {
                name: "radio".to_string(),
                start: 0x2000,
                end: 0x3000,
            }],
            linker_script: "rom/chip.x".to_string(),
            functions: vec![RomFunction {
                name: "delay".to_string(),
                address: 0x1000,
                source: "SOURCE: ESP-IDF pin components/esp_rom/chip/ld/chip.rom.ld:1".to_string(),
            }],
            sram: vec![],
        }
    }

    #[test]
    fn the_script_contains_only_sourced_function_assignments() {
        let rom = rom();
        let text = generate::rom_script(&rom).expect("ROM script");
        assert_eq!(
            text.lines().skip(1).collect::<alloc::vec::Vec<_>>(),
            [
                "/* SOURCE: ESP-IDF pin components/esp_rom/chip/ld/chip.rom.ld:1 */",
                "delay = 0x00001000;",
            ]
        );
        assert_eq!(
            text,
            generate::rom_script(&rom).expect("deterministic ROM script")
        );
        let mut empty = rom;
        empty.functions.clear();
        assert_eq!(
            generate::rom_script(&empty)
                .expect("no exports")
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn invalid_duplicate_unsourced_and_forbidden_exports_are_refused() {
        let mut duplicate = rom();
        duplicate.functions.push(duplicate.functions[0].clone());
        assert!(generate::rom_script(&duplicate).is_err());
        for name in ["", "1delay", "delay; INCLUDE vendor.x"] {
            let mut invalid = rom();
            invalid.functions[0].name = name.to_string();
            assert!(generate::rom_script(&invalid).is_err(), "{name}");
        }
        for source in [
            "",
            "SOURCE: ",
            "no citation",
            "SOURCE: */ INCLUDE vendor.x",
            "SOURCE: pin\n",
        ] {
            let mut invalid = rom();
            invalid.functions[0].source = source.to_string();
            assert!(generate::rom_script(&invalid).is_err(), "{source}");
        }
        for address in [0, 0x1001, 0x2000, 0x2ffe] {
            let mut invalid = rom();
            invalid.functions[0].address = address;
            assert!(generate::rom_script(&invalid).is_err(), "{address:#x}");
        }
    }

    #[test]
    fn radio_and_libc_script_provenance_is_refused_even_outside_forbidden_ranges() {
        for script in [
            "net80211",
            "pp",
            "phy",
            "coexist",
            "libc",
            "newlib",
            "newlib-nano",
            "libgcc",
            "rvfp",
        ] {
            let mut invalid = rom();
            invalid.functions[0].source = format!("SOURCE: ESP-IDF pin chip.rom.{script}.ld:1");
            assert!(generate::rom_script(&invalid).is_err(), "{script}");
        }
    }
}
