use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const HOLD_TICKS: usize = 16;
pub const HOLD_INTERVALS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartTrackingHold {
    pub epoch: u32,
    pub ticks: u8,
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

/// One sample per page, bounded independently of diagnostic storage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TrackingHold {
    pub id: u32,
    pub first: u8,
    pub available: u8,
    pub terminal: bool,
    pub invalid: bool,
    pub sample: Option<HoldSample>,
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
            assert!(!StartTrackingHold { epoch: 1, ticks }.valid());
        }
        assert!(!StartTrackingHold { epoch: 0, ticks: 1 }.valid());
        assert!(
            StartTrackingHold {
                epoch: 1,
                ticks: 16
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
            sample: Some(HoldSample {
                tick: 16,
                committed: 7,
                invalid: false,
                intervals: heapless::Vec::from_slice(&[interval; HOLD_INTERVALS]).unwrap(),
            }),
        };
        let mut encoder = crate::FrameEncoder::new();
        let envelope = crate::Envelope::new(1, 2, 0, 3, page.clone());
        let bytes = encoder.encode(&envelope).unwrap();
        let mut decoder = crate::FrameDecoder::new();
        let mut actual = None;
        decoder.feed(crate::WireKind::Event, bytes, |frame| {
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
