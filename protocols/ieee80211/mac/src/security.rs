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

/// What one station request admits.
///
/// A personal request never downgrades: WPA2-Personal admits PSK or, as the
/// vendor's WPA3-enabled station does, upgrades to SAE whenever the access
/// point offers it; WPA3-Personal admits SAE only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityPolicy {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

impl StaSecurityPolicy {
    /// The data protection of the link.
    pub const fn link_protection(self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal | Self::Wpa3Personal => LinkProtection::Ccmp,
        }
    }

    /// The station's management frame protection. A personal station is
    /// always capable, as the vendor supplicant's configuration sets MFPC,
    /// and WPA3-Personal requires it.
    pub const fn management_protection(self) -> ManagementProtection {
        match self {
            Self::Open => ManagementProtection::Disabled,
            Self::Wpa2Personal => ManagementProtection::Capable,
            Self::Wpa3Personal => ManagementProtection::Required,
        }
    }
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

/// The security an access point offers.
///
/// WPA2-Personal keeps the reviewed PSK profile without management frame
/// protection. WPA3-Personal follows the vendor's softAP: SAE only, with
/// management frame protection required, BIP-CMAC-128 as the group
/// management cipher of a CCMP BSS, and both SAE password element methods,
/// which it advertises through the H2E bit of its RSNXE. Each variant owns
/// the exact RSN element the beacon, the probe response and EAPOL Message 3
/// carry.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/esp_supplicant/src/esp_hostap.c`
/// (`hostap_init`: `WIFI_AUTH_WPA3_PSK` selects `WPA_KEY_MGMT_SAE`, PMF
/// required, `WPA_CIPHER_AES_128_CMAC` for CCMP) and
/// `src/ap/wpa_auth_ie.c` (`wpa_own_rsn_capab`, `rsne_write_data`,
/// `wpa_write_rsnxe`); `docs/en/api-guides/wifi-security.rst` ("For WPA3
/// SoftAP, PMF Required is mandatory").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurityPolicy {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

/// RSN element of the WPA2-Personal access point: CCMP group and pairwise
/// ciphers, PSK, no capabilities.
pub const AP_WPA2_PERSONAL_RSN_ELEMENT: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

/// RSN element of the WPA3-Personal access point: CCMP group and pairwise
/// ciphers, SAE, MFPC and MFPR, no PMKID and BIP-CMAC-128.
pub const AP_WPA3_PERSONAL_RSN_ELEMENT: [u8; 28] = [
    0x30, 26, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0xc0, 0, 0,
    0, 0, 0x0f, 0xac, 6,
];

/// RSNXE of an access point offering SAE hash-to-element: a one-octet
/// field whose length nibble is zero and whose bit 5 is SAE H2E.
pub const AP_SAE_H2E_RSNX_ELEMENT: [u8; 3] = [244, 1, 1 << 5];

impl ApSecurityPolicy {
    /// The current AP RSN profiles do not advertise SPP. A required peer is
    /// refused; a merely capable peer negotiates ordinary CCMP.
    ///
    /// The backend's [`spp::SppScope`] determines whether a future AP profile
    /// can admit different SPP modes on different associations.
    pub const fn spp_capability(self) -> SppCapability {
        SppCapability::NotCapable
    }

    /// The data protection of the BSS.
    pub const fn link_protection(self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal | Self::Wpa3Personal => LinkProtection::Ccmp,
        }
    }

    /// The access point's management frame protection: WPA3-Personal
    /// requires it, WPA2-Personal keeps the reviewed profile without it.
    pub const fn management_protection(self) -> ManagementProtection {
        match self {
            Self::Open | Self::Wpa2Personal => ManagementProtection::Disabled,
            Self::Wpa3Personal => ManagementProtection::Required,
        }
    }

    /// The security elements the beacon, the probe response and EAPOL
    /// Message 3 carry; both are empty for an Open BSS.
    pub const fn advertisement(self) -> BssSecurityElements {
        match self {
            Self::Open => BssSecurityElements {
                rsne: &[],
                rsnxe: &[],
            },
            Self::Wpa2Personal => BssSecurityElements {
                rsne: &AP_WPA2_PERSONAL_RSN_ELEMENT,
                rsnxe: &[],
            },
            Self::Wpa3Personal => BssSecurityElements {
                rsne: &AP_WPA3_PERSONAL_RSN_ELEMENT,
                rsnxe: &AP_SAE_H2E_RSNX_ELEMENT,
            },
        }
    }
}

/// The RSN element and RSNXE a BSS advertises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssSecurityElements {
    pub rsne: &'static [u8],
    pub rsnxe: &'static [u8],
}

pub mod rsn;
