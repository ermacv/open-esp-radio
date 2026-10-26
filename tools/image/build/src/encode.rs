//! The flash contents of an image, encoded at build time with the `espflash`
//! library: the ESP application image, the ROM-readable second-stage
//! bootloader, partition tables and the OTA selection.
//!
//! The ROM reads the bootloader in DIO; ESP-IDF enables QIO for the
//! application. The application is therefore encoded with the chip's
//! application flash settings and the bootloader separately with the ROM's,
//! from the same `espflash` bootloader resources; a single encoding in QIO
//! would also change the ROM image header and stop the board booting.

#![forbid(unsafe_code)]

use std::{path::Path, str::FromStr as _};

use espflash::{
    flasher::{FlashData, FlashSettings},
    image_format::idf::IdfBootloaderFormat,
    target::Chip,
};
use sha2::{Digest as _, Sha256};

mod partition_table;
pub mod stage_two;

/// The flash settings an [`Encoding`] names.
pub use espflash::flasher::{FlashFrequency, FlashMode, FlashSize};

use crate::Result;

/// Size of the OTA data partition's image.
pub const OTA_DATA_SIZE: usize = 0x2000;
/// Share of the application partition from which encoding warns that the
/// partition is nearly full, before an image stops fitting.
pub const PARTITION_WARNING_PERCENT: u64 = 90;

/// How an ESP application image is encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Encoding {
    /// The application's flash mode; `None` keeps the bootloader's.
    pub mode: Option<FlashMode>,
    pub frequency: Option<FlashFrequency>,
    pub size: Option<FlashSize>,
    pub mmu_page_size: Option<u32>,
}

impl Encoding {
    /// `espflash save-image`'s defaults for the chip.
    pub const DEFAULT: Self = Self {
        mode: None,
        frequency: None,
        size: None,
        mmu_page_size: None,
    };

    /// The application image's encoding of the chip flash map `flash`, with
    /// MMU pages of `mmu_page_size` bytes.
    pub fn application(flash: &oer_espressif_facts::FlashMap, mmu_page_size: u32) -> Result<Self> {
        let settings = &flash.application_encoding;
        Ok(Self {
            mode: Some(mode(settings.mode)),
            frequency: Some(frequency(settings.frequency_mhz)?),
            size: Some(size(settings.size_mib)?),
            mmu_page_size: Some(mmu_page_size),
        })
    }

    /// The encoding the ROM reads the bootloader in: the application's, in
    /// the flash map's bootloader mode.
    pub fn bootloader(flash: &oer_espressif_facts::FlashMap, mmu_page_size: u32) -> Result<Self> {
        Ok(Self {
            mode: Some(mode(flash.bootloader_mode)),
            ..Self::application(flash, mmu_page_size)?
        })
    }
}

fn mode(mode: oer_espressif_facts::FlashMode) -> FlashMode {
    match mode {
        oer_espressif_facts::FlashMode::Qio => FlashMode::Qio,
        oer_espressif_facts::FlashMode::Qout => FlashMode::Qout,
        oer_espressif_facts::FlashMode::Dio => FlashMode::Dio,
        oer_espressif_facts::FlashMode::Dout => FlashMode::Dout,
    }
}

/// The image header's frequency of `mhz`.
fn frequency(mhz: u32) -> Result<FlashFrequency> {
    Ok(match mhz {
        12 => FlashFrequency::_12Mhz,
        15 => FlashFrequency::_15Mhz,
        16 => FlashFrequency::_16Mhz,
        20 => FlashFrequency::_20Mhz,
        24 => FlashFrequency::_24Mhz,
        26 => FlashFrequency::_26Mhz,
        30 => FlashFrequency::_30Mhz,
        40 => FlashFrequency::_40Mhz,
        48 => FlashFrequency::_48Mhz,
        60 => FlashFrequency::_60Mhz,
        80 => FlashFrequency::_80Mhz,
        _ => return Err(format!("an ESP image header holds no {mhz}-MHz flash clock").into()),
    })
}

/// The image header's flash size of `mib` MiB.
fn size(mib: u32) -> Result<FlashSize> {
    Ok(match mib {
        1 => FlashSize::_1Mb,
        2 => FlashSize::_2Mb,
        4 => FlashSize::_4Mb,
        8 => FlashSize::_8Mb,
        16 => FlashSize::_16Mb,
        32 => FlashSize::_32Mb,
        64 => FlashSize::_64Mb,
        128 => FlashSize::_128Mb,
        256 => FlashSize::_256Mb,
        _ => return Err(format!("an ESP image header holds no {mib}-MiB flash size").into()),
    })
}

