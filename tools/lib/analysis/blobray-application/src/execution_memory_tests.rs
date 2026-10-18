use super::*;
const ORDER: ExecutionOrdering = ExecutionOrdering {
    acquire: true,
    release: true,
};
fn session(memory: &WorkingMemory, fill: Option<u8>) -> Session<'_> {
    let mut s = Session::new(memory, 4, Default::default(), &mut || Ok(())).unwrap();
    s.region(
        Mapping {
            address: 0x1000,
            length: 8,
            flags: 6,
            kind: RegionKind::Ram(RegionLifetime::Session),
        },
        fill,
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    s
}
#[test]
fn reservations_follow_physical_writes_and_each_sc_consumes_them() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, Some(0));
    let mut c = || Ok(());
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(s.load_reserved(0x1000, ORDER, &mut c).unwrap(), Some(0));
    assert!(s.write(0x1004, 4, 8, &mut c).unwrap()); // disjoint store retains reservation
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(7)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert!(s.write(0x1001, 1, 0, &mut c).unwrap()); // even an unchanged byte invalidates
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    s.load_reserved(0x1004, ORDER, &mut c).unwrap();
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.store_conditional(0x1004, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(s.store_conditional(0x5000, 9, ORDER, &mut c).unwrap(), None);
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(s.load_reserved(0x1001, ORDER, &mut c).unwrap(), None);
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert!(s.events.is_empty());
    drop(s);
    assert_eq!(memory.observation().reserved_bytes, 0);
}

#[test]
fn atomic_update_is_once_after_validation_and_invalidates_only_overlap() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, None);
    let mut c = || Ok(());
    let mut calls = 0;
    for address in [0x1000, 0x1001, 0x5000] {
        assert_eq!(
            s.modify_word(
                address,
                ORDER,
                &mut |_| {
                    calls += 1;
                    7
                },
                &mut c
            )
            .unwrap(),
            None
        );
    }
    assert_eq!(calls, 0);
    assert!(s.write(0x1000, 4, 3, &mut c).unwrap());
    let error = s
        .modify_word(
            0x1000,
            ORDER,
            &mut |_| {
                calls += 1;
                99
            },
            &mut || {
                Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "fixture exhausted work",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert_eq!(calls, 0);
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(3)
    );
    assert!(s.write(0x1004, 4, 4, &mut c).unwrap());
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        s.modify_word(
            0x1004,
            ORDER,
            &mut |n| {
                calls += 1;
                n + 1
            },
            &mut c
        )
        .unwrap(),
        Some(4)
    );
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        s.modify_word(
            0x1000,
            ORDER,
            &mut |n| {
                calls += 1;
                n + 1
            },
            &mut c
        )
        .unwrap(),
        Some(7)
    );
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(8)
    );
    assert_eq!(calls, 2);
    let mut other = session(&memory, Some(0));
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        other.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    assert!(s.events.is_empty());
}

#[test]
fn phase_release_drops_stack_and_temporary_ram_but_preserves_session_ram() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, Some(0));
    let initial = memory.observation().reserved_bytes;
    s.region(
        Mapping {
            address: 0x8000,
            length: 4096,
            flags: 6,
            kind: RegionKind::Stack,
        },
        None,
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    s.region(
        Mapping {
            address: 0x10000,
            length: 65536,
            flags: 6,
            kind: RegionKind::Ram(RegionLifetime::Phase),
        },
        Some(0),
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    assert!(memory.observation().reserved_bytes > initial + 65536);
    s.write(0x1000, 4, 77, &mut || Ok(())).unwrap();
    let observation = s
        .observation(
            ExecutionStop::Returned {
                low: None,
                high: None,
            },
            0,
            true,
            &mut || Ok(()),
        )
        .unwrap();
    s.recycle(observation);
    assert_eq!(memory.observation().reserved_bytes, initial);
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut || Ok(()))
            .unwrap(),
        Some(77)
    );
    for address in [0x8000, 0x10000] {
        assert_eq!(
            s.read(address, 4, MemoryAccess::Read, &mut || Ok(()))
                .unwrap(),
            None
        );
    }
}

