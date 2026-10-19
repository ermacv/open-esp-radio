//! Static RV32 image values: the ELF ABI, load segments and executable
//! section views. No loading or relocation.
use crate::*;
use serde::{Deserialize, Serialize};

/// ELF-declared floating-point calling convention, separate from instruction
/// coverage and from the integer analysis profile. No execution support implied.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiscvAbi {
    Ilp32,
    Ilp32f,
    Ilp32d,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSegment {
    pub address: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub flags: u32,
}

/// Borrowed executable section with structurally validated ELF data intervals.
/// Consumers must reset instruction state across each data interval.
pub struct ExecutableSectionView<'a> {
    pub section: u32,
    pub address: u32,
    pub bytes: &'a [u8],
    pub data_ranges: &'a [CodeRange],
}

/// An address range the caller asserts is read-only at run time whatever the
/// image's section flags say, such as flash mapped for reading: an analysis
/// reads constants from the file-backed bytes inside it. The caller takes it
/// from the chip's memory map; nothing infers it from the image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyWindow {
    pub start: u32,
    pub length: u64,
}

impl ReadOnlyWindow {
    /// Whether one of `windows` holds all of `[start, start + length)`.
    pub fn cover(windows: &[ReadOnlyWindow], start: u64, length: u64) -> bool {
        let Some(end) = start.checked_add(length) else {
            return false;
        };
        windows.iter().any(|window| {
            let first = u64::from(window.start);
            start >= first
                && first
                    .checked_add(window.length)
                    .is_some_and(|last| end <= last)
        })
    }
}
