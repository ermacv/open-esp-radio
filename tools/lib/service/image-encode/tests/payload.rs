use oer_image_encode::stage_two::{pack_runtime, payload_crc32};

/// A stage-two contract shaped like a staged chip's profile.
fn contract() -> oer_chip_profile::StageTwo {
    oer_chip_profile::StageTwo {
        magic: 0x3247_5453,
        abi_version: 1,
        header_bytes: 44,
        magic_offset: 0,
        abi_version_offset: 4,
        header_size_offset: 28,
        crc_offset: 40,
    }
}

fn put(image: &mut [u8], offset: u32, word: u32) {
    let at = offset as usize;
    image[at..at + 4].copy_from_slice(&word.to_le_bytes());
}

fn image() -> Vec<u8> {
    let contract = contract();
    let mut image = vec![0x5a; 128];
    image[..contract.header_bytes as usize].fill(0);
    put(&mut image, contract.magic_offset, contract.magic);
    put(
        &mut image,
        contract.abi_version_offset,
        contract.abi_version,
    );
    put(
        &mut image,
        contract.header_size_offset,
        contract.header_bytes,
    );
    image
}

#[test]
fn packing_stores_the_payload_crc_and_is_repeatable() {
    let contract = contract();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    let image = image();
    std::fs::write(&path, &image).unwrap();
    let checksum = pack_runtime(&contract, &path).unwrap();
    let packed = std::fs::read(&path).unwrap();
    assert_eq!(checksum, pack_runtime(&contract, &path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), packed);
    let at = contract.crc_offset as usize;
    assert_eq!(packed[at..at + 4], checksum.to_le_bytes());
    assert_eq!(packed[..at], image[..at]);
    assert_eq!(packed[at + 4..], image[at + 4..]);
    assert_eq!(payload_crc32(&packed, contract.crc_offset), checksum);
}

#[test]
fn malformed_payload_is_rejected_without_rewriting_it() {
    let contract = contract();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    let mut incompatible = image();
    put(
        &mut incompatible,
        contract.abi_version_offset,
        contract.abi_version + 1,
    );
    for image in [vec![], vec![0x5a; 128], incompatible] {
        std::fs::write(&path, &image).unwrap();
        assert!(pack_runtime(&contract, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), image);
    }
}

#[test]
fn a_field_outside_the_header_is_a_contract_error_not_a_panic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    std::fs::write(&path, image()).unwrap();
    for broken in [
        oer_chip_profile::StageTwo {
            crc_offset: 42,
            ..contract()
        },
        oer_chip_profile::StageTwo {
            crc_offset: u32::MAX,
            ..contract()
        },
        oer_chip_profile::StageTwo {
            magic_offset: 1 << 20,
            ..contract()
        },
    ] {
        let error = pack_runtime(&broken, &path).unwrap_err().to_string();
        assert!(error.contains("outside"), "{error}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), image());
}

/// The fixture line `key` of the stage-two packing fixture the bootstrap's
/// contract (`oer-espressif-staged-layout`) checks too.
fn fixture(key: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::read_to_string(root.join("platform/espressif/staged-layout/fixtures/stage-two.txt"))
        .unwrap()
        .lines()
        .find_map(|line| Some(line.strip_prefix(key)?.strip_prefix(' ')?.to_owned()))
        .unwrap()
}

#[test]
fn every_staged_chip_packs_the_shared_fixture_as_the_bootstrap_checks_it() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let hex = fixture("unpacked");
    let unpacked = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect::<Vec<_>>();
    let crc = u32::from_str_radix(&fixture("crc32"), 16).unwrap();
    let staged = oer_chip_profile::Profile::all(&root)
        .unwrap()
        .into_iter()
        .filter_map(|profile| profile.staged)
        .collect::<Vec<_>>();
    assert!(!staged.is_empty());
    for staged in staged {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.bin");
        std::fs::write(&path, &unpacked).unwrap();
        assert_eq!(pack_runtime(&staged.stage_two, &path).unwrap(), crc);
        let packed = std::fs::read(&path).unwrap();
        let at = staged.stage_two.crc_offset as usize;
        assert_eq!(packed[at..at + 4], crc.to_le_bytes());
    }
}
