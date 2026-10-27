use super::*;
use crate::compare::{ReviewFile, compare_field, fields};
use oer_vendor_phy::calibration_capture::boots::Lifecycle;

fn field(name: &str) -> OutputField {
    fields().into_iter().find(|f| f.name == name).unwrap()
}

fn bytes(field: &OutputField, offset: u32, initial: [u16; 2], last: [u16; 2]) -> Vec<u8> {
    let mut data = vec![0; offset as usize + field.count as usize * 2];
    for gain in 0..field.count / 2 {
        let pair = if gain == 0 { initial } else { last };
        set_pair(&mut data, offset, gain, pair);
    }
    data
}

fn set_pair(data: &mut [u8], offset: u32, gain: u32, pair: [u16; 2]) {
    for (i, value) in pair.into_iter().enumerate() {
        let start = offset as usize + gain as usize * 4 + i * 2;
        data[start..start + 2].copy_from_slice(&value.to_le_bytes());
    }
}

fn review(field: &OutputField) -> Tolerance {
    crate::compare::fixture_reviews()
        .at(Lifecycle::Cold)
        .unwrap()
        .fields
        .remove(field.name)
        .unwrap()
}

fn compare_pair(name: &str, gain: u32, pair: [u16; 2]) -> FieldSummary {
    let field = field(name);
    let vendor = bytes(&field, field.parameter, [243, 288], [259, 303]);
    let mut production = bytes(&field, field.output, [243, 288], [259, 303]);
    set_pair(&mut production, field.output, gain, pair);
    compare_field(&field, Some(&review(&field)), &[vendor], &[production]).unwrap()
}

#[test]
fn an_out_of_range_whole_initial_pair_passes_with_an_explicit_report_row() {
    for (name, gain) in [("shared-rx-dc", 10), ("wifi-rx-dc", 7)] {
        let summary = compare_pair(name, gain, [243, 288]);
        assert_eq!(summary.verdict, Verdict::Match);
        assert_eq!(summary.margin, 0);
        assert_eq!(summary.vendor[gain as usize * 2], [259, 259]);
        assert_eq!(summary.production[gain as usize * 2], [243, 243]);
        assert_eq!(
            summary.initial_pairs,
            [InitialPairMatch {
                capture: 1,
                gain,
                pair: [243, 288],
                reason: PairMatchReason::InitialPairVendorFaithful,
            }]
        );
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("initial pair, vendor-faithful"));
        assert_eq!(
            serde_json::from_str::<FieldSummary>(&json).unwrap(),
            summary
        );
    }
}

#[test]
fn noninitial_pairs_and_single_coordinate_matches_differ() {
    for pair in [[240, 280], [243, 303], [259, 288]] {
        let summary = compare_pair("shared-rx-dc", 10, pair);
        assert_eq!(summary.verdict, Verdict::Diff, "pair {pair:?}");
        assert!(summary.initial_pairs.is_empty());
    }
}

#[test]
fn an_in_range_pair_uses_the_existing_rule_without_an_initial_pair_row() {
    let summary = compare_pair("shared-rx-dc", 10, [259, 303]);
    assert_eq!(summary.verdict, Verdict::Match);
    assert!(summary.initial_pairs.is_empty());
}

#[test]
fn initial_pairs_are_correlated_to_each_capture_and_both_coordinates() {
    let field = field("shared-rx-dc");
    let vendor = bytes(&field, field.parameter, [243, 288], [259, 303]);
    let first = bytes(&field, field.output, [243, 288], [243, 288]);
    let mut other = bytes(&field, field.output, [243, 288], [259, 303]);
    // Both of this boot's coordinates exist in the other boot's initial
    // pair/ranges, but this is not its own whole initial pair.
    set_pair(&mut other, field.output, 0, [242, 289]);
    set_pair(&mut other, field.output, 10, [243, 288]);
    let vendor_other = bytes(&field, field.parameter, [242, 289], [259, 303]);
    let summary = compare_field(
        &field,
        Some(&review(&field)),
        &[vendor, vendor_other],
        &[first, other],
    )
    .unwrap();
    assert_eq!(summary.verdict, Verdict::Diff);
    assert!(summary.initial_pairs.iter().all(|p| p.capture == 1));
    assert!(summary.initial_pairs.iter().any(|p| p.gain == 10));
}

#[test]
fn gain0_and_wifi_fine_calibration_remain_range_checked() {
    let zero = compare_pair("shared-rx-dc", 0, [259, 303]);
    assert_eq!(zero.verdict, Verdict::Diff);
    for fine in 8..13 {
        let summary = compare_pair("wifi-rx-dc", fine, [243, 288]);
        assert_eq!(summary.verdict, Verdict::Diff);
        assert!(summary.initial_pairs.is_empty());
    }
}

#[test]
fn rule_requires_complete_unsigned_rx_dc_fields_and_the_current_schema() {
    let field = field("shared-rx-dc");
    for review in [
        Tolerance {
            signed: true,
            ..review(&field)
        },
        Tolerance {
            mask: Some(vec![0xffff; field.count as usize]),
            ..review(&field)
        },
        Tolerance {
            excluded: true,
            ..review(&field)
        },
    ] {
        assert!(Rule::RxDcGain0.gains(&field, &review).is_err());
    }
    for table in ["fields.common-reference", "registers.A"] {
        let text =
            format!("schema=3\n[{table}]\ninitial-pair=\"rx-dc-gain0\"\nreason=\"review\"\n");
        assert!(
            ReviewFile::read(&text, &["A".into()].into())
                .unwrap()
                .at(Lifecycle::Cold)
                .is_err()
        );
    }
    for schema in [1, 2, 4] {
        let text = format!("schema={schema}\n");
        assert!(ReviewFile::read(&text, &["A".into()].into()).is_err());
    }
}
