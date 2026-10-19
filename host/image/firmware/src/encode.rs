//! The flash contents of an image, encoded with the `espflash` library: the
//! ROM-readable second-stage bootloader, the partition table, the application
//! image of the bootstrap ELF and the OTA selection of `ota_0`.
//!
//! The ROM reads the bootloader in DIO; ESP-IDF switches the flash to QIO for
//! the application. The application is encoded with the chip's application
//! settings and the bootloader separately in the ROM's mode, from the same
//! espflash bootloader resources: one encoding in QIO would change the
//! bootloader's ROM image header and stop the board booting.

use std::{path::Path, str::FromStr as _};

use espflash::{
    flasher::{FlashData, FlashFrequency, FlashMode, FlashSettings, FlashSize},
    image_format::idf::IdfBootloaderFormat,
    target::Chip,
};

use crate::Result;
use oer_image_chip_profile as chip;

/// The staged images' MMU page: the bootstrap keeps its text on 64-KiB pages.
pub const MMU_PAGE_SIZE: u32 = 0x1_0000;
/// The application partition the bootstrap is written to.
pub const APPLICATION_PARTITION: &str = "ota_0";
/// Size of the OTA data partition's image.
pub const OTA_DATA_SIZE: usize = 0x2000;

/// One part of the flash contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub name: &'static str,
    pub offset: u32,
    pub data: Vec<u8>,
}

/// The parts in write order: the OTA selection is last, so an interrupted
/// write leaves the previous selection pointing at an image whose checksum
/// no longer validates rather than selecting a half-written one.
pub type Segments = Vec<Segment>;

/// The `espflash` chip of `name` (a chip file's `espflash-chip`).
pub fn espflash_chip(name: &str) -> Result<Chip> {
    Chip::from_str(name).map_err(|_| format!("espflash knows no chip `{name}`").into())
}

fn mode(mode: chip::Mode) -> FlashMode {
    match mode {
        chip::Mode::Qio => FlashMode::Qio,
        chip::Mode::Qout => FlashMode::Qout,
        chip::Mode::Dio => FlashMode::Dio,
        chip::Mode::Dout => FlashMode::Dout,
    }
}

/// The ESP image header's code of `mode` (byte 2).
const fn mode_code(mode: chip::Mode) -> u8 {
    match mode {
        chip::Mode::Qio => 0,
        chip::Mode::Qout => 1,
        chip::Mode::Dio => 2,
        chip::Mode::Dout => 3,
    }
}

fn frequency(mhz: u32) -> Result<FlashFrequency> {
    Ok(match mhz {
        20 => FlashFrequency::_20Mhz,
        26 => FlashFrequency::_26Mhz,
        40 => FlashFrequency::_40Mhz,
        80 => FlashFrequency::_80Mhz,
        _ => return Err(format!("this tool encodes no {mhz}-MHz flash clock").into()),
    })
}

fn size(mib: u32) -> Result<FlashSize> {
    Ok(match mib {
        4 => FlashSize::_4Mb,
        8 => FlashSize::_8Mb,
        16 => FlashSize::_16Mb,
        32 => FlashSize::_32Mb,
        _ => return Err(format!("this tool encodes no {mib}-MiB flash").into()),
    })
}

/// The espflash encoding of `elf` with the bootloader in `bootloader_mode`.
fn espflash_segments(
    elf: &[u8],
    chip: Chip,
    flash: &chip::FlashMap,
    partitions: &Path,
    flash_mode: chip::Mode,
) -> Result<Vec<(u32, Vec<u8>)>> {
    let settings = FlashSettings::new(
        Some(mode(flash_mode)),
        Some(size(flash.application_encoding.size_mib)?),
        Some(frequency(flash.application_encoding.frequency_mhz)?),
    );
    let data = FlashData::new(
        settings,
        0,
        Some(MMU_PAGE_SIZE),
        chip,
        chip.default_xtal_frequency(),
    );
    let format = IdfBootloaderFormat::new(
        elf,
        &data,
        Some(partitions),
        None,
        Some(flash.partition_table),
        Some(APPLICATION_PARTITION),
    )
    .map_err(|error| format!("espflash cannot encode the image: {error}"))?;
    Ok(format
        .flash_segments()
        .map(|segment| (segment.addr, segment.data().to_vec()))
        .collect())
}

/// Encode the bootstrap ELF `elf` as the flash contents of `chip`.
pub fn encode(elf: &[u8], chip: &chip::Chip, root: &Path) -> Result<Segments> {
    let flash = &chip.flash;
    let espflash = espflash_chip(&chip.espflash_chip)?;
    let partitions = root.join(&flash.partitions);
    check_partitions(&std::fs::read_to_string(&partitions)?, flash)
        .map_err(|error| format!("{}: {error}", partitions.display()))?;
    let application = espflash_segments(
        elf,
        espflash,
        flash,
        &partitions,
        flash.application_encoding.mode,
    )?;
    let rom = espflash_segments(elf, espflash, flash, &partitions, flash.bootloader_mode)?;
    let [(boot_at, _), (table_at, table), (app_at, app)] = <[_; 3]>::try_from(application)
        .map_err(|_| "espflash encoded other than a bootloader, a table and an application")?;
    let (rom_at, bootloader) = rom
        .into_iter()
        .next()
        .ok_or("espflash encoded no bootloader")?;
    for (name, encoded, expected) in [
        ("bootloader", boot_at, flash.bootloader),
        ("ROM bootloader", rom_at, flash.bootloader),
        ("partition table", table_at, flash.partition_table),
        ("application", app_at, flash.application),
    ] {
        if encoded != expected {
            return Err(format!(
                "espflash put the {name} at {encoded:#x}, the chip's flash map at {expected:#x}"
            )
            .into());
        }
    }
    require_image_mode("bootloader", &bootloader, espflash, flash.bootloader_mode)?;
    require_image_mode(
        "application",
        &app,
        espflash,
        flash.application_encoding.mode,
    )?;
    Ok(vec![
        Segment {
            name: "bootloader",
            offset: flash.bootloader,
            data: bootloader,
        },
        Segment {
            name: "partition-table",
            offset: flash.partition_table,
            data: table,
        },
        Segment {
            name: "application",
            offset: flash.application,
            data: app,
        },
        Segment {
            name: "otadata",
            offset: flash.otadata,
            data: ota_selector_image(0).to_vec(),
        },
    ])
}

