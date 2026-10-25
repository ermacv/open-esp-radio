extern crate std;

use std::rc::Rc;
use std::sync::{Barrier, atomic::AtomicUsize};

use super::*;

#[test]
fn an_empty_cell_refuses_every_claim() {
    let cell = ClaimCell::<u32>::new();
    assert_eq!(cell.lend(|value| *value), Err(Claim::Empty));
    assert_eq!(cell.take(), Err(Claim::Empty));
    assert_eq!(cell.seal(|value| *value), Err(Claim::Empty));
}

#[test]
fn a_cell_installs_once() {
    let cell = ClaimCell::new();
    assert_eq!(cell.install(1_u32), Ok(()));
    assert_eq!(cell.install(2), Err(2), "the second value comes back");
    assert_eq!(cell.lend(|value| *value), Ok(1));
}

#[test]
fn a_lend_changes_the_value_and_frees_the_cell() {
    let cell = ClaimCell::new();
    cell.install([1_u32, 2]).expect("an empty cell installs");
    assert_eq!(cell.lend(|value| value[1] = 5), Ok(()));
    assert_eq!(cell.lend(|value| *value), Ok([1, 5]));
}

#[test]
fn a_lent_cell_refuses_every_claim_until_the_lend_ends() {
    let cell = ClaimCell::new();
    cell.install(7_u32).expect("an empty cell installs");
    let inner = cell.lend(|_| {
        (
            cell.lend(|value| *value),
            cell.take(),
            cell.seal(|value| *value),
        )
    });
    assert_eq!(
        inner,
        Ok((Err(Claim::Lent), Err(Claim::Lent), Err(Claim::Lent)))
    );
    assert_eq!(cell.lend(|value| *value), Ok(7), "free again");
}

#[test]
fn a_taken_value_never_comes_back() {
    let cell = ClaimCell::new();
    cell.install(9_u32).expect("an empty cell installs");
    assert_eq!(cell.take(), Ok(9));
    assert_eq!(cell.lend(|value| *value), Err(Claim::Kept));
    assert_eq!(cell.take(), Err(Claim::Kept));
    assert_eq!(cell.seal(|value| *value), Err(Claim::Kept));
    assert_eq!(cell.install(3), Err(3), "a kept cell installs nothing");
}

#[test]
fn a_sealed_value_is_reached_once() {
    let cell = ClaimCell::new();
    cell.install(4_u32).expect("an empty cell installs");
    assert_eq!(
        cell.seal(|value| {
            *value += 1;
            *value
        }),
        Ok(5)
    );
    assert_eq!(cell.lend(|value| *value), Err(Claim::Kept));
    assert_eq!(cell.take(), Err(Claim::Kept));
    assert_eq!(cell.seal(|value| *value), Err(Claim::Kept));
}

#[test]
fn the_cell_drops_the_value_it_still_holds_and_no_other() {
    let free = Rc::new(());
    let sealed = Rc::new(());
    let taken = Rc::new(());
    {
        let cell = ClaimCell::new();
        cell.install(Rc::clone(&free)).expect("installs");
    }
    {
        let cell = ClaimCell::new();
        cell.install(Rc::clone(&sealed)).expect("installs");
        cell.seal(|_| ()).expect("a free cell seals");
    }
    let kept = {
        let cell = ClaimCell::new();
        cell.install(Rc::clone(&taken)).expect("installs");
        cell.take().expect("a free cell gives its value")
    };
    assert_eq!(Rc::strong_count(&free), 1);
    assert_eq!(Rc::strong_count(&sealed), 1);
    assert_eq!(Rc::strong_count(&taken), 2, "the taken value lives on");
    drop(kept);
    assert_eq!(Rc::strong_count(&taken), 1);
}

#[test]
fn a_held_value_lends_again_to_its_key_alone() {
    let cell = ClaimCell::new();
    cell.install(1_u32).expect("an empty cell installs");
    assert_eq!(
        cell.hold(0, |value| {
            *value += 1;
            *value
        }),
        Ok(Held {
            value: 2,
            first: true
        })
    );
    assert_eq!(
        cell.hold(0, |value| *value),
        Ok(Held {
            value: 2,
            first: false
        }),
        "the key reaches it again"
    );
    assert_eq!(
        cell.hold(1, |value| *value),
        Err(Claim::Kept),
        "no other key"
    );
    assert_eq!(cell.lend(|value| *value), Err(Claim::Kept));
    assert_eq!(cell.take(), Err(Claim::Kept));
    assert_eq!(cell.seal(|value| *value), Err(Claim::Kept));
}

#[test]
fn a_held_value_is_lent_to_one_call_of_its_key_at_a_time() {
    let cell = ClaimCell::new();
    cell.install(5_u32).expect("an empty cell installs");
    let inner = cell.hold(1, |_| {
        (cell.hold(1, |value| *value), cell.hold(0, |value| *value))
    });
    assert_eq!(
        inner,
        Ok(Held {
            value: (Err(Claim::Lent), Err(Claim::Kept)),
            first: true
        })
    );
    assert_eq!(
        cell.hold(1, |value| *value).map(|held| held.value),
        Ok(5),
        "held again once the call ended"
    );
}

#[test]
fn a_hold_is_refused_while_a_lend_runs_and_before_the_install() {
    let cell = ClaimCell::new();
    assert_eq!(cell.hold(0, |value: &mut u32| *value), Err(Claim::Empty));
    cell.install(3).expect("an empty cell installs");
    assert_eq!(
        cell.lend(|_| cell.hold(0, |value| *value)),
        Ok(Err(Claim::Lent))
    );
    assert_eq!(
        cell.hold(0, |value| *value).map(|held| held.first),
        Ok(true)
    );
}

#[test]
fn the_cell_drops_a_held_value() {
    let value = Rc::new(());
    {
        let cell = ClaimCell::new();
        cell.install(Rc::clone(&value)).expect("installs");
        cell.hold(0, |_| ()).expect("a free cell holds");
    }
    assert_eq!(Rc::strong_count(&value), 1);
}

#[test]
fn contending_lends_and_keyed_holds_publish_complete_changes() {
    for keyed in [false, true] {
        let cell = ClaimCell::new();
        cell.install([0_usize; 2]).expect("an empty cell installs");
        let completed = AtomicUsize::new(0);
        let start = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    start.wait();
                    for _ in 0..1_000 {
                        let change = |value: &mut [usize; 2]| {
                            assert_eq!(value[0], value[1], "a complete previous change");
                            value[0] += 1;
                            std::thread::yield_now();
                            value[1] += 1;
                        };
                        let result = if keyed {
                            cell.hold(0, change).map(|held| held.value)
                        } else {
                            cell.lend(change)
                        };
                        match result {
                            Ok(()) => {
                                completed.fetch_add(1, Ordering::SeqCst);
                            }
                            // A keyed contender may lose the second CAS
                            // after observing the idle held state.
                            Err(Claim::Lent | Claim::Kept) if keyed => {}
                            Err(Claim::Lent) => {}
                            Err(error) => panic!("an installed cell stays accessible: {error:?}"),
                        }
                    }
                });
            }
        });
        let completed = completed.load(Ordering::SeqCst);
        assert!(completed > 0, "at least one caller changed the value");
        let value = if keyed {
            assert_eq!(cell.take(), Err(Claim::Kept));
            cell.hold(0, |value| *value)
                .expect("the same key still holds the cell")
                .value
        } else {
            cell.take().expect("the driver takes the free cell")
        };
        assert_eq!(
            value, [completed; 2],
            "every completed change was published"
        );
    }
}
