use super::*;

fn job(id: &str, state: JobState) -> Job {
    Job {
        id: id.into(),
        owner: String::from("stand"),
        command: vec![String::from("run"), String::from("icmp-latency")],
        checkout: PathBuf::from("/checkout"),
        after: None,
        after_any: false,
        enqueued_unix_millis: oer_durable::unix_millis(),
        pid: Some(std::process::id()),
        pid_started_unix_millis: oer_process::proc::started_unix_millis(std::process::id()),
        log: Some(PathBuf::from("/checkout/target/hil/jobs/x.log")),
        fixed: Vec::new(),
        state,
    }
}

#[test]
fn a_job_settles_when_it_finishes_or_its_process_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    jobs.write(&job("1-a", JobState::Started)).unwrap();
    assert_eq!(jobs.settled("1-a").unwrap(), None);
    jobs.write(&job(
        "2-a",
        JobState::Finished {
            outcome: JobOutcome::Failed,
            runs: vec![String::from("r")],
        },
    ))
    .unwrap();
    assert_eq!(jobs.settled("2-a").unwrap(), Some(JobOutcome::Failed));
    assert_eq!(jobs.wait("2-a").unwrap(), JobOutcome::Failed);
    // A recycled PID has another start time.
    let mut gone = job("3-a", JobState::Started);
    gone.pid_started_unix_millis = gone.pid_started_unix_millis.map(|started| started + 5_000);
    jobs.write(&gone).unwrap();
    assert_eq!(jobs.settled("3-a").unwrap(), Some(JobOutcome::Abandoned));
    assert!(jobs.read("nope").is_err());
    assert!(jobs.read("../x").is_err());
    assert_eq!(
        jobs.unfinished()
            .iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>(),
        ["1-a"]
    );
}

#[test]
fn the_queue_names_each_job_s_phase_and_who_waits_for_whom() {
    let mut waiting = job("5-b", JobState::Pending);
    waiting.after = Some(String::from("4-a"));
    let started = job("4-a", JobState::Started);
    let text = describe_views(&[
        JobView {
            job: &started,
            phase: Phase::Building,
        },
        JobView {
            job: &waiting,
            phase: Phase::WaitingForJob,
        },
    ]);
    assert!(
        text.contains("job:     4-a stand `cargo hil run icmp-latency` building images\n"),
        "{text}"
    );
    assert!(
        text.contains("5-b stand `cargo hil run icmp-latency` waits for job 4-a\n"),
        "{text}"
    );
}

#[test]
fn a_job_whose_process_is_gone_is_recorded_abandoned_and_left_out() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    let mut gone = job("9-a", JobState::Started);
    gone.pid_started_unix_millis = gone.pid_started_unix_millis.map(|started| started + 5_000);
    jobs.write(&gone).unwrap();
    assert!(jobs.unfinished().is_empty());
    assert_eq!(jobs.settled("9-a").unwrap(), Some(JobOutcome::Abandoned));
}

#[test]
fn only_a_judged_run_lets_a_dependent_start() {
    assert!(JobOutcome::Passed.lets_dependents_start());
    assert!(JobOutcome::Failed.lets_dependents_start());
    for outcome in [
        JobOutcome::NoRun,
        JobOutcome::Blocked,
        JobOutcome::Broken,
        JobOutcome::Interrupted,
        JobOutcome::Abandoned,
    ] {
        assert!(!outcome.lets_dependents_start(), "{outcome}");
    }
}

#[test]
fn the_queue_shows_recent_jobs_that_ended_without_a_judged_run() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::at(directory.path().to_owned());
    let log = directory.path().join("x.log");
    std::fs::write(&log, "building\nerror: unknown HIL scenario 'a b'\n\n").unwrap();
    let mut lost = job(
        "1-a",
        JobState::Finished {
            outcome: JobOutcome::NoRun,
            runs: Vec::new(),
        },
    );
    lost.log = Some(log);
    jobs.write(&lost).unwrap();
    jobs.write(&job(
        "2-a",
        JobState::Finished {
            outcome: JobOutcome::Passed,
            runs: vec![String::from("r")],
        },
    ))
    .unwrap();
    let mut old = job(
        "0-a",
        JobState::Finished {
            outcome: JobOutcome::NoRun,
            runs: Vec::new(),
        },
    );
    old.enqueued_unix_millis = 1;
    jobs.write(&old).unwrap();
    let ended = jobs.recently_ended_unjudged(Duration::from_secs(3600), 5);
    assert_eq!(
        ended.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
        ["1-a"]
    );
    // A week-old record is gone.
    assert!(jobs.read("0-a").is_err());
    assert!(
        describe_ended(&ended).contains(
            "1-a stand `cargo hil run icmp-latency` no-run: error: unknown HIL scenario 'a b'"
        ),
        "{}",
        describe_ended(&ended)
    );
}

#[test]
fn a_job_s_phase_is_where_its_tickets_are_in_the_queue() {
    let started = job("1-a", JobState::Started);
    let none = || std::iter::empty::<Option<&str>>();
    assert_eq!(phase_of(&started, none(), none()), Phase::Building);
    assert_eq!(
        phase_of(&started, none(), [None, Some("1-a")].into_iter()),
        Phase::WaitingForStand
    );
    assert_eq!(
        phase_of(&started, [Some("1-a")].into_iter(), none()),
        Phase::Holding
    );
    // Another job's lease, or one of no job, is not this job's.
    assert_eq!(
        phase_of(&started, [Some("2-b"), None].into_iter(), none()),
        Phase::Building
    );
    let mut after = job("2-a", JobState::Pending);
    after.after = Some(String::from("1-a"));
    assert_eq!(phase_of(&after, none(), none()), Phase::WaitingForJob);
}