/// Fails unless `bytes` are an ESP image of `chip` whose header names `mode`.
pub fn require_image_mode(name: &str, bytes: &[u8], chip: Chip, mode: chip::Mode) -> Result<()> {
    const MAGIC: u8 = 0xe9;
    let header: &[u8; 24] = bytes
        .get(..24)
        .and_then(|header| header.try_into().ok())
        .ok_or_else(|| format!("the {name} is no ESP image"))?;
    if header[0] != MAGIC || header[1] == 0 {
        return Err(format!("the {name} is no ESP image").into());
    }
    if header[2] != mode_code(mode) {
        return Err(format!(
            "the {name}'s header names flash mode {}, not {mode:?}",
            header[2]
        )
        .into());
    }
    let id = u16::from_le_bytes([header[12], header[13]]);
    if id != chip.id() {
        return Err(format!("the {name} is an image of chip id {id}, not {chip}'s").into());
    }
    Ok(())
}

/// Fails unless the partition table CSV `csv` puts `ota_0` and the OTA data
/// where the flash map does.
pub fn check_partitions(csv: &str, flash: &chip::FlashMap) -> std::result::Result<(), String> {
    let offset = |name: &str| -> std::result::Result<u32, String> {
        let line = csv
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .find(|line| line.split(',').next().map(str::trim) == Some(name))
            .ok_or_else(|| format!("no partition `{name}`"))?;
        let field = line
            .split(',')
            .nth(3)
            .map(str::trim)
            .ok_or_else(|| format!("partition `{name}` has no offset"))?;
        let hex = field
            .strip_prefix("0x")
            .ok_or_else(|| format!("partition `{name}`'s offset {field} is not hexadecimal"))?;
        u32::from_str_radix(hex, 16).map_err(|error| format!("partition `{name}`: {error}"))
    };
    for (name, expected) in [
        (APPLICATION_PARTITION, flash.application),
        ("otadata", flash.otadata),
    ] {
        let found = offset(name)?;
        if found != expected {
            return Err(format!(
                "partition `{name}` lies at {found:#x}, the flash map puts it at {expected:#x}"
            ));
        }
    }
    Ok(())
}

/// The OTA data that makes the ESP-IDF bootloader boot application slot
/// `slot` (0 for `ota_0`): sequence number `slot + 1`, state "valid" and the
/// sequence's CRC, the second entry erased.
pub fn ota_selector_image(slot: u32) -> [u8; OTA_DATA_SIZE] {
    let sequence = slot + 1;
    let mut image = [0xff; OTA_DATA_SIZE];
    image[0..4].copy_from_slice(&sequence.to_le_bytes());
    image[24..28].copy_from_slice(&2_u32.to_le_bytes());
    image[28..32].copy_from_slice(&crc32_le(&sequence.to_le_bytes()).to_le_bytes());
    image
}

/// ESP-IDF's `esp_rom_crc32_le(UINT32_MAX, …)` of the OTA entry's sequence.
fn crc32_le(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc ^ u32::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flash() -> chip::FlashMap {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        chip::Chip::load(&root, "esp32s31").unwrap().flash
    }

    #[test]
    fn the_ota_selection_boots_the_first_slot() {
        let image = ota_selector_image(0);
        assert_eq!(&image[0..4], &1_u32.to_le_bytes());
        assert_eq!(&image[24..28], &2_u32.to_le_bytes());
        // `esp_rom_crc32_le(UINT32_MAX, …)` of sequence 1, computed
        // independently.
        assert_eq!(&image[28..32], &0x4743_989a_u32.to_le_bytes());
        assert!(image[4..24].iter().all(|byte| *byte == 0xff));
        assert!(image[32..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn the_partition_table_matches_the_flash_map() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let flash = flash();
        let csv = std::fs::read_to_string(root.join(&flash.partitions)).unwrap();
        check_partitions(&csv, &flash).unwrap();
        let moved = csv.replace("0x10000,", "0x20000,");
        assert!(
            check_partitions(&moved, &flash)
                .unwrap_err()
                .contains("ota_0")
        );
        let missing = csv.replace("otadata", "other");
        assert!(
            check_partitions(&missing, &flash)
                .unwrap_err()
                .contains("otadata")
        );
    }

    #[test]
    fn image_headers_are_checked_for_chip_and_mode() {
        let chip = espflash_chip("esp32s31").unwrap();
        let mut image = vec![0_u8; 24];
        image[0] = 0xe9;
        image[1] = 1;
        image[2] = mode_code(chip::Mode::Dio);
        image[12..14].copy_from_slice(&chip.id().to_le_bytes());
        require_image_mode("bootloader", &image, chip, chip::Mode::Dio).unwrap();
        assert!(require_image_mode("bootloader", &image, chip, chip::Mode::Qio).is_err());
        image[12] ^= 1;
        assert!(require_image_mode("bootloader", &image, chip, chip::Mode::Dio).is_err());
        assert!(require_image_mode("bootloader", &[0; 8], chip, chip::Mode::Dio).is_err());
    }
}
