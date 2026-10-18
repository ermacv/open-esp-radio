use super::*;

#[test]
fn one_role_lends_the_exact_pair_until_terminal_restore() {
    let mut owner = DatapathPairedPhysicalTx::new(0x11_u8, 0x2222_u16);
    let (ordinary, aggregate) = owner.try_lend(DatapathPairRole::Second).unwrap();

    assert_eq!(ordinary, 0x11);
    assert_eq!(aggregate, 0x2222);
    assert_eq!(owner.lent_to(), Some(DatapathPairRole::Second));
    assert_eq!(
        owner.try_lend(DatapathPairRole::First),
        Err(DatapathPairedPhysicalTxError::AlreadyLent(
            DatapathPairRole::Second
        ))
    );

    owner
        .restore(DatapathPairRole::Second, ordinary, aggregate)
        .unwrap();
    let resources = match owner.try_into_resources() {
        Ok(resources) => resources,
        Err(_) => panic!("both roles returned the physical pair"),
    };
    assert_eq!(resources, (0x11, 0x2222));
}

#[test]
fn wrong_role_cannot_return_another_roles_capabilities() {
    let mut owner = DatapathPairedPhysicalTx::new(7_u8, 9_u16);
    let (ordinary, aggregate) = owner.try_lend(DatapathPairRole::First).unwrap();

    let (error, ordinary, aggregate) = owner
        .restore(DatapathPairRole::Second, ordinary, aggregate)
        .unwrap_err();
    assert_eq!(
        error,
        DatapathPairedPhysicalTxError::WrongRole {
            expected: DatapathPairRole::First,
            actual: DatapathPairRole::Second,
        }
    );
    owner
        .restore(DatapathPairRole::First, ordinary, aggregate)
        .unwrap();
}

#[test]
fn failed_role_transition_restores_the_exact_state() {
    let mut role = DatapathPairedRoleOwner::<u16, u8>::parked(7);

    assert_eq!(
        role.try_activate(|parked| Err((11_u32, parked))),
        Err(DatapathPairedRoleTransitionError::Conversion(11))
    );
    assert!(role.is_parked());
    role.try_activate(|parked| Ok::<_, (u32, u8)>(u16::from(parked) + 1))
        .unwrap();
    assert_eq!(role.active(), Some(&8));
    assert_eq!(
        role.try_activate(|parked| Ok::<_, (u32, u8)>(u16::from(parked))),
        Err(DatapathPairedRoleTransitionError::AlreadyActive)
    );
    assert_eq!(
        role.try_park(|active| Err((13_u32, active))),
        Err(DatapathPairedRoleTransitionError::Conversion(13))
    );
    assert_eq!(role.active(), Some(&8));
    role.try_park(|active| Ok::<_, (u32, u16)>(active as u8))
        .unwrap();
    assert_eq!(
        role.try_park(|active| Ok::<_, (u32, u16)>(active as u8)),
        Err(DatapathPairedRoleTransitionError::AlreadyParked)
    );
    assert!(matches!(role.try_into_parked(), Ok(8)));
}
