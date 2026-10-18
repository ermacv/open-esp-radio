# Symbol lineage

`oer-symbol-lineage` carries source function names from an early vendor
archive to a later one whose functions carry generated names. It reads static
archives of RISC-V relocatable objects, pairs the functions of every adjacent
revision and composes those pairs from the oldest revision to the newest.

```console
cargo run --release -p oer-symbol-lineage -- \
  --repo ../esp32s31-bt-lib --library libbtdm_common.a \
  --obfuscated-prefix r_sym_ \
  --source https://github.com/espressif/esp32s31-bt-lib \
  --out target/symbol-lineage/libbtdm_common.json \
  --names target/symbol-lineage/libbtdm_common.toml \
  --redefine-syms target/symbol-lineage/libbtdm_common.syms
```

With `--repo`, revisions are every first-parent commit that changes the
library, oldest first, or the commits given by repeated `--revision`. A commit
that leaves the archive bytes unchanged is skipped. Without a repository,
repeated `--archive` paths give the revisions in order.

## Pairing

For each adjacent pair, evidence is applied from strongest to weakest. Every
step pairs only functions that are still unpaired on both sides:

1. the same symbol name, unique in both revisions;
2. the same normalized body, unique among the unpaired functions;
3. the call graph: paired callers vote for the callees at corresponding call
   positions. Call lists of equal length correspond position by position.
   Lists of different length are aligned on the callees that already
   correspond (a longest common subsequence), and only positions inside
   equally long gaps between those anchors vote. A pair needs a single,
   mutual vote and at least `--call-graph-minimum-ppm` body similarity;
4. body similarity, accepted only as a mutual best: at least
   `--minimum-similarity-ppm` with a `--similarity-margin-ppm` lead over the
   next candidate in both directions, or else at least
   `--dominant-minimum-ppm` while `--dominance-ratio` times the next
   candidate in both directions (a rewritten function with no close rival);
5. the neighbourhood: an unpaired function is expected among the callees of
   its paired callers' counterparts and the callers of its paired callees'
   counterparts. Each such relation supports a candidate. The candidate with
   the most support, then the most similar body of at least
   `--neighbourhood-minimum-ppm`, is accepted only when no other candidate
   has equal support within the similarity margin, and only as a mutual best.
   This separates identical bodies, such as small getters, by where they are
   called.

Steps 2 to 5 repeat until a round pairs nothing. Each accepted pair leaves
the candidate pools, so a later round can pair a function whose closest
rival has since been paired elsewhere, and a call-graph or neighbourhood vote
can use pairs found by similarity.

Bodies are compared by the vendor fingerprint of
[`oer-vendor-provenance`](../vendor-provenance/README.md), the one that
registers cited vendor functions: it masks exactly the bits that each RISC-V
relocation resolves at link time. Opcodes, registers, relocation types and
branch targets inside the function stay significant; target symbol names do
not. Similarity is the longest common subsequence of its per-instruction
tokens relative to both lengths.

Ambiguity is never resolved by choice. Duplicate names or bodies stay
unpaired unless their neighbourhoods tell them apart, and competing votes or
tied candidates leave functions unpaired. A function that first appears after
the named revision, or whose chain breaks, has no recovered name and needs a
manual decision.

A name is generated when it starts with an `--obfuscated-prefix`. Without a
prefix, a final `_`-separated component of at least sixteen alphanumeric
characters, containing lower case and either a digit or upper case in at
least a quarter of its characters, is treated as generated. Prefer the prefix
when the vendor scheme has one: the heuristic can misclassify a long camel-case
identifier.

## Outputs

- `--out`: JSON report with revision identities, per-step counts and, for
  every function of the last revision, its source name, origin revision, the
  evidence and body similarity of every step since that origin (`steps`), and
  its complete pairing chain whether or not a source name exists (`chain`),
  which follows a function across revisions that are all obfuscated;
- `--names`: compact TOML map from each recovered generated name to its source
  name, origin revision, weakest evidence and lowest body similarity;
- `--redefine-syms`: `generated source` lines for
  `llvm-objcopy --redefine-syms`, which renames the symbols of the last archive
  for disassembly.

The standard output lists each step's counts: pairs by evidence, changed
bodies, pairs below half similarity and unpaired functions on each side.
