use super::*;
use crate::bundle::{BootFiles, around};

fn repository() -> tempfile::TempDir {
    crate::tests::repository()
}

/// Every chip whose profile names a flash map for its images.
fn chips(root: &Path) -> Vec<oer_chip_profile::Profile> {
    let chips = oer_chip_profile::Profile::all(root)
        .unwrap()
        .into_iter()
        .filter(|profile| profile.facts.flash.is_some())
        .collect::<Vec<_>>();
    assert!(!chips.is_empty());
    chips
}

/// Byte 2 of an ESP image header: its flash mode.
fn mode_byte(mode: oer_espressif_facts::FlashMode) -> u8 {
    match mode {
        oer_espressif_facts::FlashMode::Qio => 0,
        oer_espressif_facts::FlashMode::Qout => 1,
        oer_espressif_facts::FlashMode::Dio => 2,
        oer_espressif_facts::FlashMode::Dout => 3,
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
    let repository = repository();
    let root = repository.path();
    for profile in chips(&root) {
        let flash = profile.facts.flash.unwrap();
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
    let repository = repository();
    let root = repository.path();
    for profile in chips(&root) {
        let flash = profile.facts.flash.clone().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let application = directory.path().join("app.bin");
        std::fs::write(&application, b"application").unwrap();
        let elf_path = directory.path().join("bootstrap.elf");
        std::fs::write(&elf_path, elf()).unwrap();
        let output = directory.path().join("bundle");
        let bundle = around(
            &root,
            &profile.facts.id,
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
        assert_eq!(crate::bundle::ImageBundle::load(&output).unwrap(), bundle);
        // Boot files that are not the chip's are refused.
        assert!(
            around(
                &root,
                &profile.facts.id,
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
            &profile.facts.id,
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
                &profile.facts.id,
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
    let repository = repository();
    let root = repository.path();
    for profile in chips(&root) {
        let flash = profile.facts.flash.clone().unwrap();
        let csv = root.join(&flash.partitions);
        let chip = encode::chip(&profile.facts.espflash_chip).unwrap();
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

/// A compiler result for the real staged orchestrator, with no subprocess or
/// global compiler override. The mandatory audit must reject its flash code
/// before consulting any optional gate, encoding, or publishing the bundle.
#[test]
fn build_refuses_a_cached_bootstrap_even_with_no_or_reported_gates() {
    use crate::sizes::{Provenance, Summary};
    use std::collections::BTreeMap;

    for checks in [
        None,
        Some(crate::Gates {
            enforced: crate::Enforced::Reported,
            ..crate::Gates::all(oer_image_checks::interrupts::Required::Diagnostic)
        }),
    ] {
        let repository = repository();
        let root = repository.path();
        let profile = oer_chip_profile::Profile::load(root, "esp32s31").unwrap();
        let spec = ImageSpec {
            purpose: crate::bundle::Purpose::Analysis,
            root: root.into(),
            chip: profile.facts.id.clone(),
            application: crate::Application {
                package: "fixture".into(),
                binary: "fixture".into(),
                features: vec![],
                default_features: true,
            },
            stack_policy: profile.stack_policy(),
            layout_seed: None,
            runtime_environment: vec![],
            output: root.join("bundle"),
            checks,
        };
        let mut cached = elf();
        // The existing RV32 fixture's ET_EXEC header and `.text` address.
        cached[16..18].copy_from_slice(&2_u16.to_le_bytes());
        let address = profile.facts.memory.as_ref().unwrap().flash_xip.origin;
        cached[24..28].copy_from_slice(&address.to_le_bytes());
        let section_headers = u32::from_le_bytes(cached[32..36].try_into().unwrap()) as usize;
        let text_address = section_headers + 40 + 12;
        cached[text_address..text_address + 4].copy_from_slice(&address.to_le_bytes());
        let error = build(
            &spec,
            &profile,
            &Instrumentation::default(),
            false,
            &oer_image_checks::panics::Contract::default(),
            |input, _bundle| {
                let bootstrap_elf = input.output.join(files::BOOTSTRAP_ELF);
                std::fs::write(&bootstrap_elf, &cached)?;
                Ok(Compiled {
                    bootstrap_elf,
                    runtime_elf: input.output.join(files::RUNTIME_ELF),
                    values: crate::large_values::Report::new(input.policy.max_move_bytes),
                    sizes: Summary {
                        schema: crate::sizes::SCHEMA,
                        type_min_bytes: 0,
                        provenance: Provenance {
                            commit: String::new(),
                            dirty: false,
                            rustc: String::new(),
                            chip: spec.chip.clone(),
                            package: "fixture".into(),
                            binary: "fixture".into(),
                            target: profile.facts.rust_target.clone(),
                            profile: crate::FIRMWARE_PROFILE.into(),
                            profile_settings: String::new(),
                            features: vec![],
                            default_features: true,
                            purpose: spec.purpose,
                            layout_seed: None,
                            configuration_sha256: BTreeMap::new(),
                            stack_policy: String::new(),
                            stack_policy_sources: BTreeMap::new(),
                            flags: BTreeMap::new(),
                        },
                        elfs: BTreeMap::new(),
                        image_files: BTreeMap::new(),
                        embedded_payloads: vec![],
                    },
                })
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("bootstrap cache-off placement failed"),
            "{error}"
        );
        assert!(error.contains(".text"), "{error}");
        assert!(
            !spec.output.exists(),
            "an invalid bundle must not be published"
        );
        let staging = crate::bundle::staging_directory(&spec.output).unwrap();
        assert!(
            !staging.join(files::APPLICATION).exists(),
            "audit precedes encoding"
        );
        assert!(!staging.join(crate::bundle::MANIFEST).exists());
    }
}
