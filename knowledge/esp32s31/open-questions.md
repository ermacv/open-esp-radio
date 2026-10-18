# ESP32-S31 open questions

Work that is known and decided but not done yet, and questions still open.
An item leaves this file in the change that settles it.

## Planned

- `oer_esp32s31_hal::bluetooth::validation`, the BLE driver's probe bridge
  (`validation.rs`) and the PAC's BLE probe hooks (the vendor-chosen
  `BluetoothPhyRegisterInitInputs`, the baseband, controller HAL-init, memory
  list and PHY register-init bridges of `pac::validation`) return with the
  `probe-bluetooth` image, which stays on
  `port/archive` until part 2 of the port; their hooks stay with it there.
