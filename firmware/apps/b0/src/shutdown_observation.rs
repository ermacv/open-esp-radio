//! The Controller's LP pending observation returned after each H4 uninstall.
use oer_bluetooth_hci::{HciControllerResponse, bt_hci::PacketKind};

/// Vendor event payload: subevent 1, then the sampled +0x24 and +0x2c bytes.
pub struct ShutdownObservation([u8; 5]);

impl ShutdownObservation {
    pub const fn new(pending: [u8; 2]) -> Self {
        Self([0xff, 3, 1, pending[0], pending[1]])
    }

    /// Decode only this event, including the H4 event indicator.
    pub fn from_h4(event: &[u8]) -> Option<[u8; 2]> {
        match event {
            [4, 0xff, 3, 1, software, rollover] => Some([*software, *rollover]),
            _ => None,
        }
    }
}

impl HciControllerResponse for ShutdownObservation {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonzero_pending_survives_the_product_event_boundary() {
        let event = ShutdownObservation::new([7, 3]);
        assert_eq!(event.kind(), PacketKind::Event);
        assert_eq!(event.as_bytes(), [0xff, 3, 1, 7, 3]);
        assert_eq!(
            ShutdownObservation::from_h4(&[4, 0xff, 3, 1, 7, 3]),
            Some([7, 3])
        );
        for malformed in [
            &[4, 0xff, 3, 1, 7][..],
            &[4, 0xff, 2, 1, 7, 3],
            &[4, 0xff, 3, 2, 7, 3],
        ] {
            assert_eq!(ShutdownObservation::from_h4(malformed), None);
        }
    }
}
