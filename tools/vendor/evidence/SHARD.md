# oer-vendor-evidence

The vendor evidence shard format that verification producers write and the
qualification evaluator reads. It belongs to the root workspace in
`qualification/`. HIL evidence is not sharded: it is the runs of the run store.

- `index`: the shard schema, validation, source and coverage-decision currency,
  and the cross-scenario views of a whole evidence directory.
- `store`: the shared shard reader and writer and the shard names of an index.

[`verification/evidence`](../../verification/evidence/README.md) re-exports
this API and owns verdict source policy, the producer contract and producer
orchestration. The verification scenario engine, reports, ESP32-S31 scenarios
and IEEE 802.15.4 host stand also link this format directly. Package metadata
keeps its shared verification role and foundation host layer.

```console
cargo test -p oer-vendor-evidence
```
