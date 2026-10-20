# PC-relative callback audit

Baseline: main `41298bad`, including #170 and #168. Builds and diagnostics use the same
checkout path, pinned toolchain, ROM ELF and reviewed ROM summaries. Both builds
are unconfigured `cargo fw build w0` / `cargo fw build w1` equivalents. The
baseline tool was built before the implementation change and saved separately;
bundles were copied only after successful completion. The saved baseline tools
were compiled at `82840807`; #168 changes only chip BLE memory/PAC sources and
hardware documentation, leaving their host analysis/tool sources unchanged.
Both app captures were repeated after rebasing onto `41298bad`, with identical
results to the earlier capture.

## Reused mechanisms and proof boundary

- `ImageFacts::address_uses` supplies the original relocation index, symbol,
  section, addend and resolved value. No additional ELF or DWARF walk is made.
- The narrow `cha/references.rs` adapter prepares section-scoped
  `PreparedReferences` from those observations. It uses the existing
  `RiscvDecoder::reference`, `PreparedReferences::normalized` and
  `PreparedReferences::pair`, including their uniqueness checks. There is no
  private label-pairing algorithm. Label names and instruction adjacency do
  not establish a PC-relative pair. Original observation IDs remain the
  adapter's physical IDs, and source-object identities use the model's
  controlled artifact hash over captured bytes.
- `RiscvDecoder::value_relocation` must classify the low instruction as
  `LowerPcRelative`. `lift`/`decode` and `is_base_instruction` then check the
  actual linked AUIPC/ADDI instructions and their RV32 value against the
  normalized target, independently of the metadata pairing.
- The existing register-alias prepass now also retains direct incoming targets
  and unknown control-flow/decoding barriers, keyed by section. The common code
  proof uses this information without another instruction/ELF index. All growth,
  normalization capacity, hashing and instruction work are metered.
- Existing PC-scoped DWARF fields prove the word store's type. Both callback
  resolvers consume the same per-function evidence and candidate rule.

The supported value path keeps the upper/lower result in the same register.
Unrelated instructions between AUIPC and ADDI are allowed only when they preserve
that upper value, introduce no control-flow boundary and have no alternate entry.
The subsequent lifetime permits word stores into known typed fields and must end
in a register kill. A bypass into ADDI or the later lifetime, an overwritten base,
an unknown read/store or an unclosed function graph prevents the proof. Both
absolute and PC-relative code materializations use these checks; the stronger
incoming-path checks can therefore restore previously excluded candidates too.
Register-copy propagation, constructor summaries and broader memory models are
outside this change. LO12_S metadata is normalized but is not accepted as ADDI.

A valid low operand belongs to the high's normalized target, not to the function
containing its label. Targets outside the joint function catalog are recorded as
`NonFunctionTarget`, with their original pair and resolved data address. An
invalid pair or unresolved target has no inferred callback owner and conservatively
keeps the entire joint function catalog as candidates. An attributed unknown
keeps its function in every matching site's candidate set. Empty field-type sets
still do not establish an unreachable call. #170's explicit unknown parameter
slots and arity rules remain intact.

## Real materializations

| ELF | LO12 uses | Previously NonExactAddress | Normalized outside function catalog | Function target, ControlFlow barrier | Function target, IncompleteLifetime barrier | Fully proven typed PC-relative stores |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| w0 runtime | 146 | 143 | 43 | 102 | 1 | 0 |
| w1 runtime | 146 | 143 | 42 | 103 | 1 | 0 |
| w0 bootstrap | 71 | 70 | 19 | 51 | 1 | 0 |
| w1 bootstrap | 71 | 70 | 19 | 51 | 1 | 0 |

Thus the reported 143 w1 uses are not deleted without evidence: they are part of
146 low operands, each linked to its actual high relocation and target. Function
catalog entries include reviewed vector data/assembly symbols, so the table says
"function target" in the catalog's sense. **No real pair gains a complete typed
storage proof in these images.** The positive regression demonstrates that proof;
real function targets retain their concrete barriers and remain candidates.

[pairs.tsv](pairs.tsv) contains every original low/high pair, both original symbol
identities and sections, addends, normalized target, before/after low and high
bindings, and the concrete store proof or stopping PC. Data targets are present
as positive normalization results too. [changed-uses.tsv](changed-uses.tsv) includes
every changed function/use relationship, including the absolute proofs affected
by incoming-path checks. Counts are generated from analysis results, not from a
function/address allowlist.

## Candidate changes and reports