/// The flash contents `encode` makes of an ELF.
pub struct Encoded {
    /// The ESP application image.
    pub application: Vec<u8>,
    /// The second-stage bootloader with the encoding's flash header.
    pub bootloader: Vec<u8>,
    /// The binary partition table.
    pub partition_table: Vec<u8>,
}

/// The `espflash` chip of `name` (a chip profile's `espflash-chip`).
pub fn chip(name: &str) -> Result<Chip> {
    Chip::from_str(name).map_err(|_| format!("espflash knows no chip `{name}`").into())
}

/// Encode the ELF `elf` for `chip` as `espflash save-image` does: the
/// application for the partition `application` of the CSV `partitions` at
/// `partition_table` (espflash's default table when `None`), with the
/// chip's bundled bootloader and the partition table it is written with.
pub fn encode(
    elf: &[u8],
    chip: Chip,
    encoding: Encoding,
    partitions: Option<&Path>,
    partition_table: u32,
    application: Option<&str>,
) -> Result<Encoded> {
    let settings = FlashSettings::new(
        encoding.mode,
        Some(encoding.size.unwrap_or_default()),
        encoding.frequency,
    );
    let data = FlashData::new(
        settings,
        0,
        encoding.mmu_page_size,
        chip,
        chip.default_xtal_frequency(),
    );
    let format = IdfBootloaderFormat::new(
        elf,
        &data,
        partitions,
        None,
        Some(partition_table),
        application,
    )
    .map_err(|error| format!("espflash cannot encode the image: {error}"))?;
    let mut segments = format.flash_segments();
    let mut next = |name: &str| -> Result<Vec<u8>> {
        Ok(segments
            .next()
            .ok_or_else(|| format!("espflash encoded no {name}"))?
            .data()
            .to_vec())
    };
    let bootloader = next("bootloader")?;
    let partition_table = next("partition table")?;
    let application = next("application")?;
    Ok(Encoded {
        application,
        bootloader,
        partition_table,
    })
}

/// The binary form of the partition table CSV at `csv`.
pub fn partition_table(csv: &Path) -> Result<Vec<u8>> {
    Ok(partition_table::Table::read_csv(csv)?.to_binary()?)
}

/// One partition of a partition table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Partition {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    /// An application partition (`app`), rather than data.
    pub application: bool,
    /// The OTA data partition (`data, ota`).
    pub otadata: bool,
}

/// The partitions of the table CSV at `csv`, in table order.
pub fn partitions(csv: &Path) -> Result<Vec<Partition>> {
    Ok(partition_table::Table::read_csv(csv)?.partitions()?)
}

/// A warning when an application image of `bytes` fills at least
/// [`PARTITION_WARNING_PERCENT`] of its partition of `capacity` bytes.
pub fn partition_budget_warning(bytes: u64, capacity: u64) -> Option<String> {
    let percent = bytes * 100 / capacity;
    (percent >= PARTITION_WARNING_PERCENT).then(|| {
        format!(
            "application image uses {percent}% of its partition ({bytes} of {capacity} bytes); \
             reduce code size before it stops fitting"
        )
    })
}

/// The ROM-readable part of an encoded bootloader: a hashed DIO ESP image,
/// up to and including its digest, after validating its segment checksum
/// and digest.
pub fn rom_bootloader(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() < 24 || bytes[0] != 0xe9 || bytes[1] == 0 || bytes[2] != 2 || bytes[23] != 1 {
        return Err("ROM bootloader must be a hashed DIO ESP image".into());
    }
    let mut position: usize = 24;
    let mut checksum = 0xef;
    for _ in 0..bytes[1] {
        let header = bytes
            .get(position..position + 8)
            .ok_or("truncated ROM segment header")?;
        let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        position += 8;
        let end = position
            .checked_add(length)
            .ok_or("ROM segment length overflow")?;
        let data = bytes
            .get(position..end)
            .ok_or("ROM segment exceeds the bootloader")?;
        for byte in data {
            checksum ^= byte;
        }
        position = end;
    }
    let digest_start = position.checked_add(16).ok_or("ROM image size overflow")? & !15;
    let image = bytes
        .get(..digest_start + 32)
        .ok_or("truncated ROM checksum or digest")?;
    if image[digest_start - 1] != checksum
        || Sha256::digest(&image[..digest_start]).as_slice() != &image[digest_start..]
    {
        return Err("ROM image checksum or SHA-256 mismatch".into());
    }
    Ok(image)
}

