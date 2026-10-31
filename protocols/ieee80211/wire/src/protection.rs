//! BSS protection fields of the ERP and HT Operation elements.
//!
//! These are the values a BSS advertises; the transmitter's per-PPDU choice
//! of RTS/CTS or CTS-to-self is `oer-ieee80211-core-upper-mac`'s protection
//! policy.

/// ERP element identifier.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1 (Element IDs).
pub const ERP_ELEMENT_ID: u8 = 42;

/// ERP Information element requirements on transmitters.
///
/// Stored as the element's Use_Protection (bit one) and Barker_Preamble_Mode
/// (bit two). NonERP_Present (bit zero) reports membership and is carried
/// separately by an advertising AP.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ErpProtection(u8);

impl ErpProtection {
    /// No non-ERP station requires protection.
    pub const NONE: Self = Self(0);

    /// Decode the one-byte ERP Information payload. An absent element
    /// carries no protection requirement.
    pub const fn from_information(information: Option<u8>) -> Self {
        match information {
            Some(value) => Self::new(
                ErpInformation::from_bytes([value]).use_protection(),
                ErpInformation::from_bytes([value]).barker(),
            ),
            None => Self::NONE,
        }
    }

    pub const fn new(use_protection: bool, long_preamble_required: bool) -> Self {
        Self(
            ErpInformation::from_bytes([0])
                .with_use_protection(use_protection)
                .with_barker(long_preamble_required)
                .to_bytes()[0],
        )
    }

    pub const fn use_protection(self) -> bool {
        ErpInformation::from_bytes([self.0]).use_protection()
    }

    /// Barker_Preamble_Mode: DSSS/HR frames use the long preamble.
    pub const fn long_preamble_required(self) -> bool {
        ErpInformation::from_bytes([self.0]).barker()
    }

    /// Encode the ERP Information payload with NonERP_Present.
    pub const fn information(self, non_erp_present: bool) -> u8 {
        ErpInformation::from_bytes([self.0])
            .with_non_erp(non_erp_present)
            .to_bytes()[0]
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
        match HtProtectionInformation::from_bytes([field]).mode() {
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
        HtProtectionInformation::from_bytes([self.mode.field()])
            .with_non_greenfield(self.non_greenfield_present)
            .to_bytes()[0]
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

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.13 (ERP Information element).
    pub struct ErpInformation[1] (little) {
        pub non_erp, with_non_erp: bool = 0..1;
        pub use_protection, with_use_protection: bool = 1..2;
        pub barker, with_barker: bool = 2..3;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.56 (HT Operation element).
    pub struct HtProtectionInformation[1] (little) {
        pub mode, with_mode: u8 = 0..2;
        pub non_greenfield, with_non_greenfield: bool = 2..3;
    }
}

/// The ERP elements of a run of information elements, checked once: every
/// element's Length stays within the run and every ERP element carries one
/// octet of ERP Information. Elements of other identifiers stay opaque.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1 (element format: Element ID,
/// Length, body) and 9.4.2 (the ERP element: one octet of ERP Information).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErpElements<'a> {
    elements: &'a [u8],
}

/// Information elements whose ERP elements cannot be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErpElementsError {
    /// An element's Length runs past the elements, or a lone octet trails
    /// them.
    MalformedElements,
    /// An ERP element whose body is not one octet.
    ErpLength,
}

impl<'a> ErpElements<'a> {
    /// Check `elements`, a frame's information elements.
    pub fn parse(elements: &'a [u8]) -> Result<Self, ErpElementsError> {
        let mut remaining = elements;
        while !remaining.is_empty() {
            let (element, rest) = crate::element::Element::split(remaining)
                .ok_or(ErpElementsError::MalformedElements)?;
            if element.id == ERP_ELEMENT_ID && element.body.len() != 1 {
                return Err(ErpElementsError::ErpLength);
            }
            remaining = rest;
        }
        Ok(Self { elements })
    }

    /// Every ERP element's ERP Information field, in frame order. A BSS
    /// sends one; what repeats mean is the receiver's decision.
    pub fn information(&self) -> impl Iterator<Item = u8> + 'a {
        let mut remaining = self.elements;
        core::iter::from_fn(move || {
            let (element, rest) = crate::element::Element::split(remaining)?;
            remaining = rest;
            Some(element)
        })
        .filter_map(|element| match (element.id, element.body) {
            (ERP_ELEMENT_ID, &[information]) => Some(information),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests;
