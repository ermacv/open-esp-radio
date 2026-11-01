//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of the SDK station service, of the Wi-Fi station service, of
//! the fault service, of the load service and of the base module.

#[cfg(target_arch = "riscv32")]
oer_hil_agent::image_keys! {
    /// The SDK station image: the base module, the SDK station service, the
    /// Wi-Fi station service, the fault service and the load service.
    pub(crate) KEYS = [
        oer_services_sdk_station::Request,
        oer_services_wifi_station::Request,
        oer_services_fault::Request,
        oer_services_load::Request,
    ]
}

/// The largest request payload the image takes: a connect or a join with
/// the longest SSID and passphrase, an exchange with the longest line or
/// the largest load start.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = 128;

/// What the image announces in its Hello.
#[cfg(target_arch = "riscv32")]
pub(crate) const IMAGE: oer_hil_agent_espressif::Image = oer_hil_agent_espressif::Image {
    keys: KEYS,
    maximum_payload_bytes: MAXIMUM_PAYLOAD_BYTES,
};

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_sdk_station::{Connect, Exchange, MAX_LINE, MAX_PASSPHRASE, MAX_SSID};

    #[test]
    fn the_longest_connect_join_exchange_and_load_start_fit_the_payload_bound() {
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
        let join = oer_services_wifi_station::Join {
            ssid: connect.ssid.clone(),
            passphrase: connect.passphrase.clone(),
            channel: u8::MAX,
        };
        let start = oer_services_load::Start {
            run: u32::MAX,
            port: u16::MAX,
            delay_ms: u32::MAX,
            duration_ms: u32::MAX,
            drain_ms: u32::MAX,
            transmit: Some(oer_services_load::Transmit {
                peer: [255; 4],
                peer_port: u16::MAX,
                payload: u16::MAX,
                rate_bps: u32::MAX,
            }),
            profile: Some(oer_services_load::Profile {
                period_us: u32::MAX,
            }),
        };
        let mut buffer = [0; 256];
        for encoded in [
            postcard::to_slice(&connect, &mut buffer).unwrap().len(),
            postcard::to_slice(&join, &mut buffer).unwrap().len(),
            postcard::to_slice(&exchange, &mut buffer).unwrap().len(),
            postcard::to_slice(&start, &mut buffer).unwrap().len(),
        ] {
            assert!(
                encoded <= usize::from(MAXIMUM_PAYLOAD_BYTES),
                "{encoded} bytes"
            );
        }
    }
}
