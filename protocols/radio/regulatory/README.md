# Regulatory domain

`oer-radio-regulatory` decides which channels a country permits and at what
power. It speaks frequencies and power, not protocol channels: each protocol
crate implements `RadioChannel` for its own channel type and receives a
`Permitted<C>` back.

- `RegulatoryDomain` is a validated, sorted, disjoint list of `Rule`s for one
  `Country`. The generated tables in `generated.rs` are a subset of
  `wireless-regdb` (see the repository `NOTICE`), produced by
  `tools/regulatory/generate`. Frequency bounds are const parameters checked
  at monomorphization; country and rule-order checks run in const initializers,
  so none of the generated validation has a runtime panic path.
- `RegulatoryDomain::permit` is the only way to make a `Permitted<C>`. Every
  tune, install, scan and transmission of the three radios takes one.
- A channel in a rule marked NO-IR is permitted for listening only
  (`Initiate::PassiveOnly`); a channel in a DFS rule is refused until radar
  detection exists.
- `Permitted::tx_ceiling` combines the rule's EIRP, the board's antenna gain,
  the chip's maximum and an optional owner cap into one `TxCeiling`.
- A domain is fixed for an installation: a country change is a reinstall.
