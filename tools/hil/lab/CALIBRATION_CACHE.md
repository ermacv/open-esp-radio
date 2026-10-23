# Host calibration records

`calibration_cache::Store` keeps opaque records at
`$XDG_DATA_HOME/open-esp-radio/phy-calibration/<chip>/<board>/<identity>/last-full.json`
(default data base: `$HOME/.local/share`). The board is a canonical stand
`DeviceId`; the physical identity is hex of the format owner's target-derived
bytes. They are separate identities. Chip components accept only lowercase
ASCII letters, digits and hyphens. Records stay outside git.

JSON schema 1 is bounded to 8192 bytes before parsing. It contains the key,
format/schema/algorithm/profile, exact length, CRC32C, SHA-256, opaque hex,
producing image commit and application digest, run identifier, UTC Unix seconds,
boot/ticket, and temperature with units or sensor profile. Image commit, age and
temperature are provenance. Only explicit compatibility fields control host
eligibility. The firmware still validates identity and native replay guards.

`SnapshotDownload` requires a successful cold full status and checks every
page's ticket, offset, size, metadata and temperature, then the whole CRC.
Only its sealed `VerifiedFull` can advance last-full. Partial calibration and
retained RF wake cannot create that proof. Directories are owner-only; files
are created mode 0600. Promotion takes a nonblocking per-key process lock,
then writes/fsyncs a sibling temporary, renames, and fsyncs the parent.
`promote_archived` preserves the same JSON in a run artifact before promotion;
an archive failure retains last-full. A persistence error does not undo a
successful PHY registration or claim that a cache was saved.

`oer_vendor_phy::cache` composes the store with the typed HIL link. Identity
lookup and bounded local read precede Start. Upload requests stay pinned to
the observed boot/session/ticket and share the original 500 ms deadline.
A miss is sent explicitly. Start/status RPCs release the request slot while
the target waits or registers. The host stores bytes without linking a PHY or
HAL in production. Current firmware capability declarations determine whether
this exchange is available.
