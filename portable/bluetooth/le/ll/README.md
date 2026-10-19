# Bluetooth LE Link Layer

`oer-bluetooth-le-ll`: the allocation-free Bluetooth LE Link Layer protocol and
role state.

## Known defects

- **AES key schedules are not wiped** (code review, 2026-10-16; record
  outside this repository): `Cargo.toml` enables `aes` without its `zeroize` feature,
  so the `Aes128` built in `security.rs` leaves the session key schedule in
  memory after drop.
