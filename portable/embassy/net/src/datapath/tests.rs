use super::*;

#[test]
fn pair_success_and_refusal_transfer_exactly_two_owners() {
    assert_eq!(
        finish_pair::<_, u8>(false, [Some(1), Some(2)], [None, None]),
        Err(MaterializationPairError::Refused((1, 2)))
    );
    assert_eq!(
        finish_pair::<u8, _>(true, [None, None], [Some(3), Some(4)]),
        Ok((3, 4))
    );
}

#[test]
fn broken_pair_contract_returns_all_remaining_owners_and_reported_result() {
    // A lying success used to expect a missing destination. A lying refusal
    // used to expect a missing source. Neither case may destroy the survivors.
    for reported_success in [false, true] {
        let sources = [Some(1), None];
        let destinations = [None, Some(2)];
        assert_eq!(
            finish_pair(reported_success, sources, destinations),
            Err(MaterializationPairError::ContractViolation {
                reported_success,
                sources,
                destinations,
            })
        );
    }
    assert!(matches!(
        finish_pair(false, [Some(1), Some(2)], [Some(3), None]),
        Err(MaterializationPairError::ContractViolation { .. })
    ));
    assert!(matches!(
        finish_pair(true, [Some(1), None], [Some(2), Some(3)]),
        Err(MaterializationPairError::ContractViolation { .. })
    ));
}

#[test]
fn a_pair_contract_fault_keeps_every_surviving_owner_until_the_caller_drops_it() {
    extern crate std;
    use std::{cell::Cell, rc::Rc};
    struct Owner(Rc<Cell<usize>>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let owner = || Owner(drops.clone());
    let result = finish_pair(false, [Some(owner()), None], [Some(owner()), Some(owner())]);
    assert_eq!(drops.get(), 0);
    assert!(matches!(
        result,
        Err(MaterializationPairError::ContractViolation { .. })
    ));
    drop(result);
    assert_eq!(drops.get(), 3);
}
