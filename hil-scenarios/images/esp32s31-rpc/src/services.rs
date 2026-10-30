//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of those services and of the base module.

oer_hil_agent::image_keys! {
    /// The AES image: the base module and the AES service.
    pub(crate) KEYS = [oer_services_aes::Request]
}

/// The largest request payload the image takes: an AES unwrap request.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = 64;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::Key;

    #[test]
    fn the_image_reports_the_base_module_and_the_aes_service() {
        let expected: BTreeSet<Key> = Base::KEYS
            .iter()
            .chain(<oer_services_aes::Request as Service>::KEYS)
            .copied()
            .collect();
        assert_eq!(
            KEYS.keys().iter().copied().collect::<BTreeSet<_>>(),
            expected
        );
        assert!(KEYS.keys().windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn an_unwrap_request_of_the_largest_input_fits_the_payload_bound() {
        let request = oer_services_aes::UnwrapKey {
            kek: [0xa5; 16],
            wrapped: heapless::Vec::from_slice(&[0x5a; oer_services_aes::MAX_WRAPPED]).unwrap(),
        };
        let mut buffer = [0; 256];
        let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
        assert!(encoded.len() <= usize::from(MAXIMUM_PAYLOAD_BYTES));
    }
}
