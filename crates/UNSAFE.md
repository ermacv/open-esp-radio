# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. The policy lives in standard Rust and Cargo lint settings, so every
`cargo check`, `cargo build` and `cargo clippy` enforces it:

- Most production crates start with `#![forbid(unsafe_code)]`.
- Trusted exceptions start with
  `#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]`, permit unsafe
  code only through documented item-level allowances and retain
  `unsafe_op_in_unsafe_fn = "deny"` from `[workspace.lints]`. Membership in
  the exception list is not blanket permission to add unsafe operations.
- `[workspace.lints.clippy]` denies `disallowed_methods`; `clippy.toml` lists
  them, and an intentional use carries a local `allow` with a reason.

In audited packages every
`unsafe` block and `unsafe impl` needs a `// SAFETY:` comment on the lines
immediately before it, stating why its prerequisites hold. A forwarding
call inside an `unsafe fn` names the `# Safety` contract it relies on. Put the
comment below any `#[allow(unsafe_code, ...)]` attribute: Clippy 1.97 does
not accept a comment above an attribute that spans several lines.

`cargo clippy --workspace --all-targets` applies these lints on the host.
Generated PAC code has no crate-root unsafe attribute. The table below maps
package identities to source owners.

## Generated access and trusted handwritten code

`esp32s31/pac-raw/` is the `oer-esp32s31-pac-raw` package and
`espressif/ieee80211-pac-raw/` the `oer-ieee80211-pac-raw` package of the
Wi-Fi MAC register blocks a chip places at its own addresses. Each
contains only SVD-generated register access, checked through the
generator/publisher pipeline; the shared package contains no address. The
publisher has no declaration for
handwritten modules in this package.

`esp32s31/pac/` contains the handwritten semantic radio PAC and its
generated capability catalog in `src/generated.rs`. The
[PAC provenance map](esp32s31/pac/README.md) distinguishes both generated
Rust outputs from its handwritten ownership and domain modules,
including the IEEE 802.15.4 task/interrupt split. Non-radio register
access through upstream `esp-hal`/`esp-pacs` belongs to
`adapters/esp-hal/soc/`, which also contains cache/MMU and GDMA
transactions. Its Cargo package is `oer-esp32s31-soc-esp-hal`; its implementation
is handwritten code.

The executable policy permits scoped unsafe exceptions in these handwritten
packages. Prefixes below omit `oer-` only to keep the mapping readable.

| Package suffix | Source path |
| --- | --- |
| `memory` | `base/memory/` |
| `embassy-net-owned` | `adapters/embassy/net/` |
| `esp32s31-hal` | `esp32s31/hal/` |
| `esp32s31-pac` | `esp32s31/pac/` |
| `esp32s31-soc-esp-hal` | `adapters/esp-hal/soc/` |
| `esp32s31-phy` | `esp32s31/phy/` |
| `espressif-ieee802154-engine` | `espressif/ieee802154-engine/` |
| `esp32s31-ieee80211-dma` | `esp32s31/ieee80211/dma/` |
| `espressif-executor-embassy` | `exec/executor/` |
| `espressif-interrupt-table-esp-hal` | `exec/esp-hal-irq/` |
| `interrupt-table` | `exec/interrupt-table/` |

These exceptions cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. `embassy-net-owned` has one
unsafe module, `external_rx`: it adopts a detached RX DMA buffer from an
`oer-memory` handoff slot as a Xarxa `PacketBuf` (`ExternalPacketOrigin::new`
and `adopt`) and returns the slot through `release_adopted` when the packet
drops. A slot is adopted again only after that release returned, because the
radio must first claim the slot the release freed. Preserve the proof at the
smallest operation; a safe state machine in an audited crate remains safe.

`oer-memory` holds the shared proofs for executor-crossing storage.
`AffineIndexedSpscQueue` lets an affine permission's place select its value
cell while the audited SPSC ring stores only indices; available ring space
follows from N distinct indices, without an occupancy check, a backend
free-credit pool or another await. An implementation of the unsafe
`UniqueQueueIndex` trait proves index bounds and uniqueness. Producer Release
publishes cells; consumer Acquire moves them completely before permission
return orders the next use.

`IndexedCustody` retains an owner while its affine name travels through an
executor. Its unsafe `CustodyDomain` binds one private store to its affine
place names; this is stronger than an index bound, and two same-typed
installations must not exchange names. Insert returns an initialized-cell
name; only returning that exact name permits take. A name contains only an
index, never a pointer or a borrow into the retained owner. `finish` borrows
the binding's `Stopped` proof that hardware use has ended. Destroying a
non-empty custody store is the quarantine path only: `Quarantine` is an
unsafe trait whose method erases key material in place and releases, moves
and awaits nothing, so DMA leases stay reserved until reset.

## PAC dependency authority

Direct dependencies on the semantic radio PAC are restricted independently
of unsafe syntax. The only allowed consumers are the chip's `pac-raw`, `pac`
and `hal` under `esp32s31/`, and the chip-neutral Wi-Fi MAC register
crates under `espressif/ieee80211-pac*/` that those PACs place: drivers and
adapters reach registers through HAL owners. Check the executable list when changing this boundary.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed and published in the SVD/PAC, not reconstructed
with local masks in a HAL or adapter.

## Active-owner lifetime

Normal stop proves IRQ, RX, TX and queue quiescence before returning reusable
storage. `Drop` cannot perform that asynchronous proof: abnormal drop retains
or quarantines active static storage and faults the lifecycle. It must not
reset hardware, panic as a release mechanism or claim successful shutdown.
Because safe Rust permits `mem::forget`, forgetting an owner must remain a
harmless leak rather than release memory still accessible by hardware.

Task boundaries, cancellation and rendezvous do not weaken the same ownership
or quiescence obligations. The wider execution and
memory-placement contracts are in the
[architecture](../docs/architecture.md).
