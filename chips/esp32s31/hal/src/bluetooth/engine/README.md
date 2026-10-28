# Bluetooth controller memory ownership

`Engine` retains one placed `BluetoothArena` for an installation epoch. CPU
views are phased; packet headers and list graphs become writable again only
through their completion/stop protocol. A raw hardware head resolves to an
identity inside that arena before access. Pool and epoch brands reject foreign
or stale leases; forgetting an owner never frees hardware-visible storage.

The physical layouts live in `registers/layouts/` and the generated PAC.
Compressed hardware links and full CPU anchors are distinct types. Decoders
borrow packet views only after completion returns ownership. Unassigned lanes
retain their image without invented bit meanings. SOURCE:
`BLOB_LIBBLE_MEMORY_BUFFER_HEADER`, `BLOB_LIBBLE_MEMORY_RX_PREFIX`,
`BLOB_LIBBLE_MEMORY_LINK_STATE`, `BLOB_LIBBLE_MEMORY_SCHEDULER_ITEM` in
[ble-memory.toml](../../../../registers/evidence/ble-memory.toml).

A scheduler publication retains its graph until the physical execution and
finished-list protocol return it. Live list insertion uses the bounded
lock/modify transaction; conflicts are rejected before publication. Stop,
list deletion and completion do not independently release another epoch's
storage. SOURCE: `BLOB_LIBBTDM_SCHEDULER_EXECUTION_MODIFY`,
`BLOB_LIBBTDM_SCHEDULER_CANCELLATION`, `BLOB_LIBBTDM_SCHEDULER_LIST_DELETION`
in [vendor-radio-libraries.toml](../../../../registers/evidence/vendor-radio-libraries.toml).

The DTM-only capacity profile excludes scanner/global RX graphs. Other role
builders require their arena capacity and publish detached values through
checked anchors. The retained direction-finding workspace is a hardware
baseline, not a software CTE ring. SOURCE: `BLOB_LIBBLE_MEMORY_DF_WORKSPACE`
in the memory catalog.

Successful retirement requires stopped, empty scheduler state and idle workers.
Only checked Controller reset returns the complete arena zeroed in place.
Failure retains the partitions and storage in quarantine until reset; there is
no partial extraction or automatic restoration. The
[driver](../../../../bluetooth/driver/README.md) owns orchestration and the
port runner.
