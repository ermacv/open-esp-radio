# Documentation

Cross-cutting documents. A component's own details live beside it, in its
`README.md` and rustdoc (`cargo doc`).

- [Architecture](architecture.md): goals, layers, execution, ownership,
  hardware, the radio system, ports, protocols and tools; read with
  [CLAUDE.md](../CLAUDE.md) before any task.
- [Radio ports](radio-ports.md): the principles every radio port follows.
- [Time](time.md): clocks, stamps, alarms and their domains.
- [Execution notifications](execution-wakes.md): executor contexts, signals,
  wakers and the reviewed raw-waker inventory.
- [Network](network.md): the network stack and the Wi-Fi egress boundary.
- [Source policy](source-policy.md): what may enter the repository and how a
  recovered value records its source.
- [Protocol naming](protocol-naming.md): IEEE 802.11, Wi-Fi, IEEE 802.15.4,
  Bluetooth and module names.
- [Hardware errata](hardware-errata.md): hardware and ROM behaviour seen on the
  boards.
- [Repository layout](repository-layout.md): the roots, names and where
  things go.
- [Unsafe](unsafe.md): the unsafe boundary.
- [Hardware memory](hardware-memory.md): memory the hardware shares.
- [Memory and stacks](memory.md): regions, placement, stack contracts and
  image checks.

Recovered hardware facts and research notes are in each chip's `notes/` and
`vendor/` below [chips](../chips/).
