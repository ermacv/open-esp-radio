//! One information element: Element ID, Length and body.
//!
//! SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::code::octet_code;

octet_code! { element;
    /// An Element ID: any octet, some of them named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
    ElementId {
        /// SSID.
        SSID = 0,
        /// Supported Rates and BSS Membership Selectors.
        SUPPORTED_RATES = 1,
        /// DSSS Parameter Set.
        DSSS_PARAMETER_SET = 3,
        /// TIM.
        TIM = 5,
        /// Power Capability.
        POWER_CAPABILITY = 33,
        /// Channel Switch Announcement.
        CHANNEL_SWITCH_ANNOUNCEMENT = 37,
        /// ERP.
        ERP = 42,
        /// HT Capabilities.
        HT_CAPABILITIES = 45,
        /// RSN.
        RSN = 48,
        /// Extended Supported Rates and BSS Membership Selectors.
        EXTENDED_SUPPORTED_RATES = 50,
        /// Timeout Interval.
        TIMEOUT_INTERVAL = 56,
        /// Extended Channel Switch Announcement.
        EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT = 60,
        /// HT Operation.
        HT_OPERATION = 61,
        /// Secondary Channel Offset.
        SECONDARY_CHANNEL_OFFSET = 62,
        /// Management MIC.
        MANAGEMENT_MIC = 76,
        /// Fine Timing Measurement Parameters.
        FTM_PARAMETERS = 206,
        /// TWT.
        TWT = 216,
        /// Vendor Specific, which a KDE's and Key Data padding's first
        /// octet is as well.
        ///
        /// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-35 (KDE format).
        VENDOR_SPECIFIC = 221,
        /// RSN Extension.
        RSNX = 244,
        /// An element named by its Element ID Extension.
        EXTENSION = 255,
    }
}

octet_code! { element;
    /// An Element ID Extension: any octet, some of them named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs);
    /// IEEE Std 802.11ax-2021.
    ElementIdExtension {
        /// Password Identifier.
        PASSWORD_IDENTIFIER = 33,
        /// HE Capabilities.
        HE_CAPABILITIES = 35,
        /// HE Operation.
        HE_OPERATION = 36,
        /// UL MU Power Capabilities.
        UL_MU_POWER_CAPABILITIES = 60,
        /// Rejected Groups.
        REJECTED_GROUPS = 92,
        /// Anti-Clogging Token Container.
        ANTI_CLOGGING_TOKEN_CONTAINER = 93,
    }
}

impl ElementIdExtension {
    /// The extension that names the complete element `bytes`; `None` for an
    /// element named by its Element ID.
    pub const fn read(bytes: &[u8]) -> Option<Self> {
        match *bytes {
            [id, _, extension, ..] if id == ElementId::EXTENSION.value() => {
                Some(Self::new(extension))
            }
            _ => None,
        }
    }
}

/// One element borrowed from the caller's bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: ElementId,
    pub body: &'a [u8],
    /// The complete element: Element ID, Length and body.
    pub bytes: &'a [u8],
}

impl<'a> Element<'a> {
    /// The element at the front of `bytes` and what follows it; `None` when
    /// its Length runs past `bytes`.
    pub fn split(bytes: &'a [u8]) -> Option<(Self, &'a [u8])> {
        let (&[id, length], after_header) = bytes.split_first_chunk::<2>()?;
        let (body, rest) = after_header.split_at_checked(usize::from(length))?;
        let (element, _) = bytes.split_at_checked(usize::from(length).checked_add(2)?)?;
        Some((
            Self {
                id: ElementId::new(id),
                body,
                bytes: element,
            },
            rest,
        ))
    }

    /// The one element `bytes` holds exactly; `None` for fewer or more
    /// octets.
    pub fn exact(bytes: &'a [u8]) -> Option<Self> {
        match Self::split(bytes)? {
            (element, []) => Some(element),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_element_splits_at_its_length_and_an_exact_one_has_nothing_after() {
        let bytes = [48, 2, 1, 0, 244, 1, 0x20];
        let (rsn, rest) = Element::split(&bytes).unwrap();
        assert_eq!(
            (rsn.id, rsn.body, rsn.bytes),
            (ElementId::RSN, &[1, 0][..], &bytes[..4])
        );
        assert_eq!(rest, &[244, 1, 0x20]);
        assert_eq!(
            Element::exact(rest),
            Some(Element {
                id: ElementId::RSNX,
                body: &[0x20],
                bytes: rest
            })
        );
        assert_eq!(Element::exact(&bytes), None);
        assert_eq!(Element::split(&[48, 3, 1, 0]), None);
        assert_eq!(Element::split(&[48]), None);
    }

    #[test]
    fn an_extension_names_only_an_element_of_the_extension_id() {
        assert_eq!(
            ElementIdExtension::read(&[255, 1, 36]),
            Some(ElementIdExtension::HE_OPERATION)
        );
        assert_eq!(ElementIdExtension::read(&[254, 1, 36]), None);
        assert_eq!(ElementIdExtension::read(&[255, 0]), None);
    }
}
