use std::path::PathBuf;

use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

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
fn a_job_names_what_it_was_fixed_with_and_older_records_still_read() {
    let frozen = Frozen {
        cli: PathBuf::from("/c/target/hil/jobs/cli/1/oer-hil-cli"),
        runner: PathBuf::from("/c/target/hil/runners/2/runner"),
        receipt: PathBuf::from("/c/target/hil/runners/2/receipt-3.json"),
        snapshot: Some(PathBuf::from("/c/target/hil/chip-a/source-snapshots/4")),
    };
    let mut fixed = job("1-a", JobState::Pending);
    fixed.fixed = frozen.paths();
    let text = serde_json::to_string(&fixed).unwrap();
    let read: Job = serde_json::from_str(&text).unwrap();
    assert_eq!(read.fixed.len(), 4);
    assert!(
        read.fixed
            .contains(&PathBuf::from("/c/target/hil/chip-a/source-snapshots/4"))
    );
    // A record written before jobs were fixed has no such list.
    let mut older: serde_json::Value = serde_json::from_str(&text).unwrap();
    older.as_object_mut().unwrap().remove("fixed");
    let older: Job = serde_json::from_value(older).unwrap();
    assert!(older.fixed.is_empty());
}

#[test]
fn enqueue_and_after_are_taken_before_a_double_dash() {
    let (enqueue, after, rest) = take(args(&[
        "run",
        "s",
        "--enqueue",
        "--after",
        "12-a",
        "--",
        "--enqueue",
    ]))
    .unwrap();
    assert!(enqueue);
    assert_eq!(
        after,
        Some(After {
            job: String::from("12-a"),
            any: false
        })
    );
    assert_eq!(rest, args(&["run", "s", "--", "--enqueue"]));
    let (enqueue, after, _) = take(args(&["run", "--after=7-b"])).unwrap();
    assert!(!enqueue);
    assert_eq!(after.map(|after| after.job), Some(String::from("7-b")));
    let (_, after, _) = take(args(&["run", "--after-any", "8-c"])).unwrap();
    assert_eq!(
        after,
        Some(After {
            job: String::from("8-c"),
            any: true
        })
    );
    assert!(take(args(&["run", "--after", "a", "--after-any", "b"])).is_err());
    assert!(take(args(&["run", "--after"])).is_err());
    assert!(take(args(&["run", "--after", "a", "--after=b"])).is_err());
}
