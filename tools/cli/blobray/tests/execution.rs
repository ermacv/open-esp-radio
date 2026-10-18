#![cfg(target_os = "linux")]
use blobray_application as app;
use blobray_domain::*;
use oer_riscv_model::*;
use std::time::{Duration, Instant};
mod support;
use support::{executable as elf, executable_with_symbols as elf_with_symbols};
/// Work, working-memory and time limits of one execution.
struct Budget {
    max_work_units: Option<u64>,
    working_memory_bytes: Option<u64>,
    timeout_ms: u64,
}
fn budget() -> Budget {
    Budget {
        max_work_units: None,
        working_memory_bytes: Some(32 * 1024 * 1024),
        timeout_ms: 30000,
    }
}
struct Fixture {
    /// The fixture's executables; a target names them by content.
    inputs: Vec<app::in_process::Executable>,
    /// A target mapping the first input.
    target: ExecutionTarget,
}
impl Fixture {
    fn new(code: &[u32]) -> Self {
        Self::from_inputs(vec![elf(code)])
    }
    fn from_inputs(inputs: Vec<Vec<u8>>) -> Self {
        let inputs: Vec<_> = inputs
            .into_iter()
            .map(app::in_process::Executable::new)
            .collect();
        let target = ExecutionTarget {
            executables: vec![inputs[0].id().clone()],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        Self { inputs, target }
    }
    /// The content identity of input `index`.
    fn id(&self, index: usize) -> ArtifactId {
        self.inputs[index].id().clone()
    }
    fn request(&self) -> ExecutionRequest {
        let invocation = Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
            goal: ExecutionGoal::Return,
            entry: 0x1000,
            arguments: vec![Some(0); 8],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
        };
        ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: self.target.clone(),
            replacement: Some(self.target.clone()),
            binding: Some(CompiledBinding::SharedCore),
            cases: vec![ExecutionCase {
                relation: Some(fixture_relation(true)),
                reset: SessionReset::Cold,
                stack_fill: None,
                name: "case".into(),
                vendor: invocation.clone(),
                replacement: Some(invocation),
            }],

            max_events: 16,
        }
    }
    /// Execute and compare `r` in process under the work, working-memory
    /// and time limits of `b`.
    fn run(&self, r: ExecutionRequest, b: Budget) -> Result<Executed> {
        self.run_with(&r, &[], &[], b)
    }
    fn run_with(
        &self,
        r: &ExecutionRequest,
        effects: &[EffectContract],
        projections: &[LayoutProjection],
        b: Budget,
    ) -> Result<Executed> {
        let memory = WorkingMemory::new(b.working_memory_bytes.unwrap())?;
        let result = app::in_process::verify(
            &app::in_process::InProcessComparison {
                request: r,
                executables: &self.inputs,
                effects,
                projections,
                vendor_results: None,
                dependence: None,
                patches: &[],
            },
            &blobray_backend_riscv::RiscvExecutor,
            &memory,
            &mut Limits {
                units: 0,
                limit: b.max_work_units,
                deadline: Instant::now() + Duration::from_millis(b.timeout_ms),
            },
        )?;
        Ok(Executed {
            verdict: result.verdict,
            complete: result.complete,
            records: result.records,
        })
    }
}
/// Work and time limits of one in-process execution.
struct Limits {
    units: u64,
    limit: Option<u64>,
    deadline: Instant,
}
impl RunControl for Limits {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.units = self.units.saturating_add(units);
        if self.limit.is_some_and(|limit| self.units > limit) {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "work budget exhausted",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(Error::new(ErrorCode::TimedOut, "time budget exhausted"));
        }
        Ok(())
    }
}
/// A request executed and compared in process.
#[derive(Debug)]
struct Executed {
    verdict: Option<ComparisonVerdict>,
    complete: bool,
    records: Vec<ExecutionEvidence>,
}
impl Executed {
    /// The verdict, completeness and records as JSON, for nested assertions.
    fn facts(&self) -> serde_json::Value {
        serde_json::json!({
            "verdict": self.verdict,
            "complete": self.complete,
            "records": self.records,
        })
    }
}
#[test]
fn comparison_uses_the_given_executables() {
    let f = Fixture::new(&[0x00012503, 0x00150513, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments.push(Some(7));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let (m, rows) = run(&f, request.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                verdict: ComparisonVerdict::Match,
                ..
            },
            ..
        }
    )));
    let mut different = request;
    different.cases[0].replacement.as_mut().unwrap().arguments[8] = Some(9);
    assert_eq!(run(&f, different).0.verdict, Some(ComparisonVerdict::Diff));
}
#[test]
fn concrete_memory_state_and_unknowns_are_not_invented() {
    let f = Fixture::new(&[0x00052283, 0x00128293, 0x00552023, 0x00028513, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 4,
        fill: Some(0),
        bytes: vec![],
    }));
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "second".into();
    second.vendor.memory.clear();
    r.cases.push(second);
    let run = f.run(r.clone(), budget()).unwrap();
    let facts = run.facts();
    let low: Vec<_> = facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["stop"]["low"].as_u64())
        .collect();
    assert_eq!(low, vec![1, 2]);
    r.cases[1].reset = SessionReset::Cold;
    let run = f.run(r.clone(), budget()).unwrap();
    assert!(!run.complete);
    r.cases[1].reset = SessionReset::Warm;
    r.cases[0].vendor.memory.clear();
    let run = f.run(r, budget()).unwrap();
    let facts = run.facts();
    assert_eq!(
        facts["records"][1]["stop"]["kind"],
        "blocked-by-prior-phase"
    );
}
#[test]
fn mmio_fence_and_capacity_are_observable() {
    let f = Fixture::new(&[0x00b52023, 0x00052503, 0x0330000f, 0x00008067]);
    let mut r = f.request();
    {
        let i = &mut r.cases[0].vendor;
        i.arguments[0] = Some(0x3000);
        i.arguments[1] = Some(123);
        i.models.push(register_bank(vec![RegisterCell {
            address: 0x3000,
            width: 4,
            value: 0,
        }]));
    }
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let run = f.run(r.clone(), budget()).unwrap();
    let facts = run.facts();
    assert_eq!(facts["verdict"], "MATCH");
    assert_eq!(facts["records"][0]["event"]["kind"], "write");
    assert_eq!(facts["records"][1]["event"]["value"], 123);
    assert_eq!(facts["records"][2]["event"]["kind"], "fence");
    r.max_events = 1;
    let run = f.run(r, budget());
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
}
#[test]
fn invalid_memory_and_unsupported_code_stay_incomplete() {
    for code in [
        &[0x00b52023, 0x00008067][..],
        &[0x00012503, 0x00008067][..],
        &[0x00000073][..],
    ] {
        let f = Fixture::new(code);
        let mut r = f.request();
        r.cases[0].vendor.arguments[0] = Some(0x1000);
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let run = f.run(r, budget()).unwrap();
        assert_eq!(run.facts()["verdict"], "INCOMPLETE");
    }
}
#[test]
fn loops_stop_at_the_work_memory_and_time_limits() {
    let f = Fixture::new(&[0x0000006f]);
    let r = f.request();
    let mut b = budget();
    b.max_work_units = Some(100000);
    let run = f.run(r.clone(), b);
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
    let mut b = budget();
    b.working_memory_bytes = Some(1024 * 1024);
    let run = f.run(r.clone(), b);
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
    let mut b = budget();
    b.timeout_ms = 200;
    let run = f.run(r.clone(), b);
    assert_eq!(run.unwrap_err().code, ErrorCode::TimedOut);
}

