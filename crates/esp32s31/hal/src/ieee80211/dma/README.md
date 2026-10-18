# Wi-Fi DMA ownership

This crate owns stable descriptors and packet backing. MAC owners select queues,
prepare transmission parameters and interpret completion; the runtime drives
their interrupt and deadline transitions. Ordinary TX uses internal DMA SRAM.
The diagnostic PSRAM and aggregate paths have separate admission contracts.

## Ordinary TX handoff and reuse

Every ordinary and aggregate publication carries one control frame selected by
the [protection policy](../mac/src/tx/protection.rs) for that PPDU's rate,
Address 1 receiver and PSDU length: none, CTS-to-self or RTS/CTS, with the
control-frame rate and its calibrated power. Selection happens again on every
retry and every retained-aggregate republication; it never rejects a
publication. The MAC generates the RTS or CTS frame and its Duration from the
queue request and the programmed PPDU.

The reviewed queue control has separate software RTS and CTS requests. Ordinary
descriptor-bound preparation replaces both requests with the PPDU's explicit
mode while the queue is idle. The RTS helper preserves CTS and spacing; cold HE
initialization clears CTS and retains RTS and spacing. The RTS request, optional
HE byte-threshold publication, threshold disable and cold-init reset follow the
reviewed [queue argument provenance](../../../../registers/esp32s31/evidence/vendor-libpp.toml);
they have no native compiled vendor comparison. The pinned vendor never
requests CTS-to-self, so that request has no vendor oracle; its on-air contract
comes from hardware captures.

A CTS timeout means the data PPDU was not transmitted. The ordinary owner
republishes the MPDU without the Retry bit; an aggregate owner republishes the
unchanged aggregate, also without Retry bits, until the short-retry limit.

Every arena is published by its DMA addresses in a [`DmaSpace`](src/space.rs):
`ChipDma` on the chip, where a field's address is its DMA address, and the
host models' `HostDma` (test support), where it is `HOST_ARENA_BASE` plus the
field's offset in its arena. The RX arena carries its space and checks the
published layout against it at every preparation; a TX storage takes its space
when its binding is built (`pin_static::<S>`). The arena checks run the same
code in both spaces. Test support supplies models of the hardware (the host
space, the proofs a mock walker reports, and the shared register model of the
RX walker, [`RxWalkerModel`](src/rx_dma/model.rs)), never bypasses of these
checks. A walker stop proof names the arena whose binding it stopped; an RX
ring halts or rebuilds only on a proof naming its own arena, and a proof for
another arena poisons the arena until radio reset.

## RX publication

The RX arena publishes a descriptor table: the ring and, right after it, a
reserved descriptor R with its own buffer
([`RxDescriptorTable`](src/rx_ring.rs)). `RX_DESCRIPTOR_BASE` names R and
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
closed. The rules and
their hardware basis are in
[the RX walker notes](../../../../knowledge/esp32s31/wifi/rx-walker.md).

[`PinnedTxDmaStorage`](src/tx_storage.rs) permanently retains its allocation.
Dropping the movable owner does not free or detach hardware-visible memory.
External aggregate retention takes only `PlacedBacking` leases (a placed
TX pool's, `oer_esp32s31_hal::dma_static`), whose addresses it publishes
without a window check, and stores them without automatic destruction.
Only cancellation before publication or a proven detach releases them. Safe
`forget` of an aggregate owner followed by destruction of its local retention
arena leaks the allocations; neither destructor is needed to keep DMA memory
alive. The arena itself is never referenced by a DMA descriptor.
[`TxSlot`](../mac/src/tx.rs) adds the active queue and generation cookie;
[`OrdinaryTxOwner`](../driver/src/ordinary_tx.rs) retains retry and deadline state.

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

## Vendor basis and executable boundary

The reviewed [ordinary queue provenance](../../../../registers/esp32s31/evidence/vendor-libpp.toml)
names the authenticated archive and complete function bodies. The production
operations mirror three finite vendor queue operations:

- `hal_mac_txq_enable`: the publication prefix through the call to `GetAccess`,
  with explicit production device fences. The rest of the vendor access/HE
  bookkeeping and statistics is outside this boundary.
- `hal_mac_clr_txq_state`: completion selector two, queues zero through three,
  preserving the existing clear-register image.
- `hal_mac_txq_disable`: the complete leaf clearing ENABLE and VALID while
  retaining the descriptor and other control fields.

These operations have no native compiled vendor comparison. They do not
establish physical DMA quiescence or memory-ordering sufficiency on silicon.

The inspected `lmacProcessTxComplete` body reads the completion result and
trigger-flow state before calling the completion acknowledgement. Full result
decoding, descriptor preparation and the enclosing release/abort root have no
compiled vendor comparison either.

The [storage tests](src/tx_storage/tests.rs) exercise publication authority and
quarantine. The [MAC ownership tests](../mac/tests/cases/tx_ownership.rs) exercise
repeated generations, retained payload, stale release and failed detach. These
are host models with explicit completion/detach inputs. An AP-loss/reconnect
hardware run exercised ordinary authentication and association TX through the
real owner; its functional result does not measure the exact last DMA access
or inject a failed detach readback.
