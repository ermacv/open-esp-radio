# ESP32-S31 Thread application

This example runs [OpenThread](https://github.com/esp-rs/openthread) (the
`openthread` crate) on the ESP32-S31 IEEE 802.15.4 radio. It starts the IEEE
802.15.4 client on the [shared radio system](../../../crates/runtime/esp32s31/radio/README.md),
spawns the radio system's periodic PHY tracking and hands the client's
radio port to OpenThread through the
[OpenThread radio adapter](../../../crates/adapters/openthread/i/README.md).
The device joins the network of the active operational dataset, logs its
role and addresses on every state change and echoes UDP datagrams on port
1212. It reaches the radio only through the `oer` facade's `openthread`
feature (`oer::systems::esp32s31::embassy::i`), besides the board
runtime and the Embassy executor binding.

OpenThread is built with Coordinated Sampled Listening (the `csl` feature
of the `openthread` fork). With `THREAD_CSL_PERIOD_US` set at build time,
the device runs as a synchronized sleepy end device instead: its receiver
is off when idle and it samples its parent's channel once per CSL period
in windows its radio schedules; the parent must support CSL.

It is also built with Thread Link Metrics (the `link-metrics-subject` and
`link-metrics-initiator` features, as ESP-IDF's
`CONFIG_OPENTHREAD_LINK_METRICS`): a neighbor can probe it with enhanced
ACKs, which then carry the Thread probing IE with the frame's metrics.
Thread network time synchronization is built in too (the `time-sync`
feature, as `CONFIG_OPENTHREAD_TIME_SYNC`): the radio writes the network
time into each frame's Time IE when the frame goes out.

On every role change the device sets its IEEE 802.15.4 coexistence level
as ESP-IDF's `handle_ot_role_change` does with software coexistence:
immediate transmission and reception at the low level while its receiver
stays on when idle, at the middle level as a sleepy device.

The IEEE EUI-64 is derived as ESP-IDF's `esp_read_mac(ESP_MAC_IEEE802154)`
derives it from the base MAC and the eFuse MAC extension. Entropy comes from
the hardware TRNG. Settings live in RAM and are lost on reset.

## Build

The active dataset is build-time configuration: set `THREAD_DATASET` to its
TLV hex, for example the output of `dataset active -x` on the network's
leader. Without it the device starts OpenThread but does not join.

```console
THREAD_DATASET=0e08... cargo fw build thread
cargo fw flash --device <MAC|PORT> --monitor target/firmware/esp32s31-thread/build-<id>
```

`openthread-sys` has no prebuilt library for this chip's single-float ABI,
so its build script compiles OpenThread and MbedTLS from source and generates
their bindings. That needs system-installed Clang, libclang and CMake. Run
Cargo from this workspace to check the example with its embedded target
configuration:

```console
cd examples/esp32s31/thread
cargo check --release
```

The [shared platform](../../../platform/esp32s31/README.md) initializes PSRAM
and relocates the separately linked application. `xtask` checks ELF placement
and stack frames before packaging or flashing; the image stack checks also
make the C and C++ builds emit frame sizes, so OpenThread and MbedTLS frames
are measured with the Rust ones. `openthread-sys` does not rebuild when only
compiler flags change; delete its build directory under the example's
`target/firmware/` cache after a flag change.

## Limits

The example uses the adapter's fork of the `openthread` crate, whose radio
trait carries OpenThread's MAC keys and frame counter: the radio secures
frames and enhanced ACKs as ESP-IDF's OpenThread port does. The
capabilities and limits are described in the
[adapter README](../../../crates/adapters/openthread/i/README.md). The device is a minimal end
device of an existing network: it neither forms a network nor commissions
joiners. The HIL cell `i-thread-exchange` runs OpenThread over the
same adapter against an ESP-IDF Thread leader on air; this example has no HIL
cell of its own, and Thread stays outside the radio/MAC qualification program.
