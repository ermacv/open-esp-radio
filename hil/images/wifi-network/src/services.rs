//! The HIL services the image mounts and the key set it reports: the union
//! of the keys of the Wi-Fi station service, of the load service and of the
//! base module.

oer_protocols_hil_agent::image_keys! {
    /// The network core's image: the base module, the Wi-Fi station service and
    /// the load service.
    pub(crate) KEYS = [
        oer_hil_services_wifi_station::Request,
        oer_hil_services_load::Request,
    ]
}

/// The largest request payload the image takes, as in `hil-wifi-station`: a
/// join with the longest SSID and passphrase or the largest load start.
const MAXIMUM_PAYLOAD_BYTES: u16 = 128;

/// What the image announces in its Hello.
pub(crate) const IMAGE: oer_hil_agent::Image = oer_hil_agent::Image {
    keys: KEYS,
    maximum_payload_bytes: MAXIMUM_PAYLOAD_BYTES,
};