#[test]
fn rv32_arithmetic_edges_and_machine_calls_execute_instructions() {
    for (instruction, a, b, expected) in [
        (0x02b54533, 123, 0, u32::MAX), // div by zero
        (0x02b54533, 0x80000000, u32::MAX, 0x80000000),
        (0x02b56533, 0x80000000, u32::MAX, 0), // signed remainder overflow
        (0x02b51533, u32::MAX, 2, u32::MAX),   // signed multiply high
        (0x40b55533, 0x80000000, 33, 0xc0000000), // masked shift amount
    ] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut r = f.request();
        r.replacement = None;
        r.binding = None;
        r.cases[0].replacement = None;
        r.cases[0].relation = None;
        r.cases[0].vendor.arguments[0] = Some(a);
        r.cases[0].vendor.arguments[1] = Some(b);
        let run = f.run(r, budget()).unwrap();
        let facts = run.facts();
        assert_eq!(facts["records"][0]["stop"]["low"], expected);
    }
    let f = Fixture::new(&[
        0x00008293, 0x00c000ef, 0x00028067, 0x00000013, 0x00750513, 0x00008067,
    ]);
    let run = f.run(f.request(), budget()).unwrap();
    let facts = run.facts();
    assert_eq!(facts["records"][0]["stop"]["low"], 7);
}
#[test]
fn bit_manipulation_and_code_size_extensions_execute() {
    // Encodings from LLVM's assembler; results from the Zba/Zbb/Zbs
    // definitions, with a0 and a1 as the operands.
    for (instruction, a, b, expected) in [
        (0x20b54533, 3u32, 5u32, 17u32),                  // sh2add
        (0x40b57533, 0xff00ff00, 0x0ff00000, 0xf000ff00), // andn
        (0x0ab56533, u32::MAX, 1, 1),                     // max (signed)
        (0x0ab55533, u32::MAX, 1, 1),                     // minu
        (0x60b51533, 0x80000001, 36, 0x18),               // rol, masked amount
        (0x60051513, 0x00010000, 0, 15),                  // clz
        (0x60251513, 0xf0f0f0f0, 0, 16),                  // cpop
        (0x69855513, 0x11223344, 0, 0x44332211),          // rev8
        (0x28755513, 0x00120300, 0, 0x00ffff00),          // orc.b
        (0x48555513, 0x20, 0, 1),                         // bexti 5
        (0x68b51533, 0x80000000, 63, 0),                  // binv, masked index
    ] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut r = f.request();
        r.replacement = None;
        r.binding = None;
        r.cases[0].replacement = None;
        r.cases[0].relation = None;
        r.cases[0].vendor.arguments[0] = Some(a);
        r.cases[0].vendor.arguments[1] = Some(b);
        let run = f.run(r, budget()).unwrap();
        let facts = run.facts();
        assert_eq!(
            facts["records"][0]["stop"]["low"], expected,
            "{instruction:#010x}"
        );
    }
    // cm.push {ra, s0-s1}, -16; cm.mvsa01 s1, s0; sh2add a0, s0, s1;
    // cm.popret {ra, s0-s1}, 16; c.nop. The frame round-trips ra, and the
    // saved registers carry a0 and a1 into the result.
    let f = Fixture::new(&[0xaca2b862, 0x20944533, 0x0001be62]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(3);
    r.cases[0].vendor.arguments[1] = Some(5);
    let run = f.run(r, budget()).unwrap();
    let facts = run.facts();
    assert_eq!(facts["records"][0]["stop"]["kind"], "returned");
    assert_eq!(facts["records"][0]["stop"]["low"], 23);
    // c.lbu a0, 3(a0); c.sext.b a0; c.mul a0, a1; c.jr ra.
    let f = Fixture::new(&[0x9d658168, 0x80829d4d]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.arguments[1] = Some(3);
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 4,
        fill: Some(0x80),
        bytes: vec![],
    }));
    let run = f.run(r, budget()).unwrap();
    let facts = run.facts();
    assert_eq!(facts["records"][0]["stop"]["low"], 0xfffffe80u32);
}
#[test]
fn signed_loads_and_phase_stack_reset_are_explicit() {
    let f = Fixture::new(&[0x00050503, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 1,
        fill: Some(0x80),
        bytes: vec![],
    }));
    let run = f.run(r, budget()).unwrap();
    assert_eq!(run.facts()["records"][0]["stop"]["low"], 0xffffff80u32);
    // First phase writes the fresh stack, second phase reads the reset unknown byte.
    let f = Fixture::new(&[0x00050663, 0xfea12e23, 0x00008067, 0xffc12503, 0x00008067]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(7);
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "read-stack".into();
    second.vendor.arguments[0] = Some(0);
    r.cases.push(second);
    let run = f.run(r, budget()).unwrap();
    let facts = run.facts();
    assert_eq!(facts["records"][0]["stop"]["kind"], "returned");
    // The reset stack byte is unknown, so the loaded return word is too.
    assert!(facts["records"][1]["stop"]["low"].is_null());
}

