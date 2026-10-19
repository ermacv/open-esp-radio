use super::*;

#[test]
fn validates_binary_ssids_once_for_every_role() {
    let ssid = WifiSsid::new(&[0xff, 0, b'a']).unwrap();
    assert_eq!(ssid.as_bytes(), &[0xff, 0, b'a']);
    assert_eq!(WifiSsid::new(&[]), Err(WifiSsidError::Empty));
    assert_eq!(
        WifiSsid::new(&[0; MAX_SSID_LEN + 1]),
        Err(WifiSsidError::TooLong {
            length: MAX_SSID_LEN + 1,
            maximum: MAX_SSID_LEN,
        })
    );
}
