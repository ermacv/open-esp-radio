//! Station provisioning owns secrets before, during and after derivation.
use heapless::Vec;
use oer_ieee80211_core_security_rsn::{Pmk, PmkDerivation};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialsError {
    SsidLength,
    PassphraseLength,
}

impl oer_diagnostics::DiagnosticPath for CredentialsError {
    fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
        path.name(match self {
            Self::SsidLength => "SsidLength",
            Self::PassphraseLength => "PassphraseLength",
        });
    }
}

/// Validated SSID and affine credentials. No secret supports Clone or Copy.
///
/// ```compile_fail
/// let settings = oer_esp32s31_wifi::StationSettings::wpa2_passphrase(b"ssid", b"password").unwrap();
/// let duplicate = settings.clone();
/// ```
pub struct StationSettings {
    ssid: Vec<u8, 32>,
    security: Security,
}
enum Security {
    Open,
    Passphrase(Zeroizing<Vec<u8, 63>>),
    Pmk(Pmk),
}

impl StationSettings {
    pub fn open(ssid: &[u8]) -> Result<Self, CredentialsError> {
        if ssid.is_empty() {
            return Err(CredentialsError::SsidLength);
        }
        Ok(Self {
            ssid: Vec::from_slice(ssid).map_err(|_| CredentialsError::SsidLength)?,
            security: Security::Open,
        })
    }
    pub fn wpa2_passphrase(ssid: &[u8], passphrase: &[u8]) -> Result<Self, CredentialsError> {
        let mut settings = Self::open(ssid)?;
        if !(8..=63).contains(&passphrase.len()) {
            return Err(CredentialsError::PassphraseLength);
        }
        settings.security = Security::Passphrase(Zeroizing::new(
            Vec::from_slice(passphrase).map_err(|_| CredentialsError::PassphraseLength)?,
        ));
        Ok(settings)
    }
    /// Consume provisioned PMK bytes and erase the parameter's local copy.
    pub fn wpa2_pmk(ssid: &[u8], mut pmk: [u8; 32]) -> Result<Self, CredentialsError> {
        let result = Self::open(ssid).map(|mut settings| {
            settings.security = Security::Pmk(Pmk::from_bytes(pmk));
            settings
        });
        pmk.zeroize();
        result
    }
    /// Bounded PBKDF2: at most one HMAC between cooperative yields. The
    /// passphrase is erased before the first yield; cancellation erases the
    /// derivation's intermediate state. No password is retained for WPA2.
    pub async fn prepare(self) -> Result<PreparedCredentials, CredentialsError> {
        let Self { ssid, security } = self;
        let pmk = match security {
            Security::Open => None,
            Security::Pmk(pmk) => Some(pmk),
            Security::Passphrase(password) => {
                let result = PmkDerivation::new(&password, &ssid).map_err(|error| match error {
                    oer_ieee80211_core_security_rsn::PskDerivationError::InvalidPassphraseLength => {
                        CredentialsError::PassphraseLength
                    }
                    oer_ieee80211_core_security_rsn::PskDerivationError::InvalidSsidLength => {
                        CredentialsError::SsidLength
                    }
                });
                drop(password);
                let mut derive = result?;
                Some(loop {
                    if let Some(pmk) = derive.step() {
                        break pmk;
                    }
                    embassy_futures::yield_now().await;
                })
            }
        };
        Ok(PreparedCredentials { ssid, pmk })
    }
}
impl core::fmt::Debug for StationSettings {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("StationSettings(<redacted>)")
    }
}
/// The prepared credential owner is consumed by the station profile.
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub struct PreparedCredentials {
    pub(crate) ssid: Vec<u8, 32>,
    pub(crate) pmk: Option<Pmk>,
}
impl PreparedCredentials {
    /// The station's profile of this network; the PMK moves, never copied.
    pub(crate) fn into_profile(self) -> oer_ieee80211_core_roles_sta::machine::StaProfile {
        use oer_ieee80211_core_roles_sta::machine::StaSecurity;
        let Self { ssid, pmk } = self;
        oer_ieee80211_core_roles_sta::machine::StaProfile {
            ssid,
            security: match pmk {
                None => StaSecurity::Open,
                Some(pmk) => StaSecurity::Wpa2Personal { pmk },
            },
        }
    }
}
impl core::fmt::Debug for PreparedCredentials {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PreparedCredentials(<redacted>)")
    }
}

/// Image callbacks have no core, security-algorithm or backend vocabulary.
pub trait Callbacks {
    fn nonce(&mut self) -> [u8; 32];
}
pub struct OpenCallbacks;
impl Callbacks for OpenCallbacks {
    fn nonce(&mut self) -> [u8; 32] {
        [0; 32]
    }
}

