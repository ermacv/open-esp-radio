//! The runtime's layout, and the stand W1 checks: `OER_W1_CHANNEL`,
//! `OER_W1_BSSID` (the stand router's AP interface) and `OER_W1_SSID_HEX`
//! (the stand's SSID, hex) become `$OUT_DIR/w1_config.rs`. All unset build
//! the unconfigured image. The BSSID and SSID are written only there: never
//! into a rustc environment variable, a cargo warning or a message.

use std::{env, fs, path::PathBuf};

const CHANNEL: &str = "OER_W1_CHANNEL";
const BSSID: &str = "OER_W1_BSSID";
const SSID: &str = "OER_W1_SSID_HEX";

fn main() {
    oer_esp32s31_platform_layout::build::configure_runtime("oer-esp32s31-w1");
    for name in [CHANNEL, BSSID, SSID] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let config = match (variable(CHANNEL), variable(BSSID), variable(SSID)) {
        (None, None, None) => "None".to_owned(),
        (Some(channel), Some(bssid), Some(ssid)) => {
            let channel = parse_channel(&channel)
                .unwrap_or_else(|| panic!("{CHANNEL} is no 2.4 GHz channel 1 to 13"));
            let stand = parse_bssid(&bssid).unwrap_or_else(|| {
                panic!("{BSSID} is no unicast nonzero BSSID of six hex octets (02:00:00:00:F6:D0)")
            });
            let ssid = parse_ssid(&ssid)
                .unwrap_or_else(|| panic!("{SSID} is no SSID of 1 to 32 bytes in hex"));
            format!(
                "Some(crate::W1Config {{ channel: {channel}, stand: [{}], ssid: &[{}] }})",
                octets(&stand),
                octets(&ssid)
            )
        }
        _ => panic!("{CHANNEL}, {BSSID} and {SSID} are set together, or none"),
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(
        out.join("w1_config.rs"),
        format!(
            "/// The stand this image checks; `None` builds the unconfigured image.\n\
             pub const CONFIG: Option<crate::W1Config> = {config};\n"
        ),
    )
    .expect("OUT_DIR is writable");
}

/// `0x02, 0x00, …`.
fn octets(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|octet| format!("{octet:#04x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The variable `name`; set but not Unicode is refused.
fn variable(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} is not Unicode"),
    }
}

fn parse_channel(text: &str) -> Option<u8> {
    text.trim()
        .parse::<u8>()
        .ok()
        .filter(|channel| (1..=13).contains(channel))
}

/// Six two-digit hex octets with one separator, `:` or `-`, throughout;
/// unicast and nonzero.
fn parse_bssid(text: &str) -> Option<[u8; 6]> {
    let text = text.trim();
    let separator = if text.contains(':') { ':' } else { '-' };
    let mut octets = [0; 6];
    let mut parts = text.split(separator);
    for octet in &mut octets {
        let part = parts.next()?;
        if part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        *octet = u8::from_str_radix(part, 16).ok()?;
    }
    if parts.next().is_some() || octets == [0; 6] || octets[0] & 1 != 0 {
        return None;
    }
    Some(octets)
}

/// Two hex digits per byte, 1 to 32 bytes.
fn parse_ssid(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if text.is_empty() || !text.len().is_multiple_of(2) || text.len() > 64 {
        return None;
    }
    text.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}
