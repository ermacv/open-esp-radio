# ESP32-S31 compiled verification probes

This isolated workspace builds retained Rust entry points for instruction- and
effect-level comparison with caller-owned vendor artifacts. The probes depend
on production driver crates, but no production crate or HIL firmware depends
on the probes.

Build and validate the three comparison images from the repository root:

```console
cargo verification probes --chip esp32s31
```

The images `oer-esp32s31-probe-radio-elf`, `oer-esp32s31-probe-register-elf`
and `oer-esp32s31-probe-bluetooth-elf` are built for
`riscv32imafc-unknown-none-elf` into
`target/verification/esp32s31/`, and each image's
`.blobray.probes` catalog is validated against its executable symbols. A plain
`cargo build` of one package produces the same image without that validation.

Each probe set is a `library/` and `elf/` pair: `radio/` covers Wi-Fi, PHY
and coexistence, `register/` the PAC/HAL register accessors and `bluetooth/`
the Bluetooth controller. `library/` owns stable retained wrappers and explicit
C-layout projections; `elf/` owns only the comparison image entry point and
linker layout. Neither is a board test, runtime adapter or public driver API.
The separate Bluetooth pair keeps BLE verification independent of the Wi-Fi/PHY
probe wrappers.

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
entry once with `oer_probe_macros::probe!`. It generates the C export, linker root
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
