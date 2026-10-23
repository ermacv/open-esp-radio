# Calibration cache exchange

The `phy/cache/*` and `phy/calibration/*` messages describe a bounded transfer
of opaque calibration records. The protocol defines no chip identity layout,
record codec, RF ABI value or calibration algorithm. A format's owner defines
its numeric identifier, identity bytes and record schema. Algorithm revision
and PHY profile are separate compatibility fields; an image commit is provenance.
The S31 `diagnostic-phy` image advertises `HostCalibrationCache` and serves the
manual Start path. Its caller selects `Full` or `HostCache` explicitly.

`GetCalibrationCacheIdentity` is read-only. It returns the target-derived
identity and expected format/schema/revision/profile, plus any active ticket.
`StartCalibration` selects a capture configuration and `Full` or `HostCache`.
Its `CalibrationStarted` reply releases the RPC slot before waiting for pages
or performing registration. A nonzero ticket identifies one attempt within the
existing envelope's boot identity. `CalibrationCacheRequested` carries the
same request for an automatic bootstrap, after base Hello.

The host-cache window is 500 ms from issuance of the request. Polls and pages
do not extend it. A caller checks the boot, ticket, open window and deadline
before accepting any page or completion. `WriteCalibrationCachePage` admits
nonempty sequential pages of at most 32 bytes into at most 272 bytes. Its
acknowledgement reports the next offset. Duplicates, gaps, overlaps, foreign
or zero tickets, and late requests are refused without changing the decision.
The range helpers bound byte access; the caller owns deadline and boot checks.

`CompleteCalibrationCache` seals either an explicit miss or an offer with
format, schema, revision, profile, exact length and CRC32C (Castagnoli) over
all opaque bytes. An offer needs matching compatibility fields, the complete
length, checksum and structural decode before it becomes a candidate. No
response or an incomplete upload at the deadline seals `TimedOut` and discards
the upload. A sealed decision cannot be replaced by another completion.

`CalibrationCacheDecision` describes caller admission. `CandidateSupplied`
promises only structural decode. Native registration still validates physical
identity, completion guards and table shape before any replay side effect.
`CalibrationStartStatus` carries that decision separately from the actual
`CaptureReady.path`, full-performed flag and target registration duration in
microseconds. A caller rejection before native admission remains a caller
rejection with a native full path. Registration failure stays `Failed`; a
cache miss is no authority to retry a failed hardware transition.

Only a successful full calibration may expose immutable snapshot metadata in
Ready and serve `ReadCalibrationSnapshot`. The export's ticket, metadata and
temperature observation remain stable across pages. The host checks the final
length and CRC before promoting an export. Temperature carries explicit units
or a raw sensor profile; temperature and age are provenance, not invalidation
rules. Partial registration and retained RF wake do not produce a new full
snapshot for promotion. Existing image keys advertise only services an image
actually implements; compiling the `phy` feature alone grants no capability.

The diagnostic image serves Start/Status;
`PrepareCapture` is the embedded capture configuration. Its P1 dispatcher
publishes Started/Completed before an internal request releases the P2 owner.
An already sealed decision still reaches registration at the original deadline
if console publication stalls; its caller decision does not change. Automatic
`CalibrationCacheRequested` is not advertised by this manual image.
