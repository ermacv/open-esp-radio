use super::*;

const CSV: &str = "# Name,Type,SubType,Offset,Size,Flags\n\
    nvs,data,nvs,0x9000,16K,encrypted:readonly\n\
    otadata,1,0,0xd000,8k,\n\
    phy_init,data,phy,0xf000,4K,\n\
    ota_0,app,ota_0,0x10000,1M,\n";

fn csv_file(text: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("isolated partition CSV");
    std::fs::write(file.path(), text).expect("write partition CSV fixture");
    file
}

fn table() -> Table {
    Table::read_csv(csv_file(CSV).path()).expect("valid partition CSV")
}

fn binary_file(bytes: &[u8]) -> Vec<u8> {
    let file = tempfile::NamedTempFile::new().expect("isolated binary partition table");
    std::fs::write(file.path(), bytes).expect("write binary partition fixture");
    std::fs::read(file.path()).expect("read binary partition fixture")
}

#[test]
fn an_independently_encoded_binary_fixture_is_read_and_written() {
    // A test-owned NVS entry encoded directly from the sourced field layout;
    // its MD5 was computed independently of this module with Python hashlib.
    let record = *b"\xaa\x50\x01\x02\x00\x90\x00\x00\x00\x40\x00\x00nvs\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00";
    let digest = *b"\x0b\xe0\xd6\x3f\xe9\x7d\x8c\x59\x5e\x47\x20\xe7\x7e\x7e\x0d\x81";
    let mut binary = record.to_vec();
    binary.extend_from_slice(&[0xeb, 0xeb]);
    binary.extend_from_slice(&[0xff; 14]);
    binary.extend_from_slice(&digest);
    binary.extend_from_slice(&[0xff; 32]);
    let decoded = Table::from_binary(&binary_file(&binary)).expect("independent binary fixture");
    let csv = csv_file("nvs,data,nvs,0x9000,16K,encrypted:readonly");
    assert_eq!(decoded, Table::read_csv(csv.path()).expect("same CSV data"));
    assert!(
        decoded
            .to_binary()
            .expect("encode independent fixture")
            .starts_with(&binary)
    );
}

#[test]
fn csv_and_binary_files_round_trip_all_consumed_partition_fields() {
    let original = table();
    let bytes = binary_file(&original.to_binary().expect("encode partition table"));
    let decoded = Table::from_binary(&bytes).expect("decode partition table");
    assert_eq!(decoded, original);
    assert_eq!(
        decoded.to_binary().expect("re-encode partition table"),
        bytes
    );
    let partitions = decoded.partitions().expect("partition projections");
    assert_eq!(
        partitions
            .iter()
            .map(|entry| (
                entry.name.as_str(),
                entry.offset,
                entry.size,
                entry.application,
                entry.otadata,
            ))
            .collect::<Vec<_>>(),
        [
            ("nvs", 0x9000, 0x4000, false, false),
            ("otadata", 0xd000, 0x2000, false, true),
            ("phy_init", 0xf000, 0x1000, false, false),
            ("ota_0", 0x10000, 0x100000, true, false),
        ]
    );
    let flags = decoded.entries.first().expect("NVS entry").flags;
    assert!(flags.encrypted());
    assert!(flags.readonly());
}

#[test]
fn binary_round_trip_retains_unknown_flag_bits_and_label_padding() {
    let mut table = table();
    let entry = table.entries.first_mut().expect("NVS entry");
    entry.flags = Flags::from_bytes([3, 0x42, 0x13, 0x80]);
    entry.label = *b"nvs\0padding-kept";
    let bytes = table.to_binary().expect("encode opaque fields");
    let decoded = Table::from_binary(&binary_file(&bytes)).expect("decode opaque fields");
    assert_eq!(decoded, table);
    assert_eq!(decoded.to_binary().expect("preserve opaque fields"), bytes);
}

