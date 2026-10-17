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
