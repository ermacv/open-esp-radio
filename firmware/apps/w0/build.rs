//! The runtime's layout, and the stand W0 runs on: `OER_W0_COUNTRY`, the
//! stand router radio's regulatory country, becomes `$OUT_DIR/w0_config.rs`.
//! Unset builds the unconfigured image.

use std::{env, fs, path::PathBuf};

const COUNTRY: &str = "OER_W0_COUNTRY";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    oer_esp32s31_layout::build::configure_runtime("oer-firmware-apps-w0")?;
    println!("cargo:rerun-if-env-changed={COUNTRY}");
    let country = match env::var(COUNTRY) {
        Ok(text) => match text.trim().as_bytes() {
            &[first, second] if first.is_ascii_uppercase() && second.is_ascii_uppercase() => {
                format!("Some(*b\"{}{}\")", char::from(first), char::from(second))
            }
            _ => panic!("{COUNTRY} is no two uppercase letters"),
        },
        Err(env::VarError::NotPresent) => "None".to_owned(),
        Err(env::VarError::NotUnicode(_)) => panic!("{COUNTRY} is not Unicode"),
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(
        out.join("w0_config.rs"),
        format!(
            "/// The stand router radio's regulatory country; `None` builds the unconfigured image.\n\
             pub const COUNTRY: Option<[u8; 2]> = {country};\n"
        ),
    )
    .expect("OUT_DIR is writable");
    Ok(())
}
