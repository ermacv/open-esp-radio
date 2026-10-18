# Blobray CLI: in-process operations over captured executables

`blobray-cli` is the `blobray` command line and the JSON wire types in
`blobray_cli::wire`; the Linux linker adapters live in
[`blobray-linker`](../crates/linker/src/lib.rs). Every operation runs
in the calling process over executables given as bytes and identified by
content: inventory, whole-library analysis and register accesses, synthetic
linking with explicit ROM companions, exact data export, final-image target
audits and in-process execution and comparison. Nothing is imported, stored or
published. `cargo blobray` runs this CLI.
The [architecture](../docs/design/architecture.md) owns module authority;
[contracts](../docs/design/contracts.md) owns identities, assessment and
resource rules; [workflows](../docs/design/workflows.md) lists the supported
scenarios. This page indexes the current references.

An operation's success is separate from its research coverage and from any
comparison verdict: a returned result can describe partial research or a
`DIFF`/`INCOMPLETE` comparison. See [result assessment](../docs/design/contracts.md#result-assessment).
Unsupported ISA semantics remain explicit gaps. There is no general
equivalence proof, TUI or allocation-free core.

## Reference navigation

Choose a question in the [task map](../README.md#choose-a-task), then use the
relevant reference. CLI syntax is also available through `cargo blobray --help`
and each command's `--help`.

`blobray __command-tree` prints every visible command with its long flags as
JSON, tracked in [`command-tree.json`](command-tree.json) so documentation
checks need not build Blobray. `cargo test -p blobray-cli` fails when the
tracked tree differs from the command line; run it with
`BLOBRAY_COMMAND_TREE_UPDATE=1` to rewrite the file.

| Reference | Use it to |
| --- | --- |
| [Inventory and linked images](reference/inventory-linking/README.md) | Inventory captured executables and link images from their objects. |
| [Function and library analysis](reference/analysis/README.md) | Analyze captured code and inspect coverage, symbolic values and explicit gaps; audit final images. |
| [Registers and captured data](reference/registers-data/README.md) | Report library register accesses and export exact captured data. |
| [Concrete execution and comparison](reference/execution/README.md) | Run explicit RV32 scenarios with selected inputs, device models and comparison observations. |
| [Comparison relations](reference/comparison/README.md) | Choose call, memory, branch, layout and effect relations. |
| [Resources and limits](reference/resources/README.md) | Bound an operation's work, time and working memory, and read its failures. |
| [Interfaces, identities and formats](reference/interfaces-formats/README.md) | Reference application owners, content identities and native formats. |
