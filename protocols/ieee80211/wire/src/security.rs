//! Link-security selection shared by station and access-point protocol code.

pub mod spp;

/// Data protection of one infrastructure link: what the data path, the
/// hardware key slots and replay admission follow.
///
/// This is deliberately not an ordered strength or preference. A caller
/// selects one variant and candidate/association/data paths must match it
/// exactly; there is no downgrade or mixed WPA/Open fallback. How the keys
/// were established (PSK or SAE) is not part of the link protection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkProtection {
    /// IEEE 802.11 Open System with plaintext data and no RSN element.
    Open,
    /// RSN with CCMP data protection.
    Ccmp,
}

/// Local management frame protection policy: the MFPC and MFPR an RSN
/// element advertises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementProtection {
    /// Neither MFPC nor MFPR.
    Disabled,
    /// MFPC: management frames are protected when the peer is capable.
    Capable,
    /// MFPC and MFPR: only associations that protect management frames.
    Required,
}

impl ManagementProtection {
    pub const fn capable(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub const fn required(self) -> bool {
        matches!(self, Self::Required)
    }
}

/// How an SAE exchange derives its password element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaePwe {
    HuntingAndPecking,
    HashToElement,
}

/// The authentication one RSN association negotiated. Unlike the wire
/// [`rsn::Akm`], an SAE association carries its password element method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationAkm {
    Psk,
    PskSha256,
    Sae(SaePwe),
}

impl AssociationAkm {
    /// The suite the RSN elements name.
    pub const fn akm(self) -> rsn::Akm {
        match self {
            Self::Psk => rsn::Akm::Psk,
            Self::PskSha256 => rsn::Akm::PskSha256,
            Self::Sae(_) => rsn::Akm::Sae,
        }
    }
}

/// One PMKSA identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pmkid(pub [u8; rsn::RSN_PMKID_LEN]);

/// The group management cipher protecting group-addressed robust
/// management frames; only the default is implemented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupManagementCipher {
    BipCmac128,
}

/// Whether this association authenticates the QoS A-MSDU-present bit in CCMP.
/// The decision is fixed once for an association. Its PTK and GTK
/// refer to that association; keys cannot choose an independent mode.
/// Receiving a verified MIC alone does not establish negotiated SPP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Spp {
    Negotiated,
    NotNegotiated,
}

/// SPP A-MSDU capability advertised in an RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SppCapability {
    NotCapable,
    Capable,
    Required,
}

/// An RSN element or capability pair cannot establish a consistent SPP mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SppError {
    RequiredWithoutCapability,
    LocalRequiresSupport,
    PeerRequiresSupport,
    /// An interface-wide AP cannot advertise optional SPP and admit mixed modes.
    InterfaceRequiresUniformMode,
}

impl SppCapability {
    pub const fn from_rsn(capabilities: u16) -> Result<Self, SppError> {
        match (
            capabilities & rsn::RSN_CAPABILITY_SPP_AMSDU_CAPABLE != 0,
            capabilities & rsn::RSN_CAPABILITY_SPP_AMSDU_REQUIRED != 0,
        ) {
            (false, false) => Ok(Self::NotCapable),
            (true, false) => Ok(Self::Capable),
            (true, true) => Ok(Self::Required),
            (false, true) => Err(SppError::RequiredWithoutCapability),
        }
    }

    pub const fn rsn_bits(self) -> u16 {
        match self {
            Self::NotCapable => 0,
            Self::Capable => rsn::RSN_CAPABILITY_SPP_AMSDU_CAPABLE,
            Self::Required => {
                rsn::RSN_CAPABILITY_SPP_AMSDU_CAPABLE | rsn::RSN_CAPABILITY_SPP_AMSDU_REQUIRED
            }
        }
    }

    /// Both sides must be capable. A requirement never silently downgrades.
    pub const fn negotiate(self, peer: Self) -> Result<Spp, SppError> {
        match (self, peer) {
            (Self::Required, Self::NotCapable) => Err(SppError::LocalRequiresSupport),
            (Self::NotCapable, Self::Required) => Err(SppError::PeerRequiresSupport),
            (Self::NotCapable, _) | (_, Self::NotCapable) => Ok(Spp::NotNegotiated),
            _ => Ok(Spp::Negotiated),
        }
    }
}

/// What an RSN association negotiated: its authentication, its management
/// frame protection and the PMKSA it resumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnAssociation {
    pub akm: AssociationAkm,
    /// Authentication of the QoS A-MSDU-present bit for every data key.
    pub spp: Spp,
    /// The group management cipher, present exactly when the association
    /// protects its management frames.
    pub management: Option<GroupManagementCipher>,
    pub pmkid: Option<Pmkid>,
}

/// The security one association negotiated, where station selection and
/// access point admission meet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationSecurity {
    Open,
    Rsn(RsnAssociation),
}

impl AssociationSecurity {
    pub const fn link_protection(self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Rsn(_) => LinkProtection::Ccmp,
        }
    }

    /// Whether the association protects its robust management frames.
    pub const fn protects_management(self) -> bool {
        matches!(
            self,
            Self::Rsn(RsnAssociation {
                management: Some(_),
                ..
            })
        )
    }
}

/// The RSN element and RSNXE a BSS advertises; both are empty for an Open
/// BSS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssSecurityElements {
    pub rsne: &'static [u8],
    pub rsnxe: &'static [u8],
}

impl BssSecurityElements {
    /// An Open BSS: no RSN element and no RSNXE.
    pub const OPEN: Self = Self {
        rsne: &[],
        rsnxe: &[],
    };

    /// The data protection the elements announce: an RSN BSS sets Privacy
    /// and carries its RSN element; an Open BSS does neither.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.4 (Capability Information field,
    /// Privacy subfield) and 12.6.1.1.1 (an RSNA AP includes the RSNE).
    pub const fn link_protection(&self) -> LinkProtection {
        if self.rsne.is_empty() {
            LinkProtection::Open
        } else {
            LinkProtection::Ccmp
        }
    }
}

pub mod rsn;

/// The security elements of the personal access point profiles, as fixed
/// bytes for this crate's frame tests.
#[cfg(test)]
pub(crate) mod test_elements {
    use super::BssSecurityElements;

    /// CCMP group and pairwise ciphers, PSK, no capabilities.
    pub(crate) const WPA2_PERSONAL_RSNE: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];
    /// CCMP group and pairwise ciphers, SAE, MFPC and MFPR, no PMKID and
    /// BIP-CMAC-128.
    pub(crate) const WPA3_PERSONAL_RSNE: [u8; 28] = [
        0x30, 26, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0xc0, 0,
        0, 0, 0, 0x0f, 0xac, 6,
    ];
    /// SAE hash-to-element.
    pub(crate) const SAE_H2E_RSNXE: [u8; 3] = [244, 1, 0x20];

    pub(crate) const WPA2_PERSONAL: BssSecurityElements = BssSecurityElements {
        rsne: &WPA2_PERSONAL_RSNE,
        rsnxe: &[],
    };
    pub(crate) const WPA3_PERSONAL: BssSecurityElements = BssSecurityElements {
        rsne: &WPA3_PERSONAL_RSNE,
        rsnxe: &SAE_H2E_RSNXE,
    };
}
