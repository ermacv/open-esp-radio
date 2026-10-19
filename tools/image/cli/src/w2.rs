//! Stand build parameters for the W2 active scanner. The owner cap belongs
//! to this lab profile; a board and the production driver have no default cap.

use crate::w1;

pub const IMAGE: &str = "w2";
pub const LAB_POWER_CAP_DBM: i8 = 10;
pub const ENV: [&str; 5] = [
    "OER_W2_CHANNEL",
    "OER_W2_BSSID",
    "OER_W2_SSID_HEX",
    "OER_W2_COUNTRY",
    "OER_W2_POWER_CAP_DBM",
];

pub fn runtime_env(config: &w1::Config) -> Vec<(String, Option<String>)> {
    config
        .runtime_env()
        .into_iter()
        .map(|(key, value)| (key.replace("OER_W1_", "OER_W2_"), Some(value)))
        .chain([(ENV[4].to_owned(), Some(LAB_POWER_CAP_DBM.to_string()))])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn w2_uses_the_router_country_and_an_explicit_lab_cap() {
        let config = w1::Config {
            channel: 13,
            bssid: oer_lab_router::mac::Mac::new([2, 0, 0, 0, 0, 1]).expect("MAC"),
            ssid: w1::Ssid::new(b"Lab").expect("SSID"),
            country: *b"DE",
        };
        let env = runtime_env(&config);
        assert_eq!(env.len(), 5);
        assert!(env.contains(&(ENV[3].to_owned(), Some("DE".to_owned()))));
        assert!(env.contains(&(ENV[4].to_owned(), Some("10".to_owned()))));
        assert!(env.iter().all(|(key, _)| key.starts_with("OER_W2_")));
    }
}
