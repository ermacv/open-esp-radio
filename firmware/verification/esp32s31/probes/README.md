# ESP32-S31 compiled verification probes

These packages build retained Rust entry points for instruction- and
effect-level comparison with caller-owned vendor artifacts. The probes depend
on production driver crates, but no production crate or HIL firmware depends
on the probes.

Build and validate the comparison images from the repository root:

```console
cargo verification probes --chip esp32s31
```

The images `oer-firmware-verification-esp32s31-probe-radio-elf` and
`oer-firmware-verification-esp32s31-probe-register-elf` are built for
`riscv32imafc-unknown-none-elf` by `cargo fw probe`, copied into
`target/verification/esp32s31/`, and each image's `.blobray.probes` catalog is
validated against its executable symbols. `cargo fw probe <package>` alone
builds one image without that validation.

The probe build is the only build that sets `cfg(oer_probe)`, which compiles
the probe hooks of the PACs, the HAL, the PHY and the radio crates. It builds
only `firmware/verification/` packages, with the chip's image compiler, into
`target/fw-probe`: no production image or host build compiles a hook.

Each probe set is a library and `-elf` pair: `probe-radio` covers Wi-Fi, PHY
and coexistence and `probe-register` the PAC/HAL register accessors. The
library owns stable retained wrappers and explicit C-layout projections; the
`-elf` package owns only the comparison image entry point and linker layout.
Neither is a board test, runtime adapter or public driver API. The Bluetooth
pair returns with its hooks (`knowledge/esp32s31/open-questions.md`).

The radio probe uses the MAC's executor-neutral `irq::consumer` directly;
its graph contains neither the Wi-Fi runtime nor role-sta. The retained
`open_mac_irq_publication` entry calls the production MAC and power handoffs
through opaque borrows of locally owned, split storage, preserving their waker
paths under LTO. The TX observation wrapper consumes the exclusive finite
batch; the probe owns no global IRQ register store. Their
`.rwtext` functions link inside the S31 SRAM window, separately from the
synthetic trace code. Inspect symbols and function records with Blobray;
registered waker callbacks remain indirect and need the final composition's
placement proof. This probe does not establish that executor callback proof.

## Declaration and validated build

Use the [shared probe compiler](../../harness/README.md) to declare each
entry once with `oer_tool_service_probe_macros::probe!`. It generates the C export, linker root
and embedded ABI catalog. Simple adapters use `=> expression;`; complex and naked
adapters retain explicit bodies. Symbol names and C projections are the
boundary the scenarios select.

Every ELF contains `.blobray.probes`. Build scripts derive roots from the same
Rust declarations used by the macro frontend; there is no retention list to edit
when adding an entry. The catalog is part of the captured production artifact.

The main library's `with_phy` helper keeps the Wi-Fi register and interrupt owners
alive while lending the PHY partition to an adapter. Full platform construction,
Bluetooth setup, async scheduling and assembly trampolines retain their explicit
ownership rules. No private Rust owner layout is inferred from an ABI parameter.

`ets_delay_us` and the post-delay barrier in the software-frequency adapter retain
their optimization semantics. The current ESP-HAL link resolves the delay name
to its ROM address; registration does not override that binding. The owned
software-frequency probe lets the harness-edge scenario
(`verification/esp32s31/scenarios/src/engine/harness_edges.rs`) check an ordinary
call to the captured ROM veneer, its tail transfer to `ets_delay_us`, and the
two-microsecond modeled delay. These checks concern software boundaries, not
physical timing.

No entry adapter exists: Blobray enters every vendor root and production probe
directly. Explicit words fill the argument registers and the stack, and a root's
callee-saved registers start at zero, so evidence and effect contracts bind the
real functions. Bluetooth's
pure calculation and combined calculation/publication probes share one array
projection around shipping `calculate_bluetooth_tx_gain`; only publication acquires
a radio owner. Their full behavior is exercised by the [native gain matrix](../README.md#wi-fi-and-bluetooth-gain-arithmeticpublication).
