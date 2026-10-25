//! The calibrated register state the cross-check compares: the radio-PHY
//! partition image and the analog image.
//!
//! The register model's own enumeration defines the image: every readable
//! register the ownership policy's `RadioPhyPeripherals` partition holds,
//! in address order, named by its identity in the chip root. The
//! cross-check holds no register list of its own. The analog image comes
//! from the model's finite read-safety qualification and explicit
//! comparison masks. Open header geometry alone never admits an additional
//! read.
use crate::Result;
use oer_registers_model::{
    Chip,
    chip_model::{ChipModel, ExpandedRegister, FieldClass, Location},
    chip_policy::{ChipPolicy, Claim, Holder},
};
use oer_vendor_phy::calibration_capture::space::{Register, analog_address};
use std::path::Path;

/// The chip whose register model the cross-check reads.
const CHIP: &str = "esp32s31";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";

/// The files the register model is loaded from, relative to the repository
/// root, which a baseline's signature binds.
pub fn model_files(root: &Path) -> Result<Vec<String>> {
    let registers = format!("chips/{CHIP}/registers");
    let mut files = vec![
        format!("{registers}/chip.toml"),
        format!("{registers}/policy.toml"),
    ];
    let mut types = std::fs::read_dir(root.join(&registers).join("types"))?
        .map(|entry| -> Result<String> {
            let name = entry?.file_name();
            Ok(format!("{registers}/types/{}", name.to_string_lossy()))
        })
        .collect::<Result<Vec<_>>>()?;
    types.sort();
    files.extend(types);
    Ok(files)
}

/// The bits of `register` a read returns without a side effect, if any.
fn readable(model: &ChipModel, register: &ExpandedRegister) -> Option<u32> {
    let template = model
        .instances()
        .iter()
        .find(|instance| instance.name == register.instance)
        .and_then(|instance| model.types().get(&instance.block_type))
        .and_then(|block| block.templates.get(&register.key))?;
    if template.read_side_effect.is_some() {
        return None;
    }
    let mask = register
        .fields
        .iter()
        .filter(|field| field.class.readable() && field.class != FieldClass::ReadClears)
        .fold(0, |mask, field| {
            mask | (u32::MAX >> (32 - field.width)) << field.low
        });
    (mask != 0).then_some(mask)
}

/// The MMIO registers the parts of `partition` hold whole, whose part is
/// one of `parts` (every part when empty), with their readable bits.
fn held(
    model: &ChipModel,
    policy: &ChipPolicy,
    partition: &str,
    parts: &[&str],
) -> Result<Vec<(Register, String)>> {
    let index = policy
        .partitions()
        .iter()
        .position(|candidate| candidate.name == partition)
        .ok_or_else(|| format!("the policy has no partition {partition}"))?;
    let mut registers = Vec::new();
    for (identity, claim) in policy.claims() {
        let Claim::Whole(Holder::Part(id)) = claim else {
            continue;
        };
        let part = policy.part(*id).map(|part| part.name.as_str());
        if id.partition != index
            || (!parts.is_empty() && !part.is_some_and(|part| parts.contains(&part)))
        {
            continue;
        }
        let Some(register) = model.register(identity) else {
            continue;
        };
        let Location::Mmio { address } = register.location else {
            continue;
        };
        let Some(mask) = readable(model, register) else {
            continue;
        };
        registers.push((
            Register {
                name: identity.clone(),
                address: u32::try_from(address).map_err(|_| "register above 4 GiB")?,
                mask,
            },
            part.unwrap_or_default().to_owned(),
        ));
    }
    registers.sort_by_key(|(register, _)| register.address);
    Ok(registers)
}

/// The image of `partition`, in address order.
pub fn partition(root: &Path, partition: &str) -> Result<Vec<Register>> {
    let chip = Chip::load(root, CHIP)?;
    let policy = chip.policy()?;
    Ok(held(&chip.model, &policy, partition, &[])?
        .into_iter()
        .map(|(register, _)| register)
        .collect())
}

/// The finite analog image: every byte the model qualifies for a read with
/// its comparison mask, in transport order.
pub fn analog(root: &Path) -> Result<Vec<Register>> {
    let chip = Chip::load(root, CHIP)?;
    let model = &chip.model;
    let mut image = Vec::new();
    for register in model.registers() {
        let Location::AnalogI2c(address) = register.location else {
            continue;
        };
        let Some(safe) = model
            .analog_instances()
            .iter()
            .find(|instance| instance.name == register.instance)
            .and_then(|instance| model.types().get(&instance.block_type))
            .and_then(|block| block.templates.get(&register.key))
            .and_then(|template| template.read_safe.as_ref())
        else {
            continue;
        };
        let Some(mask) = safe.comparison_mask else {
            continue;
        };
        image.push(Register {
            name: format!("analog-i2c.{:02x}.{:02x}", address.block, address.index),
            address: analog_address(address.block, address.index),
            mask,
        });
    }
    image.sort_by_key(|register| register.address);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_partition_image_is_ordered_by_address() {
        let registers = partition(&repository(), PARTITION).unwrap();
        assert!(!registers.is_empty());
        assert!(registers.windows(2).all(|w| w[0].address < w[1].address));
    }

    #[test]
    fn analog_replies_round_trip_through_their_space() {
        use oer_vendor_phy::calibration_capture::space::Space;
        let image = analog(&repository()).unwrap();
        assert!(image.windows(2).all(|w| w[0].address < w[1].address));
        let register = &image[0];
        let line = Space::Analog.line(register.address, 0xa5);
        assert_eq!(
            Space::Analog.replies(&line).unwrap()[&register.address],
            0xa5
        );
        assert!(Space::Analog.request(register.address).starts_with("a "));
    }
}

/// The checkout this package is built in, whose register model the tests
/// read.
#[cfg(test)]
pub(crate) fn repository() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Geometry of the same direct, non-destructive fields the native MAC reads:
/// the MAC's RX and TX statistics and the noise-floor measurement. Opaque
/// bits are excluded.
pub fn health(root: &Path) -> Result<Vec<oer_vendor_phy::comparison::HealthRegister>> {
    use oer_vendor_phy::comparison::{HealthField, HealthRegister};
    let chip = Chip::load(root, CHIP)?;
    let policy = chip.policy()?;
    let model = &chip.model;
    held(
        model,
        &policy,
        "WifiMacPeripherals",
        &[
            "WifiMacRxStatistics",
            "WifiMacTxStatistics",
            "WifiPhyRateOracleWifiBb",
        ],
    )?
    .into_iter()
    .filter(|(register, part)| {
        part != "WifiPhyRateOracleWifiBb" || register.name == "WIFI_BB.NOISE_FLOOR_MEASUREMENT"
    })
    .map(|(register, _)| {
        let expanded = model
            .register(&register.name)
            .ok_or("missing health register geometry")?;
        let fields = expanded
            .fields
            .iter()
            .filter(|field| !field.name.ends_with(oer_registers_model::OPAQUE_SUFFIX))
            .map(|field| {
                Ok(HealthField {
                    name: field.name.clone(),
                    offset: field.low.try_into()?,
                    width: field.width.try_into()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(HealthRegister {
            name: register.name,
            address: register.address,
            fields,
        })
    })
    .collect()
}
