use super::*;
use std::collections::BTreeSet;

/// Declared RAM the programs store into.
const RAM: u32 = 0x3000;
/// Register bank whose writes are MMIO events.
const MMIO: u32 = 0x5000;
/// Unmapped address of a call model.
const MODEL: u32 = 0x2000;
const STACK: u32 = 0x9000_0000;

/// Register, memory, branch and return dependence, from 0x1000:
/// a dead `t1`, a branch on `a0` choosing `a2`, stores of `a2` and `t1` to
/// `a3[0]` and `a3[1]`, and a reload of `a3[0]` into the returned `a0`.
const FLOW: [u32; 9] = [
    0x00700313, // 0x1000 li t1, 7
    0x00050663, // 0x1004 beq a0, zero, 0x1010
    0x00100613, // 0x1008 li a2, 1
    0x0080006f, // 0x100c j 0x1014
    0x00200613, // 0x1010 li a2, 2
    0x00c6a023, // 0x1014 sw a2, 0(a3)
    0x0066a223, // 0x1018 sw t1, 4(a3)
    0x0006a503, // 0x101c lw a0, 0(a3)
    0x00008067, // 0x1020 ret
];

/// An MMIO write of 0x55 to `MMIO`, then a call of `MODEL` with `a0 = 9`
/// and an unused `t2`, preserving `ra` in `s0`.
const DEVICE: [u32; 10] = [
    0x00008413, // 0x1000 mv s0, ra
    0x05500293, // 0x1004 li t0, 0x55
    0x00005337, // 0x1008 lui t1, 0x5
    0x00532023, // 0x100c sw t0, 0(t1)
    0x00900513, // 0x1010 li a0, 9
    0x00300393, // 0x1014 li t2, 3
    0x00002e37, // 0x1018 lui t3, 0x2
    0x000e00e7, // 0x101c jalr ra, 0(t3)
    0x00040093, // 0x1020 mv ra, s0
    0x00008067, // 0x1024 ret
];

fn target(executable: &[u8]) -> ExecutionTarget {
    ExecutionTarget {
        executables: vec![ArtifactId::of_bytes(executable)],
        abi: CallAbi::RiscvInteger,
        stack: MemorySeed {
            address: STACK,
            length: 4096,
            fill: Some(0),
            bytes: vec![],
        },
    }
}

fn invocation(arguments: [u32; 8]) -> Invocation {
    Invocation {
        observe_calls: None,
        observe_timeline: TimelineCapture::default(),
        observe_memory: vec![],
        goal: ExecutionGoal::Return,
        entry: 0x1000,
        arguments: arguments.into_iter().map(Some).collect(),
        memory: vec![],
        preload: vec![],
        models: vec![],
        calls: vec![],
    }
}

fn relation() -> ComparisonRelation {
    ComparisonRelation {
        effects: None,
        projection: None,
        calls: false,
        returns: ReturnWords {
            low: false,
            high: false,
        },
        events: EventChannels {
            timeline: TimelineCapture::default(),
            mmio_read: false,
            mmio_write: false,
            fence: false,
            delay: false,
        },
        memory: vec![],
    }
}

/// One self-comparison of `code`, with `patches` applied to the replacement.
fn compare(
    code: &[u32],
    invocation: Invocation,
    relation: ComparisonRelation,
    patches: &[app::in_process::ImagePatch],
) -> Result<app::in_process::InProcessResult> {
    let executable = elf(code);
    let sources: &[app::in_process::Executable] =
        &[app::in_process::Executable::new(executable.clone())];
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: target(&executable),
        replacement: Some(target(&executable)),
        binding: Some(CompiledBinding::SharedCore),
        cases: vec![ExecutionCase {
            name: "case".into(),
            reset: SessionReset::Cold,
            stack_fill: None,
            relation: Some(relation),
            vendor: invocation.clone(),
            replacement: Some(invocation),
        }],
        max_events: 64,
    };
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    app::in_process::verify(
        &app::in_process::InProcessComparison {
            request: &request,
            executables: sources,
            effects: &[],
            projections: &[],
            vendor_results: None,
            dependence: Some(&oer_riscv_lift::RiscvDecoder),
            patches,
        },
        &blobray_backend_riscv::RiscvExecutor,
        &memory,
        &mut || Ok(()),
    )
}

