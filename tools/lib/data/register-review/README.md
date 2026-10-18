# Sparse reviewed knowledge

`oer-tool-data-register-review` owns the generic, human-sized facts that survive
regeneration of binary-analysis observations and vendor artifact updates.

The crate deliberately does not know about specific chips, register names, ABI
catalogs, SVD, ELF or output paths. Consumers interpret
the consumer-owned `kind` and `value`, while the generic layer owns canonical
typed semantic subjects and blob-local occurrence identities. It validates:

- stable pack, assertion, vendor-bug and entity-binding identifiers;
- evidence, provenance and applicability;
- artifact SHA-256 guards;
- fail-closed conflicts between assertions with the same subject and kind;
- one semantic target per occurrence in overlapping applicability, while one
  semantic entity may intentionally collect multiple occurrences;
- exact artifact applicability and occurrence-linked evidence for bindings;
- deterministic merge and semantic fingerprints.

An empty pack is a valid generated review destination. It contributes no facts
until a reviewer adds a record.

Generated observations are not accepted facts and do not belong in these
packs. A project should add a record only after a reviewer accepts a name,
hardware semantic, ABI meaning or vendor defect.

Register-model consumers accept one `register-identity` assertion whose
canonical `register:chip/address-space/address/width` subject owns the physical
coordinates and whose scalar string value is
`REGION.NAME`. That one reviewed fact identifies existing geometry or
authorizes materializing absent geometry. The domain-agnostic review-model
keeps the semantic domain typed; the register consumer validates the scalar and
rejects the `register-declaration` and `register-name` kinds. Merely
observing software reads or writes never constitutes an identity fact and
never proves hardware access or W1C semantics.
