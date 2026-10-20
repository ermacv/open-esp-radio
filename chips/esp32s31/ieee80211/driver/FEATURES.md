# ESP32-S31 Wi-Fi capability entry point

The S31 driver implements the portable lower-MAC port. The firmware images
provide radio bring-up and scanning; they do not run a station or AP service.

Architecture navigation: [whole-radio map](../../FEATURES.md),
[MAC TX](../mac/src/tx.rs), [MAC RX](../mac/src/rx.rs), and
[shared PHY](../../phy/FEATURES.md).

## Lower-MAC core

The driver core `LowerMacCore` lives in [`src/lower_mac.rs`](src/lower_mac.rs).
The adapter [`src/port.rs`](src/port.rs) implements its lifecycle, phase,
station and access-point interfaces over that core. One P2 task polls the
port; the P3 handlers own only their interrupt epoch and static typed signals.
The task-side `RadioDevice` retains the cooperative registers, PHY actor
transaction and RX ring. No ISR accesses its task-only `RefCell` state.

Eight shared TX tickets and pinned MPDU buffers feed four hardware queues.
Each queue executes its supplied retry plan. Gate, configuration, key and
disable barriers return the original operation owners through terminals.
Uninstall returns stopped owners for a fresh installation; uncertainty keeps
the DMA allocations quarantined. RX buffers require explicit recycling.
GTK changes freeze ingress and drain preceding RX before returning their
permission. An erased key cannot authorize a returned MPDU's retransmission.

The port exposes ordinary MPDUs and RX BlockAck agreements. It does not expose
the optional aggregate, beacon-timing, monitor or air-reservation phases.
Its publication watchdog is 250 ms, as in W2; command and retirement bounds
are 400 ms, including the publication and the finite RX stop.
The operation mapping and portable limits are in
[the port's README](../../../../protocols/ieee80211/port/README.md#implementers).
