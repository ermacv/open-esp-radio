use super::run;
use super::*;
fn no_events() -> EventChannels {
    EventChannels {
        timeline: TimelineCapture::default(),
        mmio_read: false,
        mmio_write: false,
        fence: false,
        delay: false,
    }
}
fn memory_request(f: &Fixture) -> ExecutionRequest {
    let mut r = f.request();
    let v = &mut r.cases[0].vendor;
    v.arguments[0] = Some(0x3000);
    v.arguments[1] = Some(7);
    v.memory = vec![ExecutionRegion {
        lifetime: RegionLifetime::Phase,
        seed: MemorySeed {
            address: 0x3000,
            length: 32,
            fill: Some(0),
            bytes: vec![],
        },
    }];
    v.observe_memory = vec![MemorySelection {
        name: "result".into(),
        address: 0x3000,
        length: 32,
    }];
    let mut other = v.clone();
    other.arguments[0] = Some(0x4000);
    other.memory[0].seed.address = 0x4000;
    other.observe_memory[0].address = 0x4000;
    r.cases[0].replacement = Some(other);
    r.cases[0].relation = Some(ComparisonRelation {
        effects: None,
        projection: None,
        calls: false,
        returns: ReturnWords {
            low: false,
            high: false,
        },
        events: no_events(),
        memory: vec![MemoryPair {
            vendor: 0,
            replacement: 0,
        }],
    });
    r
}
fn difference(rows: &[ExecutionEvidence]) -> Option<&ComparisonDifference> {
    rows.iter().find_map(|r| match r {
        ExecutionEvidence::Comparison { result, .. } => result.difference.as_ref(),
        _ => None,
    })
}
#[test]
fn explicit_physical_pairs_compare_every_final_byte_even_when_unchanged() {
    let f = Fixture::new(&[0x00b52023, 0x00000513, 0x00008067]);
    let mut r = memory_request(&f);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let chunks: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::FinalMemory {
                replacement: false,
                chunk,
                ..
            } => Some(chunk),
            _ => None,
        })
        .collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].bytes[0], 7);
    assert_eq!(chunks[1].bytes, [0; 16]);
    assert!(chunks.iter().all(|c| c.complete()));
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        difference(&rows),
        Some(&ComparisonDifference::Memory {
            pair: 0,
            offset: 0,
            vendor: 7,
            replacement: 9
        })
    );
    let other = r.cases[0].replacement.as_mut().unwrap();
    other.arguments[1] = Some(7);
    other.memory[0].seed.bytes = vec![0; 32];
    other.memory[0].seed.bytes[29] = 42;
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        difference(&rows),
        Some(&ComparisonDifference::Memory {
            pair: 0,
            offset: 29,
            vendor: 0,
            replacement: 42
        })
    );
}
#[test]
fn unknown_and_unavailable_bytes_are_retained_without_satisfying_memory_equality() {
    let f = Fixture::new(&[0x00000513, 0x00008067]);
    let mut r = memory_request(&f);
    let v = &mut r.cases[0].vendor;
    v.observe_memory[0].length = 16;
    v.memory[0].seed.length = 4;
    v.memory[0].seed.fill = None;
    v.memory.push(ExecutionRegion {
        lifetime: RegionLifetime::Phase,
        seed: MemorySeed {
            address: 0x3008,
            length: 4,
            fill: Some(0),
            bytes: vec![],
        },
    });
    r.cases[0].replacement = Some(v.clone());
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::FinalMemory {
            chunk: FinalMemoryChunk {
                available: 0x0f0f,
                known: 0x0f00,
                ..
            },
            ..
        }
    )));
    // Excluding this observation is explicit and does not erase it from retained evidence.
    r.cases[0].relation = Some(fixture_relation(true));
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(
        rows.iter()
            .any(|r| matches!(r, ExecutionEvidence::FinalMemory { .. }))
    );
    r.cases[0].relation.as_mut().unwrap().memory = vec![MemoryPair {
        vendor: 0,
        replacement: 0,
    }];
    r.cases[0].replacement.as_mut().unwrap().memory[1]
        .seed
        .bytes = vec![9];
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        difference(&rows),
        Some(&ComparisonDifference::Memory {
            pair: 0,
            offset: 8,
            vendor: 0,
            replacement: 9
        })
    );
}
#[test]
fn interrupted_ram_states_are_evidence_but_not_completed_final_state_differences() {
    let f = Fixture::new(&[0x00b52023, 0x00000073]);
    let mut r = memory_request(&f);
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(difference(&rows).is_none());
    assert!(
        rows.iter()
            .any(|r| matches!(r,ExecutionEvidence::FinalMemory {chunk,..} if chunk.bytes[0]==9))
    );
}
#[test]
fn high_return_words_and_event_channels_are_independently_selected() {
    let f = Fixture::new(&[0x00008067]);
    let mut r = f.request();
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Match));
    r.cases[0].relation.as_mut().unwrap().returns.high = true;
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        difference(&rows),
        Some(&ComparisonDifference::Return {
            word: 1,
            vendor: 0,
            replacement: 9
        })
    );
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = None;
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Incomplete));
    let f = Fixture::new(&[0x00b52023, 0x00000513, 0x00008067]);
    let mut r = f.request();
    let v = &mut r.cases[0].vendor;
    v.arguments[0] = Some(0x3000);
    v.arguments[1] = Some(7);
    v.models = vec![register_bank(vec![RegisterCell {
        address: 0x3000,
        width: 4,
        value: 0,
    }])];
    r.cases[0].replacement = Some(v.clone());
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        difference(&rows),
        Some(&ComparisonDifference::Event { index: 0 })
    );
    r.cases[0].relation.as_mut().unwrap().events = no_events();
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::Write { value: 9, .. },
            ..
        }
    )));
}
#[test]
fn phase_memory_expires_before_next_snapshot_and_relations_belong_to_each_phase() {
    let f = Fixture::new(&[0x00000513, 0x00008067]);
    let mut r = memory_request(&f);
    let mut warm = r.cases[0].clone();
    warm.name = "warm".into();
    warm.reset = SessionReset::Warm;
    warm.vendor.memory.clear();
    warm.replacement.as_mut().unwrap().memory.clear();
    r.cases.push(warm);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::FinalMemory {
            case: 1,
            chunk: FinalMemoryChunk { available: 0, .. },
            ..
        }
    )));
    r.cases[1].relation = Some(fixture_relation(true));
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match));
}
#[test]
fn invalid_pairs_and_physical_ranges_are_rejected_before_worker_execution() {
    let f = Fixture::new(&[0x00008067]);
    let r = memory_request(&f);
    for variant in 0..6 {
        let mut bad = r.clone();
        match variant {
            0 => bad.cases[0].relation.as_mut().unwrap().memory[0].vendor = 1,
            1 => bad.cases[0].replacement.as_mut().unwrap().observe_memory[0].length = 16,
            2 => bad.cases[0].vendor.observe_memory[0].address = u32::MAX - 1,
            3 => {
                let mut overlap = bad.cases[0].vendor.observe_memory[0].clone();
                overlap.name = "overlap".into();
                overlap.address += 1;
                bad.cases[0].vendor.observe_memory.push(overlap);
            }
            4 => bad.cases[0].vendor.observe_memory[0].length = MAX_OBSERVED_MEMORY_BYTES + 1,
            _ => bad.cases[0]
                .relation
                .as_mut()
                .unwrap()
                .memory
                .push(MemoryPair {
                    vendor: 0,
                    replacement: 0,
                }),
        }
        match f.run(bad, budget()) {
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
            Ok(_) => panic!("invalid relation admitted"),
        };
    }
}
