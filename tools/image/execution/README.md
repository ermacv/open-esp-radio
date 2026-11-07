# Execution model on the host

`oer-tools-image-execution` parses and resolves a candidate image execution
model. The first instance is
[`chips/esp32s31/app/execution.toml`](../../../chips/esp32s31/app/execution.toml).
Firmware still consumes the existing chip layout and Rust declarations. This
crate is not a build dependency of an image, and generates no firmware code.

Run its host checks with:

```sh
cargo test -p oer-tools-image-execution
```

## Schema 1

The file declares the chip, the known `features`, the known `instrumentation`
classes, external participants, and the following records. All records except
`requirement` accept an optional `when`. Unknown fields are errors, including
unknown fields in a conditional record's payload.

| Record | Fields and meaning |
| --- | --- |
| `requirement` | `when`, `instrumentation`: instrumentation required by selected features |
| `state` | `name`, optional `requires`: a named runtime fact; `when` selects its availability |
| `context` | `name`, `hart`, `level`, `kind`: `thread`, `interrupt-executor`, or `handler` |
| `entry` | `name`, `context`, `line`, Rust `part` and `handler`, `class`, register-model `sources`, exemption `reason` |
| `executor` | `name`, `context`, wake `entry`, `swi` |
| `software` | `name`, `entry`, `swi`: a non-executor use, currently the radio doorbell |
| `task-set` | Rust set `name`, execution `context`, `observer` index |
| `task-root` | Inventory `name`, `context`, task-role `members` |
| `task` | Logical task-role `name` and `context` |
| `signal` | `name`, target executor, typed `raisers`, optional `active-when` |
| `peer-notify` | `name`, underlying `signal` |
| `timer` | Unique `name`, inventory row name, `entry`, target context, optional `active-when` |
| `hart-start` | Inventory `step` and `hart` |
| `number-use` | Inventory site `id`, `kind` (`swi`, `hart`, `observer`) and typed `owner` |
| `wait` | Inventory `id`, `waiter` and `served-by` endpoints, `service`, `held`, `bound`, optional `active-when` |

`when` is a feature predicate. Its `all`, `any` and `none` lists contain only
declared feature names. All listed `all` features must be selected, at least
one listed `any` feature must be selected when that list is nonempty, and no
listed `none` feature may be selected. Missing lists impose no restriction.
Unknown names, contradictory predicates and unsatisfiable `any` predicates
are refused. For example:

```toml
[[context]]
name = "hart1"
hart = 1
level = 0
kind = "thread"
when = { all = ["group"] }
```

`observe` requires `instrumentation = ["trace"]` in the supplied configuration.
Instrumentation never selects records through `when`. `configurations()`
enumerates all subsets of the declared features and adds each subset's required
instrumentation; `resolve()` refuses a caller configuration missing it.
Optional known instrumentation is also accepted.

`active-when` uses the same predicate grammar over **runtime state names**.
It is preserved during resolution. A state unavailable with the chosen
features cannot be asserted in a runtime snapshot. State prerequisites are
checked too: `network-stopping` requires `radio-started`. The current file
encodes these two phases; other service conditions, such as USB host traffic,
remain explicit manual descriptions rather than executable predicates.

The application's two timer installation paths illustrate the distinction:

```toml
[[timer]]
name = "application timer, radio started"
inventory = "application timer, radio started"
entry = "alarm"
target = "application"
when = { all = ["wifi"] }
active-when = { all = ["radio-started"] }

[[timer]]
name = "application timer, no radio started"
inventory = "application timer, no radio started"
entry = "alarm"
target = "application"
active-when = { none = ["radio-started"] }
```

With `wifi` both records remain in the resolved model. A runtime snapshot
selects exactly one installation path. Without `wifi`, `radio-started` is
unavailable and only the fallback path remains. These describe potential
bindings and waits, not an assertion that every listed operation is active.

## Resolution and checks

`Model::parse` returns the conditional input. `Model::resolve` takes a
`Configuration` and `Hardware` input from the chip facts, register model and
typed SWI parts. It produces `Resolved`, whose records contain no `when` field.
Source names have corresponding resolved numeric IDs, entry contexts have
resolved harts and levels, and runtime-state availability is a boolean.
Serialization with `toml::to_string` gives a deterministic inspection artifact.

Resolution checks selected records for duplicate names, sources, lines, SWI
ownership and observer indices; geometry and equal-level arbitration order;
executor entry/source/context compatibility; handler classes and exemptions;
task/root placement; signal direction; and all referenced participants and
runtime facts. It infers no source-specific hart restriction beyond the
hardware facts supplied to it. SWI 1 is not intrinsically hart 1.

The app test enumerates all **32 subsets** of `wifi`, `group`, `observe`,
`console` and `udp`. `udp` currently changes no execution placement. This is a
model test, without compiling 32 firmware images or claiming they all build.
For each configuration it checks:

- Selected inventory membership, executor/task-set/peer relationships, manual
  wait descriptions, and all 29 handwritten number sites by their stable IDs.
- The actual interrupt-table DSL, including Rust part/handler spelling, cfg,
  class and exemption reason, and the current chip-facts layout.
- Exact ordered equality of source IDs with real `Part::SOURCES` constants.
  No MMIO owner is constructed. The Wi-Fi aggregate is `WifiIrqPart<()>`:
  its `impl<A> InterruptSources` is independent of the acknowledger. A syntax
  guard follows the app/station target-only alias chain to that aggregate.
- Concrete signal statics' target, raising executors, entry tokens and raiser
  kinds through the currently known executor aliases. The pilot and console
  generic factories have explicit composition guards. This is a limited
  syntax adapter; unknown alias/declaration shapes require review. Full Rust
  handler and ownership typing remains the firmware compiler's check.
- Both radio states and the stopping phase, including exactly one active
  application-timer path. Optional trace instrumentation preserves placement.

Mutation cases cover unknown features/states/fields/sources, missing trace,
duplicate sources/lines, wrong SWI/context/hart direction, invalid levels and
incorrect chip selection. A real but incorrect NMI source is structurally
valid and is rejected by the separate typed-source equality check.

The source inventory's layer test remains a separate guard over known
declaration shapes and evidence locations. The new test does not turn its
manual prose into a proof. Task roles name scheduling participants; they do
not replace Rust task-set typing or infer arbitrary future/combinator identity.
The probe-only peer request remains outside this app model. Bluetooth and
IEEE 802.15.4 remain outside its composition.

## Wait risks

Bounds distinguish `none`, `boot-deadline`, `operation-deadline` and
`conditional-deadlines`, with a manually read description. `wait_risks()`
returns active waits with no local bound or only conditional deadlines for
review. In particular, application/group send and receive remain visible;
console reads/writes depend on the USB host. The network-stop operation has
its own deadline, which can end with a refusal rather than successful service.

This API performs no deadlock-cycle proof, scheduling fairness analysis or
proof of eventual wake/service. Runtime snapshots and service descriptions
are assumptions supplied for review, not observations of a running image.
Generating the interrupt table, executors, SWI sites and ELF expectations is
a later change after this schema is accepted.
