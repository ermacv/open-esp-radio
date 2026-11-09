use super::*;

#[test]
fn validates_binary_ssids_once_for_every_role() {
    let ssid = Ssid::new(&[0xff, 0, b'a']).unwrap();
    assert_eq!(ssid.as_bytes(), &[0xff, 0, b'a']);
    assert_eq!(Ssid::new(&[]), Err(SsidError::Empty));
    assert_eq!(
        Ssid::new(&[0; MAX_SSID_LEN + 1]),
        Err(SsidError::TooLong {
            length: MAX_SSID_LEN + 1,
            maximum: MAX_SSID_LEN,
        })
    );
}

#[test]
fn every_admitted_binary_length_keeps_its_exact_prefix() {
    let bytes = [0xff; MAX_SSID_LEN];
    for length in 1..=MAX_SSID_LEN {
        let ssid = Ssid::new(&bytes[..length]).unwrap();
        assert_eq!(ssid.len(), length);
        assert_eq!(ssid.as_bytes(), &bytes[..length]);
    }
}
