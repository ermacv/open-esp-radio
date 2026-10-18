# Facts recorded by the predecessor

Carried from the predecessor's issue discussions as written; unverified unless a line says otherwise.

- **92 (b).** For `tools/lib/service/lab/README.md` stand notes: "A `system-watchdog` run on the C5 always leaves it wedged: the intentional watchdog reset drops the C5's USB 30–75 s into the run and it returns half-enumerated until its hub port is power-cycled." (Possibly the same root cause as `docs/hardware-errata.md` C5 rev 1.0 entry — unverified.)
- **336 → stand arbiter / stand file.** "The arbiter state has one schema version; a mismatch is refused; no preservation of fields a newer build wrote; no legacy readers. Allowed resets derive from the chip profile (e.g. C5 `start = "power-on"`, RTS erratum) plus the board's actual wiring, not a hand-written per-board `reset = [...]` list; the board `roles` field is removed." (N still has `#[serde(flatten)]` in `stand-arbiter/src/state.rs:27` and `roles`/`reset` lists in `stand.example.toml:48-51`.)
- **748, research (C5).** From pinned C5 `libpp[trc.o]` recover `trc_init` (+0x64/+0x68/+0x6c) on 5 GHz (`rc11BSchedTbl` record 3, 1 Mbit/s DSSS, is not allowed), `rcUpdatePhyMode` 11a/n/ac/ax and `rcGetSched` (mgmt/EAPOL/Null/BAR). Done when the machine admits a 5 GHz port.
