# Linux Bluetooth helper

`open-radio-bluetooth` is the stand's finite Linux Bluetooth helper in
`tools/lab/fixture-helpers`: it owns a dedicated adapter for one operation and
restores it. Its shared request/report model is
`tools/lab/fixture/src/bluetooth/model.rs`. It holds no test sequence of a
device under test; a scenario drives the adapter through it.

## Installation

Run the fixture installation as the unprivileged operator; the installer makes one interactive sudo handoff
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

Inputs cannot select arbitrary HCI opcodes or shell commands.

The helper prints its typed report on stdout; the DTM report does not claim
RF delivery. The installed
launchers bind a committed software generation and retain its software lease
for the operation. These software leases are separate from physical adapter
and stand ownership.
