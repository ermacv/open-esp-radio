# Radio control and lifecycle

`oer-radio` exposes Wi-Fi configuration, requests and affine role
lifecycle through `wifi`. Its contracts do not acquire PAC, DMA or interrupt
owners. Concrete integration supplies the service profile and owns hardware.

The crate depends on no executor or adapter. The command mailbox and the
role-epoch actor that owns physical role frontiers live in the
[radio supervisor](../runtime/radio/supervisor/README.md), an
executor-independent runtime that depends on these contracts. Applications use the separate
[public facade](../oer/README.md), which exposes these same types without
owning or duplicating their state.

Tests use synthetic portable service profiles in `wifi/test_support.rs`.
They do not depend on a concrete chip profile. The `test-support` feature
exposes those profiles to adapter tests; it adds no production behavior.

See the [driver architecture](../README.md) for the complete ownership graph.
