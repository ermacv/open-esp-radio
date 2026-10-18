//! The firmware catalog's ESP-IDF images: vendor references and peers whose
//! own ESP-IDF bootloader loads them ([`catalog`]). A catalog build's
//! `flasher_args.json` must place its bootloader, partition table and
//! application at the chip's flash map offsets ([`flash_files`]).

pub mod catalog;

use std::path::{Path, PathBuf};

use crate::{FlashMap, Result};

/// The bootloader and partition table the ESP-IDF build at `build` flashes,
/// refused unless its `flasher_args.json` places them and the application
/// at `flash`'s offsets.
pub fn flash_files(build: &Path, flash: &FlashMap) -> Result<(PathBuf, PathBuf)> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&std::fs::read(build.join("flasher_args.json"))?)?;
    let entry = |name: &str, expected: u32| -> Result<PathBuf> {
        let offset = arguments[name]["offset"]
            .as_str()
            .and_then(|offset| u32::from_str_radix(offset.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| format!("flasher_args.json names no {name} offset"))?;
        if offset != expected {
            return Err(format!(
                "the catalog build places the {name} at {offset:#x}, the chip profile at \
                 {expected:#x}"
            )
            .into());
        }
        Ok(build.join(
            arguments[name]["file"]
                .as_str()
                .ok_or_else(|| format!("flasher_args.json names no {name} file"))?,
        ))
    };
    entry("app", flash.application)?;
    Ok((
        entry("bootloader", flash.bootloader)?,
        entry("partition-table", flash.partition_table)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> FlashMap {
        FlashMap {
            bootloader: 0x2000,
            partition_table: 0x8000,
            application: 0x10000,
            otadata: 0xd000,
            partitions: "platform/chip-a/partitions/applications.csv".into(),
            application_encoding: oer_chip_profile::FlashSettings {
                mode: oer_chip_profile::FlashMode::Qio,
                frequency_mhz: 80,
                size_mib: 16,
            },
            bootloader_mode: oer_chip_profile::FlashMode::Dio,
            start: oer_chip_profile::Start::PowerOn,
        }
    }

    fn build_with(arguments: &str) -> tempfile::TempDir {
        let build = tempfile::tempdir().unwrap();
        std::fs::write(build.path().join("flasher_args.json"), arguments).unwrap();
        build
    }

    #[test]
    fn the_catalog_build_must_place_its_files_where_the_profile_says() {
        let build = build_with(
            r#"{
                "bootloader": { "offset": "0x2000", "file": "bootloader/bootloader.bin" },
                "partition-table": { "offset": "0x8000", "file": "partition_table/partition-table.bin" },
                "app": { "offset": "0x10000", "file": "app.bin" }
            }"#,
        );
        let (bootloader, partition_table) = flash_files(build.path(), &layout()).unwrap();
        assert_eq!(bootloader, build.path().join("bootloader/bootloader.bin"));
        assert_eq!(
            partition_table,
            build.path().join("partition_table/partition-table.bin")
        );

        let moved = build_with(
            r#"{
                "bootloader": { "offset": "0x0", "file": "bootloader/bootloader.bin" },
                "partition-table": { "offset": "0x8000", "file": "p.bin" },
                "app": { "offset": "0x10000", "file": "app.bin" }
            }"#,
        );
        let error = flash_files(moved.path(), &layout())
            .unwrap_err()
            .to_string();
        assert!(error.contains("bootloader at 0x0"), "{error}");
        assert!(flash_files(build_with("{}").path(), &layout()).is_err());
    }
}
