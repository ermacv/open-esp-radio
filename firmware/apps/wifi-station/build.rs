//! The runtime's layout, and the station profile `OER_WIFI_STATION_PROFILE`
//! names: a TOML file kept outside git. Unset builds the NotConfigured image,
//! which starts no RF. A profile that does not validate fails the build; no
//! field has a default and nothing falls back to Open. The SSID and the
//! passphrase are written only to `$OUT_DIR/profile.rs`: never into a rustc
//! environment variable, a cargo message or an error.

use std::{env, fs, path::PathBuf};

#[path = "build/profile.rs"]
mod profile;

const PROFILE: &str = "OER_WIFI_STATION_PROFILE";

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    println!("cargo:rustc-check-cfg=cfg(oer_wifi_station_configured)");
    println!("cargo:rerun-if-env-changed={PROFILE}");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv32") {
        oer_esp32s31_layout::build::configure_runtime("oer-firmware-apps-wifi-station")?;
    }
    let generated = match env::var_os(PROFILE) {
        None => String::new(),
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("{PROFILE}: the profile cannot be read: {error}"))?;
            let profile = profile::parse(&text).map_err(|reason| format!("{PROFILE}: {reason}"))?;
            println!("cargo:rustc-cfg=oer_wifi_station_configured");
            profile
        }
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    fs::write(out.join("profile.rs"), generated)?;
    Ok(())
}
