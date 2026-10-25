extern crate std;

use std::rc::Rc;

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
