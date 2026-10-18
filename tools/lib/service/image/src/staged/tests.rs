use super::*;
use crate::bundle::{BootFiles, around};

fn repository() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Every chip whose profile names a flash map for its images.
fn chips(root: &Path) -> Vec<oer_chip_profile::Profile> {
    let chips = oer_chip_profile::Profile::all(root)
        .unwrap()
        .into_iter()
        .filter(|profile| profile.flash.is_some())
        .collect::<Vec<_>>();
    assert!(!chips.is_empty());
    chips
}

/// Byte 2 of an ESP image header: its flash mode.
fn mode_byte(mode: oer_chip_profile::FlashMode) -> u8 {
    match mode {
        oer_chip_profile::FlashMode::Qio => 0,
        oer_chip_profile::FlashMode::Qout => 1,
        oer_chip_profile::FlashMode::Dio => 2,
        oer_chip_profile::FlashMode::Dout => 3,
    }
}

/// A relocatable RV32 ELF whose only content is four bytes of `.text`.
pub(crate) fn elf() -> Vec<u8> {
    let code = [0x13, 0, 0, 0];
    let names = b"\0.text\0.shstrtab\0";
    let text = 52;
    let strings = text + code.len();
    let sections = (strings + names.len()).next_multiple_of(4);
    let mut out = Vec::with_capacity(sections + 120);
    out.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    for half in [1u16, 243] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    for word in [1u32, 0, 0, sections as u32, 1] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52u16, 0, 0, 40, 3, 2] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(names);
    out.resize(sections, 0);
    let header = |out: &mut Vec<u8>, fields: [u32; 10]| {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    };
    header(&mut out, [0; 10]);
    header(
        &mut out,
        [1, 1, 6, 0, text as u32, code.len() as u32, 0, 0, 2, 0],
    );
    header(
        &mut out,
        [7, 3, 0, 0, strings as u32, names.len() as u32, 0, 0, 1, 0],
    );
    out
}

#[test]
fn the_flash_map_is_the_partition_tables() {
    let root = repository();
    for profile in chips(&root) {
        let flash = profile.flash.unwrap();
        let table = partitions(&root, &flash).unwrap();
        let application = table
            .iter()
            .find(|partition| partition.application)
            .unwrap();
        assert_eq!(
            (application.name.as_str(), application.offset),
            ("ota_0", flash.application)
        );
        let otadata = table.iter().find(|partition| partition.otadata).unwrap();
        assert_eq!(otadata.offset, flash.otadata);
        assert!(flash.partition_table < otadata.offset);
        assert!(flash.bootloader < flash.partition_table);
    }
}

#[test]
fn the_boot_files_are_encoded_into_the_bundle() {
    let root = repository();
    for profile in chips(&root) {
        let flash = profile.flash.clone().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let application = directory.path().join("app.bin");
        std::fs::write(&application, b"application").unwrap();
        let elf_path = directory.path().join("bootstrap.elf");
        std::fs::write(&elf_path, elf()).unwrap();
        let output = directory.path().join("bundle");
        let bundle = around(
            &root,
            &profile.id,
            &application,
            BootFiles::Encode { elf: &elf_path },
            &output,
        )
        .unwrap();
        // The ROM reads the bootloader in the flash map's bootloader mode,
        // and it ends at its digest.
        let bootloader = std::fs::read(bundle.bootloader()).unwrap();
        assert_eq!(encode::rom_bootloader(&bootloader).unwrap(), bootloader);
        assert_eq!(bootloader[2], mode_byte(flash.bootloader_mode));
        let csv = root.join(&bundle.flash.partitions);
        assert_eq!(
            std::fs::read(bundle.partitions()).unwrap(),
            encode::partition_table(&csv).unwrap()
        );
        assert_eq!(
            std::fs::read(bundle.otadata().unwrap()).unwrap(),
            encode::ota_selector_image(0)
        );
        let segments = bundle
            .segments()
            .into_iter()
            .map(|segment| (segment.offset, segment.description))
            .collect::<Vec<_>>();
        assert_eq!(
            segments,
            [
                (flash.bootloader, "bootloader"),
                (flash.partition_table, "partition table"),
                (flash.application, "application"),
                (flash.otadata, "OTA selection"),
            ]
        );
        assert_eq!(
            oer_image_bundle::ImageBundle::load(&output).unwrap(),
            bundle
        );
        // Boot files that are not the chip's are refused.
        assert!(
            around(
                &root,
                &profile.id,
                &application,
                BootFiles::Given {
                    bootloader: &application,
                    partition_table: &application,
                },
                &directory.path().join("other"),
            )
            .is_err()
        );
        // The chip's own boot files are taken as given, for the ESP-IDF
        // bootloader.
        let given = around(
            &root,
            &profile.id,
            &application,
            BootFiles::Given {
                bootloader: &bundle.bootloader(),
                partition_table: &bundle.partitions(),
            },
            &directory.path().join("given"),
        )
        .unwrap();
        assert_eq!(given.boot, crate::Boot::EspIdfBootloader);
        assert_eq!(std::fs::read(given.bootloader()).unwrap(), bootloader);
        // A valid bootloader with something else as its partition table is
        // refused too.
        assert!(
            around(
                &root,
                &profile.id,
                &application,
                BootFiles::Given {
                    bootloader: &bundle.bootloader(),
                    partition_table: &application,
                },
                &directory.path().join("half"),
            )
            .is_err()
        );
    }
}

#[test]
fn the_application_and_the_bootloader_are_encoded_in_the_flash_maps_modes() {
    let root = repository();
    for profile in chips(&root) {
        let flash = profile.flash.clone().unwrap();
        let csv = root.join(&flash.partitions);
        let chip = encode::chip(&profile.espflash_chip).unwrap();
        let application = encode::encode(
            &elf(),
            chip,
            encode::Encoding::application(&flash, MMU_PAGE_SIZE).unwrap(),
            Some(&csv),
            flash.partition_table,
            Some("ota_0"),
        )
        .unwrap();
        let rom = encode::encode(
            &elf(),
            chip,
            encode::Encoding::bootloader(&flash, MMU_PAGE_SIZE).unwrap(),
            Some(&csv),
            flash.partition_table,
            None,
        )
        .unwrap();
        assert_eq!(
            application.application[2],
            mode_byte(flash.application_encoding.mode)
        );
        assert_eq!(rom.bootloader[2], mode_byte(flash.bootloader_mode));
        assert_eq!(application.partition_table, rom.partition_table);
    }
}
