# HIL scenario model

`oer-hil-model` defines what a HIL scenario is, independently of how a run
executes it.
It is a `formats` package; execution lives in
[`oer-hil-run`](../executor/README.md).

| Module | Owns |
| --- | --- |
| `spec` | `Spec`: the scenario's stable `ScenarioId`, its purpose type, the `Subject` contract it exercises (its `ContractName`), typed `Params`, `Criteria`, `Trace` and `Finding`, its `Resources`, `TargetCapabilities` and `Estimate`, and the pure `verdict(trace, criteria) -> Verdict<Finding>`; `Instance`, a named parameterisation that replaces copied scenarios, and its identity: SHA-256 over the canonical JSON of scenario, instance, parameters and criteria, refused for a NaN or an infinity (`IdentityError`) |
| `verdict` | `Verdict<F>`: `Pass`, or `Fail` with `Findings<F>`, which hold at least one finding when made and when read |
| `measurement` | `MeasuredValue` and `Unit`: what a run measures besides its trace, which the runner converts into the run record's format |
| `finite` | The check that parameters and criteria hold no NaN or infinite number, which JSON writes as `null`, before an identity is computed |
| `purpose` | The purpose as a marker type: `Conformance`, `Interop`, `Measurement` or `Investigation<Q: OpenQuestion>`, so the executor admits a scenario by its purpose at compile time; there is no protocol purpose, because protocol behaviour is proven on the host |
| `name` | Validated names: kebab-case scenario, instance, chip, fixture and composition names; dotted span, measurement, operation and capability names; the contract path; `name!` checks a name at compile time, `parse` at run time |
| `digest` | `Sha256` in its one canonical text form (`parse` refuses anything else with `InvalidDigest`), and `canonical_json`, which sorts object keys itself and writes negative zero as zero |
| `wait` | `Wait`: a contract's ceiling (the port's typed `CEILINGS` value) times a `Factor`, a typed protocol interval times a count (the sealed `ProtocolQuantity`: 802.11 `TimeUnits` of at least one, a BLE `ConnectionInterval` of 6 to 3200, 802.15.4 `Symbols` of at least one, each made by a checking constructor), or a measured stand span; the `Provisional` label of a stand span with too few samples |
| `profile` | `StandProfile`, readable only through its rules: measured spans, each the maximum of its samples (at least one, above zero), its margin and why it was chosen, the `RunId`s of its runs (`RunId::parse` refuses other forms with `InvalidRunId`), and `Support::Measured` only with the 2 995 samples the 99.9th percentile at 95 % needs, else `Provisional { reason }` |
| `repetitions` | `Probability` and `Confidence` in (0, 1), and the samples a quantile bound needs |
| `arithmetic` | Checked time arithmetic: an overflow is the typed `Overflow`, never a saturated value |
| `resources` | Boards by role (`Dut`, `Peer`, `Observer`) and capability (`LpClock`, U.FL), fixtures, and `Resources::unmet`, which matches needed boards to offered ones and names what the stand lacks |
| `capabilities` | `TargetCapabilities`: the chip, `Composition::Any` or the product composition (the same crate as the product's), the agent capabilities, and their union over a tier |
| `estimate`, `tier` | An instance's own time from its waits and repetitions, with its provisional labels (the plan adds the boot); the tiers and their budgets |

Scenario types live in each block's scenario crate. A conformance scenario
follows its port's harness template:
the port's `no_std` conformance crate holds its `Bounds` type with `const
CEILINGS`, its case and trace types with a `wire` feature, the harness, and
a public pure `verify(case, &trace, &bounds)`; the HIL agent sends `(case,
header, trace)` with the
[`oer_hil_protocol::trace_header`](../../../protocols/hil/protocol/src/trace_header.rs)
header; a backend checks its own bounds against the ceilings at compile
time. The executor's
[`harness_template` test](../executor/tests/harness_template.rs) shows
the shape on a toy port.

The crate's `clippy.toml` belongs to the `hil-scenario` copy group: it is
identical to the other HIL scenario crates' copies, and every HIL scenario
package has one.
