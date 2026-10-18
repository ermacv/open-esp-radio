use super::*;

fn field(name: &str, offset: u32, width: u32) -> DeclaredField {
    DeclaredField {
        name: name.into(),
        offset,
        width,
    }
}

fn register(
    address: u64,
    width_bits: u32,
    name: &str,
    fields: Vec<DeclaredField>,
) -> DeclaredRegister {
    DeclaredRegister {
        address,
        width_bits,
        name: name.into(),
        template: name.trim_end_matches(|c: char| c.is_ascii_digit()).into(),
        fields,
    }
}

fn function(symbol: &str) -> Function {
    ("libx".into(), symbol.into())
}

fn access(symbol: &str, address: u32, access: Access, bits: Option<u32>) -> Observation {
    Observation {
        function: function(symbol),
        target: Target::Address(address),
        access,
        bits,
    }
}

fn region() -> Vec<Region> {
    vec![Region {
        name: "radio".into(),
        start: 0x1000,
        end: 0x1100,
    }]
}

#[test]
fn declared_words_split_wide_registers_and_mark_opaque_bits() {
    let words = declared_words(&[
        register(
            0x1000,
            64,
            "WIDE",
            vec![field("LOW", 4, 4), field("HIGH", 36, 2)],
        ),
        register(
            0x1008,
            32,
            "CONTROL",
            vec![field("MODE", 0, 2), field("UNKNOWN_OPAQUE", 8, 4)],
        ),
        register(0x100e, 16, "LANE_OPAQUE", vec![field("VALUE", 0, 4)]),
    ]);
    assert_eq!(words[&0x1000].fields, 0xf0);
    assert_eq!(words[&0x1004].fields, 0x30);
    assert!(words[&0x1004].registers.contains("WIDE"));
    assert_eq!(words[&0x1008].opaque, 0xf00);
    // A half-word register occupies the upper lane, and its register name
    // makes every field opaque.
    assert_eq!(words[&0x100c].fields, 0xf_0000);
    assert_eq!(words[&0x100c].opaque, 0xf_0000);
}

#[test]
fn classify_reports_each_status_and_ignores_whole_word_masks() {
    let declared = declared_words(&[
        register(0x1000, 32, "A", vec![field("F", 0, 8)]),
        register(0x1004, 32, "B", vec![field("F", 0, 8)]),
        register(0x1008, 32, "C", vec![field("F_OPAQUE", 0, 8)]),
    ]);
    let observations = [
        access("full", 0x1000, Access::Store, Some(u32::MAX)),
        access("field", 0x1000, Access::Expression, Some(0x0f)),
        access("wide", 0x1004, Access::Expression, Some(0x1ff)),
        access("opaque", 0x1008, Access::Load, None),
        access("absent", 0x1010, Access::Load, None),
        access("outside", 0x2000, Access::Store, None),
    ];
    let words = classify(&region(), &declared, &[], &observations, &BTreeSet::new());
    let status: BTreeMap<u32, Status> = words.iter().map(|w| (w.address, w.status)).collect();
    assert_eq!(
        status,
        BTreeMap::from([
            (0x1000, Status::Declared),
            (0x1004, Status::UndeclaredBits),
            (0x1008, Status::Opaque),
            (0x1010, Status::Unmodeled),
        ])
    );
    let first = &words[0];
    assert!(first.whole_word);
    assert_eq!(first.field_bits, 0x0f);
    assert_eq!(first.stores, 1);
    assert_eq!(words[1].undeclared_bits, 0x100);
}

#[test]
fn byte_lane_masks_shift_into_the_word() {
    let declared = declared_words(&[register(0x1000, 32, "A", vec![field("F", 16, 8)])]);
    let words = classify(
        &region(),
        &declared,
        &[],
        &[access("lane", 0x1002, Access::Expression, Some(0xff))],
        &BTreeSet::new(),
    );
    assert_eq!(words[0].address, 0x1000);
    assert_eq!(words[0].field_bits, 0xff_0000);
    assert_eq!(words[0].status, Status::Declared);
}

#[test]
fn rank_puts_cited_words_first() {
    let declared = BTreeMap::new();
    let observations = [
        access("a", 0x1000, Access::Load, None),
        access("b", 0x1000, Access::Load, None),
        access("cited", 0x1004, Access::Load, None),
    ];
    let cited = BTreeSet::from([function("cited")]);
    let mut words = classify(&region(), &declared, &[], &observations, &cited);
    rank(&mut words);
    assert_eq!(words[0].address, 0x1004);
    assert!(words[0].cited);
    assert!(!words[1].cited);
}

#[test]
fn blobray_records_resolve_to_named_local_accesses() {
    let inputs = ["libx".to_owned()];
    let named = r#"{"input":0,"symbol":{},"name":"fn"}"#;
    let accesses = format!(
        r#"{{"schema":1,"inputs":[],"records":[
        {{"kind":"gap","input":0,"object":null,"reason":"unsupported input format"}},
        {{"kind":"observation","function":{named},"record":1,
            "fact":{{"kind":"memory-access","access":"store","width":4}},"address":4096,"alternative":null,
            "mask":{{"kind":"write-replacement","bits":3}}}},
        {{"kind":"observation","function":{named},"record":2,
            "fact":{{"kind":"callee-effect","access":"load"}},"address":4100,"alternative":null,"mask":null}},
        {{"kind":"observation","function":{named},"record":3,
            "fact":{{"kind":"memory-access","access":"load"}},"address":null,"alternative":null,"mask":null}},
        {{"kind":"blocked","function":{named},"error":{{}}}}
    ],"summary":{{}}}}"#
    );
    let observations = observations(&accesses, &inputs).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].access, Access::Store);
    assert_eq!(observations[0].bits, Some(3));
    assert_eq!(observations[0].function, function("fn"));
}