#[test]
fn factory_numeric_subtypes_and_all_ota_slots_round_trip() {
    for subtype in std::iter::once("factory".to_owned())
        .chain((0..16).map(|slot| format!("ota_{slot}")))
        .chain(["0x20".to_owned(), "255".to_owned()])
    {
        let file = csv_file(&format!("app,0,{subtype},0x10000,1m"));
        let original = Table::read_csv(file.path()).expect("supported app subtype");
        assert_eq!(
            Table::from_binary(&original.to_binary().expect("encode app subtype"))
                .expect("decode app subtype"),
            original
        );
    }
}

#[test]
fn invalid_magic_and_unknown_type_are_typed_refusals() {
    let valid = table().to_binary().expect("valid table");
    let mut bad_magic = valid.clone();
    bad_magic[0] = 0;
    assert!(matches!(
        Table::from_binary(&binary_file(&bad_magic)),
        Err(Error::Magic(_))
    ));
    let mut bad_type = valid;
    bad_type[2] = 0x7f;
    assert!(matches!(
        Table::from_binary(&binary_file(&bad_type)),
        Err(Error::UnknownType(_))
    ));
}

#[test]
fn both_partition_and_md5_corruption_are_refused() {
    let table = table();
    let valid = table.to_binary().expect("valid table");
    for offset in [4, table.entries.len() * RECORD_BYTES + 16] {
        let mut corrupt = valid.clone();
        corrupt[offset] ^= 1;
        assert!(matches!(
            Table::from_binary(&binary_file(&corrupt)),
            Err(Error::Md5Mismatch)
        ));
    }
    let mut bad_prefix = valid;
    bad_prefix[table.entries.len() * RECORD_BYTES + 2] = 0;
    assert!(matches!(
        Table::from_binary(&bad_prefix),
        Err(Error::Md5Record)
    ));
}

#[test]
fn number_and_end_address_overflows_are_typed_refusals() {
    for row in [
        "nvs,data,nvs,0x100000000,0x1000",
        "nvs,data,nvs,0xfffff000,0x1000",
        "nvs,data,nvs,0x9000,4194304K",
        "nvs,data,nvs,0x9000,4096M",
        "app,app,256,0x10000,1M",
        "app,256,0,0x10000,1M",
    ] {
        let error = Table::read_csv(csv_file(row).path()).expect_err("overflow must fail");
        match error {
            Error::Overflow => {}
            Error::Csv { source, .. } => assert!(matches!(*source, Error::Overflow)),
            other => panic!("expected overflow, got {other}"),
        }
    }
    let mut input = table();
    let entry = input.entries.last_mut().expect("app entry");
    entry.offset = 0xffff0000;
    entry.size = 0x10000;
    let mut bytes = table().to_binary().expect("valid table");
    // Overflow is rejected while reading the binary, independently of its MD5.
    bytes[4..8].copy_from_slice(&0xfffff000_u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&0x1000_u32.to_le_bytes());
    let digest_offset = table().entries.len() * RECORD_BYTES;
    let digest = Md5::digest(&bytes[..digest_offset]);
    bytes[digest_offset + 16..digest_offset + RECORD_BYTES].copy_from_slice(&digest);
    assert!(matches!(
        Table::from_binary(&binary_file(&bytes)),
        Err(Error::Overflow)
    ));
    assert!(matches!(input.to_binary(), Err(Error::Overflow)));
}

#[test]
fn lengths_termination_and_trailing_data_are_checked() {
    let valid = table().to_binary().expect("valid table");
    let end_offset = (table().entries.len() + 1) * RECORD_BYTES;
    for length in 0..valid.len() {
        let result = Table::from_binary(&valid[..length]);
        if length <= end_offset || length % RECORD_BYTES != 0 {
            assert!(result.is_err(), "truncated length {length}");
        } else {
            result.expect("shorter erased padding is valid");
        }
    }
    assert!(matches!(
        Table::from_binary(&vec![0xff; TABLE_BYTES + RECORD_BYTES]),
        Err(Error::Length(_))
    ));
    let mut trailing = valid;
    trailing[TABLE_BYTES - 1] = 0;
    assert!(matches!(
        Table::from_binary(&trailing),
        Err(Error::TrailingData)
    ));
    assert!(matches!(
        Table::from_binary(&[0xff; RECORD_BYTES]),
        Err(Error::Empty)
    ));
}

