//! The chip facts an image build reads from `firmware/platform/<chip>/chip.toml`:
//! the espflash chip, the Rust target and the `[flash]` map.

use std::path::Path;

use serde::Deserialize;

/// The crate's error type.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A flash mode of an ESP image header.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Qio,
    Qout,
    Dio,
    Dout,
}

/// How the application image's header describes the flash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct ApplicationEncoding {
    pub mode: Mode,
    pub frequency_mhz: u32,
    pub size_mib: u32,
}

/// Where every image part lies in flash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct FlashMap {
    pub bootloader: u32,
    pub partition_table: u32,
    pub application: u32,
    pub otadata: u32,
    /// The partition table CSV, relative to the repository root.
    pub partitions: String,
    pub application_encoding: ApplicationEncoding,
    /// The mode the ROM reads the bootloader in.
    pub bootloader_mode: Mode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Chip {
    pub id: String,
    pub rust_target: String,
    pub espflash_chip: String,
    pub flash: FlashMap,
}

impl Chip {
    /// The chip file of `id` below the repository `root`.
    pub fn load(root: &Path, id: &str) -> Result<Self> {
        let path = root.join("firmware/platform").join(id).join("chip.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()).into())
    }

    pub fn parse(text: &str) -> std::result::Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_s31_chip_file_names_its_flash_map() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let chip = Chip::load(&root, "esp32s31").unwrap();
        assert_eq!(chip.espflash_chip, "esp32s31");
        assert_eq!(chip.rust_target, "riscv32imafc-unknown-none-elf");
        assert_eq!(
            (
                chip.flash.bootloader,
                chip.flash.partition_table,
                chip.flash.otadata,
                chip.flash.application
            ),
            (0x2000, 0x8000, 0xd000, 0x1_0000)
        );
        assert_eq!(chip.flash.bootloader_mode, Mode::Dio);
        assert_eq!(chip.flash.application_encoding.mode, Mode::Qio);
        assert!(root.join(&chip.flash.partitions).is_file());
    }
}
