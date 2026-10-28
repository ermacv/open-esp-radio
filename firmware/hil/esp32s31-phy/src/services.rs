//! The HIL services the PHY images mount, and the key set each reports: the
//! union of the keys of those services and of the base module.

#[cfg(any(test, feature = "diagnostic-radio-health"))]
use oer_hil_protocol::health::*;
use oer_hil_protocol::phy::*;

oer_hil_agent::requests! {
    /// The calibration cache, tracking hold and capture requests, and the
    /// host-cache property.
    pub(crate) enum WireRequest(sessions = false) {
        CacheIdentity(GetCalibrationCacheIdentity), Start(StartCalibration),
        CacheWrite(WriteCalibrationCachePage), CacheComplete(CompleteCalibrationCache),
        CacheStatus(GetCalibrationStartStatus), CacheRead(ReadCalibrationSnapshot),
        Restart(RestartCaptureRadio), Stop(StopCapture),
        HoldStart(StartTrackingHold), HoldGet(GetTrackingHold), HoldStop(StopTrackingHold),
        Calibration(ReadCalibrationProjection), Registers(ReadRegisterImage), Analog(ReadAnalogImage),
    }
    publishes [HostCalibrationCache]
}

#[cfg(any(test, feature = "diagnostic-radio-health"))]
oer_hil_agent::requests! {
    /// The radio-health window requests.
    pub enum HealthRequest(sessions = false) {
        Begin(BeginHealth), Window(RunHealthWindow), Finish(FinishHealth),
        Counters(ReadHealthCounters), Signals(ReadHealthSignals), Tracking(ReadHealthTracking),
        Measurements(ReadHealthMeasurements), Release(ReleaseHealth),
    }
}

#[cfg(any(test, not(feature = "diagnostic-radio-health")))]
oer_hil_agent::image_keys! {
    /// The calibration and capture image.
    pub(crate) PHY_KEYS = [WireRequest]
}

#[cfg(any(test, feature = "diagnostic-radio-health"))]
oer_hil_agent::image_keys! {
    /// The calibration and capture image with radio-health windows.
    pub(crate) RADIO_HEALTH_KEYS = [WireRequest, HealthRequest]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::base::ImageKeySet;
    use oer_hil_protocol::{Key, Message, health, phy};

    fn set(keys: ImageKeySet<'_>) -> BTreeSet<Key> {
        keys.keys().iter().copied().collect()
    }

    #[test]
    fn each_image_reports_its_services_and_the_base_module() {
        // The image never publishes the cache request topic.
        let phy: BTreeSet<Key> = Base::KEYS
            .iter()
            .chain(phy::KEYS)
            .copied()
            .filter(|key| *key != phy::CalibrationCacheRequested::KEY)
            .collect();
        assert_eq!(set(PHY_KEYS), phy);
        let health: BTreeSet<Key> = phy.iter().chain(health::KEYS).copied().collect();
        assert_eq!(set(RADIO_HEALTH_KEYS), health);
    }
}
