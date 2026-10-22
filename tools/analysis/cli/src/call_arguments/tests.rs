use oer_analysis_blobray_application::call_arguments::ArgumentValue;

#[test]
fn argument_values_keep_their_exact_spelling() {
    let values = [
        ArgumentValue::Constant { value: 0x67 },
        ArgumentValue::Constant { value: 1 },
        ArgumentValue::EntryArgument { index: 0 },
        ArgumentValue::Alternatives {
            values: vec![
                ArgumentValue::Constant { value: 4 },
                ArgumentValue::Constant { value: 5 },
            ],
        },
        ArgumentValue::Symbol {
            name: b"phy_param".to_vec(),
            addend: 0xef,
        },
        ArgumentValue::Unknown,
        ArgumentValue::Unknown,
        ArgumentValue::Unknown,
    ];
    assert_eq!(
        values.iter().map(super::human).collect::<Vec<_>>(),
        [
            "0x67",
            "0x1",
            "arg0",
            "one-of{0x4 | 0x5}",
            "phy_param+0xef",
            "?",
            "?",
            "?"
        ]
    );
}
