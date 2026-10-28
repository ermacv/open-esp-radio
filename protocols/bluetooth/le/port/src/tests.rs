use super::*;
use crate::{LePhys, NoCapabilities};

#[test]
fn receive_needs_supported_dtm_phy_but_no_transmit_authority() {
    enum OneM {}
    impl LeCapabilities for OneM {
        const CAPS: LeRadioCapabilities = LeRadioCapabilities {
            direct_test_mode: true,
            test_phys: LePhys::LE_1M,
            ..LeRadioCapabilities::NONE
        };
    }
    let config = Receive {
        channel: TestChannel::new(19).expect("valid test channel"),
        phy: TestPhy::Le1M,
    };
    assert!(Supported::<OneM, _>::new(config).is_ok());
    assert!(Supported::<NoCapabilities, _>::new(config).is_err());
    assert!(
        Supported::<OneM, _>::new(Receive {
            phy: TestPhy::Le2M,
            ..config
        })
        .is_err()
    );
}

#[test]
#[cfg(feature = "test-support")]
fn verifier_rejects_missing_foreign_and_reconstructed_terminal_owners() {
    use crate::conformance::*;
    let original = Identity {
        slot: 0,
        epoch: 8,
        context: 13,
    };
    let admitted = Admitted {
        id: LeOperationId(91),
        epoch: 8,
    };
    let mut trace = Trace {
        original: Some(original),
        submitted: [Some(admitted), None, None, None],
        events: [
            Some(Observation {
                admitted,
                returned: Some(original),
                context: 13,
                result: EventResult::Aborted,
                cause: Cause::Withdrawn,
                count: None,
            }),
            None,
            None,
            None,
        ],
        ..Trace::default()
    };
    assert_eq!(verify(Case::Aborted, &trace), Ok(()));
    trace.events[0]
        .as_mut()
        .expect("first observed event")
        .returned = None;
    assert_eq!(verify(Case::Aborted, &trace), Err(Violation::Owners));
    trace.events[0]
        .as_mut()
        .expect("first observed event")
        .returned = Some(Identity {
        epoch: 9,
        ..original
    });
    assert_eq!(verify(Case::Aborted, &trace), Err(Violation::Owners));
    trace.events[0]
        .as_mut()
        .expect("first observed event")
        .returned = Some(original);
    trace.events[0]
        .as_mut()
        .expect("first observed event")
        .admitted
        .id = LeOperationId(92);
    assert_eq!(verify(Case::Aborted, &trace), Err(Violation::Terminal));
}
