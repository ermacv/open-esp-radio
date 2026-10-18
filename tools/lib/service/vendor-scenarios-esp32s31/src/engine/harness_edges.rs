//! Compiled harness call-boundary acceptance; no hardware timing claim.
use crate::evidence::{events, stop};
use crate::harness::{Result, case, direct};
use crate::i2c::I2c;
use crate::layout::FREQUENCY_CONTROL;
use blobray_domain::{
    CallBinding, CallBoundary, CallCapture, CallDeclaration, CallResponse, CallValue,
    ComparisonVerdict, DeviceBehavior, DeviceDeclaration, ExecutionEvent, ExecutionEvidence,
    ExecutionStop, ModelStatus, RegionLifetime, RegisterCell, SessionReset,
};

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let owned = ctx.probe("open_phy_trace_owned_software_frequency_control");
    let ordinary = ctx.probe("open_phy_trace_dis_hw_set_freq");
    let delay_entry = ctx.captured(1, "__call_ets_delay_us");
    let delay = ctx.captured(1, "ets_delay_us");
    // This retained cell is a concrete input for the captured frequency-control
    // RMW. Acceptance below concerns call boundaries, arguments and completion.
    let registers = DeviceDeclaration {
        id: "frequency-control".into(),
        applicability: "explicit boundary-test RMW input".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![RegisterCell {
                address: FREQUENCY_CONTROL,
                width: 4,
                value: 0,
            }],
        },
    };
    let mut phase = direct(owned, &[0], vec![], vec![registers], vec![]);
    phase.observe_calls = Some(CallCapture {
        include_tail: true,
        argument_words: 1,
        overrides: vec![],
    });
    phase.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "delay".into(),
        applicability: "explicit ROM delay interception".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: delay,
            boundary: CallBoundary::CapturedCode,
            allow_tail: true,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [None, None],
            outputs: vec![],
            allocation: None,
            delay_micros: Some(CallValue::Argument { word: 0 }),
        }],
    }];
    let mut row = case(
        "ordinary-delay-tail",
        phase.clone(),
        Some(phase),
        SessionReset::Cold,
        false,
    );
    row.relation.as_mut().unwrap().calls = true;
    let replacement = ctx.replacement.clone();
    let records = ctx.submit_with(
        "harness-edges",
        &replacement,
        Some(&replacement),
        None,
        vec![row],
        1024,
        Some(ComparisonVerdict::Match),
    )?;
    for side in [false, true] {
        assert!(matches!(
            stop(&records, 0, side),
            ExecutionStop::Returned { .. }
        ));
        let observed = events(&records, 0, side);
        let transfers = |target: u32, tail: bool| {
            observed.iter().any(|e| matches!(e, ExecutionEvent::CallTransfer { target: t, tail: x, .. } if *t == target && *x == tail))
        };
        assert!(transfers(ordinary, false));
        assert!(transfers(delay_entry, false));
        assert!(transfers(delay, true));
        let delays: Vec<_> = observed
            .iter()
            .filter_map(|e| match e {
                ExecutionEvent::DelayMicros { value } => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(delays, [2]);
        assert!(records.iter().all(|r| match r {
            ExecutionEvidence::Model {
                replacement,
                observation,
                ..
            } if *replacement == side => {
                observation.status == ModelStatus::Complete
            }
            ExecutionEvidence::CallModel {
                replacement,
                observation,
                ..
            } if *replacement == side => {
                observation.status == ModelStatus::Complete
            }
            _ => true,
        }));
    }
    Ok(())
}
