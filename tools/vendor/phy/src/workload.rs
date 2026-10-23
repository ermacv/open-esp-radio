//! Manual PHY investigations.
pub(crate) mod calibration;

pub(crate) mod hold;

pub(crate) mod cache;

/// Every existing R1 investigation requests and verifies a fresh full registration.
pub(crate) fn full(
    context: &oer_hil_scenario::workload::context::Context<'_>,
    capture: &oer_hil_link::SerialCapture,
    config: oer_hil_protocol::phy::PrepareCapture,
) -> crate::Result<oer_hil_protocol::phy::CaptureReady> {
    use oer_hil_protocol::phy::*;
    let (_, started) = crate::cache::start(
        &crate::cache::Link::new(capture),
        &oer_hil_lab::calibration_cache::Store::host()?,
        crate::cache::StartRequest {
            boot_id: capture.latest_boot_id().ok_or("R1 has no boot identity")?,
            session_id: 0,
            chip: context.lab.chip(),
            board: oer_lab_board::DeviceId::parse(&context.lab.dut.mac)?,
            capture: config,
            source: CalibrationSource::Full,
        },
    )?;
    match started.status.state {
        CalibrationStartState::Ready {
            decision: CalibrationCacheDecision::FullRequested,
            capture: ready,
            ..
        } if ready.full_calibration_performed && ready.path == CalibrationPath::FullForCache => {
            Ok(ready)
        }
        _ => Err("explicit full R1 request changed its native result contract".into()),
    }
}
pub(crate) mod health;
