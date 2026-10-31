# boot-smoke

The smallest ESP32-S31 staged application adopts the bootstrap's PSRAM
mapping, reports a valid panic left by the previous boot, starts hart 1 and
checks it: one request, one AES known answer on hart 1's AES and its
liveness. It declares `harts = ["boot", 1]`.

During boot, before any executor, the boot hart installs the time driver
with its own executor (`FROM_CPU0`, thread mode) as the SYSTIMER writer on
comparator 0, and grants hart 1's executor comparator 1
(`oer_espressif_executor::grant`). It then launches hart 1 with the root's
one-shot permit under a deadline on the installed clock
(`oer_esp32s31_compose::secondary::launch_hart1`), handing it, through its
`secondary_entry!` mailbox, hart 1's interrupt grant, the interrupt-mode
executor that owns `FROM_CPU1`'s line, that line's interrupt part and table
token, the timer grant, the AES block (`AesParts`, which this image places
on hart 1; images without hart 1 keep it on the boot hart) and the servers
of the boot hart's three requests. Hart 1 runs its generated entry and root
on its own stacks, installs its interrupt stack and the common exception
path and reports ready; the boot hart reports the stack pointer it reported
ready on. Hart 1's main then makes its own controller and router
(`SecondaryHandoff::adopt`), routes its alarm (SYSTIMER target 1, line 17 of
hart 1, level 3, above its executor), builds the AES engine
(`compose::crypto::key_unwrap`) over its router's `Silent` proof for the AES
interrupt, starts its executor on `FROM_CPU1` (line 16 of hart 1, level 2)
with three tasks and enables its interrupts.

The boot hart's executor runs three tasks:

- the RTC watchdog's feed;
- the liveness supervisor (`peer::liveness`), which owns its static seat and
  never ends: a beat every 100 ms, each echoed by hart 1's responder within
  50 ms (stated configuration); an overdue beat records `LivenessOverdue`
  and resets. It also watches the check's two channels (the exchange and
  the key unwrap, seats 0 and 1 of its table): a request still unanswered
  at its deadline records `PeerUnanswered` and resets, whether or not the
  check still waits;
- the check: it posts a request (7) that hart 1's task answers in place (the
  square and its hart); it hands hart 1's AES task the RFC 3394 §4.1 unwrap
  request (KEK `000102…0F`, ciphertext `1FA68B0A…CFE5`); hart 1 waits 2 ms
  on its own timer (comparator 1, programmed by the boot hart's writer,
  alarm on hart 1) and runs the unwrap on its AES engine, whose block waits
  are hart 1's timers too; the check collects the
  response in a second exchange; it compares the plaintext with the RFC's
  key data `00112233…EEFF`; it waits for the supervisor's first round (by
  that round's deadline it completed or reset the image). Each exchange has
  a 100 ms deadline on the boot hart's timer; an expiry records
  `PeerUnanswered` and resets. It writes the results and the verdict
  through the early writer, within its bounded budget per record: the image
  starts no console driver. A mismatch or a missing round fails the boot.

Any launch failure after the release records its stage and resets; the next
boot prints it as `previous fault SecondaryLaunch` or `SecondaryEarlyFault`.

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
OER-HART aes hart=1 ok
OER-HART liveness hart=1 rounds=1
OER-RESULT: pass boot-smoke
```

The liveness line counts the rounds completed when the check reached it;
any count of at least one passes.

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