#[test]
fn modeled_allocation_capacity_is_owned_but_only_requested_bytes_are_accessible() {
    for lifetime in [RegionLifetime::Phase, RegionLifetime::Session] {
        let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
        let mut s = session(&memory, None);
        s.calls
            .install(
                &[CallDeclaration {
                    repetition: blobray_domain::CallRepetition::Finite,
                    id: "allocator".into(),
                    applicability: "fixture".into(),
                    lifetime: RegionLifetime::Phase,
                    binding: CallBinding {
                        address: 0x2000,
                        boundary: CallBoundary::Unmapped,
                        allow_tail: false,
                    },
                    argument_words: 1,
                    responses: vec![CallResponse {
                        return_words: [Some(0x4000), None],
                        outputs: vec![],
                        allocation: Some(CallAllocation {
                            address: 0x4000,
                            size_argument: 0,
                            capacity: 4096,
                            lifetime,
                        }),
                        delay_micros: None,
                    }],
                }],
                &mut || Ok(()),
            )
            .unwrap();
        let before = memory.observation().reserved_bytes;
        let result = s
            .call(
                &CallInput {
                    site: 0x1000,
                    target: 0x2000,
                    tail: false,
                    indirect: false,
                    stack: Some(0x9000),
                    arguments: [Some(4), None, None, None, None, None, None, None],
                },
                &mut || Ok(()),
            )
            .unwrap();
        assert_eq!(
            result,
            CallDispatch::Returned {
                words: [Some(0x4000), None]
            }
        );
        assert!(memory.observation().reserved_bytes >= before + 8192);
        assert_eq!(
            s.read(0x4000, 4, MemoryAccess::Read, &mut || Ok(()))
                .unwrap(),
            Some(0)
        );
        for address in [0x4004, 0x4ffc] {
            assert_eq!(
                s.read(address, 4, MemoryAccess::Read, &mut || Ok(()))
                    .unwrap(),
                None
            );
            assert!(!s.write(address, 4, 7, &mut || Ok(())).unwrap());
        }
        let o = s
            .observation(
                ExecutionStop::Returned {
                    low: Some(0x4000),
                    high: None,
                },
                1,
                true,
                &mut || Ok(()),
            )
            .unwrap();
        s.recycle(o);
        assert_eq!(
            s.read(0x4000, 4, MemoryAccess::Read, &mut || Ok(()))
                .unwrap(),
            if lifetime == RegionLifetime::Session {
                Some(0)
            } else {
                None
            }
        );
        drop(s);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
}
#[test]
fn event_capacity_is_admitted_as_events_occur() {
    let event = || ExecutionEvent::DelayMicros { value: 1 };
    let size = std::mem::size_of::<ExecutionEvent>() as u64;
    let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
    let mut c = || Ok(());
    // A large declared capacity costs nothing until events are recorded.
    let mut s = Session::new(&memory, 1 << 20, Default::default(), &mut c).unwrap();
    let baseline = memory.used();
    s.event(event(), &mut c).unwrap();
    assert_eq!(
        memory.used(),
        baseline + INITIAL_EVENT_CAPACITY as u64 * size
    );
    for _ in 1..=INITIAL_EVENT_CAPACITY {
        s.event(event(), &mut c).unwrap();
    }
    assert_eq!(
        memory.used(),
        baseline + 2 * INITIAL_EVENT_CAPACITY as u64 * size
    );
    // Growth beyond the working limit fails without losing recorded events.
    let recorded = s.events.len();
    while s.events.len() < s.events.capacity() {
        s.event(event(), &mut c).unwrap();
    }
    let full = s.events.len();
    let error = loop {
        match s.event(event(), &mut c) {
            Ok(()) => continue,
            Err(error) => break error,
        }
    };
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert!(s.events.len() >= full && full > recorded);
    drop(s);
    assert_eq!(memory.used(), 0);
}

#[test]
fn a_preload_writes_known_bytes_only_inside_one_writable_region() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, None);
    s.region(
        Mapping {
            address: 0x2000,
            length: 8,
            flags: 4,
            kind: RegionKind::Image,
        },
        Some(0),
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    let preload = |address, bytes: &[u8]| MemoryPreload {
        address,
        bytes: bytes.to_vec(),
    };
    s.preload(&preload(0x1002, &[1, 2, 3, 4]), &mut || Ok(()))
        .unwrap();
    // Preloaded bytes are known; the rest of the unseeded region is not.
    assert_eq!(
        s.read(0x1002, 4, MemoryAccess::Read, &mut || Ok(()))
            .unwrap(),
        Some(0x0403_0201)
    );
    assert_eq!(
        s.read(0x1000, 2, MemoryAccess::Read, &mut || Ok(()))
            .unwrap(),
        None
    );
    // Across a region's end, outside every region, or into read-only memory.
    for (address, bytes) in [
        (0x1006, &[0u8; 4][..]),
        (0x3000, &[0; 1]),
        (0x2000, &[0; 4]),
    ] {
        assert_eq!(
            s.preload(&preload(address, bytes), &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
    }
}
