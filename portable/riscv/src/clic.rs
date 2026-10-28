//! Architectural CLIC values, independent of any chip's line count or chosen MNLBITS.
//!
//! SOURCE: [RISC-V CLIC specification](https://github.com/pulp-platform/clic/blob/master/doc/clic.adoc),
//! §4.2.2 "Specifying Interrupt Level", §4.5 "CLIC Interrupt Enable",
//! §4.6 "CLIC Interrupt Attribute" and §4.7 "CLIC Interrupt Input Control".
//! The HAL accesses individual PAC fields; this module neither packs a whole
//! register word nor overwrites privilege mode, pending or reserved bits.
#![forbid(unsafe_code)]

/// `mtvt`: base of the table of hardware-vectored handler addresses.
///
/// SOURCE: RISC-V Core-Local Interrupt Controller (CLIC) specification,
/// "CLIC CSRs", `mtvt` (CSR 0x307).
pub const MTVT: u16 = 0x307;

/// `mnxti`: takes the next pending interrupt above the current level (a
/// write form lowers `mintstatus.mil` to that interrupt's level).
///
/// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mnxti` (CSR 0x345).
pub const MNXTI: u16 = 0x345;

/// `mintstatus`: read-only machine interrupt status (`mil`).
///
/// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mintstatus`
/// (CSR 0xFB1).
pub const MINTSTATUS: u16 = 0xfb1;

/// `mintstatus.mil`, bits 31:24: the machine interrupt level the hart
/// runs at, 0 outside every interrupt handler.
///
/// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mintstatus` (field
/// `mil`, bits 31:24).
pub const MINTSTATUS_MIL_SHIFT: u32 = 24;

/// `mintthresh`: machine interrupt-level threshold.
///
/// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mintthresh`
/// (CSR 0x347).
pub const MINTTHRESH: u16 = 0x347;

/// An NLBITS value outside the architectural range 0..=8.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidLevelBits {
    pub bits: u8,
}

/// A one-based level outside the configured level range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidLevel {
    pub level: u16,
    pub level_bits: LevelBits,
}

/// Validated NLBITS; the chip facts choose its value, not this crate.
///
/// SOURCE: RISC-V CLIC specification §4.2.2: 0..=8 upper control bits
/// encode level; any remaining bits of the effective level read as ones.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct LevelBits(u8);

impl LevelBits {
    /// Validate a dynamically observed or supplied NLBITS value.
    pub const fn new(bits: u8) -> Result<Self, InvalidLevelBits> {
        if bits <= 8 {
            Ok(Self(bits))
        } else {
            Err(InvalidLevelBits { bits })
        }
    }

