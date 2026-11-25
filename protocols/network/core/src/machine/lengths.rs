//! The longest data whose headers still count it.

use super::fits;

#[test]
fn data_fits_while_its_ipv4_total_length_does() {
    // RFC 791, 3.1: a total length of 65 535, with twenty octets of header,
    // and eight of UDP's.
    assert_eq!(fits(65_507), Some(65_507));
    assert_eq!(fits(65_508), None);
    assert_eq!(fits(65_536), None);
    assert_eq!(fits(0), Some(0));
}
