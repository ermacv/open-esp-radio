use super::*;
use crate::{
    analog::i2c::{PhyI2cError, fields, registers},
    calibration::cold::{PhyColdI2cRequest, PhyColdI2cTransaction},
};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;

#[test]
fn final_read_or_write_edge_succeeds_at_the_settle_boundary() {
    let address = registers::RFPLL_CAPACITOR_LOW;
    for request in [
        PhyColdI2cRequest::read_byte(address),
        PhyColdI2cRequest::write_byte(address, 13),
        PhyColdI2cRequest::write_field(fields::TXRF_RC_CALIBRATION_ENABLE, 1),
    ] {
        let clock: VirtualClock = VirtualClock::default();
        let mut state = PhyColdI2cTransaction::new(request);
        let mut edges = 0;
        let completed = complete(
            &clock,
            &mut state,
            PhyColdI2cTransaction::action,
            |state, next| {
                match next {
                    PhyColdI2cAction::StartRead { .. } => state.read_started()?,
                    PhyColdI2cAction::StartWrite { .. } => state.write_started()?,
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                        state.observe_read_result(Ok(0))?;
                        edges += 1;
                    }
                    PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        state.observe_write_result(Ok(()))?;
                        edges += 1;
                    }
                    PhyColdI2cAction::Complete(_) => panic!("no extra terminal dispatch"),
                }
                if matches!(state.action(), PhyColdI2cAction::Complete(_)) {
                    clock.advance(Duration::from_micros(20)).unwrap();
                }
                Ok::<_, crate::calibration::cold::PhyColdI2cError>(())
            },
        );
        assert_eq!(completed, Ok(true));
        assert!(matches!(state.action(), PhyColdI2cAction::Complete(_)));
        assert_eq!(
            edges,
            if matches!(request, PhyColdI2cRequest::WriteField { .. }) {
                2
            } else {
                1
            }
        );
    }
}

#[test]
fn a_masked_read_at_the_boundary_does_not_claim_the_write_completed() {
    let clock: VirtualClock = VirtualClock::default();
    let mut state = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_field(
        fields::TXRF_RC_CALIBRATION_ENABLE,
        1,
    ));
    state.read_started().unwrap();
    let completed = complete(
        &clock,
        &mut state,
        PhyColdI2cTransaction::action,
        |state, _| {
            state.observe_read_result(Ok(0))?;
            clock.advance(Duration::from_micros(20)).unwrap();
            Ok::<_, crate::calibration::cold::PhyColdI2cError>(())
        },
    );
    assert_eq!(completed, Ok(false));
    assert!(matches!(
        state.action(),
        PhyColdI2cAction::StartWrite { .. }
    ));
}

#[test]
fn busy_hardware_still_times_out_and_does_not_consume_a_result() {
    let clock: VirtualClock = VirtualClock::default();
    let mut state =
        PhyColdI2cTransaction::new(PhyColdI2cRequest::read_byte(registers::RFPLL_CAPACITOR_LOW));
    state.read_started().unwrap();
    let completed = complete(
        &clock,
        &mut state,
        PhyColdI2cTransaction::action,
        |state, _| {
            state.observe_read_result(Err(PhyI2cError::Busy))?;
            clock.advance(Duration::from_micros(20)).unwrap();
            Ok::<_, crate::calibration::cold::PhyColdI2cError>(())
        },
    );
    assert_eq!(completed, Ok(false));
    assert!(matches!(
        state.action(),
        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
    ));
}

#[test]
fn terminal_states_and_errors_do_not_issue_another_hardware_edge() {
    let clock: VirtualClock = VirtualClock::default();
    let mut state =
        PhyColdI2cTransaction::new(PhyColdI2cRequest::read_byte(registers::RFPLL_CAPACITOR_LOW));
    assert_eq!(
        complete(&clock, &mut state, PhyColdI2cTransaction::action, |_, _| {
            Err("edge")
        }),
        Err("edge")
    );
    state.read_started().unwrap();
    state.observe_read_result(Ok(13)).unwrap();
    assert_eq!(
        complete(
            &clock,
            &mut state,
            PhyColdI2cTransaction::action,
            |_, _| -> Result<(), ()> { panic!("terminal owner touches no hardware") }
        ),
        Ok(true)
    );
}
