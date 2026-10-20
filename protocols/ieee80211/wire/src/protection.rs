//! BSS protection fields of the ERP and HT Operation elements.
//!
//! These are the values a BSS advertises; the transmitter's per-PPDU choice
//! of RTS/CTS or CTS-to-self is `oer-ieee80211-upper-mac`'s protection
//! policy.

/// ERP Information element requirements on transmitters.
///
/// Stored as the element's Use_Protection (bit one) and Barker_Preamble_Mode
/// (bit two). NonERP_Present (bit zero) reports membership and is carried
/// separately by an advertising AP.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ErpProtection(u8);

impl ErpProtection {
    const USE_PROTECTION: u8 = 1 << 1;
    const BARKER_PREAMBLE_MODE: u8 = 1 << 2;

    /// No non-ERP station requires protection.
    pub const NONE: Self = Self(0);

    /// Decode the one-byte ERP Information payload. An absent element
    /// carries no protection requirement.
    pub const fn from_information(information: Option<u8>) -> Self {
        match information {
            Some(value) => Self(value & (Self::USE_PROTECTION | Self::BARKER_PREAMBLE_MODE)),
            None => Self::NONE,
        }
    }

    pub const fn new(use_protection: bool, long_preamble_required: bool) -> Self {
        Self(
            if use_protection {
                Self::USE_PROTECTION
            } else {
                0
            } | if long_preamble_required {
                Self::BARKER_PREAMBLE_MODE
            } else {
                0
            },
        )
    }

    pub const fn use_protection(self) -> bool {
        self.0 & Self::USE_PROTECTION != 0
    }

    /// Barker_Preamble_Mode: DSSS/HR frames use the long preamble.
    pub const fn long_preamble_required(self) -> bool {
        self.0 & Self::BARKER_PREAMBLE_MODE != 0
    }

    /// Encode the ERP Information payload with NonERP_Present.
    pub const fn information(self, non_erp_present: bool) -> u8 {
        self.0 | non_erp_present as u8
    }
}

/// Two-bit HT Protection field of the HT Operation element.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HtProtectionMode {
    #[default]
    None,
    Nonmember,
    TwentyMhz,
    NonHtMixed,
}

impl HtProtectionMode {
    /// Decode a complete 24-byte HT Operation element.
    pub const fn from_operation_ie(operation: Option<&[u8; 24]>) -> Self {
        let Some(operation) = operation else {
            return Self::None;
        };
        if operation[0] != 61 || operation[1] != 22 {
            return Self::None;
        }
        Self::from_field(operation[4])
    }

    /// Decode the low two bits of HT Operation Information byte one.
    pub const fn from_field(field: u8) -> Self {
        match field & 0x03 {
            1 => Self::Nonmember,
            2 => Self::TwentyMhz,
            3 => Self::NonHtMixed,
            _ => Self::None,
        }
    }

    pub const fn field(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Nonmember => 1,
            Self::TwentyMhz => 2,
            Self::NonHtMixed => 3,
        }
    }
}

/// Protection bits of HT Operation Information byte one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtOperationProtection {
    pub mode: HtProtectionMode,
    /// An associated HT station cannot receive HT-greenfield PPDUs.
    pub non_greenfield_present: bool,
}

impl HtOperationProtection {
    /// HT Operation Information byte one: HT Protection in bits 1:0 and
    /// Nongreenfield HT STAs Present in bit 2.
    pub const fn information_byte(self) -> u8 {
        self.mode.field() | ((self.non_greenfield_present as u8) << 2)
    }
}

/// Protection facts one AP advertises for its BSS.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApBssProtection {
    pub non_erp_present: bool,
    pub erp: ErpProtection,
    pub ht: HtOperationProtection,
}

impl ApBssProtection {
    pub const fn erp_information(self) -> u8 {
        self.erp.information(self.non_erp_present)
    }
}

#[cfg(test)]
mod tests;
