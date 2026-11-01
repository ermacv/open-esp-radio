# Wi-Fi DMA ownership

This HAL module owns stable descriptors and packet backing. MAC owners select queues,
prepare transmission parameters and interpret completion; the runtime drives
their interrupt and deadline transitions. Ordinary TX uses internal DMA SRAM.
The diagnostic PSRAM and aggregate paths have separate admission contracts.

## Ordinary TX handoff and reuse

Every ordinary and aggregate publication carries the caller's explicit
protection exchange: none, CTS-to-self or RTS/CTS, together with the caller's
legacy control-frame rate. Upper-MAC and the family policy make those choices;
the driver supplies calibrated power for that rate. Each retry or retained
aggregate republication uses its supplied attempt decisions. Direct
`ManagementTxPlan` execution keeps its selected data rate and uses no protection.
The MAC generates the RTS or CTS frame and its Duration from the queue request
and the programmed PPDU.

The reviewed queue control has separate software RTS and CTS requests. Ordinary
descriptor-bound preparation replaces both requests with the PPDU's explicit
mode while the queue is idle. The RTS helper preserves CTS and spacing; cold HE
initialization clears CTS and retains RTS and spacing. The RTS request, optional
HE byte-threshold publication, threshold disable and cold-init reset follow the
reviewed [queue argument provenance](../../../../registers/evidence/vendor-libpp.toml);
they have no native compiled vendor comparison. The pinned vendor never
requests CTS-to-self, so that request has no vendor oracle; its on-air contract
comes from hardware captures.

A CTS timeout means the data PPDU was not transmitted. A single-attempt owner
reports that failure; the caller's retry plan decides whether to send again.
Fixed-rate management retries within its supplied budget. A CTS timeout or
collision never sets the MPDU Retry bit; an ACK timeout sets it before the next
publication, and later retries preserve it. Aggregate retries likewise follow
the caller's plan. Republication after protection failure retains every MPDU
unchanged, including its Retry bit.

Every arena is published by its DMA addresses in a [`DmaSpace`](space.rs):
`ChipDma` on the chip, where a field's address is its DMA address, and the
host models' `HostDma` (test support), where it is `HOST_ARENA_BASE` plus the
field's offset in its arena. Every storage is placed
([`Placed`](../../dma_static.rs)): the RX storage records its descriptor base
and buffer addresses once when it is placed (`RxDmaStorage::place`), and only
the resulting `PlacedRxDma` prepares a ring; a TX storage takes its space when
its placed binding is built (`pin::<S>`). The link keeps every placed static in
the DMA window and off an address with zero low DMA address bits
(`dma-window.x`), so no published address is checked against the window at
run time; what the walker reports back (NEXT, LAST) is resolved against the
arena. The arena checks run the same code in both spaces. Test support supplies models of the hardware (the host
space, the proofs a mock walker reports, and the shared register model of the
RX walker, [`RxWalkerModel`](rx_dma/model.rs)), never bypasses of these
checks. A walker stop proof names the arena whose binding it stopped; an RX
ring halts or rebuilds only on a proof naming its own arena, and a proof for
another arena poisons the arena until radio reset.

## RX publication

The RX arena publishes a descriptor table: the ring and, right after it, a
reserved descriptor R with its own buffer
([`RxDescriptorTable`](rx_ring.rs)). `RX_DESCRIPTOR_BASE` names R and
nothing else, by type: `RxDma::write_descriptor_base` takes only the binding.
The cold publication of an epoch is the one BASE write: BASE = R, `R.next` =
the ring head. A frame landing in R before the first ring take is counted and
dropped uncopied; that take clears `R.next`, and any later write into R
fails closed. Every live publication links the accepted tail and rings the
reload doorbell. A settled doorbell that left NEXT zero short of the new
tail is rung once more; a group that would cover the accepted tail appends
all but it. R is never observed, taken, leased or recycled. A detach
(`RxDmaCompletedUnit::detach_single`) clears the descriptor's owner bit
(`OWNER`, bit 31) and fences before the lease is returned, LAST included;
the rearm sets it again. A walker reaching a detached descriptor would stop
on it instead of writing the lease; a NEXT naming one (read by the reload
settle and the frozen cursor) is counted, `walker_on_detached`, and fails
closed. SOURCE: `HIL_W1_RX_WALKER_2026_10_18` and
`HIL_W1_OWNER_BIT_2026_10_18` in
[the evidence catalog](../../../../registers/evidence/hil-open.toml).
An exhausted enabled walker reloads from `LAST.next`; an owner-clear descriptor
stops it unwritten at NEXT. The reload does not require LAST to remain owned.
These bounded 32-descriptor W1 observations do not prove general DMA quiescence.

