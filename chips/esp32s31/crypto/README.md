# S31 crypto

`AesKeyUnwrap` owns the HAL's CPU Typical AES port by value for one client.
It drives the sole RFC 3394 machine in `oer-crypto`: submit its owned input,
wait through an owned timer, then feed back the output or a typed block error.
The mode machine contains no async code. `compose::crypto::wifi_key_unwrap`
selects the hardware engine for HIL wifi-station.

There is no sourced <=20 us completion bound in TRM v0.1 §10.5.3 or the
pinned ESP-IDF 4d80c1b2 AES HAL. Every block therefore yields before its first
completion sample. The scheduling policy samples through timer waits at
1 us intervals and refuses WORK after 1 ms; these are software policy values,
not measured or guaranteed hardware latency. A late IDLE sample completes
successfully before the timeout test. No AES IRQ55 is bound: the HAL retains
its DMA interrupt part with `Silent`, and the engine disables DMA and AES
interrupts explicitly.

The HAL owns the entire crypto clock/reset word and preserves sibling and
reserved bits through RMW. It exposes separate clock, reset, configuration,
key, block, trigger and observation atoms; no whole-word clock image exists.
Software keys are selected, pseudo-rounds are disabled, and FREE is cleared
to prevent RSA_DS preemption (TRM §10.4). The driver sequences these atoms.

The request KEK is erased immediately after loading. Normal completion,
integrity refusal and cancellation reset AES and erase all software key/input
words before disabling its clock. No DMA can reach the request storage.
Timeout and unsupported status retain the failed engine's port; subsequent
requests receive `BlockError::Unavailable` without hardware access. The
portable request, blocks, continuation and response erase secrets on drop.

The generic register port and owned timer factory keep the operation movable
and allocation-free. Tests use a per-test virtual clock and a fake port whose
block oracle is RustCrypto, including shared RFC 3394 conformance, cancellation,
typed failures, poisoning and erasure. The HAL's UI case rejects taking AES
twice from one Root.