#[test]
fn selected_companion_code_and_elf_zero_fill_obey_session_ownership() {
    let main = elf(&[0x000022b7, 0x00028067]);
    let mut companion = elf(&[0x00900513, 0x00008067]);
    for offset in [24, 60, 64] {
        companion[offset..offset + 4].copy_from_slice(&0x2000u32.to_le_bytes());
    }
    let f = Fixture::from_inputs(vec![main, companion]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    let run = f.run(r.clone(), budget()).unwrap();
    assert!(!run.complete);
    r.vendor.executables.push(f.id(1));
    let run = f.run(r.clone(), budget()).unwrap();
    assert_eq!(run.facts()["records"][0]["stop"]["low"], 9);
    // A target maps each executable at most once.
    r.vendor.executables = vec![f.id(0), f.id(0)];
    let run = f.run(r.clone(), budget());
    assert_eq!(run.unwrap_err().code, ErrorCode::InvalidRequest);
    let mut image = elf(&[0x00052283, 0x00128293, 0x00552023, 0x00028513, 0x00008067]);
    image[44..46].copy_from_slice(&2u16.to_le_bytes());
    for (offset, value) in [
        (84, 1u32),
        (92, 0x3000),
        (96, 0x3000),
        (104, 4),
        (108, 6),
        (112, 4),
    ] {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let f = Fixture::from_inputs(vec![image]);
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    r.cases[0].replacement = None;
    r.cases[0].relation = None;
    r.cases[0].vendor.arguments[0] = Some(0x3000);
    r.cases.push(r.cases[0].clone());
    for (mode, expected) in [
        (SessionReset::Warm, vec![1, 2]),
        (SessionReset::Cold, vec![1, 1]),
    ] {
        r.cases[1].reset = mode;
        let run = f.run(r.clone(), budget()).unwrap();
        let facts = run.facts();
        let values: Vec<_> = facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "outcome")
            .map(|r| r["stop"]["low"].as_u64().unwrap())
            .collect();
        assert_eq!(values, expected);
    }
}

#[path = "execution/arguments.rs"]
mod arguments;
#[path = "execution/atomics.rs"]
mod atomics;

fn ram(seed: MemorySeed) -> ExecutionRegion {
    ExecutionRegion {
        seed,
        lifetime: RegionLifetime::Session,
    }
}
#[path = "execution/goals.rs"]
mod goals;
#[path = "execution/sessions.rs"]
mod sessions;

fn register_bank(cells: Vec<RegisterCell>) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "registers".into(),
        applicability: "fixture register-bank assumption".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank { cells },
    }
}
#[path = "execution/devices.rs"]
mod devices;

