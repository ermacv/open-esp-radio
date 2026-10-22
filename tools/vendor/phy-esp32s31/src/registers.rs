//! The calibrated register state the cross-check compares: the radio-PHY
//! partition image and the analog image.
//!
//! The register model's own enumeration defines the image: every readable
//! register of the published `RadioPhyPeripherals` ownership partition, in
//! the order the production PAC reads it by index
//! (`RadioPhyRegisters::register_image`). The cross-check therefore names
//! each index exactly as production reads it, and holds no register list of
//! its own. The analog image is likewise the model's `register-image` of
//! the reviewed `PhyI2cField` domain: every analog-I2C register a reviewed
//! field occupies, in the order the production PAC reads it
//! (`PhyI2cAddress::image`).
use crate::Result;
use oer_vendor_phy::calibration_capture::space::{Register, analog_address};
use std::path::Path;

/// Published SVD and PAC API policy, relative to the repository root.
const SVD: &str = "chips/esp32s31/registers/published/radio.svd";
const API_POLICY: &str = "chips/esp32s31/registers/policy/api.toml";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";
/// The indirect-register domain of the reviewed analog-I2C fields.
pub const ANALOG_DOMAIN: &str = "PhyI2cField";

/// The image of `partition`, in production's index order.
pub fn partition(root: &Path, partition: &str) -> Result<Vec<Register>> {
    let policy = oer_registers_model::PacApiPack::load(&root.join(API_POLICY))?;
    let svd = std::fs::read_to_string(root.join(SVD))?;
    Ok(policy
        .partition_readable_registers(&svd, partition)?
        .into_iter()
        .map(|register| Register {
            name: match register.element {
                Some(element) => format!(
                    "{}.{}",
                    register.peripheral,
                    register.register.replace("%s", &element.to_string())
                ),
                None => format!("{}.{}", register.peripheral, register.register),
            },
            address: register.address,
            mask: register.mask,
        })
        .collect())
}

/// The analog image of the indirect-register `domain`, in production's
/// index order.
pub fn analog(root: &Path, domain: &str) -> Result<Vec<Register>> {
    let policy = oer_registers_model::PacApiPack::load(&root.join(API_POLICY))?;
    let domain = policy
        .indirect_register_field_domains
        .iter()
        .find(|candidate| candidate.name == domain && candidate.register_image)
        .ok_or_else(|| format!("no indirect-register domain {domain} with a register image"))?;
    Ok(domain
        .image_registers()
        .into_iter()
        .map(|(block, register)| Register {
            name: format!("{}.{block:02x}.{register:02x}", domain.name),
            address: analog_address(block, register),
            mask: domain
                .fields
                .iter()
                .filter(|field| field.bank == block && field.register == register)
                .fold(0, |mask, field| {
                    mask | (((1u32 << field.bit_width) - 1) << field.bit_offset)
                }),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_partition_image_is_ordered_by_address() {
        let registers = partition(repository().path(), PARTITION).unwrap();
        assert!(!registers.is_empty());
        assert!(registers.windows(2).all(|w| w[0].address < w[1].address));
    }

    #[test]
    fn analog_replies_round_trip_through_their_space() {
        use oer_vendor_phy::calibration_capture::space::Space;
        let image = analog(repository().path(), ANALOG_DOMAIN).unwrap();
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

#[cfg(test)]
pub(crate) fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, text) in [
        (
            "chips/esp32s31/registers/published/radio.svd",
            include_str!("../../../../chips/esp32s31/registers/published/radio.svd"),
        ),
        (
            "chips/esp32s31/registers/policy/api.toml",
            include_str!("../../../../chips/esp32s31/registers/policy/api.toml"),
        ),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}
