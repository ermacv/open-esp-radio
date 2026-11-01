//! What the image's periodic PHY tracking did within a window: the shared
//! RF actor's ticks by what they did, the branches the tracked ticks
//! committed, the PHY's temperature after them (PHY sensor units: no
//! Celsius conversion is established) and the ticks' durations. Only a
//! trace image counts them; `available` says whether it did.

use oer_services_load::Tracking;
use serde_json::{Value, json};

/// The window's record.
pub fn window(tracking: &Tracking) -> Value {
    json!({
        "available": tracking.available,
        "ticks": tracking.ticks,
        "not_due": tracking.not_due,
        "unavailable": tracking.unavailable,
        "branches": {
            "inhibited": tracking.inhibited,
            "rfpll_corrected": tracking.rfpll_corrected,
            "wifi_i2c_changed": tracking.wifi_i2c_changed,
            "wifi_tx_power": tracking.wifi_tx_power,
            "bluetooth_ieee802154_tx_power": tracking.bluetooth_ieee802154_tx_power,
            "calibration_common": tracking.calibration_common,
            "calibration_transmit": tracking.calibration_transmit,
        },
        "temperature_sensor_units": {
            "last": tracking.temperature_last,
            "min": tracking.temperature_min,
            "max": tracking.temperature_max,
        },
        "tick_us": tracking.tick_us,
        "longest_tick_us": tracking.longest_tick_us,
    })
}

/// The window's one line: the ticks, the branches they committed, the
/// temperature and the ticks' time.
pub fn summary(tracking: &Tracking) -> String {
    if !tracking.available {
        return "not counted (a trace image counts the ticks)".to_owned();
    }
    let temperature = |value: Option<i16>| value.map_or("-".to_owned(), |value| value.to_string());
    format!(
        "{} ticks ({} not due), rfpll {}, i2c {}, Wi-Fi power {}, BLE/802.15.4 power {}, \
         RX cal {}, TX cal {}; temperature {} ({}..{}) sensor units; {} us in ticks, \
         longest {} us",
        tracking.ticks,
        tracking.not_due,
        tracking.rfpll_corrected,
        tracking.wifi_i2c_changed,
        tracking.wifi_tx_power,
        tracking.bluetooth_ieee802154_tx_power,
        tracking.calibration_common,
        tracking.calibration_transmit,
        temperature(tracking.temperature_last),
        temperature(tracking.temperature_min),
        temperature(tracking.temperature_max),
        tracking.tick_us,
        tracking
            .longest_tick_us
            .map_or("-".to_owned(), |us| us.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_records_every_count_and_its_line_names_them() {
        let tracking = Tracking {
            available: true,
            ticks: 5,
            not_due: 1,
            rfpll_corrected: 2,
            wifi_tx_power: 1,
            calibration_transmit: 1,
            temperature_last: Some(81),
            temperature_min: Some(79),
            temperature_max: Some(83),
            tick_us: 6_100,
            longest_tick_us: Some(1_900),
            ..Tracking::default()
        };
        let value = window(&tracking);
        assert_eq!(value["ticks"], 5);
        assert_eq!(value["branches"]["rfpll_corrected"], 2);
        assert_eq!(value["temperature_sensor_units"]["min"], 79);
        assert_eq!(value["longest_tick_us"], 1_900);
        assert_eq!(
            summary(&tracking),
            "5 ticks (1 not due), rfpll 2, i2c 0, Wi-Fi power 1, BLE/802.15.4 power 0, \
             RX cal 0, TX cal 1; temperature 81 (79..83) sensor units; 6100 us in ticks, \
             longest 1900 us"
        );
    }

    #[test]
    fn an_image_that_does_not_count_says_so() {
        let tracking = Tracking::default();
        assert_eq!(
            window(&tracking)["temperature_sensor_units"]["last"],
            Value::Null
        );
        assert!(summary(&tracking).starts_with("not counted"));
    }
}
