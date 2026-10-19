# PHY HIL evidence

## Fault injection

`phy_fault_injection` advertises destructive checkpoints in real PHY maintenance.
`phy::ControlFault(Arm(mode))` is one-shot per boot. `Status` reports `Reached` only after the
selected physical frontier. `Release` is accepted only then, and its
acknowledgement is serialized before injecting the fault. There is no disarm or
deadline renewal command. `Cancelled` means the actual child future was dropped,
not merely that the Host stopped waiting. A fresh boot must report `Idle` and
MWDT1 reset; RF-off timing is not represented by that observation.

Wi-Fi runs no PHY maintenance of its own: the shared radio's periodic tracking
maintains the PHY, so no Wi-Fi command enters PHY calibration.

## Placement diagnostic

`phy_rx_hot_sram` reports that the direct RX-gain transaction executes from
internal SRAM without changing its graph or ROM short-delay policy.

## Delivery continuity

`network::UdpRxStarted` reports the first 256 valid single-flow UDP datagrams consumed
inside a session. It is correlated by boot/session and is distinct from
`network::SessionReady`: readiness alone does not prove delivery.

Transport and per-flow evidence optionally retain `rx_maximum_silence_micros`
for a complete single-flow UDP receive window, including trailing silence.
Missing observation is `None`, never inferred as zero from average throughput.
Concurrent RX flow windows are not projected into one session continuity value.
