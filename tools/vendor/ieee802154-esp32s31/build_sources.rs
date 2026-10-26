//! Translation units owned by the IEEE 802.15.4 host stand.

pub fn is_translation_unit(path: &str) -> bool {
    // The shared ESP-IDF ledger also pins platform sources for analysis.
    // Compile the driver and its S31 peripheral connection, not those sources.
    path == "components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c"
        || (path.starts_with("components/ieee802154/") && path.ends_with(".c"))
}

#[cfg(test)]
mod tests {
    use super::is_translation_unit;

    #[test]
    fn shared_ledger_keeps_flash_and_other_chips_out_of_the_stand() {
        let peripheral = "components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c";
        let driver = "components/ieee802154/driver/esp_ieee802154_dev.c";
        let api = "components/ieee802154/esp_ieee802154.c";
        let multipan = "components/ieee802154/esp_ieee802154_multipan.c";
        let ledger_paths = [
            peripheral,
            "components/esp_hal_mspi/spi_flash_hal.c",
            driver,
            "components/esp_hal_ieee802154/esp32c5/ieee802154_periph.c",
            api,
            "components/ieee802154/include/esp_ieee802154.h",
            multipan,
        ];

        let selected: Vec<_> = ledger_paths
            .into_iter()
            .filter(|path| is_translation_unit(path))
            .collect();

        assert_eq!(selected, [peripheral, driver, api, multipan]);
    }
}
