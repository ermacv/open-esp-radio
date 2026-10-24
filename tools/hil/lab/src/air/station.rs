//! WPA2 metadata projection. No key bytes, EAPOL key data, SSID or full
//! addresses enter the serializable evidence.
use super::{Result, mac, optional};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Direction {
    FromStation,
    ToStation,
    GroupFromAp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationFrame {
    pub at_us: u64,
    pub frequency_mhz: u16,
    pub direction: Direction,
    pub subtype: u8,
    pub protected: bool,
    pub eapol: bool,
    pub handshake_message: Option<u8>,
    pub replay_counter: Option<u64>,
    pub status: Option<u16>,
    pub key_id: Option<u8>,
}

pub(super) const FIELDS: [&str; 13] = [
    "frame.time_epoch",
    "radiotap.channel.freq",
    "wlan.fc.type_subtype",
    "wlan.ta",
    "wlan.ra",
    "wlan.bssid",
    "wlan.fc.protected",
    "eapol.type",
    "wlan_rsna_eapol.keydes.msgnr",
    "eapol.keydes.replay_counter",
    "wlan.fixed.status_code",
    "wlan.wep.key",
    "wlan.fcs.status",
];

fn decimal<T: core::str::FromStr>(s: &str) -> Result<T> {
    s.parse()
        .map_err(|_| "invalid numeric air observation".into())
}
fn hexadecimal(s: &str) -> Result<u16> {
    match s.strip_prefix("0x") {
        Some(s) => u16::from_str_radix(s, 16).map_err(|_| "invalid numeric air observation".into()),
        None => decimal(s),
    }
}

pub(super) fn parse(text: &str, target: [u8; 6], ap: [u8; 6]) -> Result<Vec<StationFrame>> {
    let mut frames = Vec::new();
    let mut previous = 0;
    for line in text.lines() {
        let fields: [&str; 13] = line
            .split('\t')
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| "invalid station air field count")?;
        let [
            time,
            frequency,
            subtype,
            transmitter,
            receiver,
            bssid,
            protected,
            eapol,
            message,
            replay,
            status,
            key,
            fcs,
        ] = fields;
        // Exclude foreign BSS frames before interpreting their optional fields.
        let transmitter = optional(transmitter, mac)?;
        let receiver = optional(receiver, mac)?;
        if optional(bssid, mac)? != Some(ap) {
            continue;
        }
        let direction = match (transmitter, receiver) {
            (Some(t), Some(r)) if t == target && r == ap => Direction::FromStation,
            (Some(t), Some(r)) if t == ap && r == target => Direction::ToStation,
            (Some(t), Some(r)) if t == ap && r[0] & 1 != 0 => Direction::GroupFromAp,
            _ => continue,
        };
        // A packet with known bad FCS establishes neither a handshake nor a
        // protected data frame. Required missing observations still fail.
        if fcs == "2" {
            continue;
        }
        let (seconds, fraction) = time.split_once('.').ok_or("invalid air timestamp")?;
        if fraction.len() < 6 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid air timestamp precision".into());
        }
        let at_us = decimal::<u64>(seconds)?
            .checked_mul(1_000_000)
            .and_then(|seconds| seconds.checked_add(fraction[..6].parse().ok()?))
            .ok_or("air timestamp overflow")?;
        if at_us < previous {
            return Err("air timestamps moved backwards".into());
        }
        previous = at_us;
        let protected = match protected {
            "0" | "False" => false,
            "1" | "True" => true,
            _ => return Err("missing protected bit in station air observation".into()),
        };
        frames.push(StationFrame {
            at_us,
            frequency_mhz: decimal(frequency)?,
            direction,
            subtype: u8::try_from(hexadecimal(subtype)?).map_err(|_| "invalid air subtype")?,
            protected,
            eapol: !eapol.is_empty(),
            handshake_message: optional(message, decimal)?,
            replay_counter: optional(replay, decimal)?,
            status: optional(status, hexadecimal)?,
            key_id: optional(key, decimal)?,
        });
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    const STA: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
    #[test]
    fn projection_preserves_m4_and_protection_without_identity_or_key_data() {
        let input = concat!(
            "1.000000000\t2437\t0x0028\t02:00:00:00:00:01\t02:00:00:00:00:02\t02:00:00:00:00:02\t0\t3\t4\t8\t\t\t1\n",
            "1.100000000\t2437\t0x0028\t02:00:00:00:00:02\t02:00:00:00:00:01\t02:00:00:00:00:02\t1\t\t\t\t\t0\t1\n"
        );
        let frames = parse(input, STA, AP).expect("decoded station");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].handshake_message, Some(4));
        assert!(frames[0].eapol);
        assert!(!frames[0].protected);
        assert!(frames[1].protected);
        assert!(!frames[1].eapol);
        let saved = serde_json::to_string(&frames).expect("secret-free evidence");
        assert!(!saved.contains("02:00"));
        assert!(!format!("{frames:?}").contains("02:00"));
    }
    #[test]
    fn malformed_or_missing_protection_cannot_become_a_protected_frame() {
        for row in [
            "secret-passphrase",
            "1.000000\t2437\t0x28\t02:00:00:00:00:01\t02:00:00:00:00:02\t02:00:00:00:00:02\t\t\t\t\t\t\t1",
        ] {
            let error = parse(row, STA, AP)
                .expect_err("invalid capture")
                .to_string();
            assert!(!error.contains("secret-passphrase"));
        }
    }
}
