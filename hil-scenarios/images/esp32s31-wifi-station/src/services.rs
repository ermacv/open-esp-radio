//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of those services and of the base module.

oer_hil_agent::image_keys! {
    /// The Wi-Fi station image: the base module and the Wi-Fi station service.
    pub(crate) KEYS = [oer_services_wifi_station::Request]
}

/// The largest request payload the image takes: a join with the longest SSID
/// and passphrase.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = 128;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::Key;
    use oer_services_wifi_station::{Join, MAX_PASSPHRASE, MAX_SSID};

    #[test]
    fn the_image_reports_the_base_module_and_the_wifi_station_service() {
        let expected: BTreeSet<Key> = Base::KEYS
            .iter()
            .chain(<oer_services_wifi_station::Request as Service>::KEYS)
            .copied()
            .collect();
        assert_eq!(
            KEYS.keys().iter().copied().collect::<BTreeSet<_>>(),
            expected
        );
        assert!(KEYS.keys().windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_join_of_the_longest_ssid_and_passphrase_fits_the_payload_bound() {
        let join = Join {
            ssid: heapless::String::try_from("s".repeat(MAX_SSID).as_str()).unwrap(),
            passphrase: heapless::String::try_from("p".repeat(MAX_PASSPHRASE).as_str()).unwrap(),
            channel: 13,
        };
        let mut buffer = [0; 256];
        let encoded = postcard::to_slice(&join, &mut buffer).unwrap();
        assert!(encoded.len() <= usize::from(MAXIMUM_PAYLOAD_BYTES));
    }
}
