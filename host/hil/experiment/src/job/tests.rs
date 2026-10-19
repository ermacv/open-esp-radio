use super::*;

#[test]
fn a_job_outcome_is_the_worst_of_its_runs() {
    assert_eq!(outcome_of_runs(&[]), JobOutcome::NoRun);
    assert_eq!(
        outcome_of_runs(&[Some(Outcome::Passed), Some(Outcome::Passed)]),
        JobOutcome::Passed
    );
    assert_eq!(
        outcome_of_runs(&[Some(Outcome::Passed), Some(Outcome::Failed)]),
        JobOutcome::Failed
    );
    assert_eq!(
        outcome_of_runs(&[Some(Outcome::Failed), Some(Outcome::Broken)]),
        JobOutcome::Broken
    );
    assert_eq!(
        outcome_of_runs(&[Some(Outcome::Skipped)]),
        JobOutcome::Blocked
    );
    assert_eq!(
        outcome_of_runs(&[Some(Outcome::BoardQuarantined), Some(Outcome::Passed)]),
        JobOutcome::Interrupted
    );
    // Every outcome has its own exit code.
    let codes = [
        JobOutcome::Passed,
        JobOutcome::Failed,
        JobOutcome::Interrupted,
        JobOutcome::Blocked,
        JobOutcome::Broken,
        JobOutcome::NoRun,
        JobOutcome::Abandoned,
    ]
    .map(JobOutcome::exit_code);
    assert_eq!(
        codes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        codes.len()
    );
    assert_eq!(JobOutcome::Passed.exit_code(), 0);
}
