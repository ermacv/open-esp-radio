use oer_analysis_blobray_domain::{
    LibraryFunction, RegisterAccess, RegisterMask, RegisterMaskKind,
};
use oer_analysis_riscv_model::{
    AbstractValue, ArtifactId, Error, ErrorCode, FunctionRecord, MemoryKind, ObjectId,
    ObjectLocation, SymbolId, SymbolTableKind,
};

use super::{AccessEntry, AccessFilter, AccessGroup, AccessGroups, GroupBy, GroupFunction};

fn function(name: &str) -> LibraryFunction {
    LibraryFunction {
        input: 0,
        symbol: SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"library"),
                location: ObjectLocation::ArchiveMember { ordinal: 0 },
            },
            table: SymbolTableKind::Static,
            table_section: 1,
            index: 1,
        },
        name: Some(name.as_bytes().to_vec()),
    }
}

fn observation(
    name: &str,
    access: MemoryKind,
    address: Option<u32>,
    mask: Option<RegisterMask>,
) -> RegisterAccess {
    RegisterAccess::Observation {
        function: function(name),
        record: 0,
        fact: Box::new(FunctionRecord::MemoryAccess {
            offset: 0,
            access,
            width: 4,
            address: address.map_or(AbstractValue::Unknown, |value| AbstractValue::Constant {
                value,
            }),
            value: None,
            relocation: None,
        }),
        address,
        alternative: None,
        indexed: None,
        stored: None,
        reads: None,
        mask,
    }
}

const REPLACE: Option<RegisterMask> = Some(RegisterMask {
    kind: RegisterMaskKind::WriteReplacement,
    bits: 0x01fc_0000,
});

fn records() -> Vec<RegisterAccess> {
    vec![
        observation("rx_init_gain", MemoryKind::Load, Some(0x2010_713c), None),
        observation(
            "rx_init_gain",
            MemoryKind::Store,
            Some(0x2010_713c),
            REPLACE,
        ),
        observation("rx_init_gain", MemoryKind::Store, Some(0x2010_7094), None),
        observation(
            "phy_set_rx_gain_table",
            MemoryKind::Store,
            Some(0x2010_713e),
            REPLACE,
        ),
        observation("phy_set_rx_gain_table", MemoryKind::Load, None, None),
        RegisterAccess::Blocked {
            function: function("blocked_leaf"),
            error: Error::new(ErrorCode::InvalidRequest, "needs-extent"),
        },
        RegisterAccess::Gap {
            input: 0,
            object: None,
            reason: "thin member".into(),
        },
    ]
}

#[test]
fn a_word_filter_keeps_its_observations_and_every_blocked_function_and_gap() {
    let filter = AccessFilter {
        words: vec![0x2010_713c],
        functions: Vec::new(),
    };
    let kept: Vec<_> = records().into_iter().filter(|r| filter.keeps(r)).collect();
    assert_eq!(
        kept.len(),
        5,
        "three observations in the word, the blocked function and the gap"
    );
    assert!(kept.iter().all(|record| match record {
        RegisterAccess::Observation { address, .. } =>
            *address == Some(0x2010_713c) || *address == Some(0x2010_713e),
        _ => true,
    }));
}

#[test]
fn a_function_filter_keeps_only_that_functions_observations_and_blocks() {
    let filter = AccessFilter {
        words: Vec::new(),
        functions: vec!["phy_set_rx_gain_table".into()],
    };
    let kept: Vec<_> = records().into_iter().filter(|r| filter.keeps(r)).collect();
    assert_eq!(
        kept.len(),
        3,
        "two observations, unresolved included, and the gap"
    );
    assert!(
        !kept
            .iter()
            .any(|record| matches!(record, RegisterAccess::Blocked { .. }))
    );
}

#[test]
fn address_groups_list_each_accessing_function_by_access_and_mask() {
    let mut groups = AccessGroups::new(GroupBy::Address);
    records().iter().for_each(|record| groups.add(record));
    let entry = |name: &str, access: &str, mask, count| AccessEntry {
        word: None,
        indexed: None,
        function: Some(GroupFunction {
            input: 0,
            name: name.into(),
            symbol: function(name).symbol,
        }),
        access: access.into(),
        width: Some(4),
        mask,
        stored: None,
        reads: None,
        count,
    };
    assert_eq!(
        groups.groups(),
        [
            AccessGroup {
                word: None,
                function: None,
                entries: vec![entry("phy_set_rx_gain_table", "load", None, 1)],
            },
            AccessGroup {
                word: Some(0x2010_7094),
                function: None,
                entries: vec![entry("rx_init_gain", "store", None, 1)],
            },
            AccessGroup {
                word: Some(0x2010_713c),
                function: None,
                entries: vec![
                    entry("phy_set_rx_gain_table", "store", REPLACE, 1),
                    entry("rx_init_gain", "load", None, 1),
                    entry("rx_init_gain", "store", REPLACE, 1),
                ],
            },
        ]
    );
}

