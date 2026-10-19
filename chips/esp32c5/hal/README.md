# ESP32-C5 radio HAL

`oer-esp32c5-hal` owns the ESP32-C5 radio registers above the closed
[PAC](../pac/src/lib.rs). This first stage covers the IEEE 802.15.4 MAC:

| Item | Contents |
| --- | --- |
| `i::mac` | Task and interrupt owners made from the PAC partition, and the vendor transmit-on delays |
| `ieee802154::ll::Ieee802154MacOwners` | The ESP32-C5 implementation of the chip-neutral [engine](../../espressif/ieee802154-engine/README.md)'s low-level interface |
| `i::{tx_power, coex, IEEE802154_DMA_WINDOW}` | The provider power levels, the coexistence priorities and the memory the MAC DMA reaches |
| `coex` | The vendor coexistence priority table |
| `analog::AnalogI2c` | The PHY's analog register bus: the chip-neutral [contract](../../espressif/analog/README.md), including its parallel writes, over the PAC's analog I2C master |
| `analog::initialization` | The 44-pair parallel analog initialization of `phy_i2c_init1` over the `phy_param` fields it reads |
| `modem_clock::ModemClocks` | The shared modem clocks over the chip-neutral [planner](../../espressif/modem-clock/README.md), with the ESP32-C5 device order and module sets, and the IEEE 802.15.4 MAC reset |

The recovered values name their pinned source in their documentation.
`ModemClocks` offers only the modules whose every device action the PAC
publishes: IEEE 802.15.4, coexistence, the modem ETM and the Bluetooth APB.
It leaves the per-domain clock-gating map to the platform, which must not add
the MODEM state to the analog I2C master map on revision v1.0 (see the
[hardware errata](../../../docs/hardware-errata.md)). The PHY and the MAC
lifecycle around these owners are not owned by this crate yet, so no
ESP32-C5 composition starts the MAC.
