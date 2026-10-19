# Shared callback address audit: w0/w1

Baseline: `4e9436a3` on main, including #160. Both revisions were built in the
same checkout with the pinned toolchain, `cargo fw build w0` and
`cargo fw build w1` (unconfigured). Each snapshot was copied only after its
build completed. No firmware sources, dependency manifests or lockfiles changed.

`callback-address-audit.json` contains machine SHA-256 values, all eight report
diffs, candidate comparisons and unknown-use counts. Full local bundles, TSVs
and check logs are retained at `/tmp/oer-callback-audit/` for this run.

## Unknown uses

Counts are **function/use relationships**, obtained from `FunctionPointerAudit`.
Each HI20/LO12 relocation retains its own result; a materialization therefore
counts twice. If arithmetic relates the source and result to different functions,
both relationships are retained. Merged symbols share one function entry and
all their uses. These are address-use counts, not unresolved-call-site counts.

| Reason | w0 runtime | w1 runtime | w0 bootstrap | w1 bootstrap | Missing fact |
| --- | ---: | ---: | ---: | ---: | --- |
| `NonExactAddress` | 4273 | 4406 | 412 | 412 | exact entry value without addend, odd bit or interior offset |
| `MissingStorageType` | 330 | 334 | 263 | 263 | one typed function-pointer field at the static word |
| `UnpairedMaterialization` | 106 | 107 | 53 | 53 | adjacent absolute HI20/LO12 pair in one section |
| `ControlFlow` | 68 | 82 | 4 | 4 | all uses across the control-flow boundary |
| `UnknownWrite` | 166 | 354 | 4 | 4 | one typed field at the word store's PC and memory operand |
| `EscapedValue` | 4 | 6 | 4 | 4 | type-preserving destination of the register read |
| **Unknown total** | **4947** | **5289** | **740** | **740** | |
| **Proven typed uses** | **77** | **67** | **39** | **39** | |

`UnsupportedRelocation`, `ForeignCode`, `UnsupportedMaterialization`,
`IncompleteLifetime` and `NoStorage` each have zero observed uses in all four
ELFs. No taken function has an empty evidence list in these inputs. These
unobserved outcomes remain conservative. Regression fixtures cover unsupported
relocation roles, missing evidence, empty candidate sets, missing DWARF and
foreign ELF facts. No function/address allowlist determines a class or a candidate.

## Candidates and reports

| ELF | Global field sites before → after | Local typed sites before → after | Local candidate edges before → after |
| --- | ---: | ---: | ---: |
| w0 runtime | 0 → 0 | 20 → 20 | 6895 → 6895 |
| w1 runtime | 0 → 0 | 27 → 27 | 9873 → 9873 |
| w0 bootstrap | 0 → 0 | 3 → 3 | 333 → 333 |
| w1 bootstrap | 0 → 0 | 3 → 3 | 333 → 333 |

Every individual candidate set is unchanged. These builds have no call field
recognized by the global resolver; they cannot demonstrate the former policy's
precision loss. Its unsupported narrowing is nevertheless removed.

The global callback regression now retains the incompatible functions with
unknown stores: its former two-candidate subset becomes five candidates and
includes the 256-byte `deep` frame. Another regression gives `deep` an unresolved
transfer: the former declaration-only subset hides the hole, while the audited
candidate set reaches it and the root becomes partial. This loss of resolved
coverage is intentional. Both resolvers also retain the same candidate for a
signature cast, one unknown write among typed uses, and an addend.

All **14 machine files are byte-identical**: `runtime.elf`, `runtime.bin`,
`bootstrap.elf`, `application.bin`, `bootloader.bin`, `partitions.bin`,
`otadata.bin` for each image.

All **eight reports are semantically identical**. `placement.txt` and
`interrupt-stack.txt` are byte-identical for both images. Only the `analysis:`
resource row changes in each `runtime-stack.txt` and `bootstrap-stack.txt`.
All other lines, including bounds, unresolved places/reasons, paths, coverage,
assumptions and headroom, are byte-identical. The task and bootstrap
unresolved-site counts remain unchanged. The experimental local resolver is
still excluded from the firmware gate.

| ELF | Work units before → after | Peak accounted bytes before → after |
| --- | ---: | ---: |
| w0 runtime | 76279527 → 78176064 | 151710638 → 156842205 |
| w1 runtime | 98624730 → 102890620 | 151303924 → 158451909 |
| w0 bootstrap | 15680867 → 15710693 | 16160571 → 16348534 |
| w1 bootstrap | 15680897 → 15710723 | 16279359 → 16467322 |

The common DWARF walk now retains local locations for the global resolver's
address audit, and that audit retains register aliases and per-use outcomes.
The declaration/symbol/trait-arity indexes are gone. The table records the net
cost; no second ELF/DWARF traversal or image-fact index was added.

## Reproduction

Capture `target/fw/w0` and `target/fw/w1` under a `before/` or `after/` directory
after each successful build. Use the same checkout path and toolchain for both
revisions; ELF debug paths are part of the byte comparison.

For each captured ELF, run the diagnostic with the platform's pinned ROM and
reviewed summaries (the ROM path below is the normal host-wide pin store):

```sh
cargo build --locked --profile tools -p oer-analysis-riscv-stack --example callback-audit
AUDIT_ROOT=/tmp/oer-callback-audit
AUDIT_ROM="$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf"
for app in w0 w1; do
  for stage in runtime bootstrap; do
    target/tools/examples/callback-audit \
      "$AUDIT_ROOT/after/$app/$stage.elf" "$AUDIT_ROM" \
      chips/esp32s31/platform/rom-functions.toml \
      > "$AUDIT_ROOT/after/$app-$stage-callbacks.tsv"
  done
done
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  "$AUDIT_ROOT/before" "$AUDIT_ROOT/after" --output "$AUDIT_ROOT/comparison.json"
```

The TSV includes every function/use/index/site/role/addend outcome, typed-use
and unknown-class totals, each resolver's recognized sites, and every candidate
edge. The comparator checks each machine byte, preserves complete report diffs,
compares candidate sets per site, and groups newly admitted targets by the
unknown reasons in their actual audit results. It has no function allowlist.

For the baseline API, use the same diagnostic setup, DWARF mode, companions,
summaries and site/candidate rendering, replacing the three audit/resolver calls
with its former signatures and omitting the new per-use rendering block:

```rust
let taken = taken_addresses(&image, &mut keep, &mut control)?;
let global = function_pointer_resolutions(&analysis, &types, &taken, &mut keep, &mut control)?;
let typed = typed_function_pointer_resolutions(&image, &analysis, &types, &mut keep, &mut control)?;
```

## Removed duplicate policies and preparation

- Global `TypeFacts::{function_shape,can_be,untyped_can_be,candidates}` declaration
  matching; both resolvers now call `FunctionPointerAudit::candidates`.
- Callback-only function/declaration, linkage-name, merged-symbol, untyped-unit,
  sized-symbol and trait-arity tables, including the extra symbol-table walk.
- Trait-method parsing and subprogram signature inheritance used by that filter.
- The experimental resolver's private aggregate `Uses { fields, unknown }` and
  its private address audit/candidate loop. Shared per-use evidence replaces it.
- Resolver-private register-alias preparation. It is prepared once in the audit
  and shared with local field discovery.

The existing field-discovery rules, straight-line materialization proof and
executor assumption are preserved. Interprocedural propagation, constructor
summaries and new memory models remain subsequent work.
