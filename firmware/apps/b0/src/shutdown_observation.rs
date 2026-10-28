//! H4 framing of the Controller's LP pending observation after uninstall.
use oer_bluetooth_hci::VendorShutdownObservationEvent;

/// Decode the product event only when framed with the H4 event indicator.
pub fn pending_from_h4(event: &[u8]) -> Option<[u8; 2]> {
    let [4, body @ ..] = event else {
        return None;
    };
    VendorShutdownObservationEvent::from_hci_bytes(body).map(|event| event.pending())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonzero_pending_survives_the_product_h4_boundary() {
        assert_eq!(pending_from_h4(&[4, 0xff, 3, 1, 7, 3]), Some([7, 3]));
        for malformed in [
            &[4, 0xff, 3, 1, 7][..],
            &[4, 0xff, 2, 1, 7, 3],
            &[4, 0xff, 3, 2, 7, 3],
            &[1, 0xff, 3, 1, 7, 3],
            &[4, 0xff, 3, 1, 7, 3, 0],
            &[],
        ] {
            assert_eq!(pending_from_h4(malformed), None);
        }
    }
}
