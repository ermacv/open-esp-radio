//! The credentials a run sets on the stand's managed access points and
//! stations: generated for the run from the host's random source, never
//! read from a file and never written to a log or the run store. Their
//! `Debug` shows nothing of them.

use std::{fmt, io::Read as _};

/// A generated credential.
#[derive(Clone, Eq, PartialEq)]
pub struct Generated(String);

impl Generated {
    /// The value, for the one place that sets it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Generated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<generated>")
    }
}

fn random_hex(bytes: usize) -> crate::Result<String> {
    let mut buffer = vec![0; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| format!("read the host's random source: {error}"))?;
    Ok(hex::encode(buffer))
}

/// An SSID for a run: `prefix`, a dash and 8 random hex digits.
pub fn ssid(prefix: &str) -> crate::Result<Generated> {
    Ok(Generated(format!("{prefix}-{}", random_hex(4)?)))
}

/// A WPA2 passphrase for a run: 32 random hex digits.
pub fn passphrase() -> crate::Result<Generated> {
    Ok(Generated(random_hex(16)?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_credentials_differ_and_fit_wpa2_and_show_nothing() {
        let (first, second) = (
            super::ssid("oer-w1").unwrap(),
            super::ssid("oer-w1").unwrap(),
        );
        assert_ne!(first, second);
        assert!(first.expose().starts_with("oer-w1-") && first.expose().len() == 15);
        let passphrase = super::passphrase().unwrap();
        assert!((8..=63).contains(&passphrase.expose().len()));
        assert_eq!(format!("{passphrase:?}"), "<generated>");
    }
}
