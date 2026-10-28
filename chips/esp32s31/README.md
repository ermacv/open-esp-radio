# ESP32-S31

The chip implementation contains the register model, generated PAC, HAL,
PHY, radio-system actor and protocol drivers. Images compose these owners
through the Espressif platform contracts.

| Owner | Current scope |
| --- | --- |
| [Register model](registers/README.md) and [PAC](pac/README.md) | Sourced hardware geometry, typed access and one root claim |
| [HAL](hal/README.md) | Owned register transactions, routing, clocks, power and DMA |
| [PHY](phy/README.md) | Calibration, tracking, retained state and RF lifecycle |
| [Radio system](radio/system/README.md) | One shared-domain actor, client membership and ordered handoff |
| [Wi-Fi driver](ieee80211/driver/README.md) | Lower-MAC port backend and its DMA/IRQ lifecycle |
| [Bluetooth driver](bluetooth/driver/README.md) | Controller engine, scheduler and DTM physical runner |
| [IEEE 802.15.4 driver](ieee802154/driver/README.md) | Family MAC engine bound to S31 HAL and permanent frame storage |
| [Coexistence](../espressif/coex/README.md) | Family priority, timer and schedule policy; no combined schedule runtime |
| [Platform facts](platform/README.md) and [board](board/README.md) | Hart geometry, image layout and board mapping adoption |

Exclusive root ownership and inactive route selection do not establish safe
active protocol switching. PHY maintenance admission, protocol power-save
policy and physical RF/clock shutdown have separate owners. Standalone
protocol paths do not establish joint-radio arbitration or RF qualification.
