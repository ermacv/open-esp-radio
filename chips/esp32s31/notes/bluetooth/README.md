# ESP32-S31 Bluetooth knowledge notes

`esp32s31-bt-lib` has no license. Under "Sources without a license" in
[the source policy](../../../../docs/source-policy.md), the repository may record
only facts from it: values, layouts, addresses, order of operations and names.
It records neither code nor verbatim data blocks; the facts are kept here.

Clean-room rule: these notes hold facts only. Our code is written from the
Bluetooth Core Specification, these facts and hardware measurements, never
from vendor code. Values marked "to be measured" must come from a named
hardware run.

| Note | Contents |
| --- | --- |
| [phy-register-init.md](phy-register-init.md) | Order and kind of the BLE PHY register-init writes, inputs, ETM route |
| [controller-hal-init.md](controller-hal-init.md) | Controller HAL-init write order, input domains, tick rate |
| [scheduler-execution-lock-modify.md](scheduler-execution-lock-modify.md) | Observable execution lock and modify protocol |
| [le-coexistence-priorities.md](le-coexistence-priorities.md) | Lane layout and advertising and connection priority rules |
