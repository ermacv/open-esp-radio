use super::*;

#[test]
fn independent_errors_are_retained_and_do_not_prevent_later_checks() {
    let mut checks = Checks::default();
    checks.run("first", || Err("missing tool".into())).unwrap();
    checks.run("second", || Ok(())).unwrap();
    checks
        .run("third", || Err("missing fixture".into()))
        .unwrap();
    assert!(!checks.passed());
    assert_eq!(checks.checks.len(), 3);
    assert_eq!(checks.checks[0].failure.as_deref(), Some("missing tool"));
    assert!(checks.checks[1].passed);
    assert_eq!(checks.checks[2].failure.as_deref(), Some("missing fixture"));
}
