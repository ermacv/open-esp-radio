/* ESP-IDF Kconfig defaults for an ESP32-S31 IEEE 802.15.4 build at the pinned
 * revision (`components/ieee802154/Kconfig`). Debug, record, statistic,
 * multi-PAN, sleep, power-management and coexistence options keep their
 * default `n`, so they are left undefined exactly as sdkconfig.h does. */
#pragma once

#define CONFIG_IDF_TARGET_ESP32S31 1
#define CONFIG_IEEE802154_ENABLED 1
#define CONFIG_IEEE802154_RX_BUFFER_SIZE 20
#define CONFIG_IEEE802154_CCA_ED 1
#define CONFIG_IEEE802154_CCA_MODE 1
#define CONFIG_IEEE802154_CCA_THRESHOLD -75
#define CONFIG_IEEE802154_PENDING_TABLE_SIZE 20
#ifndef CONFIG_IEEE802154_INTERFACE_NUM
#define CONFIG_IEEE802154_INTERFACE_NUM 1
#endif
#define CONFIG_IEEE802154_TIMING_OPTIMIZATION 1
