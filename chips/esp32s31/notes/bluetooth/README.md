# ESP32-S31 Bluetooth knowledge notes

The current `esp32s31-bt-lib` pin includes an Apache-2.0 binary distribution
license. These knowledge notes retain the facts-only boundary in
[the source policy](../../../../docs/source-policy.md): values, layouts,
addresses, order of operations and names. They contain neither vendor code
nor verbatim data blocks.

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
