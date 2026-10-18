//! A MAC address that is only ever shown masked.

use std::fmt;

/// A unicast, nonzero MAC address. `Debug` and `Display` show only its last
/// two octets (`xx:xx:xx:xx:F6:D0`).
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Mac([u8; 6]);

impl Mac {
    /// `octets`, unicast and nonzero; the message never repeats them.
    pub fn new(octets: [u8; 6]) -> Result<Self, String> {
        if octets == [0; 6] {
            return Err("the MAC address is all zero".to_owned());
        }
        if octets[0] & 1 != 0 {
            return Err("the MAC address is a multicast address".to_owned());
        }
        Ok(Self(octets))
    }

    /// Six `:`-separated two-digit hex octets, any case, unicast and
    /// nonzero.
    pub fn parse(text: &str) -> Option<Self> {
        let mut octets = [0; 6];
        let mut parts = text.split(':');
        for octet in &mut octets {
            let part = parts.next()?;
            if part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            *octet = u8::from_str_radix(part, 16).ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Self::new(octets).ok()
    }

    /// The octets, for the one place that needs them whole.
    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }

    /// `xx:xx:xx:xx:HH:HH`: the only form in which it is shown.
    pub fn masked(&self) -> String {
        let [.., fifth, sixth] = self.0;
        format!("xx:xx:xx:xx:{fifth:02X}:{sixth:02X}")
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Mac({})", self.masked())
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.masked())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_two_octets_are_shown() {
        let mac = Mac::parse("02:ab:cd:ef:f6:d0").unwrap();
        assert_eq!(mac.octets(), [0x02, 0xAB, 0xCD, 0xEF, 0xF6, 0xD0]);
        for shown in [
            format!("{mac}"),
            format!("{mac:?}"),
            format!("{mac:#?}"),
            mac.masked(),
        ] {
            assert!(shown.contains("xx:xx:xx:xx:F6:D0"), "{shown}");
            for hidden in ["AB", "CD", "EF", "ab", "cd", "ef"] {
                assert!(!shown.contains(hidden), "{shown}");
            }
        }
    }

    #[test]
    fn only_a_unicast_nonzero_address_of_six_octets_parses() {
        assert!(Mac::new([0; 6]).unwrap_err().contains("zero"));
        assert!(
            Mac::new([0x03, 0, 0, 0, 0xF6, 0xD0])
                .unwrap_err()
                .contains("multicast")
        );
        for refused in [
            "03:5e:c0:de:f6:d0",
            "00:00:00:00:00:00",
            "02:5e:c0:de:f6",
            "02:5e:c0:de:f6:d0:11",
            "02-5e-c0-de-f6-d0",
            "2:5e:c0:de:f6:d0",
            "02:5e:c0:de:f6:g0",
            "",
        ] {
            assert_eq!(Mac::parse(refused), None, "{refused}");
        }
    }
}