#[test]
fn the_format_accepts_an_explicitly_checksum_free_table() {
    let table = table();
    let mut bytes = table.to_binary().expect("valid table");
    bytes[table.entries.len() * RECORD_BYTES..].fill(0xff);
    assert_eq!(
        Table::from_binary(&binary_file(&bytes)).expect("MD5 is optional in ESP-IDF"),
        table
    );
}

#[test]
fn duplicate_md5_or_a_partition_after_md5_is_refused() {
    let table = table();
    let mut bytes = table.to_binary().expect("valid table");
    let md5_offset = table.entries.len() * RECORD_BYTES;
    let next = md5_offset + RECORD_BYTES;
    bytes.copy_within(md5_offset..next, next);
    assert!(matches!(Table::from_binary(&bytes), Err(Error::Md5Record)));
    bytes.copy_within(..RECORD_BYTES, next);
    assert!(matches!(Table::from_binary(&bytes), Err(Error::Md5Record)));
}

#[test]
fn table_capacity_leaves_room_for_md5_and_the_end_marker() {
    let csv = (0..MAX_PARTITIONS)
        .map(|index| format!("p{index},data,0,{},4096\n", 0x9000 + index * 4096))
        .collect::<String>();
    let table = Table::read_csv(csv_file(&csv).path()).expect("maximum table");
    assert_eq!(
        Table::from_binary(&table.to_binary().expect("maximum table fits"))
            .expect("maximum table decodes"),
        table
    );
    let oversized = format!(
        "{csv}extra,data,0,{},4096\n",
        0x9000 + MAX_PARTITIONS * 4096
    );
    assert!(matches!(
        Table::read_csv(csv_file(&oversized).path()),
        Err(Error::TooMany)
    ));
}

#[test]
fn invalid_csv_and_non_utf8_files_return_errors() {
    for text in [
        "",
        "# only a comment",
        "name,data",
        "name,data,nvs,,4K",
        "name,data,nvs,0x9000,",
        "name,unknown,nvs,0x9000,4K",
        "name,data,unknown,0x9000,4K",
        "app,app,ota_16,0x10000,1M",
        "name,data,nvs,0x9000,4K,bogus",
        "abcdefghijklmnopq,data,nvs,0x9000,4K",
        "name\0suffix,data,nvs,0x9000,4K",
        "name,data,nvs,0x9001,4K",
        "app,app,factory,0x9000,1M",
        "name,data,nvs,0x9000,0",
        "name,data,nvs,0x9000,4K,,",
        "same,data,nvs,0x9000,8K\nother,data,ota,0xa000,8K",
        "same,data,nvs,0x9000,4K\nsame,data,ota,0xa000,8K",
    ] {
        assert!(
            Table::read_csv(csv_file(text).path()).is_err(),
            "invalid CSV {text:?}"
        );
    }
    let file = csv_file(CSV);
    std::fs::write(file.path(), [0xff]).expect("non-UTF-8 file");
    assert!(matches!(
        Table::read_csv(file.path()),
        Err(Error::Read { .. })
    ));
    let missing = file.path().with_extension("missing");
    assert!(matches!(Table::read_csv(&missing), Err(Error::Read { .. })));
}

#[test]
fn invalid_binary_labels_are_refused_without_panicking() {
    let mut invalid = table();
    invalid.entries.first_mut().expect("NVS entry").label = [0xff; 16];
    assert!(matches!(invalid.to_binary(), Err(Error::Label)));
    let mut bytes = table().to_binary().expect("valid table");
    bytes[12] = 0xff;
    let digest_offset = table().entries.len() * RECORD_BYTES;
    let digest = Md5::digest(&bytes[..digest_offset]);
    bytes[digest_offset + 16..digest_offset + RECORD_BYTES].copy_from_slice(&digest);
    assert!(matches!(
        Table::from_binary(&binary_file(&bytes)),
        Err(Error::Label)
    ));
    let original = Table::from_csv("abcdefghijklmnop,data,0xff,0x9000,4K").expect("16-byte name");
    assert_eq!(
        Table::from_binary(&original.to_binary().expect("encode full label"))
            .expect("decode full label"),
        original
    );
}