    /// Validate fixed chip facts at compile time.
    ///
    /// ```compile_fail,E0080
    /// let _ = oer_riscv::clic::LevelBits::constant::<9>();
    /// ```
    pub const fn constant<const BITS: u8>() -> Self {
        const {
            match Self::new(BITS) {
                Ok(bits) => bits,
                Err(_) => panic!("CLIC NLBITS must be in 0..=8"),
            }
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Number of distinct levels (one when NLBITS is zero, 256 at eight).
    pub const fn levels(self) -> u16 {
        1u16.wrapping_shl(self.0 as u32)
    }

    /// Encode a one-based level with all lower priority bits set to one.
    /// Implemented control bits are a chip fact: if fewer than NLBITS are
    /// implemented, hardware aliases levels by reading missing bits as ones.
    ///
    /// SOURCE: RISC-V CLIC specification §4.2.2 and §4.7 (`clicintctl`).
    pub const fn control(self, level: u16) -> Result<Control, InvalidLevel> {
        if level == 0 || level > self.levels() {
            return Err(InvalidLevel {
                level,
                level_bits: self,
            });
        }
        let low = 8u8.wrapping_sub(self.0) as u32;
        Ok(Control(
            (level.wrapping_sub(1).wrapping_shl(low) | 1u16.wrapping_shl(low).wrapping_sub(1))
                as u8,
        ))
    }
}

/// A `clicintctl` byte, including raw observations of all eight bits.
///
/// SOURCE: RISC-V CLIC specification §4.7: implemented bits are left
/// justified and unimplemented bits read as ones.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Control(u8);

impl Control {
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Encode a fixed one-based level and NLBITS at compile time.
    ///
    /// ```compile_fail,E0080
    /// let _ = oer_riscv::clic::Control::level::<0, 3>();
    /// ```
    /// ```compile_fail,E0080
    /// let _ = oer_riscv::clic::Control::level::<9, 3>();
    /// ```
    pub const fn level<const LEVEL: u16, const BITS: u8>() -> Self {
        const {
            match LevelBits::constant::<BITS>().control(LEVEL) {
                Ok(control) => control,
                Err(_) => panic!("CLIC level is outside NLBITS"),
            }
        }
    }
}

/// The individual `clicintattr.trig` field values.
///
/// SOURCE: RISC-V CLIC specification §4.6: bit 0 selects edge versus
/// level, bit 1 selects negative versus positive polarity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Trigger {
    PositiveLevel = 0b00,
    PositiveEdge = 0b01,
    NegativeLevel = 0b10,
    NegativeEdge = 0b11,
}

/// Individual CLIC line fields; privilege mode and pending are untouched.
///
/// SOURCE: RISC-V CLIC specification §4.5 (`clicintie`, one enable bit),
/// §4.6 (`clicintattr.shv`, one vectoring bit, and `trig`), §4.7 (`clicintctl`).
/// Booleans represent the two single-bit fields, not complete register bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineState {
    /// Individual `clicintie.ie` field.
    pub enabled: bool,
    /// Individual `clicintattr.shv` field: hardware vectoring when set.
    pub vectored: bool,
    /// Individual `clicintattr.trig` field.
    pub trigger: Trigger,
    /// `clicintctl` level and priority byte.
    pub control: Control,
}

impl LineState {
    /// Disabled with zero control; not a claim about WARL readback/reset state.
    pub const DISABLED: Self = Self {
        enabled: false,
        vectored: false,
        trigger: Trigger::PositiveLevel,
        control: Control::from_bits(0),
    };

    /// Enabled, hardware-vectored, positive-level triggered at `control`.
    pub const fn vectored(control: Control) -> Self {
        Self {
            enabled: true,
            vectored: true,
            trigger: Trigger::PositiveLevel,
            control,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "test assertions and bounded input enumeration"
)]
mod tests {
    use super::*;

    #[test]
    fn each_level_preserves_order_and_lower_priority_ones() {
        for bits in 0..=8 {
            let geometry = LevelBits::new(bits).expect("architectural NLBITS");
            let mut previous = None;
            for level in 1..=geometry.levels() {
                let control = geometry.control(level).expect("level inside geometry");
                if let Some(previous) = previous {
                    assert!(control.bits() > previous);
                }
                let missing = 8u32.wrapping_sub(bits.into());
                assert_eq!(u16::from(control.bits()).wrapping_add(1), level << missing);
                previous = Some(control.bits());
            }
            assert_eq!(previous, Some(u8::MAX));
        }
    }

    #[test]
    fn invalid_inputs_are_typed_refusals_including_zero_and_full_width() {
        for bits in 9..=u8::MAX {
            assert_eq!(LevelBits::new(bits), Err(InvalidLevelBits { bits }));
        }
        for bits in 0..=8 {
            let level_bits = LevelBits::new(bits).expect("architectural NLBITS");
            for level in [0, level_bits.levels() + 1, u16::MAX] {
                assert_eq!(
                    level_bits.control(level),
                    Err(InvalidLevel { level, level_bits })
                );
            }
        }
        assert_eq!(Control::level::<1, 0>().bits(), 255);
        assert_eq!(Control::level::<1, 8>().bits(), 0);
        assert_eq!(Control::level::<256, 8>().bits(), 255);
    }
}
