# ESP32-S31 RPC image

The image the HIL scenarios of the ESP32-S31 share. It starts the shared
staged platform, serves the HIL base module over the USB Serial/JTAG console
and mounts these services:

| Service | Handler |
| --- | --- |
| [AES](../../services/aes/README.md) | `oer_services_aes::Engine` over the product key-unwrap engine (`oer_esp32s31_compose::crypto::key_unwrap`), one request at a time; a request while one waits is refused as `Busy` |

The boot reads the reset causes and peeks the previous boot's platform panic
before serving boot evidence; complete output clears the record. The image
keeps no post-mortem checkpoints. A failed boot check is retained and
resets (`src/fatal.rs`). An exception keeps the runtime's trap-time frame
and the platform's observations after the trap
(`oer_esp32s31_compose::exception`), both printed by the next boot; the
console's transport faults reset through the platform's retained path. It starts no radio and transmits
nothing on air.

`cargo fw build esp32s31-rpc` builds it; a scenario's harness builds and
flashes it.