/// A boot-seeded stream retained across associations; its private cipher
/// state is erased on drop. Entropy is obtained once before transfer to P2.
pub struct NonceSource(oer_ieee80211_core_security_rsn::nonce::SNonceSource);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonceExhausted;
impl NonceSource {
    pub fn from_entropy(fill: impl FnOnce(&mut [u8; 32])) -> Self {
        Self(oer_ieee80211_core_security_rsn::nonce::SNonceSource::from_entropy(fill))
    }
    pub fn next_nonce(&mut self) -> Result<[u8; 32], NonceExhausted> {
        self.0.next_nonce().map_err(|_| NonceExhausted)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]
mod tests {
    use super::*;
    use core::future::Future;
    use zeroize::ZeroizeOnDrop;
    #[test]
    fn provisioning_validates_bounds_without_retaining_invalid_secrets() {
        for ssid in [&b""[..], &[0; 33][..]] {
            assert!(matches!(
                StationSettings::open(ssid),
                Err(CredentialsError::SsidLength)
            ));
        }
        for length in [0, 7, 64] {
            assert!(matches!(
                StationSettings::wpa2_passphrase(b"ssid", &[0; 64][..length]),
                Err(CredentialsError::PassphraseLength)
            ));
        }
        assert!(StationSettings::wpa2_passphrase(&[0; 32], &[0; 63]).is_ok());
    }
    #[test]
    fn open_and_pmk_provisioning_are_ready_without_derivation() {
        let open =
            embassy_futures::block_on(StationSettings::open(b"ssid").unwrap().prepare()).unwrap();
        assert_eq!(&open.ssid, b"ssid");
        assert!(open.pmk.is_none());
        let key = embassy_futures::block_on(
            StationSettings::wpa2_pmk(b"ssid", [0x5a; 32])
                .unwrap()
                .prepare(),
        )
        .unwrap();
        assert!(key.pmk.is_some());
        fn zeroizing<T: ZeroizeOnDrop>() {}
        zeroizing::<Pmk>();
        zeroizing::<PmkDerivation>();
        zeroizing::<Zeroizing<Vec<u8, 63>>>();
    }
    #[test]
    fn prepared_credentials_become_the_profile_of_their_network() {
        use oer_ieee80211_core_roles_sta::machine::StaSecurity;
        let open = embassy_futures::block_on(StationSettings::open(b"lab").unwrap().prepare())
            .unwrap()
            .into_profile();
        assert_eq!(open.ssid.as_slice(), b"lab");
        assert!(matches!(open.security, StaSecurity::Open));
        let protected = embassy_futures::block_on(
            StationSettings::wpa2_pmk(b"lab", [0x5a; 32])
                .unwrap()
                .prepare(),
        )
        .unwrap()
        .into_profile();
        assert_eq!(protected.ssid.as_slice(), b"lab");
        assert!(matches!(
            protected.security,
            StaSecurity::Wpa2Personal { .. }
        ));
    }
    #[test]
    fn passphrase_and_provisioned_pmk_have_the_same_ieee_key_material() {
        let prepared = embassy_futures::block_on(
            StationSettings::wpa2_passphrase(b"IEEE", b"password")
                .unwrap()
                .prepare(),
        )
        .unwrap();
        let expected = Pmk::from_bytes([
            0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a,
            0x5f, 0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e,
            0x97, 0x10, 0xa1, 0x2e,
        ]);
        let context = oer_ieee80211_core_security_rsn::PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: [4; 6],
            authenticator_nonce: [6; 32],
            supplicant_nonce: [8; 32],
        };
        let actual = prepared
            .pmk
            .unwrap()
            .derive_ptk(oer_ieee80211_wire::security::rsn::Akm::Psk, context);
        let expected = expected.derive_ptk(oer_ieee80211_wire::security::rsn::Akm::Psk, context);
        assert_eq!(actual.kck(), expected.kck());
        assert_eq!(actual.kek(), expected.kek());
    }
    #[test]
    fn passphrase_derivation_yields_and_can_be_cancelled() {
        let settings = StationSettings::wpa2_passphrase(b"ssid", b"password").unwrap();
        let mut future = core::pin::pin!(settings.prepare());
        let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        // Drop uses the same zeroizing PmkDerivation owner as RSN's
        // cancellation/erasure tests; no unprotected password survives.
    }
    #[test]
    fn facade_nonce_source_seeds_once_and_never_restarts_between_requests() {
        let mut calls = 0;
        let mut source = NonceSource::from_entropy(|seed| {
            calls += 1;
            seed.fill(0x42);
        });
        assert_ne!(source.next_nonce().unwrap(), source.next_nonce().unwrap());
        assert_eq!(calls, 1);
    }
}
