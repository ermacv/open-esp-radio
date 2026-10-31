# ESP32-S31 PAC

The PAC is the exact model of the hardware, generated from the
[register model](../registers/README.md) (`types/`, `layouts/`, `chip.toml`,
`evidence/`) and its schema-6 `policy.toml` by
`cargo registers generate --chip esp32s31`
([renderer](../../../tools/registers/model/README.md#the-thin-pac)). It
holds no logic: every sequence, poll, retry, routing and lifecycle decision
over it is the HAL's, each vendor step with its `SOURCE:`.

| File | What it holds |
| --- | --- |
| [`src/generated.rs`](src/generated.rs) | Generated, never edited: a module per block type with its register modules (masks, offsets, `R`, `W`, `Snapshot`, value types) and the owners of its parts; the field accessors of each field's class; typed DMA pointers; `Interrupt` and `instances` (bases, sizes, bound sources); `ownership` (the partitions, `Claims`, `partition`); the irreducible policy items (whole-word images, masked acknowledgements, interrupt snapshots, whole-word-bits writes, pointer ORs); `layouts`; `analog_i2c` |
| [`src/lib.rs`](src/lib.rs) | Exposes the generated API and its single root claim: `take()` claims every part once and returns the partitions |

- **Accessors by class.** Reads, read-modify-writes that keep every bit
  they do not set, writes from zero that take the fields no write may leave
  zero, triggers, doorbells, command starts refusing while busy
  (`oer_mmio::CommandBusy`), write-one-to-clear samples and their
  acknowledgements, FIFO pops and pushes, data-port stores and the strobes
  and stores of shared parts.
- **Typed pointers.** A field the hardware reads as an address is set only
  from an `oer_memory::DmaRef` of its target layout or a `DmaWindowAddress`,
  never from an integer; a pointer's high field only to its placement's
  prefix.
- **Ownership.** Each part of the policy is one owner type over its words:
  the interrupt and task sides of a block, the parts of a divided block,
  read-only and `Copy` views and shared blocks. A part that serves an
  interrupt names its source (`oer_mmio::InterruptSource`). `take()` is the
  one place the claimed words are assumed exclusive; the generated
  `disjoint_blocks` checks prove no two claimed blocks of one hart's context
  list one word. The CLIC is hart-local: its `Clic` and `ClicObserver`
  members are arrays whose element `h` is hart `h`'s own controller.

Radio words have separate parts for modem clocks/resets, PHY Wi-Fi baseband,
modem power mapping, Bluetooth LP timer clocks, coexistence LP clocks, RTC
slow-clock selection and PHY PWDET circuit mode. The coexistence policy
owns timers 0–4 through `CoexPolicyTimers`; `PhyGrantProtectTimer` owns only
timer 5. Partial arrays expose fixed-index accessors and their policy
operations name individual elements, so no operation spans both owners.

There are no handwritten register modules. The HAL's `root::Root::take`
wraps this one claim; `into_bootstrap` and `into_runtime` compose the
phase-specific owners. The PAC holds no boot or radio sequencing.

The USB endpoint owns its raw interrupt observation alongside its enables and
masked status. The system timer owner holds RAW/ST observation alongside the
comparator configuration. Timer group zero's movable program part owns T0
counter/configuration, alarm, load and the entire ENA word. Its separate
permanent interrupt part owns the entire RAW/ST words and only T0's WT CLR
field; T1/WDT CLR fields are unclaimed. There are no other
TIMG0 interrupt consumers: the watchdog uses reset actions. Plain and status
words have one owner; the schema does not split their fields.

The watchdog partition owns each group's clock/reset word. Runtime startup
selects XTAL and enables T0 through the watchdog owner's narrow operation,
which changes only T0's source and gate. MWDT0 retains the clock/reset owner inside
`StoppedMainWatchdog`, so T0 cannot reset the group and restore flash-boot
watchdog enable. The HAL exposes the two disjoint parts through
`Hardware::timg0_alarm`, without arming or acknowledging during adoption.

`tests/` exercises the generated layouts over host memory; the renderer's
own tests prove the accessors' behaviour and what they refuse.

The generated `Hart` and `SecondaryHart` identities and `HARTS` come from the
platform's one HP-hart list. They grant no ownership or launch permission.
Per-hart stall/status lanes, clock/reset words, reset causes, secondary boot
addresses, matrix instances and assist-debug words are indexed families.
`Hart::index` and `SecondaryHart::index` select their respective PAC arrays;
fixed identities are admitted at compile time and raw identities return errors.
The local CLIC remains a context-relative boot-hart owner.