[`PinnedTxDmaStorage`](tx_storage.rs) permanently retains its descriptor
allocation and holds the one `PlacedBacking` buffer lease attached to it
while free; reservation publishes that lease's placed address. Dropping the
movable owner does not free or detach hardware-visible memory: a lease the
hardware may still read is leaked, never returned. The lease leaves only
while the descriptor is free again.
External aggregate retention takes only `PlacedBacking` leases (a placed
TX pool's, `oer_esp32s31_hal::memory::dma_static`), whose addresses it publishes
without a window check, and stores them without automatic destruction.
Only cancellation before publication or a proven detach releases them, and
the release hands each lease back to its caller in push order. Safe
`forget` of an aggregate owner followed by destruction of its local retention
arena leaks the allocations; neither destructor is needed to keep DMA memory
alive. The arena itself is never referenced by a DMA descriptor.
[`TxSlot`](../../../../ieee80211/driver/src/mac/tx.rs) adds the active queue and generation cookie;
[`OrdinaryTxOwner`](../../../../ieee80211/driver/src/ordinary_tx.rs) retains retry and deadline state.

| State | Permitted operation | Memory ownership |
| --- | --- | --- |
| `Free` | Encode the next MPDU, then reserve | CPU may mutate packet backing |
| `Reserved` | Prepare queue registers or cancel reservation | Descriptor and payload remain retained; hardware queue is not published |
| `HardwareOwned` | Observe completion or begin a qualified abort | No safe buffer mutation or reservation cancellation |
| `Completed` | Detach the exact queue and descriptor | Completion alone does not permit reuse |
| `ResetRequired` | Return failure to the radio lifecycle owner | No local release or reuse, including after a late completion |

Reservation prepares the descriptor, including its hardware ownership word.
That word alone is not the queue publication edge. `TxDmaPublication::commit`
records software `HardwareOwned` before passing the start authority to the PAC.
`start_bound_mac_tx` checks the prepared descriptor head; the production
`start_prepared_mac_tx` fences device access and updates the existing control
word's ENABLE and VALID fields without reconstructing other queue fields.

The PAC checks the queue completion event, samples result registers and trigger
flow, then acknowledges completion. The slot returns the decoded completion and retains its
generation. Normal turnover disables the retained queue, fences, and reads back
ENABLE and VALID. Its scoped detach proof also identifies the descriptor head.
Only that proof permits `Completed -> Free`. A failed readback quarantines the
slot even if the transmission status reported success. A stale software cookie
cannot release a later generation; this does not identify an untagged stale
hardware event after a radio reset, which remains an IRQ-epoch responsibility.

Timeout abort has a separate force-CCA/settle/invalidate/disable sequence.
An executor deadline without the required hardware event quarantines storage.
Neither a timeout nor cancellation is an implicit completion. Per-queue turnover
is not proof that all radio DMA has stopped during global teardown.

## Source and test boundary

SOURCE: `BLOB_LIBPP_ORDINARY_TX_OWNERSHIP_EDGES` in
[the catalog](../../../../registers/evidence/vendor-libpp.toml) names the
publication, acknowledgement and disable bodies. The operations have no native
compiled vendor comparison and do not establish physical DMA quiescence or
memory-ordering sufficiency on silicon.

The [storage tests](tx_storage/tests.rs) exercise publication authority and
quarantine. The [MAC ownership tests](../../../../ieee80211/driver/tests/cases/tx_ownership.rs) exercise
repeated generations, retained payload, stale release and failed detach. These
are host models with explicit completion/detach inputs.
