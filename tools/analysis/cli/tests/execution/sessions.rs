use super::*;

#[test]
fn root_callee_saved_registers_start_known_and_arguments_stay_explicit() {
    // Entry prelude sets s0, then tail-enters a conventional save/restore leaf.
    // The alternate entry has the same body without the prelude: a root's
    // callee-saved registers start at zero, so its prologue can spill s0.
    let f = Fixture::new(&[
        0x00000413, 0x0080006f, 0x00000013, 0xff010113, 0x00812023, 0x00700513, 0x00012403,
        0x01010113, 0x00008067,
    ]);
    let mut request = f.request();
    request.vendor.stack.fill = Some(0xa5);
    request.replacement.as_mut().unwrap().stack.fill = Some(0xa5);
    request.cases[0].name = "explicit-saved-register".into();
    let mut unknown = request.cases[0].clone();
    unknown.name = "undeclared-saved-register".into();
    unknown.vendor.entry = 0x100c;
    unknown.replacement.as_mut().unwrap().entry = 0x100c;
    request.cases.push(unknown);
    let run = f.run(request, budget()).unwrap();
    let facts = run.facts();
    let records = facts["records"].as_array().unwrap();
    for replacement in [false, true] {
        let stop = |case| {
            &records
                .iter()
                .find(|r| {
                    r["kind"] == "outcome" && r["case"] == case && r["replacement"] == replacement
                })
                .unwrap()["stop"]
        };
        for case in [0, 1] {
            assert_eq!(stop(case)["kind"], "returned");
            assert_eq!(stop(case)["low"], 7);
        }
    }
    let verdicts: Vec<_> = records
        .iter()
        .filter(|r| r["kind"] == "comparison")
        .map(|r| r["result"]["verdict"].as_str().unwrap())
        .collect();
    assert_eq!(verdicts, ["MATCH", "MATCH"]);
    // An argument register that is not supplied stays unknown: `mv a0, a1`.
    let f = Fixture::new(&[0x00058513, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments = vec![Some(0)];
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    let facts = run.facts();
    let stop = &facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "outcome")
        .unwrap()["stop"];
    assert_eq!(stop["kind"], "returned");
    assert!(stop["low"].is_null(), "{stop}");
}

fn phases(f: &Fixture, lifetime: RegionLifetime) -> ExecutionRequest {
    let mut request = f.request();
    let first = &mut request.cases[0];
    first.name = "setup".into();
    first.vendor.arguments = vec![Some(0x3000), Some(41)];
    first.vendor.memory = vec![ExecutionRegion {
        seed: MemorySeed {
            address: 0x3000,
            length: 8,
            fill: Some(0),
            bytes: vec![],
        },
        lifetime,
    }];
    first.replacement = Some(first.vendor.clone());
    let mut second = first.clone();
    second.name = "read".into();
    second.reset = SessionReset::Warm;
    second.vendor.entry = 0x1008;
    second.vendor.memory.clear();
    second.replacement = Some(second.vendor.clone());
    request.cases.push(second);
    request
}

#[test]
fn multi_entry_setup_warm_cold_and_blocking_run_as_one_request() {
    // Separate setup and read entries over the same captured address space.
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut request = phases(&f, RegionLifetime::Session);
    let mut cold_read = request.cases[1].clone();
    cold_read.name = "cold-has-no-prior-ram".into();
    cold_read.reset = SessionReset::Cold;
    request.cases.push(cold_read);
    let mut blocked = request.cases[0].clone();
    blocked.name = "blocked-setup-cannot-resume-chain".into();
    blocked.reset = SessionReset::Warm;
    request.cases.push(blocked);
    let mut restart = request.cases[0].clone();
    restart.name = "new-independent-chain".into();
    restart.vendor.arguments[1] = Some(99);
    restart.replacement = Some(restart.vendor.clone());
    request.cases.push(restart);
    let mut last = request.cases[1].clone();
    last.name = "new-chain-read".into();
    request.cases.push(last);
    let run = f.run(request.clone(), budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["complete"], false);
    assert_eq!(result["verdict"], "INCOMPLETE");
    let records = result["records"].as_array().unwrap();
    // Six phases of outcome/outcome/comparison, then one coverage record per side.
    assert_eq!(records.len(), 20);
    assert_eq!(records[3]["stop"]["low"], 41);
    assert_eq!(records[6]["stop"]["reason"]["address"], 0x3000);
    assert_eq!(records[9]["stop"]["kind"], "blocked-by-prior-phase");
    assert_eq!(records[9]["steps"], 0);
    assert_eq!(records[15]["stop"]["low"], 99);
}

#[test]
fn phase_ram_expires_and_redeclaration_uses_new_seed_without_changing_session_owner() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut request = phases(&f, RegionLifetime::Phase);
    let run = f.run(request.clone(), budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["records"][3]["stop"]["kind"], "incomplete");
    let mut region = request.cases[0].vendor.memory[0].clone();
    region.seed.bytes = 77u32.to_le_bytes().to_vec();
    request.cases[1].vendor.memory.push(region);
    request.cases[1].replacement = Some(request.cases[1].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["records"][3]["stop"]["low"], 77);
    assert_eq!(result["verdict"], "MATCH");
    let mut bad = phases(&f, RegionLifetime::Session);
    let mut region = bad.cases[0].vendor.memory[0].clone();
    region.lifetime = RegionLifetime::Phase;
    bad.cases[1].vendor.memory.push(region);
    bad.cases[1].replacement = Some(bad.cases[1].vendor.clone());
    let error = f.run(bad, budget()).unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    // The conflict names both regions and the kind of the existing one.
    assert!(
        error
            .message
            .contains("overlaps the session's Ram(Session) region"),
        "{}",
        error.message
    );
}

