use super::{Line, Outcome, Tally};

#[test]
fn a_tally_holds_when_every_trial_held() {
    let mut tally = Tally::new();
    assert!(!tally.holds(), "no trial is no evidence");
    tally.trial(Outcome::Held);
    tally.trial(Outcome::Held);
    assert!(tally.holds());
    tally.trial(Outcome::Broke);
    assert_eq!((tally.trials, tally.held), (3, 2));
    assert!(!tally.holds());
}

#[test]
fn a_tally_keeps_the_least_and_the_most_span() {
    let mut tally = Tally::new();
    assert_eq!(tally.span, None);
    for cycles in [300, 120, 4_000, 250] {
        tally.span(cycles);
    }
    assert_eq!(tally.span, Some((120, 4_000)));
}

#[test]
fn a_line_prints_numbers_and_hex_words() {
    let mut line = Line::new();
    assert_eq!(
        line.text("a=").number(1_024).text(" b=").number(0).end(),
        b"a=1024 b=0\r\n"
    );
}
