use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const HOLD_TICKS: usize = 16;
pub const HOLD_INTERVALS: usize = 8;

/// Diagnostic branch selection; natural policy does not force debug or retained state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum HoldPolicy {
    #[default]
    Forced,
    Natural,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartTrackingHold {
    pub epoch: u32,
    pub ticks: u8,
    pub policy: HoldPolicy,
}
impl StartTrackingHold {
    pub const fn valid(self) -> bool {
        self.epoch != 0 && self.ticks > 0 && self.ticks as usize <= HOLD_TICKS
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HoldStarted {
    pub id: u32,
    pub epoch: u32,
    pub ticks: u8,
    pub policy: HoldPolicy,
    pub channel: u8,
    pub radio: super::CaptureRadio,
    pub force: [u8; 2],
    pub period_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[repr(u8)]
pub enum HoldOperation {
    Tracking,
    Rfpll,
    Common,
    Transmit,
    WifiBasebandOff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HoldInterval {
    pub operation: HoldOperation,
    pub started_micros: u64,
    pub finished_micros: u64,
    pub failed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HoldSample {
    pub tick: u8,
    /// RFPLL/common/transmit committed bits, from the production outcome.
    pub committed: u8,
    pub invalid: bool,
    pub intervals: heapless::Vec<HoldInterval, HOLD_INTERVALS>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetTrackingHold {
    pub id: u32,
    pub first: u8,
}

/// Aggregate engine polls for this hold, including partial failed ticks.
/// Updated after T7 returns; no telemetry is sent by an engine poll.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HoldPolls {
    pub polls: u16,
    pub pending: u16,
    pub total_micros: u32,
    pub maximum_micros: u32,
    pub resumes: u16,
    pub lateness_micros: u32,
    pub maximum_lateness_micros: u32,
    pub ready_edges: u16,
    pub maximum_ready_micros: u32,
    pub invalid: bool,
}
impl HoldPolls {
    pub fn merge(&mut self, other: Self) {
        self.invalid |= other.invalid;
        for (total, value) in [
            (&mut self.polls, other.polls),
            (&mut self.pending, other.pending),
            (&mut self.resumes, other.resumes),
            (&mut self.ready_edges, other.ready_edges),
        ] {
            match total.checked_add(value) {
                Some(sum) => *total = sum,
                None => self.invalid = true,
            }
        }
        for (total, value) in [
            (&mut self.total_micros, other.total_micros),
            (&mut self.lateness_micros, other.lateness_micros),
        ] {
            match total.checked_add(value) {
                Some(sum) => *total = sum,
                None => self.invalid = true,
            }
        }
        self.maximum_micros = self.maximum_micros.max(other.maximum_micros);
        self.maximum_lateness_micros = self
            .maximum_lateness_micros
            .max(other.maximum_lateness_micros);
        self.maximum_ready_micros = self.maximum_ready_micros.max(other.maximum_ready_micros);
    }
}

/// One sample per page, bounded independently of diagnostic storage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TrackingHold {
    pub id: u32,
    pub first: u8,
    pub available: u8,
    pub terminal: bool,
    pub invalid: bool,
    pub sample: Option<HoldSample>,
    pub engine: HoldPolls,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopTrackingHold {
    pub id: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HoldStopped {
    pub id: u32,
    pub samples: u8,
    pub complete: bool,
    pub debug_restored: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hold_configuration_is_bounded_and_pages_fit_the_wire() {
        for ticks in [0, 17, 255] {
            assert!(
                !StartTrackingHold {
                    epoch: 1,
                    ticks,
                    policy: HoldPolicy::Forced
                }
                .valid()
            );
        }
        assert!(
            !StartTrackingHold {
                epoch: 0,
                ticks: 1,
                policy: HoldPolicy::Natural
            }
            .valid()
        );
        assert!(
            StartTrackingHold {
                epoch: 1,
                ticks: 16,
                policy: HoldPolicy::Natural,
            }
            .valid()
        );
        let interval = HoldInterval {
            operation: HoldOperation::Tracking,
            started_micros: u64::MAX,
            finished_micros: u64::MAX,
            failed: false,
        };
        let page = TrackingHold {
            id: u32::MAX,
            first: 15,
            available: 16,
            terminal: true,
            invalid: false,
            engine: HoldPolls {
                polls: u16::MAX,
                pending: u16::MAX,
                total_micros: u32::MAX,
                maximum_micros: u32::MAX,
                resumes: u16::MAX,
                lateness_micros: u32::MAX,
                maximum_lateness_micros: u32::MAX,
                ready_edges: u16::MAX,
                maximum_ready_micros: u32::MAX,
                invalid: true,
            },
            sample: Some(HoldSample {
                tick: 16,
                committed: 7,
                invalid: false,
                intervals: heapless::Vec::from_slice(&[interval; HOLD_INTERVALS]).unwrap(),
            }),
        };
        let mut encoder = oer_hil_protocol::FrameEncoder::new();
        let envelope = oer_hil_protocol::Envelope::new(1, 2, 0, 3, page.clone());
        let bytes = encoder.encode(&envelope).unwrap();
        let mut decoder = oer_hil_protocol::FrameDecoder::new();
        let mut actual = None;
        decoder.feed(oer_hil_protocol::WireKind::Event, bytes, |frame| {
            actual = Some(
                frame
                    .unwrap()
                    .decode::<TrackingHold>()
                    .unwrap()
                    .unwrap()
                    .body,
            );
        });
        assert_eq!(actual, Some(page));
    }
}

#[cfg(test)]
mod poll_tests {
    use super::HoldPolls;
    #[test]
    fn merging_keeps_totals_maxima_and_partial_invalid_evidence() {
        let mut report = HoldPolls {
            polls: 4,
            pending: 3,
            total_micros: 8,
            maximum_micros: 3,
            resumes: 3,
            lateness_micros: 7,
            maximum_lateness_micros: 4,
            ready_edges: 1,
            maximum_ready_micros: 9,
            invalid: false,
        };
        report.merge(HoldPolls {
            polls: 3,
            pending: 2,
            total_micros: 5,
            maximum_micros: 4,
            resumes: 2,
            lateness_micros: 3,
            maximum_lateness_micros: 2,
            ready_edges: 1,
            maximum_ready_micros: 8,
            invalid: true,
        });
        assert_eq!(
            (
                report.polls,
                report.pending,
                report.total_micros,
                report.maximum_micros
            ),
            (7, 5, 13, 4)
        );
        assert_eq!(
            (
                report.resumes,
                report.lateness_micros,
                report.maximum_lateness_micros
            ),
            (5, 10, 4)
        );
        assert_eq!((report.ready_edges, report.maximum_ready_micros), (2, 9));
        assert!(report.invalid);
        report.polls = u16::MAX;
        report.merge(HoldPolls {
            polls: 1,
            ..HoldPolls::default()
        });
        assert!(report.invalid);
        assert_eq!(report.polls, u16::MAX);
    }
}
