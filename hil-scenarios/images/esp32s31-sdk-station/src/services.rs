//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of the SDK station service and of the base module.

#[cfg(target_arch = "riscv32")]
oer_hil_agent::image_keys! {
    /// The SDK station image: the base module and the SDK station service.
    pub(crate) KEYS = [oer_services_sdk_station::Request]
}

/// The largest request payload the image takes: a connect with the longest
/// SSID and passphrase, or an exchange with the longest line.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = 128;

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_sdk_station::{Connect, Exchange, MAX_LINE, MAX_PASSPHRASE, MAX_SSID};

    #[test]
    fn the_longest_connect_and_exchange_fit_the_payload_bound() {
        let connect = Connect {
            ssid: heapless::String::try_from("s".repeat(MAX_SSID).as_str()).unwrap(),
            passphrase: heapless::String::try_from("p".repeat(MAX_PASSPHRASE).as_str()).unwrap(),
        };
        let exchange = Exchange {
            peer: [255; 4],
            port: u16::MAX,
            line: heapless::Vec::from_slice(&[0xff; MAX_LINE]).unwrap(),
            timeout_ms: u32::MAX,
        };
        let mut buffer = [0; 256];
        for encoded in [
            postcard::to_slice(&connect, &mut buffer).unwrap().len(),
            postcard::to_slice(&exchange, &mut buffer).unwrap().len(),
        ] {
            assert!(
                encoded <= usize::from(MAXIMUM_PAYLOAD_BYTES),
                "{encoded} bytes"
            );
        }
    }
}
