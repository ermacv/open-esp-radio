use super::*;

fn budget(attempts: u32) -> BluetoothDiagnosticReadBudget {
    BluetoothDiagnosticReadBudget::new(attempts).unwrap()
}

#[test]
fn zero_attempt_budget_is_rejected() {
    assert_eq!(BluetoothDiagnosticReadBudget::new(0), None);
    assert_eq!(budget(3).attempts(), 3);
}

#[test]
fn first_agreeing_attempt_is_the_sample() {
    let mut attempts = 0;
    let sample = settle(budget(4), || {
        attempts += 1;
        (attempts == 2).then_some(true)
    });
    assert_eq!(sample, Ok(true));
    assert_eq!(attempts, 2);
}

#[test]
fn a_pair_that_never_agrees_stops_at_the_budget() {
    let mut attempts = 0;
    let sample: Result<bool, _> = settle(budget(5), || {
        attempts += 1;
        None
    });
    assert_eq!(sample, Err(BluetoothDiagnosticUnsettled));
    assert_eq!(attempts, 5);
}
