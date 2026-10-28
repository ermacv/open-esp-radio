# Linux Bluetooth helper

`open-radio-bluetooth` is the finite Linux helper in `tools/hil/fixture`.
Its shared request/report model is
`tools/lab/fixture/src/bluetooth/model.rs`; host-side library operations are
in [Bluetooth fixtures](../fixtures-bluetooth/README.md). The HIL runner's
current CLI exposes `fixture check SCENARIO` and no Bluetooth-specific check
subcommand.

## Installation

Run the [fixture installation](../run/fixtures.md#linux-fixture-software-installation)
as the unprivileged operator; the installer makes one interactive sudo handoff
for system activation.

```console
cargo stand fixture install --provider linux-bluetooth --dry-run
cargo stand fixture install --provider linux-bluetooth
```

The policy admits a finite list of adapters, defaulting to `hci0`; repeated
`--adapter hciN` arguments select other dedicated controllers. Installation
checks software identity and the helper's `capabilities`, without exercising
an adapter. Runtime checks require the installed finite sudo grants, Linux
Bluetooth interfaces and rfkill access.

## Finite operations

| Helper command | Operation |
| --- | --- |
| `capabilities` | Print the exact supported interface without opening an adapter |
| `check` | DTM on LE 1M, channel 0, PRBS9, with fixed 100 ms RX/TX windows |
| `connect-reset` | Connect to a public peer, exchange bounded ACL data and execute the selected termination |
| `security-failure` | Exercise one named key or MIC failure with a public peer |
| `att-parameters` | Hold fixed 7.5 ms kernel ATT defaults until stdin closes, up to 120 seconds |
| `restore-att-parameters` | Restore an interrupted durable ATT timing snapshot |

For an authorized check of a dedicated adapter:

```console
sudo -n /usr/local/libexec/open-radio-bluetooth check --adapter hci0
sudo -n /usr/local/libexec/open-radio-bluetooth check --adapter hci0 --dtm-version v1
```

DTM defaults to v2. The helper admits `receive-transmit`, `transmit` and
`receive-silence` profiles through `--profile`; the last ends RX with HCI
Reset and records no RX count. Missing command support, malformed responses,
timeouts and restoration failures fail the check. The helper does not switch
command versions after a failure.

The DTM owner snapshots power and soft rfkill, holds the exclusive HCI user
channel, resets after the operation, then restores and verifies the saved
state. Cancellation follows the same cleanup path; abrupt termination or
adapter loss can prevent restoration. DTM TX is real RF activity. Command
acceptance alone does not establish RF delivery or qualify a DUT.

`connect-reset` selects `peer-reset`, `peer-rfkill`, `target-disconnect` or
`target-reset`; `--hold-ms` is bounded to 0–5000. `--encrypted` selects the
fixed public test LTK; `--key-refresh` requires encryption and replaces that
key on the same connection. The model defines the typed reports and explicit
failure cases; inputs cannot select arbitrary HCI opcodes or shell commands.

The helper prints its typed report on stdout. The host library can archive
it and stderr in a caller-owned directory, validate it and write
`result.json`; the DTM summary records `rf_verified: false`. The installed
launchers bind a committed software generation and retain its software lease
for the operation. These software leases are separate from physical adapter
and stand ownership.
