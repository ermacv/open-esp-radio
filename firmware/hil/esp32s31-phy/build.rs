use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    oer_esp32s31_layout::build::configure_runtime("oer-firmware-hil-esp32s31-phy");

    // Resolve the diagnostic selector from the same enumeration as the typed
    // PAC image. Never duplicate its index or field geometry in the image.
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .join("../../..");
    let policy = root.join("chips/esp32s31/registers/policy/api.toml");
    let svd = root.join("chips/esp32s31/registers/published/radio.svd");
    println!("cargo:rerun-if-changed={}", policy.display());
    println!("cargo:rerun-if-changed={}", svd.display());
    let policy = oer_registers_model::PacApiPack::load(&policy)?;
    let svd = fs::read_to_string(svd)?;
    let registers = policy.partition_readable_registers(&svd, "RadioPhyPeripherals")?;
    let index = registers
        .iter()
        .position(|register| {
            register.peripheral == "PHY_BASEBAND_CONFIG_ORACLE"
                && register.register == "I2C_TX_RATE_CONTROL"
                && register.element.is_none()
        })
        .expect("TX rate control belongs to the typed PHY image");
    let geometry = oer_registers_model::svd_geometry(&svd)?;
    let register = geometry
        .iter()
        .find(|register| register.name == "PHY_BASEBAND_CONFIG_ORACLE.I2C_TX_RATE_CONTROL")
        .expect("published TX rate control geometry");
    let extract = |name: &str| {
        let field = register
            .fields
            .iter()
            .find(|field| field.name == name)
            .expect("published TX rate control field");
        assert!(field.width <= u8::BITS, "diagnostic field fits one byte");
        let shifted = if field.offset == 0 {
            "raw".to_owned()
        } else {
            format!("(raw >> {})", field.offset)
        };
        format!("({shifted} & {}) as u8", (1u32 << field.width) - 1)
    };
    let low = extract("BITS_0_1_OPAQUE");
    let high = extract("BITS_18_25_OPAQUE");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo build output directory"));
    fs::write(
        output.join("tx_rate_checkpoint.rs"),
        format!(
            "const IMAGE_INDEX: usize = {index};\n\
             fn decode(raw: u32) -> I2cTxRateControl {{\n\
                 I2cTxRateControl {{ raw, low: {low}, high: {high} }}\n\
             }}\n"
        ),
    )?;
    let observed = [
        ("PHY_BB_RECOVERED_GAPS", "CCA_COUNTER_CONTROL"),
        ("PHY_BB_RECOVERED_GAPS", "CCA_COUNTER_RESULT_0"),
        ("PHY_BB_RECOVERED_GAPS", "CCA_COUNTER_RESULT_1"),
        ("MODEM_SYSCON_RADIO", "WIFI_BB_CFG"),
        ("MODEM_SYSCON_RADIO", "CLK_CONF"),
        ("MODEM_SYSCON_RADIO", "CLK_CONF1"),
        ("PHY_AGC_ORACLE", "RX_11B_PATH_CONTROL_0"),
        (
            "PHY_BASEBAND_CONFIG_ORACLE",
            "FRONT_END_AND_TONE_STOP_CONTROL",
        ),
    ];
    let mut selectors = String::from("const REGISTERS: &[(&str, usize)] = &[\n");
    for (peripheral, name) in observed {
        let index = registers
            .iter()
            .position(|r| r.peripheral == peripheral && r.register == name && r.element.is_none())
            .expect("CCA observation belongs to the typed PHY image");
        selectors.push_str(&format!("    (\"{peripheral}.{name}\", {index}),\n"));
    }
    selectors.push_str("];\n");
    selectors.push_str("const ENABLE_REGISTERS: &[(&str, usize)] = &[\n");
    for (peripheral, name) in [
        ("PHY_PBUS", "STATUS_CLOCK_FORCE"),
        ("PHY_BASEBAND_CONFIG_ORACLE", "NOISE_FLOOR_CONTROL"),
        ("PHY_AGC_RECOVERED_GAPS", "WORD_001C_OPAQUE"),
    ] {
        let index = registers
            .iter()
            .position(|r| r.peripheral == peripheral && r.register == name && r.element.is_none())
            .expect("CCA enable observation belongs to the typed PHY image");
        selectors.push_str(&format!("    (\"{peripheral}.{name}\", {index}),\n"));
    }
    selectors.push_str("];\n");
    fs::write(output.join("cca_checkpoint.rs"), selectors)?;
    Ok(())
}
