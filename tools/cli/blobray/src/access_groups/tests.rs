use blobray_domain::{LibraryFunction, RegisterAccess, RegisterMask, RegisterMaskKind};
use oer_riscv_model::{
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
    assert_eq!(
        groups.human(),
        [
            "unresolved",
            "  phy_set_rx_gain_table (input 0, member 0) load 4B x1",
            "0x20107094",
            "  rx_init_gain (input 0, member 0) store 4B x1",
            "0x2010713c",
            "  phy_set_rx_gain_table (input 0, member 0) store 4B replace 0x01fc0000 x1",
            "  rx_init_gain (input 0, member 0) load 4B x1",
            "  rx_init_gain (input 0, member 0) store 4B replace 0x01fc0000 x1",
        ]
    );
}

#[test]
fn function_groups_list_each_accessed_word() {
    let mut groups = AccessGroups::new(GroupBy::Function);
    records().iter().for_each(|record| groups.add(record));
    assert_eq!(
        groups.human(),
        [
            "phy_set_rx_gain_table (input 0, member 0)",
            "  unresolved load 4B x1",
            "  0x2010713c store 4B replace 0x01fc0000 x1",
            "rx_init_gain (input 0, member 0)",
            "  0x20107094 store 4B x1",
            "  0x2010713c load 4B x1",
            "  0x2010713c store 4B replace 0x01fc0000 x1",
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
    assert_eq!(
        groups.human(),
        [
            "local_helper (input 0, member 2)",
            "  0x2010702c store 4B x1",
            "local_helper (input 0, member 5)",
            "  0x2010702c store 4B x2",
        ]
    );
}

#[test]
fn stored_runs_render_high_bits_first() {
    use super::stored_human;
    use blobray_domain::{StoredBits, StoredBitsSource};
    let load = |low, width, same_word| StoredBits {
        low,
        width,
        source: StoredBitsSource::Load {
            address: Some(0x2010_9004),
            width: 4,
            low,
            same_word,
        },
    };
    assert_eq!(
        stored_human(&[
            load(0, 18, true),
            StoredBits {
                low: 18,
                width: 7,
                source: StoredBitsSource::EntryRegister {
                    register: 10,
                    low: 0
                },
            },
            StoredBits {
                low: 25,
                width: 1,
                source: StoredBitsSource::Constant { value: 1 },
            },
            StoredBits {
                low: 26,
                width: 2,
                source: StoredBitsSource::EntryRegister {
                    register: 8,
                    low: 4
                },
            },
            load(28, 2, false),
            StoredBits {
                low: 30,
                width: 2,
                source: StoredBitsSource::Unknown,
            },
        ]),
        "[31:30]=? [29:28]=load 4B 0x20109004[29:28] [27:26]=entry x8[5:4] [25]=0x1 [24:18]=arg0[6:0] [17:0]=kept"
    );
}

#[test]
fn loads_of_different_widths_stay_separate_entries() {
    use blobray_domain::{StoredBits, StoredBitsSource};
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
    assert_eq!(
        groups.human(),
        [
            "store_byte (input 0, member 0)",
            "  0x2010702c store 4B [7:0]=load 1B 0xa[7:0] x1",
            "  0x2010702c store 4B [7:0]=load 4B 0xa[7:0] x1",
        ]
    );
}

#[test]
fn an_indexed_observation_is_selected_by_the_words_its_progression_reaches() {
    use blobray_domain::IndexedAddress;
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
    assert_eq!(
        groups.human(),
        [
            "unresolved",
            "  coex_hw_timer_set (input 0, member 0) indexed 0x2010f400+0x10*i (i<6) store 4B x1",
        ]
    );
}
