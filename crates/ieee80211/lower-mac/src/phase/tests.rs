use super::*;
use core::{cell::Cell, num::NonZeroU8};
use std::vec::Vec;

fn stage(rate: u8) -> RetryStage<u8> {
    RetryStage {
        rate,
        transmissions: NonZeroU8::new(rate + 1).unwrap(),
        protection: if rate.is_multiple_of(2) {
            Protection::RtsCts
        } else {
            Protection::CtsToSelf
        },
    }
}

fn plan() -> RetryPlan<u8> {
    (1..4).fold(RetryPlan::new(stage(0)), |plan, rate| {
        plan.then(stage(rate)).unwrap()
    })
}

#[test]
fn filtering_every_stage_subset_keeps_order_budget_and_protection() {
    for mask in 0..16_u8 {
        let filtered = plan().filter_map(|rate| (mask & (1 << rate) != 0).then_some(rate));
        let expected: Vec<_> = (0..4)
            .filter(|rate| mask & (1 << rate) != 0)
            .map(|rate| {
                let stage = stage(rate);
                (rate, stage.transmissions, stage.protection)
            })
            .collect();
        match filtered {
            None => assert!(expected.is_empty()),
            Some(plan) => {
                let actual: Vec<_> = plan
                    .stages()
                    .map(|stage| (stage.rate, stage.transmissions, stage.protection))
                    .collect();
                assert_eq!(actual, expected);
                assert_eq!(
                    plan.transmissions(),
                    expected
                        .iter()
                        .map(|(_, count, _)| u16::from(count.get()))
                        .sum()
                );
            }
        }
    }
}

#[test]
fn limiting_and_appending_keep_the_initialized_tail_contiguous() {
    let complete: Vec<_> = plan()
        .stages()
        .flat_map(|stage| core::iter::repeat_n(stage.rate, usize::from(stage.transmissions.get())))
        .collect();
    for limit in 1..=12 {
        let cut = plan().limit(NonZeroU8::new(limit).unwrap());
        let actual: Vec<_> = cut
            .stages()
            .flat_map(|stage| {
                core::iter::repeat_n(stage.rate, usize::from(stage.transmissions.get()))
            })
            .collect();
        assert_eq!(
            actual,
            complete
                .iter()
                .copied()
                .take(usize::from(limit))
                .collect::<Vec<_>>()
        );
        let count = cut.stages().count();
        match cut.then(stage(9)) {
            Ok(appended) => {
                assert!(count < RETRY_STAGES);
                assert_eq!(appended.stages().count(), count + 1);
                assert_eq!(appended.stages().last().unwrap().rate, 9);
            }
            Err((unchanged, refused)) => {
                assert_eq!(count, RETRY_STAGES);
                assert_eq!(unchanged.stages().count(), count);
                assert_eq!(refused.rate, 9);
            }
        }
    }
}

#[test]
fn a_full_plan_returns_every_owner_with_the_refused_fifth_stage() {
    #[derive(Debug)]
    struct Rate<'a>(&'a Cell<u8>);
    impl Drop for Rate<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let dropped = Cell::new(0);
    let stage = || RetryStage {
        rate: Rate(&dropped),
        transmissions: NonZeroU8::MAX,
        protection: Protection::None,
    };
    let plan = (1..RETRY_STAGES).fold(RetryPlan::new(stage()), |plan, _| {
        plan.then(stage()).unwrap()
    });
    let (plan, refused) = plan.then(stage()).unwrap_err();
    assert_eq!(dropped.get(), 0);
    assert_eq!(plan.transmissions(), 4 * u16::from(u8::MAX));
    drop(refused);
    assert_eq!(dropped.get(), 1);
    drop(plan);
    assert_eq!(dropped.get(), 5);
}

#[test]
fn mapping_keeps_all_stages_and_stops_at_the_first_rate_error() {
    let mapped = plan().map(|rate| rate + 10);
    assert!(mapped.stages().map(|stage| stage.rate).eq(10..14));
    assert_eq!(mapped.transmissions(), plan().transmissions());
    let mapped = plan().try_map(|rate| Ok::<_, ()>(rate + 20)).unwrap();
    assert!(mapped.stages().map(|stage| stage.rate).eq(20..24));
    assert_eq!(mapped.transmissions(), plan().transmissions());

    let mut visited = Vec::new();
    let failure = plan().try_map(|rate| {
        visited.push(rate);
        if rate == 2 { Err(rate) } else { Ok(rate) }
    });
    assert_eq!(failure.unwrap_err(), 2);
    assert_eq!(visited, [0, 1, 2]);
}
