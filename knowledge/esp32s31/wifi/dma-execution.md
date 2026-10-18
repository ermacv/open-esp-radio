# Wi-Fi DMA: execution-level rules

Source: the Wi-Fi execution form of the predecessor repository
(`crates/runtime/esp32s31/ieee80211-form` there). The form itself was not
carried: its interrupt model (a critical section shared by ISR and task) is
replaced by the P3/P2 contexts. The buffer-level custody rules it relied on live in the carried
code (`oer-base-memory` `indexed_custody`, the `dma` and `mac` crates); the rules
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

The tests of `dma`, `mac` and `driver` run through the product path. A
descriptor or buffer address derives from a pointer as a `u32`, which does not
fit on a 64-bit host, so the arenas carry a DMA address space
(`dma/src/space.rs`): `ChipDma`, the product's, takes the field's own address;
`HostDma` (test support only) places the arena at `HOST_ARENA_BASE` and gives
each field its offset there. The range and alignment checks are the same in
both spaces. RX arenas carry the space as a type parameter; TX and A-MPDU
storages take it where the binding is built (`pin_static::<S>`), and the
A-MPDU binding maps external lease regions through the same space.

Test support holds models of the hardware, never bypasses of the product's
checks:

- the `HostDma` space;
- the proofs a mock `RxDma`/`TxDma` reports from its registers
  (`for_host_test` constructors);
- `Descriptor::write_word0`, the DMA engine's completion write-back;
- `RxDmaStorage<…, HostDma>::dma_write`/`dma_read`, the DMA engine's memory
  port into the arena's ring buffers (an address outside them is an error);
- `Descriptor::inject_corrupt_words`, a fault model (memory corruption or a
  foreign write), used to show that the product's checks catch it;
- `dma` `rx_dma::model::RxWalkerModel`, the RX walker's registers (LAST,
  NEXT, BASE, WALKER_ENABLE, the reload doorbell) with an operation log; a
  test refuses the stop or delays the doorbell through its fields;
- `mac` `irq::model::{MacInterruptModel, PowerInterruptModel}`, both
  interrupt banks: latched events, the enable state, the masked STATUS
  image, `line_asserted()` and an operation log; an
  `ActiveEpoch::for_host_test` owns them like the PAC banks;
- the driver's receive tests compose these into the whole W1 RX path: the
  interrupt line runs through a leaked `HandlerCell` and the same line
  functions the image's handlers call, a host doorbell runs its handler
  synchronously, and frames complete by descriptor word and LAST/NEXT. A
  chained unit needs the DMA engine's write into each non-terminal buffer
  (`dma_write`, which borrows the live ring), so chained and oversized drops
  are tested over the service and the counting sink, not the runner;
- the driver's `rx/test_ring.rs` (test only) is the shared host fixture of
  the product service: a live ring over a leaked `HostDma` arena, completed
  by descriptor word (`complete`) or with received bytes (`complete_with`),
  run until it drains or blocks on staging capacity (`settle`); the
  `S31Prefix` builder writes an RX-control prefix (RSSI, stamp, RXEND
  state, tail RX state, `sig_len`, dump length) before an MPDU; and
  `CountingSink`, the publisher that counts and releases each lease at once;
- the `rx/upper.rs` tests run that service into `QueueSink` and take frames
  through `FrameReceiver`: exact free capacity and no refused send after a
  credit, the receiver's wake, chained and oversized units that never reach
  P1, prefix refusals counted by reason, and beacons through the survey
  (`beacons_through_service_reach_the_scan_table`). The runner tests over
  the queue cover a full queue woken by a P1 drop and both teardown ends:
  P1 drains (`Halted`) and P1 never drains (`DrainTimeout`);
- `mac` `tx::ampdu::model`, a reference model of the vendor's A-MPDU assembly
  for cross-checking, not a product path.

The reload-repair evidence lives in the arena's words beside its lifecycle
(`RxArenaWords`), so arenas do not share it and the ring token stays one
pointer wide. The HE-Control and Trigger MSDU-length facts the removed
test-only A-MPDU write path held are in [he-tb-ampdu.md](he-tb-ampdu.md).

An RX descriptor whose low 20 address bits are zero would read as an exhausted
list in the walker's NEXT register; the ring refuses such a layout
(`descriptor_aliases_an_exhausted_list`) and `cargo fw` warns about a DMA
section placed there.
