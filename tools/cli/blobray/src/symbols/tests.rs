use super::{matches, string_at, strings};

#[test]
fn a_pattern_without_a_star_matches_a_substring_and_a_star_globs_the_whole_name() {
    assert!(matches("rx_gain", "phy_set_rx_gain_table"));
    assert!(!matches("tx_gain", "phy_set_rx_gain_table"));
    assert!(matches("phy_*_table", "phy_set_rx_gain_table"));
    assert!(!matches("phy_*_tab", "phy_set_rx_gain_table"));
    assert!(matches("*gain*", "rx_init_gain"));
    assert!(matches("*", "anything"));
    assert!(
        !matches("ab*ba", "aba"),
        "prefix and suffix must not overlap"
    );
}

#[test]
fn strings_are_terminated_text_runs_with_exact_escapes() {
    let bytes = b"0x%x,0x%x\0ab\0line\n\ttab\0caf\xc3\xa9\xff\0\x01\x02bad\0tail";
    assert_eq!(
        strings(bytes, 4),
        [
            (0, "0x%x,0x%x".to_string()),
            (13, "line\\n\\ttab".to_string()),
            (23, "caf\\xc3\\xa9\\xff".to_string()),
        ],
        "short, control-byte and unterminated runs are not strings"
    );
    assert_eq!(string_at(bytes, 0).as_deref(), Some("0x%x,0x%x"));
    assert_eq!(string_at(bytes, 10).as_deref(), Some("ab"));
    assert_eq!(string_at(bytes, 30), None, "control bytes are not text");
    assert_eq!(string_at(bytes, 40), None, "no terminator");
    assert_eq!(string_at(bytes, 400), None);
}
