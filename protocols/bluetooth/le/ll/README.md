# Bluetooth LE Link Layer

`oer-bluetooth-le-ll`: the allocation-free Bluetooth LE Link Layer protocol and
role state.

HCI is the world boundary. Among conflicting connections the one closest to
its supervision timeout wins. An internal backend inconsistency is a radio
fault that poisons the port. Host backpressure withholds acknowledgement
instead of dropping the link.

## Known defects

- **AES key schedules are not wiped:** `Cargo.toml` enables `aes` without its `zeroize` feature,
  so the `Aes128` built in `security.rs` leaves the session key schedule in
  memory after drop.