| ELF | Global field sites before → after | Local typed sites before → after | Candidate edges before → after | Added edges | Removed edges |
| --- | ---: | ---: | ---: | ---: | ---: |
| w0 runtime | 0 → 0 | 20 → 20 | 6895 → 6845 | 70 | 120 |
| w1 runtime | 0 → 0 | 27 → 27 | 9873 → 9738 | 0 | 135 |
| w0 bootstrap | 0 → 0 | 3 → 3 | 333 → 321 | 0 | 12 |
| w1 bootstrap | 0 → 0 | 3 → 3 | 333 → 321 | 0 | 12 |

All removed edges refer to false callback candidates whose **every previous use
was a PCREL_LO12 label operand**. Normalization places each such use at a different
actual target; none has another address use in the new audit. There are six such
entries in w0 runtime, five in w1 runtime, and four in each bootstrap. This
explanation is asserted for every removed candidate by `explain.py`, using the
observed pairs rather than a list of permitted names/addresses.

Four distinct w0 runtime targets gain 70 candidate edges. Three now have an
`IncomingPath` barrier and one has a `ControlFlow` barrier; the earlier absolute
materialization proof did not justify excluding them. [candidate-proofs.json](candidate-proofs.json)
records every changed target and call site, the unknown-use witnesses for additions,
and the normalized low-index witnesses for every removal. Join those indexes to
`pairs.tsv` for the complete original relocation identities.

**14/14 machine files are byte-identical**, checked byte for byte and by SHA-256:
runtime ELF/bin, bootstrap ELF, application, bootloader, partitions and OTA data
for each app. [comparison.json](comparison.json) records all digests, all eight
report diffs and per-site candidate counts. All eight reports retain every semantic
line: bounds, holes/reasons, paths, coverage, assumptions and headroom. Placement
and interrupt reports are byte-identical. Only the `analysis:` resource row changes
in each runtime/bootstrap stack report. The experimental local resolver remains
outside the firmware gate; these images have no recognized global callback field.

| ELF | Work units before → after | Peak accounted bytes before → after |
| --- | ---: | ---: |
| w0 runtime | 78170401 → 77820380 | 156917392 → 156917392 |
| w0 bootstrap | 15711248 → 15717303 | 16354258 → 16354258 |
| w1 runtime | 102551129 → 100152921 | 158355908 → 158355908 |
| w1 bootstrap | 15710523 → 15716606 | 16468946 → 16468946 |

The new normalization/metadata work and the earlier exits at unclosed-control-flow
barriers are both charged. Runtime work decreases because many formerly scanned
absolute lifetimes are now stopped at such a barrier; bootstrap work increases.
The measured peak memory is unchanged: the new audit's admitted transient storage
stays below the existing whole-image analysis peak. Resource changes are not used
as evidence for candidate exclusion.

## Validation and reproduction

141 stack regressions pass, including the original 131 (#170 included). New cases
cover typed storage, nonadjacent instructions and relocation observations, equal
symbol names, missing/ambiguous/wrong/cross-section pairs, nonzero low addends,
upper addends and interior targets, base clobbers, bypasses into ADDI and stores,
unknown storage, another unknown use, distinct ELF identities at equal addresses,
unsupported lower forms, dead values, cancellation and memory limits.

Tests and clippy pass for the 37-package selection consisting of the stack package,
its reverse dependencies, the six base ELF/RISC-V analysis packages and
`oer-test-layers`, with `oer-hil-lab/test-support` and serial test execution.
Formatting and dependency-layer checks pass. Clippy uses repository warning-level
lints. No manifests or dependency edges changed.

Save each successful w0/w1 bundle under `before/<app>` or `after/<app>`. Run the
existing `callback-audit` example on each runtime/bootstrap ELF with the pinned
ROM and summaries, as in the [previous capture instructions](../callback-address-audit.md#reproduction).
The baseline executable was captured from main before changing the audit; it
emits the original `use`/`site`/`candidate` TSV rows. The new executable additionally
emits `proof` and `pcrel` rows with original identities and normalized meanings.

```sh
cargo build --locked --profile tools -p oer-analysis-riscv-stack --example callback-audit
AUDIT_ROOT=/tmp/oer-pcrel-main41298-audit
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
python3 tools/analysis/riscv-stack/validation/pcrel/explain.py \
  "$AUDIT_ROOT/before" "$AUDIT_ROOT/after" "$AUDIT_ROOT/explanations"
```

All bundles, complete diagnostics, full JSON relationships and check logs from
this run are retained locally at `/tmp/oer-pcrel-main41298-audit/`. The committed tables are
generated views of those results and are not inputs to the analyzer.
