# ESP32-S31 scheduler execution lock and modify

Source: `esp32s31-bt-lib` `libbtdm_common.a` (pin `10c50778`), member `18.o`
`r_sym_bt_9H3AnHbaHJ3auzSvPDme` (role `r_btdm_sched_execution_lock`),
`r_sym_bt_rPoPGH6BBYjZaunDU5FV` (role `r_btdm_sched_execution_modify`) and
the shared idle preamble `r_sym_bt_B8fDByebTHuwRe8FZuRt`; extracted from
the predecessor repository's `chips/esp32s31/hal/src/bluetooth/scheduler_execution_lock.rs`
and `.../scheduler_execution_modify.rs`. The surrounding insertion and
cancellation contracts are in
[the scheduler lists reference](../notes/bluetooth-scheduler-lists.md).

Facts only: this note describes what can be observed on the registers, not
the vendor's control flow.

## Signals

- BUSY: the MAC diagnostic pair with selector `0x38` written to
  `0x2010_11e8`; `0x2010_11ec` is read until two consecutive complete reads
  agree, and bit 7 of the agreed value is BUSY.
- Engines idle: command 0 (`0x2010_1254`) bit 26 reports the lock engine
  idle and command 1 (`0x2010_1258`) bit 18 the modify engine idle. The
  vendor reads bit 18 only when bit 26 is set.
- Progress (modify only): the diagnostic pair with selector `0x0c01`. A
  conflict state is bits 2:0 equal to 2, or equal to 3 with bits 13:12 equal
  to `0b01`.
- Settle (modify only): the diagnostic pair with selector 7. It has settled
  when the complete agreed value differs from 9.

The vendor issues either command only when BUSY is clear or both engines
report idle.

## Execution lock (command 0)

- Request: `0x2010_1254` bit 31 START, bits 23:20 hardware list index, bits
  19:0 compressed item.
- Done: when BUSY is clear or bit 24 is set.
- Result: bits 29:27; 0 locks, 2, 5, 6 and 7 fail; 1, 3 and 4 are treated as
  impossible.
- Release: the owner clears START when the bracketed list change is complete.

## Execution modify (command 1)

- Request: `0x2010_1258` bit 31 START, bit 16 mode flag (set for whole-list
  deletion), bits 15:0 hardware list mask. The progress selector is chosen
  before START is written.
- Repetition: if any progress sample of an attempt shows a conflict state,
  the attempt is repeated: once it has settled, START is cleared and the
  request is issued again from the idle condition.
- Done: once BUSY is clear or bit 17 is set, and then the settle signal has
  left 9.
- Rejection: bit 19 set after a non-repeated attempt; the vendor asserts that
  it is clear.
- Release: the owner clears START.

The vendor runs each attempt inside one critical section with polling loops
that assert after 9999 iterations. Our implementation is to be designed
independently against the observable protocol above.

## Not established (to measure)

- The effect of execution lock, execution modify and the lock-modify request
  on item execution, and the meaning of each result code.
- The modify mode flag, the lock-modify result and when the environment
  enables the lock-modify request.
- What the progress conflict states (2, and 3 with `0b01`) and the settle
  value 9 mean in hardware.

Each is to be measured on hardware: issue the command against a live list
and record list heads, item status words and diagnostic samples.
