//! The calibrated register state the cross-check compares: the radio-PHY
//! partition image and the analog image.
//!
//! The register model's own enumeration defines the image: every readable
//! register of the published `RadioPhyPeripherals` ownership partition, in
//! the order the production PAC reads it by index
//! (`RadioPhyRegisters::register_image`). The cross-check therefore names
//! each index exactly as production reads it, and holds no register list of
//! its own. The analog image comes from the native model's finite R1
//! read-safety qualification and explicit comparison masks. Open header
//! geometry alone never admits an additional read.
use crate::Result;
use oer_vendor_phy::calibration_capture::space::{Register, analog_address};
use std::path::Path;

/// Published SVD and PAC API policy, relative to the repository root.
const SVD: &str = "chips/esp32s31/registers/published/radio.svd";
const API_POLICY: &str = "chips/esp32s31/registers/policy/api.toml";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";
const ANALOG_MODEL: &str = "chips/esp32s31/registers/model/analog/phy.toml";

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

/// The finite R1 analog image, in the production capture order.
pub fn analog(root: &Path) -> Result<Vec<Register>> {
    let space = oer_registers_model::analog::AnalogSpace::load(&root.join(ANALOG_MODEL))?;
    Ok(space
        .geometry("esp32s31")?
        .into_iter()
        .filter_map(|register| {
            let mask = register.comparison_mask?;
            register.read_safe_epoch?;
            Some(Register {
                name: format!(
                    "analog-i2c.{:02x}.{:02x}",
                    register.address.block, register.address.index
                ),
                address: analog_address(register.address.block, register.address.index),
                mask: u32::from(mask),
            })
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
        let image = analog(repository().path()).unwrap();
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
        (
            ANALOG_MODEL,
            include_str!("../../../../chips/esp32s31/registers/model/analog/phy.toml"),
        ),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}

/// Geometry of the same direct, non-destructive fields the native MAC reads.
/// Opaque bits, RX-hang banks and the unknown-semantics ten-bit NF bank are excluded.
pub fn health(root: &Path) -> Result<Vec<oer_vendor_phy::comparison::HealthRegister>> {
    use oer_vendor_phy::comparison::{HealthField, HealthRegister};
    let svd = std::fs::read_to_string(root.join(SVD))?;
    let geometry = oer_registers_model::svd_geometry(&svd)?;
    let words = partition(root, "WifiMacPeripherals")?;
    words
        .into_iter()
        .filter(|r| {
            r.name.starts_with("WIFI_MAC_RX_STATISTICS.")
                || r.name.starts_with("WIFI_MAC_TX_STATISTICS.")
                || r.name == "WIFI_PHY_RATE_ORACLE.NOISE_FLOOR_MEASUREMENT"
        })
        .map(|r| {
            let geometry = geometry
                .iter()
                .find(|g| g.name == r.name)
                .ok_or("missing health register geometry")?;
            let fields = geometry
                .fields
                .iter()
                .filter(|f| !f.name.contains("OPAQUE"))
                .map(|f| {
                    Ok(HealthField {
                        name: f.name.clone(),
                        offset: f.offset.try_into()?,
                        width: f.width.try_into()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(HealthRegister {
                name: r.name,
                address: r.address,
                fields,
            })
        })
        .collect()
}
