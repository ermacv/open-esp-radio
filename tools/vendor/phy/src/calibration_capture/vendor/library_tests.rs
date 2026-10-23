use super::*;
#[test]
fn report_hex_keeps_case_acceptance_and_malformed_rejection() {
    assert_eq!(decode_hex("00aF"), Some(vec![0, 175]));
    assert_eq!(decode_hex(""), Some(vec![]));
    for malformed in ["0", "0g", "é"] {
        assert_eq!(decode_hex(malformed), None);
    }
}
