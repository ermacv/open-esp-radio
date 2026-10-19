/* Modem retention hooks called by `ieee802154_mac_init`/`deinit`. */
#pragma once
typedef enum {
    SLEEP_MODEM_WIFI = 0,
    SLEEP_MODEM_BT = 1,
    SLEEP_MODEM_IEEE802154 = 2,
} sleep_modem_t;
void esp_phy_modem_init(sleep_modem_t modem);
void esp_phy_modem_deinit(sleep_modem_t modem);
