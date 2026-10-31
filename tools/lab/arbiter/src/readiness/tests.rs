use super::*;

#[test]
fn the_whole_stand_holds_every_resource_and_a_board_claim_its_board() {
    let stand = vec!["board:x".to_owned(), ROUTER.to_owned()];
    assert_eq!(held(&[Claim::stand()], &stand), vec!["board:x", ROUTER]);
    assert_eq!(held(&[Claim::board("x")], &stand), vec!["board:x"]);
    assert!(held(&[Claim::board("y")], &stand).is_empty());
}

#[test]
fn the_air_a_range_and_the_whole_stand_use_the_air() {
    assert!(uses_air(&[Claim::shared(AIR)]));
    assert!(uses_air(&[Claim::stand()]));
    assert!(!uses_air(&[Claim::board("x")]));
    assert!(uses_air(&[Claim::shared(
        "spectrum:2400000-2483500:tolerant:normal"
    )]));
}

#[test]
fn only_a_resource_in_recovery_or_unconfirmed_may_radiate() {
    assert!(Readiness::Ready { generation: 1 }.emission_known());
    assert!(
        Readiness::InUse {
            lease: 1,
            generation: 1
        }
        .emission_known()
    );
    assert!(
        !Readiness::Recovering {
            lease: None,
            generation: 1
        }
        .emission_known()
    );
    let unavailable = |emission| Readiness::Unavailable {
        reason: String::new(),
        emission,
        generation: 1,
    };
    assert!(unavailable(Emission::Quiet).emission_known());
    assert!(!unavailable(Emission::Unknown).emission_known());
}