/// Count mapped records in an encoded ESP image, after the packer has
/// merged ELF sections. The shared D/I loader identifies them by load
/// address, not executable flags; ELF cache-off placement is checked first.
/// SOURCE: ESP-IDF e9da155a72624fce88b8ef2cf3cde9aee2e6067f,
/// bootloader_support/src/bootloader_utility.c:816-841 and esp_app_format.h
/// (24-byte image header, 8-byte segment header).
pub(crate) fn mapped_segment_count(
    bytes: &[u8],
    flash: oer_espressif_facts::Region,
) -> Result<usize> {
    let image_header = bytes.get(..24).ok_or("truncated ESP image header")?;
    let count = image_header
        .get(1)
        .copied()
        .ok_or("missing ESP segment count")?;
    if image_header.first() != Some(&0xe9) || count == 0 {
        return Err("not an ESP image with segment headers".into());
    }
    let mut position = 24_usize;
    let mut mapped = 0;
    for _ in 0..count {
        let data_start = position
            .checked_add(8)
            .ok_or("ESP header offset overflow")?;
        let header = bytes
            .get(position..data_start)
            .ok_or("truncated ESP segment header")?;
        let address = u32::from_le_bytes(
            header
                .get(..4)
                .ok_or("truncated ESP load address")?
                .try_into()?,
        );
        let length = u32::from_le_bytes(
            header
                .get(4..)
                .ok_or("truncated ESP data length")?
                .try_into()?,
        );
        let end = data_start
            .checked_add(usize::try_from(length)?)
            .ok_or("ESP segment length overflow")?;
        bytes
            .get(data_start..end)
            .ok_or("truncated ESP segment data")?;
        if (flash.origin..flash.end()).contains(&address) {
            if length == 0
                || !flash.contains_range(u64::from(address), u64::from(address) + u64::from(length))
            {
                return Err("empty or out-of-window mapped ESP segment".into());
            }
            mapped += 1;
        }
        position = end;
    }
    Ok(mapped)
}

/// The OTA data that makes the ESP-IDF bootloader boot application slot
/// `slot` (0 for `ota_0`, 1 for `ota_1`): its sequence number modulo the
/// number of slots, plus one.
pub fn ota_selector_image(slot: u32) -> [u8; OTA_DATA_SIZE] {
    let sequence = slot + 1;
    let mut image = [0xff; OTA_DATA_SIZE];
    image[0..4].copy_from_slice(&sequence.to_le_bytes());
    image[24..28].copy_from_slice(&2_u32.to_le_bytes());
    image[28..32].copy_from_slice(&crc32_idf(&sequence.to_le_bytes()).to_le_bytes());
    image
}

fn crc32_idf(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc ^ u32::MAX
}

/// Fails unless `bytes` begin an ESP image built for `chip`: the image magic,
/// at least one segment and the chip id of its extended header. A given
/// bootloader or application of another chip, or no ESP image at all, is
/// refused before it reaches a bundle.
pub fn require_chip_image(bytes: &[u8], chip: Chip) -> Result<()> {
    const MAGIC: u8 = 0xe9;
    const HEADER_BYTES: usize = 24;
    if bytes.len() < HEADER_BYTES || bytes[0] != MAGIC || bytes[1] == 0 {
        return Err("not an ESP image (no image magic, segments or extended header)".into());
    }
    let id = u16::from_le_bytes([bytes[12], bytes[13]]);
    if id != chip.id() {
        return Err(format!("an ESP image of chip id {id}, not {chip}'s ({})", chip.id()).into());
    }
    Ok(())
}

/// Fails unless `bytes` are a binary ESP-IDF partition table.
pub fn require_partition_table(bytes: &[u8]) -> Result<()> {
    partition_table::Table::from_binary(bytes)?;
    Ok(())
}

/// Fails unless the ELF holds the ESP-IDF application descriptor the
/// second-stage bootloader requires.
pub fn require_app_descriptor(elf: &[u8]) -> Result<()> {
    let elf = oer_analysis_elf::Elf::parse(elf)?;
    if elf.symbols().any(|symbol| symbol.name == "esp_app_desc")
        || elf.section_by_name(".flash.appdesc").is_some()
        || elf.section_by_name(".rodata_desc").is_some()
    {
        Ok(())
    } else {
        Err("the ELF has no ESP-IDF application descriptor (`esp_app_desc`)".into())
    }
}

#[cfg(test)]
mod tests;
