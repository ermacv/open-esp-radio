//! The runtime's layout, and the stand W1 checks: `OER_W1_CHANNEL` and
//! `OER_W1_EXPECT_BSSID` become `$OUT_DIR/w1_config.rs`. Both unset build the
//! unconfigured image. The BSSID is written only there: never into a rustc
//! environment variable, a cargo warning or a message.

use std::{env, fs, path::PathBuf};

const CHANNEL: &str = "OER_W1_CHANNEL";
const BSSID: &str = "OER_W1_EXPECT_BSSID";

fn main() {
    oer_esp32s31_platform_layout::build::configure_runtime("oer-esp32s31-w1");
    println!("cargo:rerun-if-env-changed={CHANNEL}");
    println!("cargo:rerun-if-env-changed={BSSID}");
    let config = match (variable(CHANNEL), variable(BSSID)) {
        (None, None) => "None".to_owned(),
        (Some(channel), Some(bssid)) => {
            let channel = parse_channel(&channel)
                .unwrap_or_else(|| panic!("{CHANNEL} is no 2.4 GHz channel 1 to 13"));
            let stand = parse_bssid(&bssid).unwrap_or_else(|| {
                panic!("{BSSID} is no unicast nonzero BSSID of six hex octets (02:00:00:00:F6:D0)")
            });
            let octets = stand.map(|octet| format!("{octet:#04x}")).join(", ");
            format!("Some(crate::W1Config {{ channel: {channel}, stand: [{octets}] }})")
        }
        _ => panic!("{CHANNEL} and {BSSID} are set together, or neither"),
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
