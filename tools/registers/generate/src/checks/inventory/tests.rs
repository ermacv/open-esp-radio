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
        reset_value: None,
        reset_mask: None,
        reserved: None,
        fields,
    }
}

#[test]
fn reserved_sources_follow_byte_lanes_across_word_boundaries() {
    let mut declaration = register(0x100e, 64, "WIDE", vec![]);
    declaration.reserved = Some(oer_registers_model::ReservedBits {
        mask: 0x10002,
        sources: vec!["vendor-header".into()],
    });
    let words = declared_words(&[declaration]);
    assert_eq!(words[&0x100c].reserved["vendor-header"], 0x20000);
    assert_eq!(words[&0x1010].reserved["vendor-header"], 1);
    assert_eq!(words[&0x100c].fields | words[&0x1010].fields, 0);
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
    let words = |observation: Indexed| -> Vec<(u32, usize)> {
        observation
            .addresses(&arrays, &declared)
            .into_iter()
            .filter(|address| (0x1000..0x1100).contains(address))
            .map(|address| (address, 1))
            .collect()
    };
    // Unbounded: the array the base is an element of, at its stride.
    assert_eq!(
        words(indexed(0x1010, 0x10, None)),
        [(0x1000, 1), (0x1010, 1), (0x1020, 1)]
    );
    assert_eq!(
        words(indexed(0x1014, -0x10, None)),
        [(0x1004, 1), (0x1014, 1)],
        "a negative stride matches by its magnitude"
    );
    assert_eq!(
        words(indexed(0x1000, 0x20, None)),
        [(0x1000, 1), (0x1020, 1)],
        "no array has that stride: the run of declared words from the base"
    );
    assert!(
        words(indexed(0x1008, 0x10, None)).is_empty(),
        "the base is no element"
    );
    // A small bound: exactly the reached words inside the regions.
    assert_eq!(
        words(indexed(0x10f8, 4, Some(4))),
        [(0x10f8, 1), (0x10fc, 1)],
        "indices two and three leave the region"
    );
    // A type-wide bound reaches only the array's elements inside it.
    assert_eq!(
        words(indexed(0x1020, -0x10, Some(256))),
        [(0x1000, 1), (0x1010, 1), (0x1020, 1)],
        "no word beyond the declared array"
    );
    assert_eq!(
        words(indexed(0x1010, 0x10, Some(256))),
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
    let reached = indexed(0x1040, 0x10, Some(256)).addresses(&super::arrays(&single), &declared);
    assert_eq!(reached, [0x1040, 0x1050], "0x1060 is undeclared");
}

fn indexed(base: u32, stride: i32, count: Option<u32>) -> Indexed {
    Indexed {
        base,
        stride,
        count,
    }
}

#[test]
fn inherited_reset_and_mask_reach_every_expanded_register() {
    let xml = r#"<device schemaVersion="1.3"><name>TEST</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><size>32</size><resetValue>165</resetValue><resetMask>255</resetMask><peripherals><peripheral><name>DEV</name><baseAddress>4096</baseAddress><registers><register><dim>2</dim><dimIncrement>4</dimIncrement><dimIndex>0,1</dimIndex><name>VALUE%s</name><addressOffset>0</addressOffset></register></registers></peripheral></peripherals></device>"#;
    let registers: Vec<_> = oer_registers_model::svd_geometry(xml)
        .expect("synthetic geometry")
        .into_iter()
        .map(DeclaredRegister::from)
        .collect();
    let words = declared_words(&registers);
    assert_eq!(words.len(), 2);
    assert!(
        words
            .values()
            .all(|word| word.reset_value == 165 && word.reset_known == 255)
    );
}

#[test]
fn conflicting_alias_resets_remain_unknown_and_wide_resets_keep_their_lanes() {
    let mut wide = register(0x1002, 64, "WIDE", vec![]);
    wide.reset_value = Some(0x123456789abcdef0);
    let words = declared_words(&[wide]);
    assert_eq!(words[&0x1000].reset_known, 0xffff0000);
    assert_eq!(words[&0x1000].reset_value, 0xdef00000);
    assert_eq!(words[&0x1004].reset_value, 0x56789abc);
    assert_eq!(words[&0x1008].reset_value, 0x1234);
    let mut a = register(0x1000, 32, "A", vec![]);
    a.reset_value = Some(0);
    let mut b = a.clone();
    b.reset_value = Some(1);
    let words = declared_words(&[a.clone(), b, a]);
    assert_eq!(words[&0x1000].reset_known, !1);
}
