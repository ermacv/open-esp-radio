use super::matches;

#[test]
fn literal_trait_and_generic_names_are_not_regular_expressions() {
    assert!(matches(
        "* as core::fmt::Display>::fmt*",
        "<Thing<[u8; 6]> as core::fmt::Display>::fmt.123"
    ));
    assert!(!matches("core::fmt::write", "core::fmt::write_suffix"));
    assert!(matches(
        "<core::fmt::Formatter>::*",
        "<core::fmt::Formatter>::pad"
    ));
    assert!(!matches(
        "<core::fmt::Formatter>::*",
        "<esp_hal::fmt::Formatter>::pad"
    ));
    assert!(!matches("a*b*c", "a---c---b"));
}
