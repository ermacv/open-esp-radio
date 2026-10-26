# B0 Controller bring-up composition

The configured image serves H4 commands through USB Serial/JTAG on P1:
Reset, LE Rand, 1M Receiver/Transmitter Test v1/v2 and Test End. The physical
P2 worker joins the shared radio actor, completes initial tracking and
publishes the P3 owners. Its command and independent clock mailboxes retain
transactions across cancellation.

Reset retires the timer task and the Controller (the handler's gate gives
back the released bank and explicitly quiets the LP timer), leaves the actor,
resets the Controller and returns its arena and the gate before a new
installation can send Command Complete. The empty vendor command `0xfc00` performs the
same ordinary uninstall and sends its completion after graph return.
Each successful uninstall sends an HCI vendor event (`0xff`, subevent `1`)
before Command Complete: its three parameter bytes are the subevent and the
sampled LP `+0x24`/`+0x2c` bytes, including zeros. P2 queues that observation
before answering uninstall; P1 sends it without inserting text into H4. The
physical B0 procedure logs both bytes and returns every observation in
`lp_timer_pending` in its run report.

The actor, branded lifecycle and large futures occupy static PSRAM storage;
the arena and P3 state occupy their typed DMA/critical storage. One
interrupt table entry binds the Bluetooth handler's part once at boot: the
bank's primary and NRT sources, the modem low-power timer and the radio
doorbell (`FROM_CPU` line 3) on one P3 line. P2 opens and closes each epoch
through the part's gate, whose requests, replies and timer owners cross
through one-slot queues announced by the doorbell; no route is removed and
nothing is masked at the interrupt controller. P1 alone initializes, activates, cancels and retires clock bookkeeping;
P2 constructs the epoch-keyed endpoint and serves its mailbox.

`cargo fw build b0` builds an unconfigured image that prints `unconfigured`
and stays idle. `cargo fw build b0 --analysis` selects the fictitious DE/0 dBm
profile; its Analysis bundle cannot be flashed. The configured composition
passes the ordinary frame and IRQ-stack checks. `cargo fw b0 --board s31-a
--peer-board c5-a --base-sizes <common-image/sizes.json>` builds a Flash bundle with the laboratory country and an
explicit 0 dBm cap, leases both boards and exclusive air, and runs the
two-boot 1M DTM procedure against the pinned ESP-IDF peer. Its evidence goes
to the user's XDG data directory. Hardware conformance is separate from
this RF/count procedure; a successful build alone proves neither.

A non-command H4 kind sends Hardware Error and restores the parser boundary.
An incomplete command expires after 100 ms of inter-byte silence; the next
H4 kind starts a fresh command. These input errors leave the radio enabled.
Each response is one frame written whole to the console's endpoint, which
returns once the host read it. Physical faults request fail-stop before a
single attempt to report Hardware Error that never waits, so host
backpressure cannot delay stopping. A refused port submission returns an ordinary HCI
status without awaiting a terminal. A failed installation answers P1 before
parking with its retained owner.

The command reader keeps what one read brings until the parser takes it, and
waits for the host only until the unfinished command's deadline. P2 retains an active receiver between finite 1 ms command turns, so Test End
can stop a quiet receiver without waiting for a packet. Confirmed stop
reclaims an interrupted event without counting it; naturally completed
events count only valid received packets. Every DTM event restores the
reviewed link-state policy before publication.

The two-boot 1M RF/count gate is qualified on s31-a/c5-a. Hardware port
conformance is unqualified.

A Bluetooth gate invariant failure quarantines all gate and interrupted task
owners without returning or dropping them. The handler records a `GateFault`
through safe `fatal::reset_recording` and resets immediately. The 44-byte retained
record includes the exact reason, entry LP cause, timer phase, NRT words when
the bank is present, and stopped-timer pending bytes or unexpected status.
A vacant timer with nonzero LP status follows the same path. The next boot
prints one `OER-B0-FAULT: bluetooth-gate` line on the early console, before H4
opens, then consumes the record. The application watchdog does not drive this
fault transition.
