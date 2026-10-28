use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Bounded independently of the full image; does not enlarge framing buffers.
pub const CAPTURE_PAGE: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CaptureRadio {
    Wifi,
    Ieee802154 { power_dbm: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PrepareCapture {
    pub channel: u8,
    pub radio: CaptureRadio,
}

impl PrepareCapture {
    pub const fn valid(self) -> bool {
        match self.radio {
            CaptureRadio::Wifi => self.channel >= 1 && self.channel <= 14,
            CaptureRadio::Ieee802154 { power_dbm } => {
                self.channel >= 11
                    && self.channel <= 26
                    && power_dbm >= -24
                    && power_dbm <= 21
                    && power_dbm % 3 == 0
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CaptureLifecycle {
    Cold,
    Restart,
    Ieee802154,
    Ieee802154Restart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationPath {
    FullForCache,
    FullUncached,
    FullAfterRejectedCache,
    PartialFromCache,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaptureReady {
    pub epoch: u32,
    pub lifecycle: CaptureLifecycle,
    pub full_calibration_performed: bool,
    pub path: CalibrationPath,
    pub projection_schema: u16,
    pub calibration_words: u16,
    pub registers: u16,
    pub analog_bytes: u16,
    pub channel: u8,
    pub radio: CaptureRadio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RestartCaptureRadio {
    pub epoch: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaptureRestarted {
    pub ready: CaptureReady,
    pub rf_closed: bool,
    pub rf_woken: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopCapture {
    pub epoch: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaptureStopped {
    pub epoch: u32,
    pub rf_closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaptureWindow {
    pub epoch: u32,
    pub first: u16,
    pub count: u8,
}

impl CaptureWindow {
    /// Validate before touching hardware or slicing an immutable snapshot.
    pub fn range(self, epoch: u32, total: usize) -> Option<core::ops::Range<usize>> {
        let first = usize::from(self.first);
        let count = usize::from(self.count);
        let end = first.checked_add(count)?;
        (epoch != 0 && self.epoch == epoch && count > 0 && count <= CAPTURE_PAGE && end <= total)
            .then_some(first..end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadCalibrationProjection(pub CaptureWindow);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadRegisterImage(pub CaptureWindow);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadAnalogImage(pub CaptureWindow);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationProjection {
    pub epoch: u32,
    pub first: u16,
    pub total: u16,
    pub values: heapless::Vec<u16, CAPTURE_PAGE>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RegisterImage {
    pub epoch: u32,
    pub first: u16,
    pub total: u16,
    pub values: heapless::Vec<u32, CAPTURE_PAGE>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AnalogImage {
    pub epoch: u32,
    pub first: u16,
    pub total: u16,
    pub values: heapless::Vec<u8, CAPTURE_PAGE>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_validates_each_radio_before_registration() {
        for (radio, first, last) in [
            (CaptureRadio::Wifi, 1, 14),
            (CaptureRadio::Ieee802154 { power_dbm: 21 }, 11, 26),
        ] {
            for channel in 0..=u8::MAX {
                assert_eq!(
                    PrepareCapture { channel, radio }.valid(),
                    (first..=last).contains(&channel)
                );
            }
        }
        for power_dbm in i8::MIN..=i8::MAX {
            assert_eq!(
                PrepareCapture {
                    channel: 15,
                    radio: CaptureRadio::Ieee802154 { power_dbm }
                }
                .valid(),
                (-24..=21).contains(&power_dbm) && power_dbm % 3 == 0
            );
        }
    }

    #[test]
    fn a_window_must_belong_to_the_active_epoch_and_stay_inside_the_image() {
        let page = CaptureWindow {
            epoch: 3,
            first: 2,
            count: 4,
        };
        assert_eq!(page.range(3, 6), Some(2..6));
        assert_eq!(page.range(4, 6), None);
        assert_eq!(page.range(0, 6), None);
        assert_eq!(page.range(3, 5), None);
        assert_eq!(CaptureWindow { count: 0, ..page }.range(3, 6), None);
        assert_eq!(CaptureWindow { count: 17, ..page }.range(3, 100), None);
        assert_eq!(
            CaptureWindow {
                first: u16::MAX,
                ..page
            }
            .range(3, 6),
            None
        );
    }

    #[test]
    fn capture_pages_round_trip_through_the_existing_frame() {
        let mut values = heapless::Vec::new();
        values.extend_from_slice(&[u32::MAX; CAPTURE_PAGE]).unwrap();
        let reply = RegisterImage {
            epoch: u32::MAX,
            first: u16::MAX,
            total: u16::MAX,
            values,
        };
        let envelope = oer_hil_protocol::Envelope::new(1, 2, 0, 3, reply.clone());
        let mut encoder = oer_hil_protocol::FrameEncoder::new();
        let bytes = encoder.encode(&envelope).unwrap();
        let mut decoder = oer_hil_protocol::FrameDecoder::new();
        let mut actual = None;
        decoder.feed(oer_hil_protocol::WireKind::Event, bytes, |frame| {
            actual = Some(frame.unwrap().decode::<RegisterImage>().unwrap().unwrap());
        });
        assert_eq!(actual.unwrap().body, reply);
    }
}
