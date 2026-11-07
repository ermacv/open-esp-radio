use super::*;

fn at(micros: u64) -> TsfInstant {
    TsfInstant::from_micros(micros)
}

#[test]
fn the_first_set_jumps_and_a_set_within_the_drift_keeps_the_generation() {
    let mut relation = TsfRelation::new(3, TsfDuration::from_micros(1));
    let initial = relation.generation();
    assert_eq!(relation.set(at(0), at(1_000)), Ok(TsfSetKind::Jump));
    let anchored = relation.generation();
    assert_ne!(anchored, initial);
    // 200 ppm of one second is 200 µs, plus one microsecond of uncertainty.
    assert_eq!(
        relation.set(at(1_001_000), at(1_001_201)),
        Ok(TsfSetKind::Drift)
    );
    assert_eq!(relation.generation(), anchored);
    assert_eq!(
        relation.set(at(2_001_201), at(2_001_403)),
        Ok(TsfSetKind::Jump)
    );
    assert_ne!(relation.generation(), anchored);
}

#[test]
fn a_reading_below_the_last_set_and_a_break_both_start_a_generation() {
    let mut relation = TsfRelation::new(0, TsfDuration::from_micros(0));
    assert_eq!(relation.set(at(10), at(500)), Ok(TsfSetKind::Jump));
    let anchored = relation.generation();
    assert_eq!(relation.set(at(400), at(400)), Ok(TsfSetKind::Jump));
    assert_ne!(relation.generation(), anchored);
    let before_break = relation.generation();
    relation.break_relation();
    assert_ne!(relation.generation(), before_break);
    assert_eq!(relation.set(at(400), at(400)), Ok(TsfSetKind::Jump));
}

#[test]
fn the_tolerance_rounds_the_drift_up_and_refuses_overflow() {
    let relation = TsfRelation::new(0, TsfDuration::from_micros(2));
    assert_eq!(
        relation.tolerance(TsfDuration::from_micros(1)),
        Ok(TsfDuration::from_micros(3))
    );
    let uncertain = TsfRelation::new(0, TsfDuration::from_micros(u64::MAX));
    assert_eq!(
        uncertain.tolerance(TsfDuration::from_micros(1)),
        Err(TsfTimingError::ToleranceOverflow)
    );
}
