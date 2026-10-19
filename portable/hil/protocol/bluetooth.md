# Bluetooth HIL protocol

## Trouble GATT

The `bluetooth::Gatt` property identifies the separate plaintext Trouble
application image. `bluetooth::GetGatt` returns `BluetoothGattEvidence` with
the Controller address, application connection/read/write observations and
the CPU0 stack measurement. All observations belong to the envelope's boot;
the query neither resets the Controller nor submits ATT/HCI work. Counters are
not independent proof of ATT delivery. The Linux peer validates actual replies
and reconnection. This property does not imply pairing or secure GATT.

The `bluetooth::Trace` property marks an image that records the Bluetooth LE
trace (`bluetooth-trace`): the Controller, radio role and runtime events of
`oer-bluetooth-trace`, drained through the
[trace commands](diagnostics.md#event-trace). It adds no message.

The separate `bluetooth::SecureGatt` property identifies the actual secure
application with one caller-owned RAM bond slot. `bluetooth::GetSecureGatt`
reports traffic counters, the current unanswered Numeric Comparison challenge,
independent confirmation/bond/reconnect/notification counters and application
failure. It does not export keys. `bluetooth::ConfirmGatt` requires the exact boot,
session zero, challenge ID and six-digit number. Stale/duplicate replies are
rejected. `bluetooth::GattDecisionRecorded` acknowledges a queued UI decision,
not successful pairing or protected access. Cancellation of the application
prompt revokes even a queued answer. These commands cannot submit HCI or ATT
traffic, clear bonds or force a security transition.

`bluetooth::RestartGatt { epoch }` accepts only the currently running secure
application epoch, on session zero and the discovered boot. Duplicate, stale
and stopped-application requests are rejected. Its initial snapshot acknowledges
the request, not completed retirement. Completion requires a new `epoch`,
`restarting = false`, a physical `cold_releases` increment and `old_hci_closed`:
both command and read operations on the retired handle must reject access.
The target rebuilds Host/Controller without resetting the SoC and retains only
the caller's RAM bond store and comparison history, not connections or ATT state.
Independent encrypted peer traffic without another pairing establishes key reuse;
these lifecycle counters alone do not.

`bluetooth::GattResetReadGate { epoch, release }` controls a one-shot secure HIL
reader gate on the discovered boot and current epoch. Arming (`release = false`)
requires an advertising, disconnected application and does not send Reset.
An explicit restart drops the old producers and submits its real final Reset;
the wrapper suspends the sole new reader before it consumes a response.
`reset_read_gate = ReaderHeld` is the reached checkpoint, not merely an arm ACK.
Release is accepted only at that checkpoint during the same restart. It wakes
the same reader; no response is fabricated, discarded or matched to a new Reset.
The hardware runner remains polled. Cancellation does not open the gate.
The gate does not report RF cessation or model a hung silicon operation. While
held, the unchanged epoch and cold-release count must be observed; release must
be followed by actual retirement/restart and independent encrypted peer traffic.

`bluetooth::FailGattResetRead { epoch }` selects failure instead of release at the
same reached checkpoint. Early, stale or duplicate requests are rejected.
`FailureRequested` acknowledges only the command; `ReadFailed` means the wrapper
actually returned its distinct injected error without reading or discarding the
real response. The epoch runner must report `InjectedReceiveFailure`, preserve
its original stop cause and retain physical execution. Release/restart are then
rejected. This tests a Host-facing I/O error, not a silicon or PHY failure; no
RF-stop, physical-close or autonomous-recovery claim follows from retention.

`bluetooth::FailNextGattBondLoad { epoch }` is a one-shot secure HIL diagnostic.
It requires a connected, bonded application on the current boot/epoch, rejects
duplicates and excludes a concurrent restart. Arming it does not stop the Host.
The next actual application store load returns a backend failure without
changing the RAM record. The scenario causes that load by disconnecting its
peer. `bond_load_failures` counts consumed injections; `shutdown` reports the
redacted cause and Reset outcome from the completed Host epoch. Neither proves
physical release: `cold_releases` and `old_hci_closed` remain separate checks.
An unresolved Reset may retain physical execution. The diagnostic does not
inject silicon failure, measure RF cessation or introduce a shutdown timeout.

Secure and plaintext GATT images are mutually exclusive. Both snapshots carry
common traffic observations, which alone never establish security. Notification
counters mean Host queue acceptance; the independent peer must receive the value.

## Direct Test Mode and raw HCI

The `bluetooth-dtm` image declares `bluetooth_dtm` and `bluetooth_hci` and
runs the production Controller behind an in-image HCI Host.
`bluetooth::RunDtm(operation)` runs one fixed LE 1M, channel 0, 37-byte PRBS9
Receiver Test, Transmitter Test, Test End or Reset and returns
`BluetoothDtmEvidence` with the boot's reset reason and the counted packets.

`bluetooth::ExchangeHci(Command { opcode, parameters })` sends one HCI command with at
most `BLUETOOTH_HCI_PARAMETER_BYTES` parameter octets and returns its Command
Complete or Command Status packet (`Completed`), `Timeout` or
`TransportFailed`. Other events that arrive meanwhile are queued.
`bluetooth::ExchangeHci(NextEvent { wait_ms })` returns the oldest queued event, or the
next one within `wait_ms` (`Event { packet, dropped }`), or `NoEvent`;
`dropped` counts events lost to the bounded queue since the last returned one.
Host workloads drive advertising, scanning and connections through these two
requests with standard HCI.
