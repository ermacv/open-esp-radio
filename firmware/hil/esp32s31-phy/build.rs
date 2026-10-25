use std::{env, fs, path::PathBuf};

use oer_registers_model::{
    Chip,
    chip_model::{ChipModel, ExpandedRegister, FieldClass, Location},
    chip_policy::{ChipPolicy, Claim, Holder, PartId},
};

/// The partition whose diagnostic image the HAL's `register_image` reads.
const PARTITION: &str = "RadioPhyPeripherals";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    oer_esp32s31_layout::build::configure_runtime("oer-firmware-hil-esp32s31-phy");

    // Resolve the diagnostic selectors from the register model and its
    // ownership policy, in the order the PAC renders the partition's image.
    // Never duplicate an index or field geometry in the image.
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .join("../../..");
    let directory = Chip::directory(&root, "esp32s31");
    println!("cargo:rerun-if-changed={}", directory.display());
    let chip = Chip::load(&root, "esp32s31")?;
    let policy = chip.policy()?;
    let image = partition_image(&chip.model, &policy)?;

    let index_of = |identity: &str| {
        image
            .iter()
            .position(|register| register.identity == identity)
            .unwrap_or_else(|| panic!("{identity} belongs to the radio-PHY image"))
    };
    let tx_rate = "WIFI_MAC.I2C_TX_RATE_CONTROL";
    let index = index_of(tx_rate);
    let register = image[index];
    let extract = |name: &str| {
        let field = register
            .fields
            .iter()
            .find(|field| field.name == name)
            .expect("modelled TX rate control field");
        assert!(field.width <= u8::BITS, "diagnostic field fits one byte");
        let shifted = if field.low == 0 {
            "raw".to_owned()
        } else {
            format!("(raw >> {})", field.low)
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
             const _: () = assert!(\n\
                 oer_esp32s31_hal::phy::RadioPhyRegisters::REGISTER_IMAGE_LEN == {len},\n\
                 \"the model's radio-PHY image is the HAL's\"\n\
             );\n\
             fn decode(raw: u32) -> I2cTxRateControl {{\n\
                 I2cTxRateControl {{ raw, low: {low}, high: {high} }}\n\
             }}\n",
            len = image.len(),
        ),
    )?;
    let mut selectors = String::from("const REGISTERS: &[(&str, usize)] = &[\n");
    for identity in [
        "WIFI_BB.CCA_COUNTER_CONTROL",
        "WIFI_BB.CCA_COUNTER_RESULT_0",
        "WIFI_BB.CCA_COUNTER_RESULT_1",
        "MODEM_SYSCON.WIFI_BB_CFG",
        "MODEM_SYSCON.CLK_CONF",
        "MODEM_SYSCON.CLK_CONF1",
        "WIFI_BB.RX_11B_PATH_CONTROL_0",
        "FE.FRONT_END_AND_TONE_STOP_CONTROL",
    ] {
        selectors.push_str(&format!("    (\"{identity}\", {}),\n", index_of(identity)));
    }
    selectors.push_str("];\n");
    selectors.push_str("const ENABLE_REGISTERS: &[(&str, usize)] = &[\n");
    for identity in [
        "FE.STATUS_CLOCK_FORCE",
        "WIFI_BB.NOISE_FLOOR_CONTROL",
        "WIFI_BB.WORD_001C_OPAQUE",
    ] {
        selectors.push_str(&format!("    (\"{identity}\", {}),\n", index_of(identity)));
    }
    selectors.push_str("];\n");
    fs::write(output.join("cca_checkpoint.rs"), selectors)?;
    Ok(())
}

/// The words of [`PARTITION`]'s diagnostic image, as the PAC renders its
/// partition image read: each part in policy order, each part's owned words
/// whose read has a value and changes nothing, ascending by offset.
fn partition_image<'m>(
    model: &'m ChipModel,
    policy: &ChipPolicy,
) -> Result<Vec<&'m ExpandedRegister>, String> {
    let (partition, parts) = policy
        .partitions()
        .iter()
        .enumerate()
        .find(|(_, partition)| partition.name == PARTITION)
        .map(|(index, partition)| (index, partition.parts.len()))
        .ok_or_else(|| format!("the policy has no {PARTITION}"))?;
    let mut image = Vec::new();
    for part in 0..parts {
        let id = PartId { partition, part };
        let mut words = policy
            .claims()
            .iter()
            .filter(|(_, claim)| **claim == Claim::Whole(Holder::Part(id)))
            .map(|(identity, _)| {
                model
                    .register(identity)
                    .ok_or_else(|| format!("{identity} is no register of the model"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        words.retain(|register| readable(model, register));
        words.sort_by_key(|register| offset(model, register));
        image.extend(words);
    }
    Ok(image)
}

/// A read of `register` has a value and changes nothing: no read side
/// effect, no field a read clears, no FIFO, and a field with a readable
/// value that a write may set to more than zero.
fn readable(model: &ChipModel, register: &ExpandedRegister) -> bool {
    let template = model
        .instances()
        .iter()
        .find(|instance| instance.name == register.instance)
        .and_then(|instance| model.types().get(&instance.block_type))
        .and_then(|block| block.templates.get(&register.key))
        .unwrap_or_else(|| panic!("{} has a template", register.identity));
    template.read_side_effect.is_none()
        && template.fifo.is_none()
        && !template
            .fields
            .iter()
            .any(|field| field.class == FieldClass::ReadClears)
        && template.fields.iter().any(|field| {
            field.class.readable()
                && !field
                    .write_range
                    .is_some_and(|range| range.min == 0 && range.max == 0)
        })
}

/// `register`'s offset from its instance's base.
fn offset(model: &ChipModel, register: &ExpandedRegister) -> u64 {
    let Location::Mmio { address } = register.location else {
        panic!("{} is an MMIO word", register.identity);
    };
    let base = model
        .instances()
        .iter()
        .find(|instance| instance.name == register.instance)
        .map(|instance| instance.base)
        .unwrap_or_else(|| panic!("{} has an instance", register.instance));
    address - base
}