/// Executed and observed replacement instructions of one self-comparison.
fn dependence(
    code: &[u32],
    invocation: Invocation,
    relation: ComparisonRelation,
) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let result = compare(code, invocation, relation, &[]).unwrap();
    assert_eq!(result.verdict, Some(ComparisonVerdict::Match));
    let observed = result.observed.unwrap();
    (observed.executed, observed.observed)
}

fn set(addresses: &[u32]) -> BTreeSet<u32> {
    addresses.iter().copied().collect()
}

fn flow_invocation() -> Invocation {
    let mut input = invocation([0, 0, 0, RAM, 0, 0, 0, 0]);
    input.memory = vec![ExecutionRegion {
        lifetime: RegionLifetime::Phase,
        seed: MemorySeed {
            address: RAM,
            length: 32,
            fill: Some(0),
            bytes: vec![],
        },
    }];
    input.observe_memory = vec![MemorySelection {
        name: "second word".into(),
        address: RAM + 4,
        length: 4,
    }];
    input
}

#[test]
fn returned_values_depend_on_registers_memory_and_branches() {
    let mut returns = relation();
    returns.returns.low = true;
    let (executed, observed) = dependence(&FLOW, flow_invocation(), returns);
    assert_eq!(
        executed,
        set(&[0x1000, 0x1004, 0x1010, 0x1014, 0x1018, 0x101c, 0x1020])
    );
    // The returned reload depends on the store of `a2`, the `a2` the branch
    // selected and the branch; reaching the returning `ret` is the goal. The
    // dead `t1` and its store are executed but unobserved.
    assert_eq!(observed, set(&[0x1004, 0x1010, 0x1014, 0x101c, 0x1020]));
}

#[test]
fn compared_final_memory_depends_on_its_last_writers() {
    let mut memory = relation();
    memory.memory = vec![MemoryPair {
        vendor: 0,
        replacement: 0,
    }];
    let (_, observed) = dependence(&FLOW, flow_invocation(), memory);
    assert_eq!(observed, set(&[0x1000, 0x1018, 0x1020]));
}

fn device_invocation(arguments: u16) -> Invocation {
    let mut input = invocation([0; 8]);
    input.models = vec![DeviceDeclaration {
        id: "bank".into(),
        applicability: "synthetic register bank".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![RegisterCell {
                address: MMIO,
                width: 4,
                value: 0,
            }],
        },
    }];
    input.calls = vec![CallDeclaration {
        repetition: CallRepetition::Unbounded,
        id: "external".into(),
        applicability: "synthetic external ABI assumption".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: MODEL,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 0,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
    }];
    input.observe_calls = (arguments > 0).then_some(CallCapture {
        include_tail: false,
        argument_words: arguments,
        overrides: vec![],
    });
    input
}

/// Steps every returning run of `DEVICE` observes: saving and restoring
/// `ra`, the call that the restore follows, its target and the return.
const DEVICE_RETURN: [u32; 5] = [0x1000, 0x1018, 0x101c, 0x1020, 0x1024];

#[test]
fn compared_events_depend_on_the_values_and_addresses_they_carry() {
    let mut writes = relation();
    writes.events.mmio_write = true;
    let (executed, observed) = dependence(&DEVICE, device_invocation(0), writes);
    assert_eq!(
        executed,
        (0..DEVICE.len() as u32).map(|i| 0x1000 + 4 * i).collect()
    );
    let mut expected = set(&DEVICE_RETURN);
    expected.extend([0x1004, 0x1008, 0x100c]);
    assert_eq!(observed, expected);
    // Compared call arguments depend on the argument registers instead.
    let mut calls = relation();
    calls.calls = true;
    let (_, observed) = dependence(&DEVICE, device_invocation(1), calls);
    let mut expected = set(&DEVICE_RETURN);
    expected.insert(0x1010);
    assert_eq!(observed, expected);
}

/// A branch on `a1` around a `nop`, then a branch on `a0` around a call of a
/// function returning 5 in `a0`, preserving `ra` in `s0`.
const CALLED: [u32; 11] = [
    0x00008413, // 0x1000 mv s0, ra
    0x00058463, // 0x1004 beq a1, zero, 0x100c
    0x00000013, // 0x1008 nop
    0x00050663, // 0x100c beq a0, zero, 0x1018
    0x00000097, // 0x1010 auipc ra, 0
    0x014080e7, // 0x1014 jalr ra, 0x14(ra)
    0x00040093, // 0x1018 mv ra, s0
    0x00008067, // 0x101c ret
    0x00000013, // 0x1020 nop
    0x00500513, // 0x1024 li a0, 5
    0x00008067, // 0x1028 ret
];

