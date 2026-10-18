//! The input sections the stage-two runtime linker script places in regions
//! the boot zeroes.
//!
//! These output sections are `NOLOAD`: no bytes of their inputs reach the
//! image, and the boot clears the range. A static placed there by name must
//! therefore start as all zero bytes, or its initializer is silently lost.
//! That no input carries initialized bytes there is to be checked by
//! `cargo fw` after the link (planned; nothing checks it yet). A test keeps
//! [`RUNTIME_ZEROED_REGIONS`] equal to the linker script.

/// One output section the boot zeroes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroedRegion {
    /// The output section in `linker/runtime/sections.x`.
    pub output: &'static str,
    /// The symbols bounding the zeroed range.
    pub start: &'static str,
    pub end: &'static str,
    /// Who zeroes it.
    pub zeroed_by: &'static str,
    /// The input section patterns the script places there: `name` or
    /// `name.*`.
    pub inputs: &'static [&'static str],
}

/// The runtime's zeroed regions.
pub const RUNTIME_ZEROED_REGIONS: &[ZeroedRegion] = &[
    ZeroedRegion {
        output: ".dram2_uninit.unsupported",
        start: "_dram2_uninit_bss_start",
        end: "_dram2_uninit_bss_end",
        zeroed_by: "esp-riscv-rt at reset",
        inputs: &[
            ".dram2_uninit.bss",
            ".dram2_uninit.bss.*",
            ".dram2_uninit",
            ".dram2_uninit.*",
        ],
    },
    ZeroedRegion {
        output: ".rtc_fast.bss",
        start: "_rtc_fast_bss_start",
        end: "_rtc_fast_bss_end",
        zeroed_by: "esp-hal at reset",
        inputs: &[".rtc_fast.bss", ".rtc_fast.bss.*"],
    },
    ZeroedRegion {
        output: ".critical.bss",
        start: "__runtime_critical_bss_start",
        end: "__runtime_critical_bss_end",
        zeroed_by: "the runtime entry (`entry.rs`)",
        inputs: &[
            ".critical.bss",
            ".critical.bss.*",
            ".flash.critical.bss",
            ".flash.critical.bss.*",
        ],
    },
    ZeroedRegion {
        output: ".dma.bss",
        start: "__runtime_dma_bss_start",
        end: "__runtime_dma_bss_end",
        zeroed_by: "the runtime entry (`entry.rs`)",
        inputs: &[".dma.bss", ".dma.bss.*"],
    },
    ZeroedRegion {
        output: ".bss",
        start: "__runtime_data_bss_start",
        end: "__runtime_data_bss_end",
        zeroed_by: "the bootstrap, from the stage-two header's BSS range",
        inputs: &[
            ".psram.bss",
            ".psram.bss.*",
            ".sbss",
            ".sbss.*",
            ".bss",
            ".bss.*",
            "COMMON",
        ],
    },
];

/// Whether an input section named `name` lands in a zeroed region.
pub fn is_zeroed_input(name: &str) -> bool {
    RUNTIME_ZEROED_REGIONS
        .iter()
        .flat_map(|region| region.inputs)
        .any(|pattern| matches_pattern(pattern, name))
}

/// A linker-script input pattern: `name` exactly, or `name.*` for any
/// dotted suffix.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => name
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.len() > 1 && rest.starts_with('.')),
        None => name == pattern,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::{string::String, vec::Vec};

    use super::*;

    const SCRIPT: &str = include_str!("../../linker/runtime/sections.x");

    #[test]
    fn dma_storage_is_checked_against_the_generated_window() {
        let script: String = SCRIPT.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(script.contains("INCLUDE dma-window.x"), "{SCRIPT}");
        for check in [
            "__runtime_dma_data_start >= DMA_WINDOW_ORIGIN",
            "__runtime_dma_bss_end <= DMA_WINDOW_ORIGIN + DMA_WINDOW_LENGTH",
            "(__runtime_dma_data_start & ((1 << DMA_ADDRESS_LOW_BITS) - 1)) != 0",
            "(__runtime_dma_data_start >> DMA_ADDRESS_LOW_BITS) == ((__runtime_dma_bss_end - 1) >> DMA_ADDRESS_LOW_BITS)",
        ] {
            assert!(script.contains(check), "{check}");
        }
    }

    /// The body of output section `name` and whether it is `NOLOAD`.
    fn output_section(name: &str) -> Option<(bool, &'static str)> {
        let header = SCRIPT.lines().find(|line| {
            let line = line.trim_start();
            line.split_whitespace().next() == Some(name) && line.trim_end().ends_with(':')
        })?;
        let start = SCRIPT.find(header)? + header.len();
        let open = start + SCRIPT[start..].find('{')?;
        let close = open + SCRIPT[open..].find('}')?;
        Some((header.contains("(NOLOAD)"), &SCRIPT[open + 1..close]))
    }

    /// The input patterns of a section body: every name inside `*( … )`.
    fn patterns(body: &str) -> Vec<String> {
        let mut patterns = Vec::new();
        let mut rest = body;
        while let Some(at) = rest.find("*(") {
            let inner = &rest[at + 2..];
            let end = inner.find(')').unwrap();
            patterns.extend(inner[..end].split_whitespace().map(String::from));
            rest = &inner[end..];
        }
        patterns
    }

    #[test]
    fn every_zeroed_region_matches_its_noload_output_section() {
        for region in RUNTIME_ZEROED_REGIONS {
            let (noload, body) = output_section(region.output)
                .unwrap_or_else(|| panic!("{} is not in the linker script", region.output));
            assert!(noload, "{} must be NOLOAD", region.output);
            let mut expected: Vec<String> =
                region.inputs.iter().map(|p| String::from(*p)).collect();
            let mut found = patterns(body);
            expected.sort();
            found.sort();
            assert_eq!(found, expected, "{}", region.output);
            assert!(
                body.contains(&std::format!("{} = ABSOLUTE(.)", region.start)),
                "{}",
                region.start
            );
            assert!(
                body.contains(&std::format!("{} = ABSOLUTE(.)", region.end)),
                "{}",
                region.end
            );
        }
    }

    #[test]
    fn every_noload_section_with_a_bss_range_is_listed() {
        for line in SCRIPT.lines() {
            let line = line.trim_start();
            if !line.contains("(NOLOAD)") || !line.trim_end().ends_with(':') {
                continue;
            }
            let name = line.split_whitespace().next().unwrap();
            let (_, body) = output_section(name).unwrap();
            if body.contains("bss_start = ABSOLUTE(.)") {
                assert!(
                    RUNTIME_ZEROED_REGIONS
                        .iter()
                        .any(|region| region.output == name),
                    "{name} is a zeroed NOLOAD section missing from RUNTIME_ZEROED_REGIONS"
                );
            }
        }
    }

    #[test]
    fn patterns_match_exact_names_and_dotted_suffixes() {
        assert!(is_zeroed_input(".psram.bss.open_radio_pool"));
        assert!(is_zeroed_input(".critical.bss"));
        assert!(is_zeroed_input(".bss.x"));
        assert!(is_zeroed_input("COMMON"));
        assert!(!is_zeroed_input(".psram.noinit.flags"));
        assert!(!is_zeroed_input(".psram.bssx"));
        assert!(!is_zeroed_input(".data.x"));
        assert!(!is_zeroed_input(".rela.dma.bss.x"));
    }
}
