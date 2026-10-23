//! Diagnostic-only copies of existing estimator completions. The RF owner is unchanged.
use crate::calibration::estimator::PhyDcIqCompletion;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Context {
    pub gain: u8,
    pub measurement: u8,
    pub shared_radio: bool,
    pub radio_stage: bool,
    pub fine_code: Option<u8>,
    pub high_level: bool,
    pub frequency_code: Option<u16>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Correction {
    pub context: Context,
    pub configuration: [u16; 2],
    pub iterations: u8,
    pub converged: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub context: Context,
    pub iteration: u8,
    pub chain: u8,
    pub control: u16,
    pub mode: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Pending { activity: bool },
    Captured { sums: Option<[i32; 3]> },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DcIq {
    /// Brackets the existing hardware atom, not exact ready/accumulator edges.
    pub started_micros: u64,
    pub finished_micros: u64,
    pub request: Request,
    pub completion: Completion,
}
impl DcIq {
    pub(crate) fn observed(
        started_micros: u64,
        finished_micros: u64,
        completion: PhyDcIqCompletion,
        context: Context,
    ) -> Option<Self> {
        let (request, completion) = match completion {
            PhyDcIqCompletion::CapturePending { request, snapshot } => (
                request,
                Completion::Pending {
                    activity: snapshot.activity,
                },
            ),
            PhyDcIqCompletion::CaptureFinished {
                request,
                accumulators,
            } => (
                request,
                Completion::Captured {
                    sums: accumulators.map(|a| [a.i, a.q, a.power]),
                },
            ),
            _ => return None,
        };
        Some(Self {
            started_micros,
            finished_micros,
            completion,
            request: Request {
                context,
                iteration: request.iteration,
                chain: request.chain,
                control: request.control,
                mode: request.mode,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SarPhase {
    DetectorConfigured,
    ToneConfigured,
    ReferenceControl,
    ToneArmed,
    Triggered,
    Ready,
    TimedOut,
    Captured,
    ToneCleared,
    ToneStopped,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pwdet {
    pub started_micros: u64,
    pub finished_micros: u64,
    pub phase: SarPhase,
    pub measurement: Option<u8>,
    pub sample: Option<u8>,
    pub value: Option<u16>,
}
impl Pwdet {
    pub(crate) fn observed(
        started_micros: u64,
        finished_micros: u64,
        c: crate::tx::power_detector::PhyPwdetCompletion,
    ) -> Option<Self> {
        use crate::tx::power_detector::PhyPwdetCompletion as C;
        let (phase, measurement, sample, value) = match c {
            C::PowerDetectorConfigured => (SarPhase::DetectorConfigured, None, None, None),
            C::ToneConfigured => (SarPhase::ToneConfigured, None, None, None),
            C::ReferenceControlWritten { value } => {
                (SarPhase::ReferenceControl, None, None, Some(value))
            }
            C::ToneArmed {
                measurement_index,
                sample_index,
            } => (
                SarPhase::ToneArmed,
                Some(measurement_index),
                Some(sample_index),
                None,
            ),
            C::SarTriggered {
                measurement_index,
                sample_index,
            } => (
                SarPhase::Triggered,
                Some(measurement_index),
                Some(sample_index),
                None,
            ),
            C::SarReadyObserved {
                measurement_index,
                sample_index,
                not_ready_samples,
            } => (
                SarPhase::Ready,
                Some(measurement_index),
                Some(sample_index),
                Some(not_ready_samples),
            ),
            C::SarReadyDeadlineElapsed {
                measurement_index,
                sample_index,
            } => (
                SarPhase::TimedOut,
                Some(measurement_index),
                Some(sample_index),
                None,
            ),
            C::SarSampled {
                measurement_index,
                sample_index,
                value,
            } => (
                SarPhase::Captured,
                Some(measurement_index),
                Some(sample_index),
                Some(value),
            ),
            C::ToneArmCleared {
                measurement_index,
                sample_index,
            } => (
                SarPhase::ToneCleared,
                Some(measurement_index),
                Some(sample_index),
                None,
            ),
            C::ToneStopped => (SarPhase::ToneStopped, None, None, None),
            _ => return None,
        };
        Some(Self {
            started_micros,
            finished_micros,
            phase,
            measurement,
            sample,
            value,
        })
    }
}
