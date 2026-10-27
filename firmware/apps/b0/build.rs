use std::{env, fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rustc-check-cfg=cfg(oer_b0_configured)");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv32") {
        oer_esp32s31_layout::build::configure_runtime("oer-firmware-apps-b0")?;
    }
    for key in ["OER_B0_COUNTRY", "OER_B0_POWER_CAP_DBM"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let config = match (
        env::var("OER_B0_COUNTRY").ok(),
        env::var("OER_B0_POWER_CAP_DBM").ok(),
    ) {
        (None, None) => "None".to_owned(),
        (Some(country), Some(cap)) => {
            assert!(
                country.len() == 2 && country.bytes().all(|b| b.is_ascii_uppercase()),
                "B0 country is two uppercase letters"
            );
            let cap = cap.parse::<i8>().expect("B0 cap is integer dBm");
            println!("cargo:rustc-cfg=oer_b0_configured");
            format!("Some(crate::Profile {{country:*b\"{country}\",cap_dbm:{cap}}})")
        }
        _ => panic!("B0 country and power cap are supplied together"),
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output")).join("profile.rs"),
        format!("pub const PROFILE: Option<crate::Profile> = {config};\n"),
    )
    .expect("write B0 profile");
    Ok(())
}
