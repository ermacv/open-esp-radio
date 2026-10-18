# HIL scenario executor

`oer-tool-service-hil-executor` executes the typed scenarios of
[`oer-tool-data-hil-model`](../../model/README.md) ([#761](https://github.com/the predecessor repository/issues/761)).

| Module | Owns |
| --- | --- |
| `executor` | `Model` around a `ModelDriver` (the host model) and `Stand::open(session, driver)` around a `StandDriver` marker: the stand executor holds the `StandSession` of a granted stand lease, made only from the arbiter's `Grant` (`test-support` adds `StandSession::for_tests`, for dev-dependencies only); the sealed `Executor`, their kinds, and `RunsOn`: which purposes each kind admits; the model runs conformance only |
| `execute` | `Run<X>`: a scenario's execution by executor `X`, typically generic over a HIL trait of reports, implemented by the model agent around the port's harness and by the stand's proxy, so that the model and the stand run the same code; `execute`, which admits the pair only when the executor's kind runs the scenario's purpose, runs one instance, judges its trace with the scenario's pure verdict and records which executor ran it; `Failed` with its `Cause`: `Infrastructure`, which leaves no trace, or an `IdentityError` of the scenario |
| `recorder` | `Recorder`: a run's counts and measurements as the model's `MeasuredValue`s; a name is a count or a measurement, never both, and a count that would overflow is refused |
| `plan` | A tier's plan before anything is built or leased: `Planned::of` an instance; refusal of what the stand lacks (boards, capabilities such as a 32 kHz crystal, fixtures); one image per chip and composition, split into segments (one image on one lane, one lease each) whose lease limit is the estimate times a margin and at most the stand's longest lease; waits sized by the contracts' ceilings; each instance's boot added from the stand profile; the provisional stand spans used; refusal of a plan over the tier's budget, naming its largest scenarios, and of a scenario no lease can hold |

The crate's `clippy.toml` forbids every `core::time::Duration`
constructor (`new`, `from_secs`, `from_millis`, `from_micros`,
`from_nanos`, `from_nanos_u128`, `from_mins`, `from_hours`,
`from_secs_f32`, `from_secs_f64`, `try_from_secs_f32`,
`try_from_secs_f64`), the `oer_base_time::Duration` constructors
(`from_micros`, `from_millis`, `from_secs`), `thread::sleep`,
`thread::sleep_ms` and `spin_loop`: every wait is an
`oer_tool_data_hil_model::Wait`. It is a copy of the `hil-scenario` group, identical in every
HIL scenario crate.

The [`harness_template` test](tests/harness_template.rs) is the conformance
harness template that the per-port conformance crates follow ( Q3).
