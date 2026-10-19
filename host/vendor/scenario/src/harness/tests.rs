//! Host regressions for request preparation; no hardware or private inputs.
use super::*;
use oer_probe_codegen::{Argument, Entry};

#[test]
fn signed_words_and_explicit_padding() {
    assert_eq!(abi_word("i8", Some(-128)).unwrap(), Some(0xffff_ff80));
    assert_eq!(abi_word("u16", Some(65535)).unwrap(), Some(65535));
    assert_eq!(abi_word("u32", None).unwrap(), None);
    assert_eq!(abi_word("bool", Some(1)).unwrap(), Some(1));
    assert_eq!(abi_word("bool", Some(0)).unwrap(), Some(0));
    assert_eq!(
        abi_word("&mut [u8; 4]", Some(0x1000)).unwrap(),
        Some(0x1000)
    );
    assert_eq!(words_padded(&[7], 2, 0).unwrap(), [7, 0, 0, 0, 0, 0, 0, 0]);
    for (kind, value) in [
        ("i8", 128),
        ("u32", -1),
        ("u64", 1),
        ("Private", 0),
        ("bool", 2),
    ] {
        assert!(abi_word(kind, Some(value)).is_err(), "{kind} {value}");
    }
    assert!(words_padded(&[1, 2], 1, 0).is_err());
}

#[test]
fn phase_building_preserves_unknown_memory() {
    let memory = region(0x1000, 4, &[1], None, RegionLifetime::Session).unwrap();
    assert_eq!(memory.seed.fill, None);
    let right = invocation(20, vec![None], vec![memory.clone()], vec![], vec![]);
    let row = case(
        "warm",
        invocation(10, vec![None], vec![memory], vec![], vec![]),
        Some(right),
        SessionReset::Warm,
        true,
    );
    assert_eq!(row.reset, SessionReset::Warm);
    assert_eq!(
        row.relation.unwrap().memory,
        [MemoryPair {
            vendor: 0,
            replacement: 0
        }]
    );
    assert!(region(0xffff_ffff, 4, &[], None, RegionLifetime::Phase).is_err());
    assert!(region(0, 1, &[1, 2], None, RegionLifetime::Phase).is_err());
}

fn catalog(entries: Vec<Entry>) -> Catalog {
    Catalog {
        schema: 1,
        image: "fixture".into(),
        entries,
    }
}

fn entry(symbol: &str, arguments: &[(&str, &str)]) -> Entry {
    Entry {
        symbol: symbol.into(),
        signature: "example".into(),
        adapter: "body".into(),
        arguments: arguments
            .iter()
            .map(|(name, rust_type)| Argument {
                name: (*name).into(),
                rust_type: (*rust_type).into(),
            })
            .collect(),
    }
}

#[test]
fn catalog_requires_unique_declared_entries_and_exact_arguments() {
    let probes =
        ProbeCatalog::new(catalog(vec![entry("sample", &[("a", "i8")])]), |_| Ok(32)).unwrap();
    assert_eq!(probes.entry("sample").unwrap(), 32);
    assert_eq!(
        probes.arguments("sample", &[("a", Some(-1))]).unwrap(),
        [Some(0xffff_ffff)]
    );
    assert!(probes.entry("undeclared").is_err());
    assert!(probes.arguments("sample", &[("b", Some(0))]).is_err());
    assert!(
        probes
            .arguments("sample", &[("a", Some(0)), ("a", Some(1))])
            .is_err()
    );
    let duplicate = catalog(vec![entry("sample", &[]), entry("sample", &[])]);
    assert!(ProbeCatalog::new(duplicate, |_| Ok(32)).is_err());
    let mut wrong_schema = catalog(vec![entry("sample", &[])]);
    wrong_schema.schema = 2;
    assert!(ProbeCatalog::new(wrong_schema, |_| Ok(32)).is_err());
    assert!(ProbeCatalog::new(catalog(vec![entry("1bad", &[])]), |_| Ok(32)).is_err());
    // Every declaration resolves, not only entries a scenario selects.
    assert!(
        ProbeCatalog::new(catalog(vec![entry("sample", &[])]), |_| Err(invalid(
            "absent"
        )))
        .is_err()
    );
}

#[test]
fn catalog_prepares_buffers_without_inventing_private_layouts() {
    let probes = |kind: &str| {
        ProbeCatalog::new(catalog(vec![entry("sample", &[("output", kind)])]), |_| {
            Ok(32)
        })
        .unwrap()
    };
    let phase = probes("&mut [[u16; 4]; 3]")
        .invoke(
            "sample",
            vec![("output", Buffer::new(0x1000, [1u8, 2]).session().into())],
            vec![],
            vec![],
        )
        .unwrap();
    assert_eq!(phase.arguments, [Some(0x1000)]);
    assert_eq!(
        phase.memory,
        [region(0x1000, 24, &[1, 2], None, RegionLifetime::Session).unwrap()]
    );
    assert_eq!(phase.entry, 32);
    // Direct entry pads unused argument registers with zero and keeps stack words.
    let entered = direct(32, &[7], vec![], vec![], vec![]);
    assert_eq!(entered.entry, 32);
    assert_eq!(entered.arguments[..2], [Some(7), Some(0)]);
    assert_eq!(entered.arguments.len(), 8);
    assert_eq!(
        direct(32, &[1; 10], vec![], vec![], vec![]).arguments.len(),
        10
    );
    let sample = probes("&mut [[u16; 4]; 3]");
    assert!(
        sample
            .invoke(
                "sample",
                vec![("output", Buffer::new(0x1001, []).into())],
                vec![],
                vec![]
            )
            .is_err()
    );
    assert!(
        sample
            .invoke(
                "sample",
                vec![("output", Buffer::new(0x1000, [0u8; 25]).into())],
                vec![],
                vec![]
            )
            .is_err()
    );
    assert!(
        probes("&mut PrivateOwner")
            .invoke(
                "sample",
                vec![("output", Buffer::new(0x1000, []).into())],
                vec![],
                vec![]
            )
            .is_err()
    );
    let paired = ProbeCatalog::new(
        catalog(vec![entry(
            "paired",
            &[("left", "&mut [u8; 8]"), ("right", "&mut [u8; 8]")],
        )]),
        |_| Ok(32),
    )
    .unwrap();
    let overlap = vec![
        ("left", Buffer::new(0x1000, []).into()),
        ("right", Buffer::new(0x1004, []).into()),
    ];
    assert!(paired.invoke("paired", overlap, vec![], vec![]).is_err());
}

#[test]
fn authentication_precedes_capture() {
    crate::install(&crate::chip::TEST);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("source");
    fs::write(&source, b"changed input").unwrap();
    let inputs = [Input {
        role: "phy",
        path: &source,
        sha256: Some("wrong-hash"),
    }];
    assert!(authenticate(&inputs).is_err());
}

#[test]
fn a_missing_input_names_its_role_and_path() {
    crate::install(&crate::chip::TEST);
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("absent.elf");
    let inputs = [Input {
        role: "phy-sdk",
        path: &missing,
        sha256: None,
    }];
    let error = authenticate(&inputs).err().unwrap().to_string();
    assert!(error.contains("phy-sdk: cannot read"), "{error}");
    assert!(error.contains("absent.elf"), "{error}");
}
