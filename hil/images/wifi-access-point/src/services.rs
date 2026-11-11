//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of the access-point service and of the base module.

oer_protocols_hil_agent::image_keys! {
    /// The access-point image: the base module and the access-point service.
    pub(crate) KEYS = [oer_hil_services_wifi_access_point::Request]
}

/// The largest request payload the image takes: a serve request.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = 128;

/// What the image announces in its Hello.
#[cfg(target_arch = "riscv32")]
pub(crate) const IMAGE: oer_hil_agent::Image = oer_hil_agent::Image {
    keys: KEYS,
    maximum_payload_bytes: MAXIMUM_PAYLOAD_BYTES,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_services_wifi_access_point::{MAX_PASSPHRASE, MAX_SSID, Serve};
    use oer_protocols_hil_agent::base::{Base, Service};
    use oer_protocols_hil_protocol::Key;

    #[test]
    fn the_image_reports_the_base_module_and_the_access_point_service() {
        let expected: BTreeSet<Key> = Base::KEYS
            .iter()
            .chain(<oer_hil_services_wifi_access_point::Request as Service>::KEYS)
            .copied()
            .collect();
        assert_eq!(
            KEYS.keys().iter().copied().collect::<BTreeSet<_>>(),
            expected
        );
        assert!(KEYS.keys().windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_longest_serve_fits_the_payload_bound() {
        let serve = Serve {
            ssid: heapless::String::try_from("s".repeat(MAX_SSID).as_str()).unwrap(),
            passphrase: heapless::String::try_from("p".repeat(MAX_PASSPHRASE).as_str()).unwrap(),
            channel: 13,
            clients: 4,
        };
        let mut buffer = [0; 256];
        let encoded = postcard::to_slice(&serve, &mut buffer).unwrap();
        assert!(encoded.len() <= usize::from(MAXIMUM_PAYLOAD_BYTES));
    }
}
