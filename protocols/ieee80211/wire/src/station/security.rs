//! The RSN element and RSNXE of a station's association request.

use super::*;

use crate::security::{AssociationAkm, AssociationSecurity, Pmkid, RsnAssociation};

use crate::security::rsn::{
    RSN_CIPHER_CCMP, RSNXE_ONE_OCTET_LEN, RsnExtendedCapabilities, rsn_single_suite_element,
    rsnx_element,
};

/// The association request's RSN element and RSNXE, and the security they
/// negotiate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRsn {
    length: oer_wire::PrefixLength<SELECTED_SECURITY_IES_CAPACITY>,
    bytes: [u8; SELECTED_SECURITY_IES_CAPACITY],
    security: AssociationSecurity,
}

impl SelectedRsn {
    /// An Open association: no security element.
    pub const OPEN: Self = Self {
        length: oer_wire::PrefixLength::ZERO,
        bytes: [0; SELECTED_SECURITY_IES_CAPACITY],
        security: AssociationSecurity::Open,
    };

    /// The security elements of the association request.
    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }

    /// The security the association negotiates.
    pub const fn security(&self) -> AssociationSecurity {
        self.security
    }

    /// The same selection naming `pmkid`: an SAE association that resumes a
    /// cached PMKSA lists its PMKID after the RSN capabilities, as the
    /// vendor's `wpa_gen_wpa_ie_rsn` does for the current PMKSA. `None` for
    /// a selection that is not SAE or already names a PMKID: only SAE associations are cached.
    pub fn with_pmkid(self, pmkid: Pmkid) -> Option<Self> {
        let AssociationSecurity::Rsn(mut association) = self.security else {
            return None;
        };
        if !matches!(association.akm, AssociationAkm::Sae(_)) {
            return None;
        }
        if association.pmkid.is_some() {
            return None;
        }
        association.pmkid = Some(pmkid);
        let mut resumed = self;
        resumed.security = AssociationSecurity::Rsn(association);
        let mut reader = oer_wire::Reader::new(self.as_bytes());
        reader.array::<SELECTED_RSN_IE_LEN>().ok()?;
        let rsnxe = reader.remaining();
        resumed.bytes[1] = resumed.bytes[1].checked_add(PMKID_LIST_LEN as u8)?;
        crate::encoding::write_array::<SELECTED_RSN_IE_LEN, _, _>(
            &mut resumed.bytes,
            &1_u16.to_le_bytes(),
        );
        crate::encoding::write_array::<{ SELECTED_RSN_IE_LEN + 2 }, _, _>(
            &mut resumed.bytes,
            &pmkid.0,
        );
        let rsnxe_start = SELECTED_RSN_IE_LEN + PMKID_LIST_LEN;
        oer_wire::Writer::new(resumed.bytes.get_mut(rsnxe_start..)?)
            .write(rsnxe)
            .ok()?;
        resumed.length = oer_wire::PrefixLength::new(rsnxe_start.checked_add(rsnxe.len())?)?;
        Some(resumed)
    }
}

impl SelectedRsn {
    /// The RSN element of `association` with `capabilities`, and the RSNXE
    /// of `extended` when there is one; the caller chose every field.
    ///
    /// The element names CCMP, the one cipher this codec encodes, as group
    /// data and pairwise cipher, and the association's single AKM. It omits
    /// the Group Management Cipher Suite: BIP-CMAC-128 is the default
    /// (IEEE 802.11-2020 9.4.2.24.1). A PMKID follows through
    /// [`Self::with_pmkid`]; `association.pmkid` is recorded as `None`.
    pub fn rsn(
        association: RsnAssociation,
        capabilities: u16,
        extended: Option<RsnExtendedCapabilities>,
    ) -> Self {
        let mut selected = Self::OPEN;
        selected.security = AssociationSecurity::Rsn(RsnAssociation {
            pmkid: None,
            ..association
        });
        crate::encoding::write_array::<0, _, _>(
            &mut selected.bytes,
            &rsn_single_suite_element(
                RSN_CIPHER_CCMP,
                RSN_CIPHER_CCMP,
                association.akm.akm().suite_selector()[3],
                capabilities,
            ),
        );
        selected.length = oer_wire::PrefixLength::constant::<SELECTED_RSN_IE_LEN>();
        if let Some(extended) = extended {
            crate::encoding::write_array::<SELECTED_RSN_IE_LEN, _, _>(
                &mut selected.bytes,
                &rsnx_element(extended),
            );
            selected.length =
                oer_wire::PrefixLength::constant::<{ SELECTED_RSN_IE_LEN + RSNXE_ONE_OCTET_LEN }>();
        }
        selected
    }
}
