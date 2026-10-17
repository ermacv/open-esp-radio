# Wi-Fi DMA: execution-level rules

Source: the Wi-Fi execution form of the predecessor repository
(`crates/runtime/esp32s31/ieee80211-form` there). The form itself was not
carried: its interrupt model (a critical section shared by ISR and task) is
replaced by the P3/P2 contexts. The buffer-level custody rules it relied on live in the carried
code (`oer-memory` `indexed_custody`, the `dma` and `mac` crates); the rules
below are the execution-level ones the rebuild must keep. They are the W1
checklist: W1 is the first step where receive DMA runs.

## Rules

1. **DMA-reachable state lives in permanent storage.** The driver's
   descriptors, queues, IRQ banks and execution records sit in one `'static`
   SRAM backing that the image declares once and hands over as a unique
   `&'static mut`. Dropping, forgetting or timing out the future that uses it
   never frees it: an actor still running against it must find it alive.
   Inline storage with a bounded timeout in `Drop` is not safe.
2. **Nothing DMA-reachable is dropped without a proof that DMA stopped.**
   Teardown masks the MAC, power and driver interrupts, closes admission,
   waits for entered actors, then calls `stop` once. Only a stop that proves
   DMA quiescence returns the owners for reuse. Without that proof the owners
   are quarantined: no `Drop`, no backing recovery, no restart, until chip
   reset.
3. **The DMA owner is one type.** Which executor and which stop proof belong
   to which DMA is fixed by the DMA owner's type; a proof of one owner cannot
   release another's frames, and the proof borrows the stopped owners so no
   step can restart DMA before the release.
4. **Keys are erased after every completed stop.** After every stop whose drain
   completed, whatever the DMA fact, pending execution records are withdrawn
   and their key copies zeroized before any owner can be quarantined. A key
   programmed into hardware is zeroized in its record before the reply is
   published.
5. **Execution records survive no reset.** The execution store is placed in
   the critical data range that the staged runtime re-copies or zeroes at every
   reset (`.critical.data.*`), so key copies left by a drain timeout cannot
   outlive the next boot.
6. **A completed reply is a receipt.** Withdrawal returns `Withdrawn`, never
   `Applied`; only an applied close-gate or disable can produce the gate or
   disabled proof.

## What remained open

Starting real DMA and proving its terminal on hardware, multi-descriptor MPDUs
and CCMP on a descriptor chain were never shown on the S31 in the form; the
model had no MMIO. Which register reads prove DMA quiescence is still open.

## Host-test paths

The DMA crates derive a descriptor or buffer address from a pointer as a
`u32`, which does not fit on a 64-bit host. Host tests therefore swap or bypass
product code under `cfg(any(test, feature = "test-support"))`:

- A2: `dma/src/rx_storage.rs` `validate_descriptor_base` and
  `validate_buffer_addresses` skip the address checks.
- A3: `dma/src/rx_storage.rs` `prepare_halted` accepts a raw ring over a
  `Reusable` arena; production returns `Busy`.
- A4: `dma/src/tx_ampdu_storage.rs` `external_dma_address` truncates the
  address without the range check.
- A5: `mac/src/tx/ampdu.rs` gives `HtAmpduTxStorage` inline buffers and an
  upper-only write path (`commit_*`, `can_commit_*`, `next_frame_buffer`,
  `retry::{completed_frame, retain_for_ampdu_retry}`, `ampdu::model`);
  production writes through `HtAmpduTxResources` and the DMA backing.
- B: raw test entries into product types: `dma/src/rx_ring.rs`
  `RxRingHalted::prepare`, `RxRingStopped::prepare`, `take_completed*`,
  `recycle_completed_*`, `observe_completed_unit_link_release`,
  `build_cold_ring`, `publish_cold_ring`, `enable_receive`, `disable_receive`,
  `rearm_descriptor`; `dma/src/descriptor.rs` `publish`, `write_word0`;
  `mac/src/tx.rs` `mark_hardware_owned`; `driver/src/rx/frontier/lifecycle.rs`
  `start`.

At W1 the tests of `dma`, `mac` and `driver` run through the product path. The
choice of how is made then: (1) a DMA address-space type parameter whose
default is the product's pointer-to-`u32` and range check, with a host space
of synthetic low addresses; or (2) running the DMA crates' tests on a 32-bit
host target (for example `i686-unknown-linux-gnu`), where a pointer fits a
`u32` and only the range checks still differ.

A separate layout question, A1: `dma/src/rx_ring.rs` keeps the reload-repair
evidence in six global `AtomicU32` in production, outside the movable ring
token so as not to grow nested async state, but in a token field in tests so
parallel tests cannot race. Whether it moves into the arena next to the
lifecycle word is decided with the same change.
