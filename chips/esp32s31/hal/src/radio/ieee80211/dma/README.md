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
reviewed [queue argument provenance](../../../../../registers/evidence/vendor-libpp.toml);
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

Every table and storage is published by its DMA addresses in a
[`DmaSpace`](space.rs): `ChipDma` on the chip, where a field's address is its
DMA address, and the host models' `HostDma` (test support), where it is
`HOST_ARENA_BASE` plus the field's offset. Every storage is placed
([`Placed`](../../dma_static.rs)): the receive ring's table is projected once
(`RxTable::place`), its buffers are an arena over a placed static, and a TX
storage takes its space when its placed binding is built (`pin::<S>`). The
link keeps every placed static in the DMA window and off an address with zero
low DMA address bits (`dma-window.x`), so no published address is checked
against the window at run time; what the walker reports back (NEXT, LAST) is
resolved against the ring. Test support supplies models of the hardware (the
host space, the ring's model walker, the MAC's reading of a published chain,
`CommitTxDma::read_chain`), never bypasses of these checks.

## The receive ring over the arena

`receive::RxRing` is the receive ring over 5.1's arena (`oer_portable_memory::arena`):
`N` descriptors and R after them, every one always holding a buffer lent
to the walker, and the walker's registers. `publish_cold` arms and links
the ring, R and BASE = R, in its place; `enable_walker` starts the walker.
`serve(clock, f)` lends `f` the completed units up to the `LAST` it read,
in the ring's order: a single unit is exchanged for a spare (`lend`), a
chain or a single is copied out through its `Frame` reader and re-armed in
place (`copy_in_place`), or a unit is dropped and stays, ending the view.
Each re-armed descriptor is linked behind the tail as it is armed; after
`f` the ring rings the doorbell once if anything was re-armed, and again
when NEXT stayed zero short of the tail, each wait within `RELOAD_SETTLE`.
A length the walker must not write and a doorbell it does not accept are
its anomalies (`RingAnomaly`): the view stops at the first, `serve` returns
it with `f`'s result, and the keeper raises the one indication. A unit's header is read from the
floor of the `Handle` its holder has (`Arena::floor`), by the PAC's
layouts; bytes 48 to 52 of the prefix, which the MAC writes late, hold no
field of `WifiRxControlPrefix`.

## The ring's start and stop

The walker's registers leave the Wi-Fi client at the partition's split
(`WifiCold::from_partition`) and live in the ring's parts (`RxRingParts`),
which the cold preparation borrows for its two `RX_CONTROL` writes. The
proofs chain one atom to the next, each made by one HAL operation:

| Step | Takes | Gives |
|---|---|---|
| `WifiPowered::enable_clocks` | | `MacFirstReset`, one per clocking |
| `RadioDomain::reset_wifi_mac_block` | `MacFirstReset` | `MacBlockReset` |
| `initialize_wifi_mac` | `MacBlockReset` | `MacPrepared`, one per preparation |
| `WifiMacHal::open_core` | `MacPrepared` | `MacCoreOpen` |
| `RxRing::publish_cold` | `RxRingParts`, `MacCoreOpen` | the ring, `WalkerStart` |
| `RxRing::enable_walker` | `WalkerStart` | `WalkerEnabled` |
| `WifiMacHal::start_receive` | `&WalkerEnabled` | |
| `RxRing::stop_walker` | `WalkerEnabled` | `WalkerEdge` |
| `WifiMacHal::request_stop` | `WalkerEdge` | `StopRequested` |
| `RadioDomain::reset_stopped_wifi_mac_block` | `StopRequested` | `MacBlockReset`, `MacStopReset` |
| `RxRing::release` | the ring, `MacStopReset` | `RxRingParts`, every buffer's handle |

So a pulse of `RST_WIFIMAC` follows a clocking or a stop request, and a
reset under a running MAC cannot be written. A stop ends in the block's
reset; the next start is cold, from the preparation, with the stop's
`MacBlockReset`.

## Source and test boundary

SOURCE: `BLOB_LIBPP_ORDINARY_TX_OWNERSHIP_EDGES` in
[the catalog](../../../../../registers/evidence/vendor-libpp.toml) names the
publication, acknowledgement and disable bodies. The operations have no native
compiled vendor comparison and do not establish physical DMA quiescence or
memory-ordering sufficiency on silicon.

The [storage tests](tx_storage/tests.rs) exercise publication authority and
quarantine. The [MAC ownership tests](../../../../../radio/ieee80211/backend/tests/cases/tx_ownership.rs) exercise
repeated generations, retained payload, stale release and failed detach. These
are host models with explicit completion/detach inputs.
