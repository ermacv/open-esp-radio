# ESP32-S31 SDK RPC image

The image of the radio-free scenarios ([AES](../../aes/README.md)): an
application of the entry `oer-espressif` (`sdk/espressif`, features
`esp32s31` and `console`, without `wifi`). Its boot takes the AES service
with the library's defaults (`Aes::start`, four client seats) and the USB
Serial/JTAG console (`Console::open`); the library places their
interrupts, executor and service task and records and resets on the
image's behalf. The image declares no interrupt, DMA or service task of
its own, starts no radio and transmits nothing on air. It runs the boot
hart alone (`entry!(boot)`). `oer-test-layers` checks that the image takes
nothing the SDK places.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../hil-agent/espressif/README.md); the boot evidence is the library's (`Boot::evidence`): hart 0's reset cause, the previous boot's panic, and its CPU exception as the post-mortem (`Fault::Exception`), which the library already printed and cleared |
| [AES](../../services/aes/README.md) | One worker on the application executor answers the requests in order through one client of the library's AES service (`oer_services_aes::Engine` over `AesClient`, of the library's key data capacity `AES_CAPACITY`); a request while one waits is refused as `Busy` |

## Faults

A boot error and an application error record their named cause and reset
the chip (the library's failure table); a console transport fault ends
the application with `sdk-console`. The image builds in no credentials.

`cargo fw build esp32s31-sdk-rpc` builds it; the scenario's harness builds
and flashes it.
