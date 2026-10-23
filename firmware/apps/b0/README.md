# B0 Controller bring-up composition

The configured image serves H4 commands through USB Serial/JTAG on P1:
Reset, LE Rand, 1M Receiver/Transmitter Test v1/v2 and Test End. The physical
P2 worker joins the shared radio actor, completes initial tracking and
publishes the P3 owners. Its command and independent clock mailboxes retain
transactions across cancellation.

Reset removes the BLE routes, retires the timer and Controller, leaves the
actor, resets the Controller and returns its arena before a new installation
can send Command Complete. The empty vendor command `0xfc00` performs the
same ordinary uninstall and sends its completion after graph return.

The actor, branded lifecycle and large futures occupy static PSRAM storage;
the arena and P3 state occupy their typed DMA/critical storage. Timer owners
cross P3/P2 through affine queues. P2 handover touches the handler cell only
with the BLE routes removed and moves the complete IRQ half by value.

`cargo fw build b0` builds an unconfigured image that prints `unconfigured`
and stays idle. `cargo fw build b0 --analysis` selects the fictitious DE/0 dBm
profile; its Analysis bundle cannot be flashed. The configured composition
passes the ordinary frame and IRQ-stack checks. `cargo fw b0 --board s31-a
--peer-board c5-a --base-sizes <common-image/sizes.json>` builds a Flash bundle with the laboratory country and an
explicit 0 dBm cap, leases both boards and exclusive air, and runs the
two-boot 1M DTM procedure against the pinned ESP-IDF peer. Its evidence goes
to the user's XDG data directory. Hardware conformance is separate from
this RF/count procedure; a successful build alone proves neither.
