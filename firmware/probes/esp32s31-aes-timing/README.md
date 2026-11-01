# esp32s31-aes-timing

How long the protocol layer's AES operations take on the ESP32-S31, in
software and on the AES accelerator, to choose where the protocol layer
spends its AES time.

The image is a small composition of the staged boot on the boot hart alone:
its thread-mode executor, the system timer and the RTC watchdog's feed. It
takes the accelerator with `AesParts::into_interrupting` and binds its
interrupt part to a handler at level 2 (line 18) that stamps `mcycle` at its
entry and acknowledges (`AesInterrupt::clear`). One task runs every case,
each timed with `mcycle` around the operation:

| Mode | The block |
| --- | --- |
| `software` | The product's software block (`SoftwareAes128`, RustCrypto AES-128) |
| `software-crate` | CCMP's and BIP's RustCrypto calls as the product makes them (`ccm`, `cmac`, key schedule included) |
| `flag` | The accelerator, polling `STATE` until it reads idle |
| `interrupt` | The accelerator with `INT_ENA` set, spinning until the handler counts an entry |
| `wifi` | The Wi-Fi station's engine (`oer-esp32s31-crypto`): a timer-woken sample every 1 µs through the system timer |
| `dma-flag` | DMA-AES over AXI-DMA channel 0, polling `STATE` for `DONE` and the output descriptor's `OWNER` (`INT_ENA` cleared) |
| `dma-interrupt` | DMA-AES with `INT_ENA` set, spinning until the handler counts the completion, then the same check |

| Operation | Inputs |
| --- | --- |
| `block` | One AES-128 encryption under a loaded key, 256 times |
| `unwrap` | RFC 3394 under a fresh key load: 40 bytes (32 of key data) and 512 bytes (the largest RSN key data) |
| `ccm-seal` | CCMP-128 seal (RFC 3610, 22-byte AAD) of 64, 512 and 1500 bytes, key load included |
| `cmac` | BIP's AES-CMAC (RFC 4493) of 64 and 512 bytes, key load included |
| `start-to-idle` | From the start of a block to `STATE` reading idle |
| `start-to-handler` | From the start of a block to the AES handler's entry |
| `start-to-done` | From the start of a one-block DMA transfer to `DONE` read |

The DMA modes run `block` and `unwrap` as one one-block ECB transfer per
block, `ccm-seal` as a CBC-MAC transfer (B0, the AAD and the body) and a
CTR transfer (the MAC and the body) and `cmac` as an ECB transfer for the
subkey and a CBC transfer (`ops::ccm_seal_passes`, `ops::cmac_passes`,
host-tested against the block-by-block code). The HAL's placed DMA storage
(`peripherals::aes_dma`) holds three buffer pairs (16, 128 and 1568 bytes)
so a transfer copies about its own length in and out. After each DMA case
a `split` line gives the mean cycles per run of its phases: `load` (the
copy into the DMA buffer), `start` (IV, descriptors, channel reset and
start, AES start), `wait` and `finish` (`DMA_EXIT` and the copy out).

`unwrap`, `ccm-seal` and `cmac` run the same code over every block
(`src/ops.rs`, checked on the host against RustCrypto's `ccm` and `cmac`);
`unwrap` uses `oer-crypto`'s RFC 3394 machine (`BlockKeyUnwrap`). The
`wifi` mode runs only `unwrap`, the one operation the station hands the
accelerator. Every hardware result is compared with the software one.

The console prints one line per case,
`OER-PROBE aes-timing <mode> <operation> bytes=<n> runs=<n> min=<cycles>
mean=<cycles> max=<cycles>`, the CPU cycles of 10 ms of the system timer
(`clock cycles-per-10ms=`), and `OER-RESULT: pass aes-timing` when every
hardware result agrees with software (`fail` otherwise). When the interrupt
does not come in Typical mode, it prints `interrupt none-in-typical-mode` and
skips the `interrupt` operations.

Run it with `cargo fw run ../probes/esp32s31-aes-timing --board <board>`
under a lease.
