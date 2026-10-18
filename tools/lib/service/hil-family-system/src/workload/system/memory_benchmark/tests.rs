use super::*;

fn nominal() -> MemoryBenchmarkEvidence {
    MemoryBenchmarkEvidence {
        request: MemoryBenchmarkRequest {
            mode: MemoryBenchmarkMode::CpuCopy,
            source: MemoryBenchmarkSource::Psram,
            bytes: 1514,
            frames: 1,
            iterations: 32,
        },
        completed_iterations: 32,
        elapsed_micros: 100,
        elapsed_cycles: u64::from(u32::MAX) + 100,
        elapsed_instructions: 1000,
        foreground_cycles: u64::from(u32::MAX) + 100,
        foreground_instructions: 1000,
        polls: 0,
        stop: MemoryBenchmarkStop::Completed,
    }
}

#[test]
fn case_matrix_covers_each_size_source_and_mode_once() {
    let cases = requests(&Config {
        boots: 2,
        iterations: 32,
        sizes: &[64, 256, 512, 1514, 4096],
        batch_sizes: &[1],
    });
    assert_eq!(cases.len(), 30);
    for (index, case) in cases.iter().enumerate() {
        assert!(case.validate());
        assert!(!cases[..index].contains(case));
    }
}

#[test]
fn batch_matrix_preserves_source_size_batch_mode_order() {
    let cases = requests(&Config {
        boots: 2,
        iterations: 32,
        sizes: &[64, 512, 1514],
        batch_sizes: &[1, 2, 8, 32],
    });
    assert_eq!(cases.len(), 72);
    for (index, case) in cases.iter().enumerate() {
        assert!(case.validate());
        assert!(!cases[..index].contains(case));
    }
    assert_eq!(cases[0].frames, 1);
    assert_eq!(cases[3].frames, 2);
    assert_eq!(cases[12].bytes, 512);
    assert_eq!(cases[35].source, MemoryBenchmarkSource::Sram);
    assert_eq!(cases[36].source, MemoryBenchmarkSource::Psram);
    assert_eq!(cases[71].frames, 32);
    assert_eq!(cases[71].mode, MemoryBenchmarkMode::GdmaAsync);
}

#[test]
fn completed_cases_accept_large_counters_without_a_speed_floor() {
    let mut evidence = nominal();
    assert!(validate(evidence.request, evidence).is_ok());
    evidence.request.mode = MemoryBenchmarkMode::GdmaAsync;
    evidence.foreground_cycles = 100;
    evidence.foreground_instructions = 50;
    evidence.polls = 32;
    evidence.elapsed_micros = 10_000_000;
    assert!(validate(evidence.request, evidence).is_ok());
}

#[test]
fn every_failed_terminal_state_remains_a_failure() {
    for stop in [
        MemoryBenchmarkStop::PrepareFailed,
        MemoryBenchmarkStop::TransferFailed,
        MemoryBenchmarkStop::TimedOut,
        MemoryBenchmarkStop::DataMismatch,
        MemoryBenchmarkStop::GuardCorrupted,
    ] {
        let mut evidence = nominal();
        evidence.stop = stop;
        assert!(validate(evidence.request, evidence).is_err());
    }
}

#[test]
fn mismatched_request_partial_completion_and_wrong_counter_scopes_fail() {
    let valid = nominal();
    let mut mismatch = valid;
    mismatch.request.bytes = 64;
    assert!(validate(valid.request, mismatch).is_err());
    mismatch = valid;
    mismatch.request.frames = 2;
    assert!(validate(valid.request, mismatch).is_err());
    let mut incomplete = valid;
    incomplete.completed_iterations -= 1;
    assert!(validate(valid.request, incomplete).is_err());
    let mut invalid_scope = valid;
    invalid_scope.foreground_cycles += 1;
    assert!(validate(valid.request, invalid_scope).is_err());
    invalid_scope = valid;
    invalid_scope.foreground_cycles -= 1;
    assert!(validate(valid.request, invalid_scope).is_err());
    let mut invalid_async = valid;
    invalid_async.request.mode = MemoryBenchmarkMode::GdmaAsync;
    assert!(validate(invalid_async.request, invalid_async).is_err());
}

#[test]
fn a_failed_case_keeps_its_request_and_observation() {
    let mut target = nominal();
    target.request.frames = 32;
    target.completed_iterations = 7;
    target.stop = MemoryBenchmarkStop::GuardCorrupted;
    let reports = [CaseReport {
        boot: 2,
        requested: target.request,
        target,
    }];
    let results = oer_hil_workload::results::Results::default();
    results.observe("case-001", &reports[0]);
    let observed = &results.snapshot().observations[0];
    assert_eq!(observed.name, "case-001");
    let case = &observed.value;
    assert_eq!(case["requested"]["frames"], 32);
    assert_eq!(case["target"]["request"]["frames"], 32);
    assert_eq!(case["target"]["completed_iterations"], 7);
    assert_eq!(case["target"]["stop"], "GuardCorrupted");
    assert_eq!(case["boot"], 2);
}