#[test]
fn branches_control_their_region_including_calls() {
    let mut returns = relation();
    returns.returns.low = true;
    let (executed, observed) = dependence(&CALLED, invocation([1, 1, 0, 0, 0, 0, 0, 0]), returns);
    assert_eq!(
        executed,
        set(&[
            0x1000, 0x1004, 0x1008, 0x100c, 0x1010, 0x1014, 0x1018, 0x101c, 0x1024, 0x1028
        ])
    );
    // The callee's result is control dependent on the branch that guards the
    // call. The branch around the `nop` reconverges without an observable
    // effect, so neither is observed.
    assert_eq!(
        observed,
        set(&[
            0x1000, 0x100c, 0x1010, 0x1014, 0x1018, 0x101c, 0x1024, 0x1028
        ])
    );
}

fn patch(address: u32, original: u32, replacement: u32) -> app::in_process::ImagePatch {
    app::in_process::ImagePatch {
        address,
        original: original.to_le_bytes().to_vec(),
        replacement: replacement.to_le_bytes().to_vec(),
    }
}

#[test]
fn point_mutants_patch_the_loaded_replacement_image() {
    let mut returns = relation();
    returns.returns.low = true;
    let verdict = |patches: &[app::in_process::ImagePatch]| {
        compare(&FLOW, flow_invocation(), returns.clone(), patches).map(|r| r.verdict)
    };
    // `li a2, 2` becomes `li a2, 3`: the observed line's mutant is killed.
    assert_eq!(
        verdict(&[patch(0x1010, 0x00200613, 0x00300613)]).unwrap(),
        Some(ComparisonVerdict::Diff)
    );
    // `li t1, 7` becomes `li t1, 8`: the unobserved line's mutant survives.
    assert_eq!(
        verdict(&[patch(0x1000, 0x00700313, 0x00800313)]).unwrap(),
        Some(ComparisonVerdict::Match)
    );
    // Original bytes that differ from the image, and addresses outside its
    // code, are invalid.
    for invalid in [patch(0x1000, 0x00200613, 0x00300613), patch(0x2000, 0, 1)] {
        let error = verdict(&[invalid]).err().unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
}

#[test]
fn unobserved_instructions_are_classified_by_what_depends_on_them() {
    let mut returns = relation();
    returns.returns.low = true;
    let classify = |lifetime| {
        let mut input = flow_invocation();
        input.memory[0].lifetime = lifetime;
        compare(&FLOW, input, returns.clone(), &[])
            .unwrap()
            .observed
            .unwrap()
    };
    // The dead `t1` and its store reach session memory that outlives the
    // phase: the session's state. Phase memory ends with the phase, so the
    // same store then reaches nothing.
    let session = classify(RegionLifetime::Session);
    assert!(session.state.contains(&0x1000) && session.state.contains(&0x1018));
    assert!(!session.effect.contains(&0x1018));
    let phase = classify(RegionLifetime::Phase);
    assert!(!phase.state.contains(&0x1000) && !phase.state.contains(&0x1018));
    let mut calls = relation();
    calls.calls = true;
    let observed = compare(&DEVICE, device_invocation(1), calls, &[])
        .unwrap()
        .observed
        .unwrap();
    // The MMIO write is an effect the relation does not compare; the unused
    // `t2` reaches nothing.
    for pc in [0x1004, 0x1008, 0x100c] {
        assert!(!observed.observed.contains(&pc) && observed.effect.contains(&pc));
    }
    assert!(!observed.effect.contains(&0x1014) && !observed.state.contains(&0x1014));
}

/// A jump whose target is a dead instruction, followed by the returned value.
const JUMP: [u32; 5] = [
    0x0080006f, // 0x1000 j 0x1008
    0x00000013, // 0x1004 nop
    0x00300293, // 0x1008 li t0, 3
    0x00500513, // 0x100c li a0, 5
    0x00008067, // 0x1010 ret
];

#[test]
fn an_unconditional_transfer_is_observed_when_any_later_step_is() {
    let mut returns = relation();
    returns.returns.low = true;
    let (executed, observed) = dependence(&JUMP, invocation([0; 8]), returns);
    assert_eq!(executed, set(&[0x1000, 0x1008, 0x100c, 0x1010]));
    // Everything after the jump runs only through it, although the step it
    // leads to directly is dead.
    assert_eq!(observed, set(&[0x1000, 0x100c, 0x1010]));
}

#[test]
fn projected_final_state_depends_on_its_last_writers() {
    let (executable, entry) = super::goals::symbol_elf(&FLOW, 0x1000, 0x1000);
    let endpoint = LayoutEndpoint {
        entry: CallEndpoint {
            object: entry.object.clone(),
            symbol: Some(entry),
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        },
        domains: vec![LayoutDomain {
            address: RAM,
            length: 32,
        }],
    };
    // Only the first word, which the branch-selected `a2` fills, is projected.
    let projection = LayoutProjection {
        vendor: endpoint.clone(),
        replacement: endpoint,
        fields: vec![LayoutField {
            name: "selected".into(),
            vendor: FieldLocation {
                domain: 0,
                offset: 0,
            },
            replacement: FieldLocation {
                domain: 0,
                offset: 0,
            },
            width: 4,
            count: 1,
            final_state: true,
            timeline: false,
        }],
        branches: vec![],
        applicability: "synthetic RAM word".into(),
        reason: "the same word on both sides".into(),
    };
    let mut input = flow_invocation();
    input.observe_memory[0].address = RAM;
    input.observe_memory[0].length = 8;
    let mut projected = relation();
    projected.projection = Some(app::in_process::projection_id(&projection).unwrap());
    let sources: &[app::in_process::Executable] =
        &[app::in_process::Executable::new(executable.clone())];
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: target(&executable),
        replacement: Some(target(&executable)),
        binding: Some(CompiledBinding::SharedCore),
        cases: vec![ExecutionCase {
            name: "case".into(),
            reset: SessionReset::Cold,
            stack_fill: None,
            relation: Some(projected),
            vendor: input.clone(),
            replacement: Some(input),
        }],
        max_events: 64,
    };
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let result = app::in_process::verify(
        &app::in_process::InProcessComparison {
            request: &request,
            executables: sources,
            effects: &[],
            projections: std::slice::from_ref(&projection),
            vendor_results: None,
            dependence: Some(&oer_riscv_lift::RiscvDecoder),
            patches: &[],
        },
        &blobray_backend_riscv::RiscvExecutor,
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(result.verdict, Some(ComparisonVerdict::Match));
    // The store of `a2`, its branch-selected value and the branch; the second
    // word's `t1` store is outside the projection.
    assert_eq!(
        result.observed.unwrap().observed,
        set(&[0x1004, 0x1010, 0x1014, 0x1020])
    );
}

/// A store of the unknown `tp` through `a3`, then the returned 5.
const UNKNOWN_STORE: [u32; 3] = [
    0x0046a023, // 0x1000 sw tp, 0(a3)
    0x00500513, // 0x1004 li a0, 5
    0x00008067, // 0x1008 ret
];

#[test]
fn an_unknown_register_stored_to_memory_leaves_it_unknown() {
    let mut returns = relation();
    returns.returns.low = true;
    let mut input = flow_invocation();
    input.observe_memory[0].address = RAM;
    let result = compare(&UNKNOWN_STORE, input, returns.clone(), &[]).unwrap();
    assert_eq!(result.verdict, Some(ComparisonVerdict::Match));
    let chunk = result
        .records
        .iter()
        .find_map(|r| match r {
            ExecutionEvidence::FinalMemory {
                replacement: true,
                chunk,
                ..
            } => Some(*chunk),
            _ => None,
        })
        .unwrap();
    assert_eq!(chunk.known & 0xf, 0, "the stored bytes are unknown");
    // A device would observe the value, so it never accepts an unknown one.
    let mut device = device_invocation(0);
    device.arguments[3] = Some(MMIO);
    let result = compare(&UNKNOWN_STORE, device, returns, &[]).unwrap();
    assert!(result.records.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            stop: ExecutionStop::Incomplete {
                reason: ExecutionGap::UnknownRegister { register: 4 },
                ..
            },
            ..
        }
    )));
}
