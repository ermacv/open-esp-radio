use super::*;
use crate::calibration::estimator::{
    PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateRequest, PhyDcIqReadinessBinding,
};

fn request() -> PhyDcIqEstimateRequest {
    PhyDcIqEstimateRequest {
        iteration: 0,
        chain: 1,
        control: 0x800,
        mode: 1,
    }
}
fn binding(samples: u16) -> PhyDcIqReadinessBinding {
    PhyDcIqReadinessBinding::new(PhyDcIqAction::AwaitReadinessEdge {
        request: request(),
        readiness_samples: samples,
        readiness_activity_edges: 0,
    })
    .expect("readiness action")
}

#[test]
fn observation_limit_cleans_up_without_sampling_ready_or_accumulators() {
    let mut io = Io::default();
    let result = io
        .dc_iq::<TestDelay>(binding(crate::HARDWARE_EDGE_LIMIT))
        .expect("tail settles");
    assert_eq!(io.dc_iq_events, ["measurement-off", "settle", "start-off"]);
    assert_eq!(
        result,
        PhyDcIqCompletion::CaptureFinished {
            request: request(),
            accumulators: None
        }
    );
}