#[test]
fn function_groups_list_each_accessed_word() {
    let mut groups = AccessGroups::new(GroupBy::Function);
    records().iter().for_each(|record| groups.add(record));
    let groups = groups.groups();
    assert_eq!(
        groups
            .iter()
            .map(|g| g.function.as_ref().unwrap().name.as_str())
            .collect::<Vec<_>>(),
        ["phy_set_rx_gain_table", "rx_init_gain"]
    );
    assert_eq!(
        groups[0]
            .entries
            .iter()
            .map(|e| (e.word, e.access.as_str(), e.count))
            .collect::<Vec<_>>(),
        [(None, "load", 1), (Some(0x2010_713c), "store", 1)]
    );
    assert_eq!(
        groups[1]
            .entries
            .iter()
            .map(|e| (e.word, e.access.as_str(), e.count))
            .collect::<Vec<_>>(),
        [
            (Some(0x2010_7094), "store", 1),
            (Some(0x2010_713c), "load", 1),
            (Some(0x2010_713c), "store", 1)
        ]
    );
}

#[test]
fn same_named_functions_of_different_members_stay_apart() {
    let in_member = |ordinal| {
        let mut record = observation("local_helper", MemoryKind::Store, Some(0x2010_702c), None);
        if let RegisterAccess::Observation { function, .. } = &mut record {
            function.symbol.object.location = ObjectLocation::ArchiveMember { ordinal };
        }
        record
    };
    let mut groups = AccessGroups::new(GroupBy::Function);
    for record in [in_member(2), in_member(5), in_member(5)] {
        groups.add(&record);
    }
    let groups = groups.groups();
    assert_eq!(
        groups
            .iter()
            .map(|g| (
                g.function.as_ref().unwrap().symbol.object.location.clone(),
                g.entries[0].count
            ))
            .collect::<Vec<_>>(),
        [
            (ObjectLocation::ArchiveMember { ordinal: 2 }, 1),
            (ObjectLocation::ArchiveMember { ordinal: 5 }, 2)
        ]
    );
}

#[test]
fn loads_of_different_widths_stay_separate_entries() {
    use oer_analysis_blobray_domain::{StoredBits, StoredBitsSource};
    let store = |width| {
        let mut record = observation("store_byte", MemoryKind::Store, Some(0x2010_702c), None);
        if let RegisterAccess::Observation { stored, .. } = &mut record {
            *stored = Some(vec![StoredBits {
                low: 0,
                width: 8,
                source: StoredBitsSource::Load {
                    address: Some(0xa),
                    width,
                    low: 0,
                    same_word: false,
                },
            }]);
        }
        record
    };
    let mut groups = AccessGroups::new(GroupBy::Function);
    for record in [store(1), store(4)] {
        groups.add(&record);
    }
    let groups = groups.groups();
    assert_eq!(groups.len(), 1);
    let entries = &groups[0].entries;
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0].stored, entries[1].stored);
    assert_eq!((entries[0].count, entries[1].count), (1, 1));
}

#[test]
fn an_indexed_observation_is_selected_by_the_words_its_progression_reaches() {
    use oer_analysis_blobray_domain::IndexedAddress;
    let mut record = observation("coex_hw_timer_set", MemoryKind::Store, None, None);
    if let RegisterAccess::Observation { indexed, .. } = &mut record {
        *indexed = Some(IndexedAddress {
            base: 0x2010_f400,
            stride: 0x10,
            count: Some(6),
        });
    }
    let filter = |word| AccessFilter {
        words: vec![word],
        functions: vec![],
    };
    assert!(filter(0x2010_f450).keeps(&record), "index five");
    assert!(
        !filter(0x2010_f460).keeps(&record),
        "index six is out of bound"
    );
    assert!(!filter(0x2010_f404).keeps(&record), "between elements");
    let mut groups = AccessGroups::new(GroupBy::Address);
    groups.add(&record);
    let groups = groups.groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].word, None);
    assert_eq!(
        groups[0].entries[0].indexed,
        Some(IndexedAddress {
            base: 0x2010_f400,
            stride: 0x10,
            count: Some(6)
        })
    );
}

#[test]
fn read_classes_stay_separate_and_render_sinks_precision_and_limits() {
    use oer_analysis_blobray_domain::{
        ReadLimit, ReadLimitKind, ReadPrecision, ReadSink, ReadUse, ReadUses,
    };
    let mut active = observation("read_bits", MemoryKind::Load, Some(0x2010_702c), None);
    let mut unused = active.clone();
    if let RegisterAccess::Observation { reads, .. } = &mut active {
        *reads = Some(ReadUses {
            uses: vec![
                ReadUse {
                    bits: 1,
                    sink: ReadSink::Branch { site: 32 },
                    precision: ReadPrecision::Exact,
                },
                ReadUse {
                    bits: 8,
                    sink: ReadSink::Store {
                        site: 48,
                        address: Some(0x2010_702c),
                        same_word: true,
                    },
                    precision: ReadPrecision::May,
                },
            ],
            unknown: vec![ReadLimit {
                bits: 2,
                limit: ReadLimitKind::StackSpill,
            }],
            preserved: 4,
            unused: !15,
        });
    }
    if let RegisterAccess::Observation { reads, .. } = &mut unused {
        *reads = Some(ReadUses {
            uses: Vec::new(),
            unknown: Vec::new(),
            preserved: 0,
            unused: u32::MAX,
        });
    }
    let mut groups = AccessGroups::new(GroupBy::Function);
    groups.add(&active);
    groups.add(&active);
    groups.add(&unused);
    assert_eq!(
        groups
            .groups()
            .first()
            .expect("function group")
            .entries
            .len(),
        2
    );

    let wire = serde_json::to_value(groups.groups()).expect("group JSON");
    let decoded: Vec<AccessGroup> = serde_json::from_value(wire).expect("group roundtrip");
    assert_eq!(decoded, groups.groups());
}