#[test]
fn phase_entry_reset_validation_and_shared_budget_fail() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    for which in 0..3 {
        let mut request = phases(&f, RegionLifetime::Session);
        match which {
            0 => request.cases[0].reset = SessionReset::Warm,
            1 => request.cases[1].vendor.entry = 0x1009,
            _ => request.cases[1].replacement.as_mut().unwrap().entry = u32::MAX - 1,
        }
        let err = match f.run(request, budget()) {
            Ok(_) => panic!("invalid phase was admitted"),
            Err(err) => err,
        };
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }
    let mut request = phases(&f, RegionLifetime::Session);
    // A later phase loops forever; a completed setup does not end the request.
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x0000006f]);
    request.vendor = f.target.clone();
    request.replacement = Some(f.target.clone());
    let mut b = budget();
    b.max_work_units = Some(100000);
    let run = f.run(request, b);
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
}

#[test]
fn cold_reset_releases_both_sides_before_allocating_the_next_address_spaces() {
    let f = Fixture::new(&[0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.memory = vec![ram(MemorySeed {
        address: 0x10000000,
        length: 4,
        fill: None,
        bytes: vec![],
    })];
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request.cases[0].replacement.as_mut().unwrap().memory[0]
        .seed
        .length = 10 * 1024 * 1024;
    let mut second = request.cases[0].clone();
    second.name = "swap-large-side-on-cold-reset".into();
    second.vendor.memory[0].seed.length = 10 * 1024 * 1024;
    second.replacement.as_mut().unwrap().memory[0].seed.length = 4;
    request.cases.push(second);
    // Each phase fits the 32 MiB budget. Retaining the old replacement while
    // preparing the new vendor would overlap two 20 MiB byte/knownness pairs.
    let run = f.run(request, budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "MATCH");
}

#[test]
fn a_setup_case_runs_both_sides_and_records_no_comparison() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut request = phases(&f, RegionLifetime::Session);
    // The setup case stores on both sides without comparing anything: the
    // replacement stores at another address, so the setup returns differ,
    // and only the compared read, of each side's stored word, decides.
    request.cases[0].relation = None;
    request.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(0x3004);
    request.cases[1].replacement.as_mut().unwrap().arguments[0] = Some(0x3004);
    let run = f.run(request, budget()).unwrap();
    // Reading the execution back re-verifies its record order.
    let result = run.facts();
    let comparisons: Vec<_> = result["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "comparison")
        .map(|r| r["case"].as_u64().unwrap())
        .collect();
    assert_eq!(comparisons, [1]);
    assert_eq!(result["verdict"], "MATCH");
    // Compared, the same setup case is a difference.
    let mut compared = phases(&f, RegionLifetime::Session);
    compared.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(0x3004);
    compared.cases[1].replacement.as_mut().unwrap().arguments[0] = Some(0x3004);
    let run = f.run(compared, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "DIFF");
}
