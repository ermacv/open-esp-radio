# Wi-Fi runtime

`oer-ieee80211-runtime`: portable Wi-Fi execution primitives — monitor
handoffs, task shutdown, station network ownership and poll boundaries.

## Known defects

Found in a code review of 2026-10-16 (record outside this repository).

- **`station_service.rs::serve` steps once per loop**, then handles the
  event, tick, command and transmit in one pass; a later input sees the
  machine before the previous one was stepped.
- **Queue bounds are not tied to the backend.** `sta`'s `StaData::new`
  takes a caller `depth` and `ArbiterPolicy::checked` a caller
  `publication_bound`; neither comes from `TX_QUEUE_DEPTH` /
  `PUBLICATION_BOUND` of the lower-MAC owner, so they can disagree.
- **No bound on a gate cycle or a drain.** `drain_port_error` has no
  deadline for a gate that never closes, and `MAX_INPUT_STEPS_PER_BATCH`
  only yields instead of raising a contract fault.
- **Low:** retired-service fields `_pending`, `_arbiter`, `_places` and the
  `MsduQueue` name remain in `station_service.rs`.
