# B0 Controller bring-up composition

The configured image joins the shared radio actor, completes initial tracking,
publishes the P3 owners, executes one 1M DTM RX event on channel 19 and ends it.
Normal teardown removes the BLE routes, retires the timer and Controller,
leaves the actor, resets the Controller and retains its returned arena. The
USB console reports `OER-B0-BRINGUP: returned graph` or `fail`.

The actor, branded lifecycle and large futures occupy static PSRAM storage;
the arena and P3 state occupy their typed DMA/critical storage. Timer owners
cross P3/P2 through affine queues. P2 handover touches the handler cell only
with the BLE routes removed and moves the complete IRQ half by value.

`cargo fw build b0` builds an unconfigured image that prints `unconfigured`
and stays idle. `cargo fw build b0 --analysis` selects the fictitious DE/0 dBm
profile; its Analysis bundle cannot be flashed. The configured composition
currently fails the ordinary frame and IRQ-stack checks. It has no HCI
transport, port-device mailbox or repeating Reset lifecycle and has not been
qualified on hardware.
