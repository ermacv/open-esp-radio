# boot-smoke

The smallest ESP32-S31 staged application adopts the bootstrap's PSRAM
mapping, reports a valid panic left by the previous boot, starts hart 1 and
exchanges one request with it. It declares `harts = ["boot", 1]`: during
boot, before its executor, it launches hart 1 with the root's one-shot
permit (`oer_esp32s31_compose::secondary::launch_hart1`), handing it, through
its `secondary_entry!` mailbox, hart 1's interrupt grant, the interrupt-mode
executor that owns `FROM_CPU1`'s line, that line's interrupt part and table
token and the exchange's server. Hart 1 runs its generated entry and root on
its own stacks, installs its interrupt stack and the common exception path
and reports ready; the boot hart reports the stack pointer it reported ready
on. Hart 1's main then makes its own controller and router
(`SecondaryHandoff::adopt`), starts its executor on `FROM_CPU1` (line 16 of
hart 1, level 2), routed to hart 1's bank, with one task, and enables its
interrupts.

The boot hart's thread-mode executor (`FROM_CPU0`) runs the RTC watchdog's
feed and the exchange: its task posts a request (7) into the one in-place
slot and raises hart 1's executor's signal (`FromPeerHart`); hart 1's task
answers in place (the square and its hart) and raises the boot hart's
executor's signal. The boot task waits for the answer under its executor's
timer with a 100 ms deadline; an expiry records `PeerUnanswered` and resets.
On the answer it writes the exchange and the verdict through the early
writer, within its bounded budget per record: the image starts no console
driver. Any launch failure after the release records its stage and resets;
the next boot prints it as `previous fault SecondaryLaunch` or
`SecondaryEarlyFault`.

```console
cargo fw build boot-smoke
cargo fw run boot-smoke --board s31-c
```

The USB console carries [typed diagnostic records](../../../portable/diagnostics/README.md).
`cargo fw run` and `cargo fw monitor` render them on the host:

```text
OER-PANIC previous src/main.rs:42:7 on hart 0: example panic
OER-HART ready hart=1 sp=0x5002a680
OER-HART exchange hart=1 request=7 answer=49
OER-RESULT: pass boot-smoke
```

The panic line is present only when a retained record survived. A formatted
message is shown as `(formatted message not kept)`; an interrupt panic also
includes its interrupted PC. All retained fields and their order are kept.
The final typed verdict closes the sequence so loss of a previous-panic
record is an error. Target code performs no text formatting.

`target/fw/runs/<time>-boot-smoke/log.txt` is the rendered view;
`console.raw.txt` keeps the encoded console lines. Replay the latter with
`cargo fw decode-console <path>`. A plain serial terminal sees the encoded
`OER-DIAG:01…` lines. No panic handler or retained layout is changed.

The HIL scenario also named `boot-smoke` exercises its own dedicated HIL
image and timer rearm; it is separate from this minimal app.
