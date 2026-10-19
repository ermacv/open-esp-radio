use super::*;
fn declaration(
    lifetime: RegionLifetime,
    samples: Option<Vec<u32>>,
    busy: u32,
) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "transport".into(),
        applicability: "synthetic shared bus".into(),
        lifetime,
        behavior: DeviceBehavior::CommandBank(CommandBank {
            selector_mask: 0xff,
            data_mask: 0xff00,
            read_command: 0x10000,
            write_command: 0x20000,
            busy_mask: 0x40000,
            reset_command: Some(0x80000),
            ports: vec![
                CommandPort {
                    address: 0x3000,
                    initial: 0,
                    initial_busy_reads: 0,
                    busy_reads: busy,
                },
                CommandPort {
                    address: 0x3004,
                    initial: 0,
                    initial_busy_reads: 0,
                    busy_reads: busy,
                },
            ],
            cells: vec![CommandCell {
                selector: 1,
                initial: 42,
                reads: samples,
            }],
        }),
    }
}
fn request(f: &Fixture, d: DeviceDeclaration, word: u32) -> ExecutionRequest {
    let mut r = f.request();
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(word)];
    r.cases[0].vendor.models = vec![d];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r.cases[0].relation.as_mut().unwrap().returns.low = false;
    r
}
fn rows(f: &Fixture, r: ExecutionRequest) -> serde_json::Value {
    f.run(r, budget()).unwrap().facts()
}
fn models(facts: &serde_json::Value) -> Vec<ModelObservation> {
    facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "model" && r["replacement"] == false)
        .map(|r| serde_json::from_value(r["observation"].clone()).unwrap())
        .collect()
}
#[test]
fn command_issue_busy_ready_phases_preserve_pending_ownership() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut r = request(
        &f,
        declaration(RegionLifetime::Session, Some(vec![7]), 1),
        0x10001,
    );
    for name in ["busy", "ready"] {
        let mut next = r.cases[0].clone();
        next.name = name.into();
        next.reset = SessionReset::Warm;
        next.vendor.entry = 0x1008;
        next.vendor.models.clear();
        next.replacement = Some(next.vendor.clone());
        r.cases.push(next);
    }
    let facts = rows(&f, r);
    assert_eq!(facts["verdict"], "MATCH");
    let m = models(&facts);
    assert_eq!(
        m.iter()
            .map(|m| m.commands.unwrap().pending)
            .collect::<Vec<_>>(),
        [1, 1, 0]
    );
    assert_eq!(
        m.iter().map(|m| m.status).collect::<Vec<_>>(),
        [ModelStatus::Open, ModelStatus::Open, ModelStatus::Complete]
    );
    assert_eq!(m[2].commands.unwrap().scripted_reads, 1);
    assert_eq!(m[2].commands.unwrap().completed, 1);
    let values: Vec<_> = facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| {
            r["kind"] == "event" && r["replacement"] == false && r["event"]["kind"] == "read"
        })
        .map(|r| r["event"]["value"].as_u64().unwrap())
        .collect();
    assert_eq!(values, [0x50701, 0x10701]);
}
#[test]
fn returned_code_cannot_complete_pending_commands_or_unused_samples() {
    let f = Fixture::new(&[0x00b52023, 0x00008067]);
    let facts = rows(
        &f,
        request(&f, declaration(RegionLifetime::Phase, None, 0), 0x10001),
    );
    assert_eq!(facts["verdict"], "INCOMPLETE");
    assert_eq!(models(&facts)[0].commands.unwrap().pending, 1);
    assert_eq!(models(&facts)[0].status, ModelStatus::Incomplete);
    let f = Fixture::new(&[0x00008067]);
    let facts = rows(
        &f,
        request(&f, declaration(RegionLifetime::Phase, Some(vec![7]), 0), 0),
    );
    assert_eq!(models(&facts)[0].remaining_reads, 1);
    assert_eq!(facts["verdict"], "INCOMPLETE");
}
#[test]
fn failed_environment_blocks_warm_execution_and_cold_starts_a_fresh_bank() {
    let f = Fixture::new(&[0x00b52023, 0x00008067, 0x00052503, 0x00008067]);
    let mut r = request(&f, declaration(RegionLifetime::Phase, None, 0), 0x10001);
    let mut warm = r.cases[0].clone();
    warm.name = "blocked".into();
    warm.reset = SessionReset::Warm;
    warm.vendor.entry = 0x1008;
    warm.vendor.models.clear();
    warm.replacement = Some(warm.vendor.clone());
    r.cases.push(warm);
    let mut cold = r.cases[0].clone();
    cold.name = "fresh".into();
    cold.vendor.entry = 0x1008;
    cold.replacement = Some(cold.vendor.clone());
    r.cases.push(cold);
    let facts = rows(&f, r);
    let stops: Vec<_> = facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "outcome" && r["replacement"] == false)
        .map(|r| &r["stop"])
        .collect();
    assert_eq!(stops[1]["kind"], "blocked-by-prior-phase");
    assert_eq!(stops[2]["kind"], "returned");
    assert_eq!(stops[2]["low"], 0);
    let m = models(&facts);
    assert_eq!(m.last().unwrap().commands.unwrap().issued, 0);
    assert_eq!(m.last().unwrap().status, ModelStatus::Complete);
}
#[test]
fn invalid_commands_width_overwrite_and_exhaustion_have_explicit_model_gaps() {
    for (code, word, samples, issue) in [
        (
            vec![0x00b52023, 0x00008067],
            0x10002,
            None,
            DeviceIssue::UnknownSelector { selector: 2 },
        ),
        (
            vec![0x00b52023, 0x00008067],
            0x90001,
            None,
            DeviceIssue::InvalidCommand { value: 0x90001 },
        ),
        (
            vec![0x00b50023, 0x00008067],
            1,
            None,
            DeviceIssue::AccessWidth,
        ),
        (
            vec![0x00b52023, 0x00b52023, 0x00008067],
            0x10001,
            None,
            DeviceIssue::PendingCommand,
        ),
        (
            vec![0x00b52023, 0x00008067],
            0x10001,
            Some(vec![]),
            DeviceIssue::ExhaustedReads,
        ),
    ] {
        let f = Fixture::new(&code);
        let facts = rows(
            &f,
            request(&f, declaration(RegionLifetime::Phase, samples, 0), word),
        );
        assert_eq!(facts["verdict"], "INCOMPLETE");
        assert_eq!(models(&facts)[0].issue, Some(issue));
    }
}
#[test]
fn selected_reply_differences_and_capacity_failure_use_the_same_application_path() {
    let f = Fixture::new(&[0x00b52023, 0x00052503, 0x00008067]);
    let mut r = request(
        &f,
        declaration(RegionLifetime::Phase, Some(vec![7]), 0),
        0x10001,
    );
    if let DeviceBehavior::CommandBank(b) =
        &mut r.cases[0].replacement.as_mut().unwrap().models[0].behavior
    {
        b.cells[0].reads = Some(vec![9]);
    }
    let facts = rows(&f, r.clone());
    assert_eq!(facts["verdict"], "DIFF");
    r.max_events = 1;
    let failed = f.run(r, budget());
    assert_eq!(failed.unwrap_err().code, ErrorCode::ResourceLimited);
}