#[test]
fn observations_of_unknown_inputs_or_unnamed_functions_fail() {
    let accesses = |function: &str| {
        format!(
            r#"{{"records":[{{"kind":"observation","function":{function},
            "fact":{{"kind":"memory-access","access":"load"}},"address":4096,"mask":null}}]}}"#
        )
    };
    assert!(observations(&accesses(r#"{"input":0,"name":"f"}"#), &[]).is_err());
    let inputs = ["libx".to_owned()];
    assert!(observations(&accesses(r#"{"input":0,"name":null}"#), &inputs).is_err());
}

fn indexed(symbol: &str, base: u32, stride: i32, count: Option<u32>) -> Observation {
    Observation {
        function: function(symbol),
        target: Target::Indexed(Indexed {
            base,
            stride,
            count,
        }),
        access: Access::Store,
        bits: None,
    }
}

#[test]
fn an_indexed_access_touches_its_bounded_words_or_its_declared_array() {
    let registers = [
        register(0x1000, 32, "TIMER0", vec![field("VALUE_OPAQUE", 0, 8)]),
        register(0x1010, 32, "TIMER1", vec![field("VALUE_OPAQUE", 0, 8)]),
        register(0x1020, 32, "TIMER2", vec![field("VALUE_OPAQUE", 0, 8)]),
        register(0x1004, 32, "PARAMETER0", vec![field("F", 0, 8)]),
        register(0x1014, 32, "PARAMETER1", vec![field("F", 0, 8)]),
        register(0x1080, 32, "SINGLE", vec![field("F", 0, 8)]),
    ];
    let arrays = arrays(&registers);
    assert_eq!(
        arrays,
        [vec![0x1004, 0x1014], vec![0x1000, 0x1010, 0x1020]],
        "templates with two or more elements at one stride"
    );
    let declared = declared_words(&registers);
    let words = |observation: Observation| -> Vec<(u32, usize)> {
        classify(
            &region(),
            &declared,
            &arrays,
            &[observation],
            &BTreeSet::new(),
        )
        .into_iter()
        .map(|word| (word.address, word.indexed))
        .collect()
    };
    // Unbounded: the array the base is an element of, at its stride.
    assert_eq!(
        words(indexed("timer_set", 0x1010, 0x10, None)),
        [(0x1000, 1), (0x1010, 1), (0x1020, 1)]
    );
    assert_eq!(
        words(indexed("descending", 0x1014, -0x10, None)),
        [(0x1004, 1), (0x1014, 1)],
        "a negative stride matches by its magnitude"
    );
    assert_eq!(
        words(indexed("other_stride", 0x1000, 0x20, None)),
        [(0x1000, 1), (0x1020, 1)],
        "no array has that stride: the run of declared words from the base"
    );
    assert!(
        words(indexed("between", 0x1008, 0x10, None)).is_empty(),
        "the base is no element"
    );
    // A small bound: exactly the reached words inside the regions.
    assert_eq!(
        words(indexed("masked", 0x10f8, 4, Some(4))),
        [(0x10f8, 1), (0x10fc, 1)],
        "indices two and three leave the region"
    );
    // A type-wide bound reaches only the array's elements inside it.
    assert_eq!(
        words(indexed("queue", 0x1020, -0x10, Some(256))),
        [(0x1000, 1), (0x1010, 1), (0x1020, 1)],
        "no word beyond the declared array"
    );
    assert_eq!(
        words(indexed("queue", 0x1010, 0x10, Some(256))),
        [(0x1010, 1), (0x1020, 1)],
        "an element below the base needs a negative index"
    );
    // Registers declared one by one at a stride: the run from the base ends
    // at the first undeclared word.
    let single = [
        register(0x1040, 32, "QUEUE_Q0", vec![field("F", 0, 8)]),
        register(0x1050, 32, "QUEUE_Q1_HIGH", vec![field("F", 0, 8)]),
        register(0x1070, 32, "QUEUE_Q3_HIGH", vec![field("F", 0, 8)]),
    ];
    let declared = declared_words(&single);
    let reached: Vec<u32> = classify(
        &region(),
        &declared,
        &super::arrays(&single),
        &[indexed("queue", 0x1040, 0x10, Some(256))],
        &BTreeSet::new(),
    )
    .into_iter()
    .map(|word| word.address)
    .collect();
    assert_eq!(reached, [0x1040, 0x1050], "0x1060 is undeclared");
}

#[test]
fn blobray_indexed_records_become_indexed_targets() {
    let inputs = ["libx".to_owned()];
    let accesses = r#"{"records":[
        {"kind":"observation","function":{"input":0,"name":"timer_set"},
            "fact":{"kind":"memory-access","access":"store"},"address":null,
            "indexed":{"base":4100,"stride":-16,"count":null},"mask":null},
        {"kind":"observation","function":{"input":0,"name":"pointer"},
            "fact":{"kind":"memory-access","access":"load"},"address":null,"mask":null}
    ]}"#;
    let observations = observations(accesses, &inputs).unwrap();
    assert_eq!(
        observations.len(),
        1,
        "an unindexed unresolved address is skipped"
    );
    assert_eq!(
        observations[0].target,
        Target::Indexed(Indexed {
            base: 4100,
            stride: -16,
            count: None,
        })
    );
}