#[path = "execution/calls.rs"]
mod calls;

fn fixture_relation(low: bool) -> ComparisonRelation {
    ComparisonRelation {
        effects: None,
        projection: None,
        calls: false,
        returns: ReturnWords { low, high: false },
        events: EventChannels {
            timeline: TimelineCapture::default(),
            mmio_read: true,
            mmio_write: true,
            fence: true,
            delay: true,
        },
        memory: vec![],
    }
}

#[path = "execution/comparison.rs"]
mod comparison;

#[path = "execution/capture.rs"]
mod capture;

#[path = "execution/coverage.rs"]
mod coverage;
#[path = "execution/dependence.rs"]
mod dependence;
#[path = "execution/timeline.rs"]
mod timeline;

#[path = "execution/effects.rs"]
mod effects;
#[path = "execution/projections.rs"]
mod projections;

#[path = "execution/command_bank.rs"]
mod command_bank;

#[path = "execution/unaligned.rs"]
mod unaligned;

/// Execute and compare `r` in process under the default budget.
fn run(f: &Fixture, r: ExecutionRequest) -> (Verified, Vec<ExecutionEvidence>) {
    let executed = f.run(r, budget()).unwrap();
    (
        Verified {
            verdict: executed.verdict,
            complete: executed.complete,
        },
        executed.records,
    )
}

/// Outcome of an in-process comparison.
struct Verified {
    verdict: Option<ComparisonVerdict>,
    complete: bool,
}
/// Compare `r` in process with the effect contracts and layout projections
/// its relations select by content.
fn verify(
    f: &Fixture,
    r: &ExecutionRequest,
    effects: &[EffectContract],
    projections: &[LayoutProjection],
) -> Result<(Verified, Vec<ExecutionEvidence>)> {
    let executed = f.run_with(r, effects, projections, budget())?;
    Ok((
        Verified {
            verdict: executed.verdict,
            complete: executed.complete,
        },
        executed.records,
    ))
}
