# Register evidence

The source catalogs the register model cites (`[[sources]]`, each with its
`id` and its `SOURCE:` description); `cargo registers validate` checks that
every cited id is defined here.

## Probes no longer in the tree

| Probe | Last revision that builds it | Evidence that cites it |
|---|---|---|
| `hil/probes/esp32s31-facts` (`oer-hil-probes-esp32s31-facts`): the PAC-only hardware facts image | `ae4ced2f8` | `hw-sar.toml` (`HW17_S31_SAR_PREPARATION_20261027`) |

A citation of a probe's file reads it at that revision
(`git show ae4ced2f8:hil/probes/esp32s31-facts/…`).
