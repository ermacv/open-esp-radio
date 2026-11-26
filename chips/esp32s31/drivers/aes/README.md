# S31 crypto

`AesBlocks` owns the HAL's CPU Typical AES port by value for one owner and
is a synchronous AES-128 engine (`oer_portable_crypto::block::Aes128Engine`): loading
a key sets the accelerator up, and each block is written, started and waited
for in place on the `STATE` flag, its output read back before the call
returns. `oer_portable_crypto::block::unwrap_now` runs `oer-portable-crypto`'s one RFC 3394
unwrap over it, in place in the request's buffer. The engine runs on
the hart its composition places the AES owner on and never yields.
`runtime::crypto::aes_engine` builds it for its owner: the Wi-Fi station,
which unwraps its EAPOL-Key data in its own step; the image that takes it
from the library (`oer`, `AesPeripheral::engine`), such as the HIL
SDK RPC image, which serves its unwraps to the host; or
boot-smoke's hart-1 task, which unwraps RFC 3394 §4.1 for the boot hart.

A block waits on `STATE` in the named settle `BLOCK_SETTLE`
(`oer_portable_settle`, 10 µs on the engine's settle counter: the HAL's
`CycleCounter` at the adopted CPU rate in `runtime`). TRM v0.1 §10.5.3 and the pinned ESP-IDF 4d80c1b2 AES HAL state
no block latency; the bound comes from the `esp32s31-aes-timing` probe (removed; last built at 6ef20cfb2),
which measured 270–279 cycles (0.87 µs) from the start to `STATE` reading
idle over 256 blocks at 320 MHz. A block still working at the end of the
settle stops the engine as `AesStop::Timeout`; any status but idle or
working stops it as `AesStop::InvalidState`. The engine's callers see only
`EngineStopped` (`oer_portable_crypto::block`): what the stop observed stays
in the engine's record, which its holder reads with `last_stop` (the HIL
`aes` service answers it). No AES IRQ55 is bound: the HAL retains its DMA
interrupt part with `Silent`, and the engine disables DMA and AES interrupts
explicitly.

The HAL owns the entire crypto clock/reset word and preserves sibling and
reserved bits through RMW. It exposes separate clock, reset, configuration,
key, block, trigger and observation atoms; no whole-word clock image exists.
Software keys are selected, pseudo-rounds are disabled, and FREE is cleared
to prevent RSA_DS preemption (TRM §10.4). The driver sequences these atoms
and selects encryption or decryption by the direction of each block.

The request KEK is erased immediately after loading. When its keyed view
drops (after the last block, a refusal or an integrity failure), the engine
resets AES and erases all software key/input words before disabling its
clock. No DMA can reach the request storage. A block that times out or ends
in an unsupported status returns its typed error and erases the block; it
ends its keyed run, whose later blocks answer the same error without
hardware access. Nothing of a failure outlives the keyed view: its drop is
the same stop, and the next key starts the accelerator as the first did
(clock, reset, the `STATE` check), as ESP-IDF resets it at every acquire.
The portable request, blocks, continuation and response erase secrets on
drop.

Tests use a fake port whose block oracle is RustCrypto, including the shared
RFC 3394 conformance, the flag wait and its settle on a counter that advances at
every sample, both directions, typed
failures, the ended run, the restart after a failure and erasure. The HAL's UI case rejects taking AES twice
from one Root.
