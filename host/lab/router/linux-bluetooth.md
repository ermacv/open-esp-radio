# Linux Bluetooth peer: notes

Facts about a Linux host adapter as the Bluetooth LE peer of a board, from
stand use (an Intel AX211 adapter, BlueZ, kernel ATT sockets; records outside this repository),
2026-09 to 2026-10 (AX211 Test End quirk 2026-09-28, the connection
collision 2026-10-09). The stand core here has no Bluetooth fixture yet; these
are what the next one must handle.

## Adapter ownership

- The adapter is dedicated to the test. Before a raw HCI test a fixture
  locks it, saves power and soft-rfkill state, unblocks it, powers the
  kernel controller down and takes the exclusive HCI user channel; afterwards
  it resets the controller, releases the channel and restores and re-checks
  power and rfkill. Forced termination or device removal can skip the restore.
- Kernel ATT paths (GATT, advertising workloads) use the kernel and BlueZ
  instead of the user channel: the adapter must start powered off; BlueZ,
  `busctl` and user access to `/dev/rfkill` are needed. After rfkill is
  cleared BlueZ re-registers the adapter for a short while; setup needs a
  bounded retry there.

## Direct Test Mode

- Profile used: LE 1M, channel 0 (2402 MHz), 100 ms RX and TX windows,
  37-byte PRBS9 packets, a 2 s command deadline; both runs end with
  `LE Test End`.
- **AX211:** after an RX test that received nothing, `LE Test End` never
  completes. End such a test with HCI Reset instead and report "ended by
  reset" rather than a count.
- **`bt-hci` 0.10.1** assigns the LE Transmitter Test v1 command the opcode of
  LE Read Supported States; a v1 transmit test needs a corrected opcode.
- A command acceptance check alone shows nothing about RF: a zero RX count is
  valid without a coordinated transmitter. A board-to-PC DTM run holds the
  ESP's TX while the PC counts, then keeps the ESP in RX while the PC is
  silent and then transmitting; each receiver has a positive count and a
  zero-count control with the other side inactive.

## Connection parameters

- The kernel's default LE connection parameters (minimum and maximum
  interval, latency, supervision timeout) are set with MGMT Set Default
  System Configuration and read back with Read Default System Configuration;
  a run sets 7.5 ms, latency 0, 2 s on a powered-off adapter, saving
  the old four values first and restoring them afterwards. A cached per-peer
  override in BlueZ beats the default; the board must report the interval it
  actually got.

## Connections the board never received

- A `CONNECT_IND` can collide on air with another device's response to the
  same `ADV_IND` (for example a nearby scanner's `SCAN_REQ`). The adapter then
  reports the connection created and closes it with 0x3E (Connection Failed
  to be Established) while the board never saw it.
- The kernel can report an ATT socket connected before the first procedure,
  so the host's connect proves nothing. An attempt counts as established only
  when the board reports the connection (a fixture waits 2 s).
- Retry only when the board's own state proves the connection never reached
  it: no new connection, not connected, still advertising. A fixture allows
  three attempts in all and records every attempt.
- A confirmed connection can also end by supervision timeout (0x08) before
  any ATT exchange; that is a separate loss from one never established.

## Pairing through BlueZ

- An agent registered (`DisplayYesNo`) on the same D-Bus connection that
  issues `Pair` receives the callbacks without replacing the desktop agent;
  accept callbacks only from that BlueZ owner for the selected device. A BlueZ
  restart invalidates the ownership.
- Numeric Comparison can be automated by comparing the number the board
  prints with the one the agent receives; replying only on a match.
  Agent callback limit 25 s, D-Bus call limit 35 s.
- Notifications are received through `AcquireNotify`, not inferred from a
  cached characteristic value.
- BlueZ persists the peer's bond. A device record left by an interrupted run
  must not be deleted or adopted silently; check its absence before a run and
  remove only the record the run created.
