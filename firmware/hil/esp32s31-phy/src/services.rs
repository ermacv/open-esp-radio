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
    use super::*;
    use oer_hil_protocol::base::ImageKeySet;

    fn same(new: ImageKeySet<'_>, features: &[&str]) {
        let old =
            oer_hil_protocol::feature_keys::image_keys(&|feature| features.contains(&feature));
        assert_eq!(new.hello(0), old.as_set().hello(0), "{features:?}");
        assert_eq!(new.page(0), old.as_set().page(0), "{features:?}");
        assert_eq!(new.page(48), old.as_set().page(48), "{features:?}");
    }

    #[test]
    fn each_image_reports_the_keys_its_features_reported() {
        same(PHY_KEYS, &["diagnostic-phy"]);
        same(
            RADIO_HEALTH_KEYS,
            &["diagnostic-phy", "diagnostic-radio-health"],
        );
    }
}
