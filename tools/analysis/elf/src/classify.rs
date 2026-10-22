//! Classification of raw ELF inventory values, without parsing another file.
use object::elf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolSection {
    Undefined,
    Absolute,
    Common,
    Section(u32),
    /// XINDEX without its extended table entry; never a section index.
    Extended,
    Reserved(u16),
}

impl SymbolSection {
    pub fn index(self) -> Option<u32> {
        match self {
            Self::Section(index) => Some(index),
            _ => None,
        }
    }

    pub fn is_defined(self) -> bool {
        self != Self::Undefined
    }
}

/// XINDEX uses the supplied extended index even inside the reserved range.
/// UNDEF, ABS, COMMON and other reserved values do not name sections.
pub fn symbol_section(raw: u16, extended: Option<u32>) -> SymbolSection {
    match raw {
        elf::SHN_UNDEF => SymbolSection::Undefined,
        elf::SHN_ABS => SymbolSection::Absolute,
        elf::SHN_COMMON => SymbolSection::Common,
        elf::SHN_XINDEX => extended.map_or(SymbolSection::Extended, SymbolSection::Section),
        index if index < elf::SHN_LORESERVE => SymbolSection::Section(u32::from(index)),
        other => SymbolSection::Reserved(other),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SectionFlags(pub u64);

impl SectionFlags {
    pub fn allocated(self) -> bool {
        self.0 & u64::from(elf::SHF_ALLOC) != 0
    }

    pub fn executable(self) -> bool {
        self.0 & u64::from(elf::SHF_EXECINSTR) != 0
    }

    pub fn writable(self) -> bool {
        self.0 & u64::from(elf::SHF_WRITE) != 0
    }

    pub fn strings(self) -> bool {
        self.0 & u64::from(elf::SHF_STRINGS) != 0
    }

    pub fn tls(self) -> bool {
        self.0 & u64::from(elf::SHF_TLS) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Relocatable,
    Executable,
    Other(u16),
}

pub fn object_kind(raw: u16) -> ObjectKind {
    match raw {
        elf::ET_REL => ObjectKind::Relocatable,
        elf::ET_EXEC => ObjectKind::Executable,
        other => ObjectKind::Other(other),
    }
}

/// The captured header profile shared by RV32 operations, independently of
/// whether the object is relocatable or executable.
pub fn is_riscv32_little_endian(bits: u8, little_endian: bool, machine: u16) -> bool {
    bits == 32 && little_endian && machine == elf::EM_RISCV
}

/// Header-only recognition of a little-endian executable. This deliberately
/// does not validate the remaining ELF structure or select an ISA.
pub fn is_executable_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&elf::ELFMAG)
        && bytes.get(5) == Some(&elf::ELFDATA2LSB)
        && bytes
            .get(16..18)
            .and_then(|kind| <[u8; 2]>::try_from(kind).ok())
            .is_some_and(|kind| object_kind(u16::from_le_bytes(kind)) == ObjectKind::Executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_indices_and_special_definitions_are_distinct() {
        assert_eq!(
            symbol_section(elf::SHN_XINDEX, Some(70_000)).index(),
            Some(70_000)
        );
        assert_eq!(
            symbol_section(elf::SHN_XINDEX, None),
            SymbolSection::Extended
        );
        for (raw, expected) in [
            (elf::SHN_UNDEF, SymbolSection::Undefined),
            (elf::SHN_ABS, SymbolSection::Absolute),
            (elf::SHN_COMMON, SymbolSection::Common),
        ] {
            let section = symbol_section(raw, Some(123));
            assert_eq!(section, expected);
            assert_eq!(section.index(), None);
        }
        assert_eq!(symbol_section(7, None).index(), Some(7));
        assert_eq!(symbol_section(elf::SHN_LORESERVE, None).index(), None);
    }

    #[test]
    fn section_storage_and_flags_keep_their_classification() {
        assert_eq!(
            crate::SectionKind::of(elf::SHT_NOBITS),
            crate::SectionKind::NoBits
        );
        assert_eq!(
            crate::SectionKind::of(elf::SHT_PROGBITS),
            crate::SectionKind::Program
        );
        for (raw, expected) in [
            (elf::SHF_ALLOC, [true, false, false, false, false]),
            (elf::SHF_EXECINSTR, [false, true, false, false, false]),
            (elf::SHF_WRITE, [false, false, true, false, false]),
            (elf::SHF_STRINGS, [false, false, false, true, false]),
            (elf::SHF_TLS, [false, false, false, false, true]),
            (
                elf::SHF_ALLOC | elf::SHF_WRITE | elf::SHF_TLS,
                [true, false, true, false, true],
            ),
            (0, [false; 5]),
        ] {
            let flags = SectionFlags(u64::from(raw));
            assert_eq!(
                [
                    flags.allocated(),
                    flags.executable(),
                    flags.writable(),
                    flags.strings(),
                    flags.tls(),
                ],
                expected
            );
        }
    }

    #[test]
    fn rv32_profile_requires_the_machine_class_and_byte_order() {
        assert!(is_riscv32_little_endian(32, true, elf::EM_RISCV));
        for machine in [elf::EM_NONE, elf::EM_386, elf::EM_X86_64] {
            assert!(!is_riscv32_little_endian(32, true, machine));
        }
        assert!(!is_riscv32_little_endian(64, true, elf::EM_RISCV));
        assert!(!is_riscv32_little_endian(32, false, elf::EM_RISCV));
    }

    #[test]
    fn executable_recognition_uses_only_the_header_and_rejects_short_inputs() {
        let mut bytes = [0; 18];
        bytes[..4].copy_from_slice(&elf::ELFMAG);
        bytes[5] = elf::ELFDATA2LSB;
        bytes[16..18].copy_from_slice(&elf::ET_EXEC.to_le_bytes());
        assert!(is_executable_image(&bytes));
        for end in 0..bytes.len() {
            assert!(!is_executable_image(&bytes[..end]));
        }
        bytes[16..18].copy_from_slice(&elf::ET_REL.to_le_bytes());
        assert!(!is_executable_image(&bytes));
        bytes[16..18].copy_from_slice(&elf::ET_EXEC.to_le_bytes());
        bytes[5] = elf::ELFDATA2MSB;
        assert!(!is_executable_image(&bytes));
    }
}

#[cfg(test)]
mod header_tests {
    use super::is_executable_image;
    #[test]
    fn only_a_little_endian_executable_header_is_an_image() {
        let mut header = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0];
        header.resize(16, 0);
        let image = [header.clone(), vec![2, 0]].concat();
        assert!(is_executable_image(&image));
        assert!(
            !is_executable_image(&[header.clone(), vec![1, 0]].concat()),
            "ET_REL"
        );
        let mut big = image.clone();
        big[5] = 2;
        assert!(!is_executable_image(&big), "big-endian");
        assert!(!is_executable_image(b"!<arch>\n"), "an archive");
        assert!(!is_executable_image(&header), "a truncated header");
    }
}
