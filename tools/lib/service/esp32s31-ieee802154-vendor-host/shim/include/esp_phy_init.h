/* Closed PHY client calls used by the driver, recorded by the host stand. */
#pragma once
typedef enum {
    PHY_MODEM_WIFI = 1,
    PHY_MODEM_BT = 2,
    PHY_MODEM_IEEE802154 = 4,
} esp_phy_modem_t;
void esp_phy_enable(esp_phy_modem_t modem);
void esp_phy_disable(esp_phy_modem_t modem);
void esp_btbb_enable(void);
void esp_btbb_disable(void);
