use super::*;
use std::path::PathBuf;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// This checkout's first chip; a catalog image of it boots through its own
/// ESP-IDF bootloader.
fn application_chip() -> String {
    oer_chip_profile::Profile::all(&repository()).unwrap()[0]
        .id
        .clone()
}

/// Boot files of [`application_chip`] as a catalog build gives them: an ESP
/// image header of the chip standing in for its bootloader, and the binary
/// form of the chip's partition table.
fn boot_files(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let profile = oer_chip_profile::Profile::load(root, &application_chip()).unwrap();
    let chip = oer_image_encode::chip(&profile.espflash_chip).unwrap();
    let mut bootloader = vec![0; 24];
    bootloader[0] = 0xe9;
    bootloader[1] = 1;
    bootloader[12..14].copy_from_slice(&chip.id().to_le_bytes());
    let table =
        oer_image_encode::partition_table(&root.join(profile.flash.unwrap().partitions)).unwrap();
    (bootloader, table)
}

#[test]
fn boot_files_of_another_chip_or_no_boot_files_are_refused() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let (mut bootloader, table) = boot_files(&root);
    bootloader[12] ^= 0x40;
    let foreign = file("foreign.bin", &bootloader);
    let table = file("table.bin", &table);
    let output = directory.path().join("bundle");
    let error = around(
        &root,
        &application_chip(),
        &application,
        BootFiles::Given {
            bootloader: &foreign,
            partition_table: &table,
        },
        &output,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("chip id"), "{error}");
    // Nothing was staged or published.
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
}

#[test]
fn an_esp_idf_bundle_takes_its_catalog_boot_files_and_has_no_ota_selection() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let (bootloader_bytes, table_bytes) = boot_files(&root);
    let bootloader = file("bootloader.bin", &bootloader_bytes);
    let table = file("partition-table.bin", &table_bytes);
    let output = directory.path().join("bundle");
    let bundle = around(
        &root,
        &application_chip(),
        &application,
        BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &table,
        },
        &output,
    )
    .unwrap();
    assert_eq!(bundle.boot, Boot::EspIdfBootloader);
    assert_eq!(bundle.otadata(), None);
    let segments = bundle.segments();
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.offset, std::fs::read(&segment.file).unwrap()))
            .collect::<Vec<_>>(),
        [
            (0x2000, bootloader_bytes),
            (0x8000, table_bytes),
            (0x1_0000, b"application".to_vec()),
        ]
    );
    // A loaded bundle is the written one; one without its files is refused.
    assert_eq!(ImageBundle::load(&output).unwrap(), bundle);
    std::fs::remove_file(bundle.partitions()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(error.contains("partition table"), "{error}");
    let elf = file("app.elf", b"");
    assert!(
        around(
            &root,
            &application_chip(),
            &application,
            BootFiles::Encode { elf: &elf },
            &directory.path().join("other"),
        )
        .is_err()
    );
}

#[test]
fn a_run_flashes_a_copy_holding_exactly_its_archived_application() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let (bootloader_bytes, table_bytes) = boot_files(&root);
    let bootloader = file("bootloader.bin", &bootloader_bytes);
    let table = file("table.bin", &table_bytes);
    let built = around(
        &root,
        &application_chip(),
        &application,
        BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &table,
        },
        &directory.path().join("built"),
    )
    .unwrap();
    let archived = file("archived.bin", b"application");
    let copy = built
        .with_application(&archived, &directory.path().join("run"))
        .unwrap();
    assert_eq!(std::fs::read(copy.application()).unwrap(), b"application");
    assert_eq!(std::fs::read(copy.bootloader()).unwrap(), bootloader_bytes);
    assert_eq!(ImageBundle::load(&copy.directory).unwrap(), copy);
    let other = file("other.bin", b"other");
    assert!(
        built
            .with_application(&other, &directory.path().join("refused"))
            .is_err()
    );
}

#[test]
fn a_failed_rebuild_leaves_the_published_bundle_untouched() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let (bootloader_bytes, table_bytes) = boot_files(&root);
    let bootloader = file("bootloader.bin", &bootloader_bytes);
    let table = file("table.bin", &table_bytes);
    let output = directory.path().join("bundle");
    let published = around(
        &root,
        &application_chip(),
        &application,
        BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &table,
        },
        &output,
    )
    .unwrap();
    // The rebuild copies its new application, then fails encoding the boot
    // files of an empty ELF.
    let replacement = file("new.bin", b"replacement");
    let elf = file("app.elf", b"");
    assert!(
        around(
            &root,
            &application_chip(),
            &replacement,
            BootFiles::Encode { elf: &elf },
            &output,
        )
        .is_err()
    );
    assert_eq!(ImageBundle::load(&output).unwrap(), published);
    assert_eq!(
        std::fs::read(published.application()).unwrap(),
        b"application"
    );
}
