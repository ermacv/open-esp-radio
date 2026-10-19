//! The Espressif station's link supervision: how long it waits for beacons
//! and how it probes the access point before it leaves.

use oer_base_time::Duration;
use oer_ieee80211_sta::link_monitor::StaLinkProbePolicy;

/// How long a connected station goes without a beacon from its access point
/// before it starts probing it.
///
/// SOURCE(esp32s31): the ESP-IDF `esp_wifi_set_inactive_time` contract: a
/// station without a beacon from its access point for the inactive time,
/// 6 s by default, leaves it. Complete pinned
/// `libnet80211[wl_cnx.o]::sta_reset_beacon_timeout` rearms that timeout
/// in seconds from each beacon, and its expiry in
/// `cnx_beacon_timeout_process` probes the access point before
/// disconnecting.
pub const STATION_INACTIVE_TIME: Duration = Duration::from_secs(6);

/// How the station probes a silent access point: five probes 500 ms apart,
/// the first three addressed to it and the rest broadcast.
///
/// SOURCE(esp32s31): complete `libnet80211[wl_cnx.o]::send_ap_probe` sends
/// a Probe Request to the access point's address while the probe count it
/// keeps is at most two and to the broadcast address afterwards, counts the
/// probe and rearms the probe timer for 500 ms. Complete
/// `libnet80211[wl_cnx.o]::mgd_probe_send_timeout_process` calls
/// `send_ap_probe` again while that count is at most four and otherwise
/// stops probing and leaves the BSS through `ieee80211_sta_new_state`.
pub const STATION_LINK_PROBE: StaLinkProbePolicy = StaLinkProbePolicy {
    interval: Duration::from_millis(500),
    attempts: 5,
    directed: 3,
};
