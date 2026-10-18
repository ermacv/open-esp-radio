use super::*;
use oer_hil_scenario_catalog::link::{AccessPointSecurity, ManagementFrameProtection};

fn observation(phy: PhyExpectation) -> OpenWrtRadio {
    OpenWrtRadio {
        enabled: true,
        channel: 13,
        htmode: if phy == PhyExpectation::He20 {
            "HE20"
        } else {
            "HT20"
        }
        .into(),
        geometry: "Interface phy0-ap0\n\tchannel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz\n"
            .into(),
        ht: true,
        he: phy == PhyExpectation::He20,
        beacon_interval_tu: None,
        dtim_period: None,
    }
}

#[test]
fn ht20_width_does_not_prove_he20() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::He20,
        channel: 13,
        management_frame_protection: ManagementFrameProtection::Disabled,
        access_point_security: AccessPointSecurity::Wpa2Personal,
        beacon: None,
    };
    let mut observed = observation(PhyExpectation::Ht20);
    assert!(profile.verify(&observed).is_err());
    observed.htmode = "HE20".into();
    assert!(
        profile.verify(&observed).is_err(),
        "UCI intent alone is insufficient"
    );
    observed.he = true;
    profile.verify(&observed).unwrap();
    observed.channel = 11;
    assert!(profile.verify(&observed).is_err());
}

#[test]
fn a_legacy_link_is_noht_and_proven_without_ht() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::Legacy,
        channel: 13,
        management_frame_protection: ManagementFrameProtection::Disabled,
        access_point_security: AccessPointSecurity::Wpa2Personal,
        beacon: None,
    };
    assert_eq!(profile.htmode(), "NOHT");
    let mut observed = observation(PhyExpectation::Ht20);
    observed.htmode = "NOHT".into();
    // An access point still advertising HT is not a legacy BSS.
    observed.geometry = "channel 13 (2472 MHz), width: 20 MHz (no HT), center1: 2472 MHz".into();
    assert!(profile.verify(&observed).is_err());
    observed.ht = false;
    profile.verify(&observed).unwrap();
    // Nor is an HT20 channel.
    observed.geometry = "channel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz".into();
    assert!(profile.verify(&observed).is_err());
}

#[test]
fn ht40_requires_the_requested_secondary_channel() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::Ht40,
        channel: 13,
        management_frame_protection: ManagementFrameProtection::Disabled,
        access_point_security: AccessPointSecurity::Wpa2Personal,
        beacon: None,
    };
    let mut observed = observation(PhyExpectation::Ht20);
    observed.htmode = "HT40-".into();
    observed.geometry = "channel 13 (2472 MHz), width: 40 MHz, center1: 2482 MHz".into();
    assert!(profile.verify(&observed).is_err());
    observed.geometry = "channel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz".into();
    profile.verify(&observed).unwrap();
}

#[test]
fn capability_is_interface_specific_and_respects_regulation() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::He20,
        channel: 13,
        management_frame_protection: ManagementFrameProtection::Disabled,
        access_point_security: AccessPointSecurity::Wpa2Personal,
        beacon: None,
    };
    let caps = "Supported interface modes:\n * AP\n HT20/HT40\n HE Iftypes: managed\n * 2472 MHz [13] (20.0 dBm)\n";
    assert!(verify_capabilities(profile, caps).is_err());
    let caps = caps.replace("HE Iftypes: managed", "HE Iftypes: AP");
    verify_capabilities(profile, &caps).unwrap();
    assert!(verify_capabilities(profile, &caps.replace("(20.0 dBm)", "(disabled)")).is_err());
    assert!(
        verify_capabilities(profile, &caps.replace("(20.0 dBm)", "(20.0 dBm) (no IR)")).is_err()
    );
}

#[derive(Clone)]
struct Fake {
    calls: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    fail: &'static str,
    before: Value,
}

impl Backend for Fake {
    fn invoke(
        &self,
        operation: &str,
        _options: &Value,
        up: bool,
        _ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>> {
        self.calls.borrow_mut().push(format!("{operation}:{up}"));
        if operation == self.fail {
            return Err("injected failure after remote mutation".into());
        }
        if operation == "restore" && self.fail == "ap-state" {
            let mut after = self.before.clone();
            after["ap_enabled"] = json!(false);
            return Ok(Zeroizing::new(after.to_string()));
        }
        Ok(Zeroizing::new(self.before.to_string()))
    }
    fn observe(&self) -> Result<OpenWrtRadio> {
        if self.fail == "observe" {
            return Err("injected missing hostapd".into());
        }
        Ok(observation(PhyExpectation::He20))
    }
}

#[test]
fn restoring_uci_and_radio_up_is_insufficient_if_the_original_ap_is_missing() {
    let fake = Fake {
        calls: Default::default(),
        fail: "ap-state",
        before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
    };
    let mut owner = AccessPoint::prepare(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
            management_frame_protection: ManagementFrameProtection::Disabled,
            access_point_security: AccessPointSecurity::Wpa2Personal,
            beacon: None,
        },
        json!({}),
    )
    .unwrap();
    assert!(owner.restore().is_err());
    assert!(!owner.restored);
    owner.backend.fail = "";
    owner.restore().unwrap();
    assert!(owner.restored);
}

#[test]
fn uncommitted_router_changes_are_refused_before_any_mutation() {
    let fake = Fake {
        calls: Default::default(),
        fail: "",
        before: json!({"up": true, "ap_enabled": true, "options": {"radio0": {"htmode": "HT20"}}, "pending": {"radio0": {"htmode": true, "channel": false}}}),
    };
    let calls = fake.calls.clone();
    let Err(error) = AccessPoint::prepare(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
            management_frame_protection: ManagementFrameProtection::Disabled,
            access_point_security: AccessPointSecurity::Wpa2Personal,
            beacon: None,
        },
        json!({}),
    ) else {
        panic!("an uncommitted change must be refused");
    };
    let message = error.to_string();
    assert!(message.contains("radio0.htmode"), "{message}");
    assert!(!message.contains("radio0.channel"), "{message}");
    assert!(message.contains("uci revert wireless"), "{message}");
    assert_eq!(*calls.borrow(), ["snapshot:true"]);
}

#[test]
fn restores_after_partial_apply_readback_failure_and_stop_failure() {
    for fail in ["apply", "observe", "state", ""] {
        let calls = Default::default();
        let fake = Fake {
            calls,
            fail,
            before: json!({"up": false, "ap_enabled": false, "options": {"radio0": {"htmode": "HT40"}}, "pending": {"radio0": {"htmode": false}}}),
        };
        let observed = fake.calls.clone();
        let owner = AccessPoint::prepare(
            fake,
            Profile {
                ht40_above: false,
                phy: PhyExpectation::He20,
                channel: 13,
                management_frame_protection: ManagementFrameProtection::Disabled,
                access_point_security: AccessPointSecurity::Wpa2Personal,
                beacon: None,
            },
            json!({}),
        );
        if let Ok(mut owner) = owner {
            let _ = owner.stop();
        }
        let calls = observed.borrow();
        assert_eq!(calls.first().unwrap(), "snapshot:true");
        assert_eq!(
            calls.last().unwrap(),
            "restore:false",
            "restore initial down state after {fail}"
        );
        assert_eq!(
            calls.iter().filter(|call| *call == "restore:false").count(),
            1
        );
    }
}

#[test]
fn restore_failure_is_not_a_successful_owner_release() {
    let directory = tempfile::tempdir().unwrap();
    let scope = oer_hil_workload::fixture::cleanup::Scope::new(directory.path());
    let fake = Fake {
        calls: Default::default(),
        fail: "restore",
        before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
    };
    let mut owner = AccessPoint::prepare(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
            management_frame_protection: ManagementFrameProtection::Disabled,
            access_point_security: AccessPointSecurity::Wpa2Personal,
            beacon: None,
        },
        json!({}),
    )
    .unwrap();
    assert!(owner.restore().is_err());
    assert!(!owner.restored);
    drop(owner);
    assert!(
        scope
            .finish()
            .unwrap()
            .iter()
            .any(|record| record.failure.is_some())
    );
    assert!(oer_hil_workload::fixture::cleanup::require_healthy().is_err());
}

#[test]
fn existing_ap_is_verified_without_mutation_even_on_stop_or_drop() {
    let fake = Fake {
        calls: Default::default(),
        fail: "",
        before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
    };
    let calls = fake.calls.clone();
    let mut owner = AccessPoint::attach(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
            management_frame_protection: ManagementFrameProtection::Disabled,
            access_point_security: AccessPointSecurity::Wpa2Personal,
            beacon: None,
        },
        json!({}),
    )
    .unwrap();
    assert!(owner.stop().is_err());
    assert!(owner.restart().is_err());
    owner.restore().unwrap();
    drop(owner);
    assert_eq!(*calls.borrow(), ["verify:true"]);
}

#[test]
fn existing_ap_mismatch_never_falls_back_to_apply_or_restore() {
    for failure in ["verify", "observe", ""] {
        let fake = Fake {
            calls: Default::default(),
            fail: failure,
            before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
        };
        let calls = fake.calls.clone();
        // Fake observes HE20, so even successful verification must reject HT40.
        assert!(
            AccessPoint::attach(
                fake,
                Profile {
                    ht40_above: false,
                    phy: PhyExpectation::Ht40,
                    channel: 13,
                    management_frame_protection: ManagementFrameProtection::Disabled,
                    access_point_security: AccessPointSecurity::Wpa2Personal,
                    beacon: None,
                },
                json!({})
            )
            .is_err()
        );
        assert_eq!(*calls.borrow(), ["verify:true"]);
    }
}

#[test]
fn existing_generic_ht40_setting_still_requires_exact_active_geometry() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::Ht40,
        channel: 13,
        management_frame_protection: ManagementFrameProtection::Disabled,
        access_point_security: AccessPointSecurity::Wpa2Personal,
        beacon: None,
    };
    let mut observed = observation(PhyExpectation::Ht40);
    observed.htmode = "HT40".into();
    observed.geometry = "channel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz".into();
    profile.verify(&observed).unwrap();
    observed.geometry = "channel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz".into();
    assert!(profile.verify(&observed).is_err());
}

#[test]
fn management_frame_protection_selects_the_openwrt_ieee80211w_option() {
    let config = oer_hil_lab::config::LabConfig::for_test();
    let oer_hil_lab::config::StationFixtureConfig::OpenWrt(openwrt) = &config.station_fixture
    else {
        panic!("OpenWrt test lab required");
    };
    for (protection, expected) in [
        (ManagementFrameProtection::Disabled, "0"),
        (ManagementFrameProtection::Optional, "1"),
        (ManagementFrameProtection::Required, "2"),
    ] {
        let options = Profile::new(
            openwrt,
            PhyExpectation::Ht20,
            protection,
            AccessPointSecurity::Wpa2Personal,
            None,
        )
        .options(openwrt, &config.station);
        assert_eq!(options[&openwrt.ap_section]["ieee80211w"], expected);
    }
}

#[test]
fn wpa3_security_selects_sae_and_management_frame_protection() {
    let config = oer_hil_lab::config::LabConfig::for_test();
    let oer_hil_lab::config::StationFixtureConfig::OpenWrt(openwrt) = &config.station_fixture
    else {
        panic!("OpenWrt test lab required");
    };
    for (security, encryption, ieee80211w) in [
        (AccessPointSecurity::Wpa2Personal, "psk2", "0"),
        (AccessPointSecurity::Wpa3Personal, "sae", "2"),
        (AccessPointSecurity::Wpa3Transition, "sae-mixed", "1"),
    ] {
        let options = Profile::new(
            openwrt,
            PhyExpectation::Ht20,
            ManagementFrameProtection::Disabled,
            security,
            None,
        )
        .options(openwrt, &config.station);
        assert_eq!(options[&openwrt.ap_section]["encryption"], encryption);
        assert_eq!(options[&openwrt.ap_section]["ieee80211w"], ieee80211w);
    }
}

#[test]
fn a_beacon_schedule_sets_the_radio_interval_and_the_bss_dtim_and_is_verified() {
    let config = oer_hil_lab::config::LabConfig::for_test();
    let oer_hil_lab::config::StationFixtureConfig::OpenWrt(openwrt) = &config.station_fixture
    else {
        panic!("OpenWrt test lab required");
    };
    let beacon = oer_hil_scenario_catalog::link::AccessPointBeacon {
        interval_tu: 100,
        dtim_period: 3,
    };
    let profile = Profile::new(
        openwrt,
        PhyExpectation::Ht20,
        ManagementFrameProtection::Disabled,
        AccessPointSecurity::Wpa2Personal,
        Some(beacon),
    );
    let options = profile.options(openwrt, &config.station);
    assert_eq!(options[&openwrt.radio]["beacon_int"], "100");
    assert_eq!(options[&openwrt.ap_section]["dtim_period"], "3");
    // Without a schedule the router's own stays.
    let plain = Profile {
        beacon: None,
        ..profile
    }
    .options(openwrt, &config.station);
    assert!(plain[&openwrt.radio].get("beacon_int").is_none());
    assert!(plain[&openwrt.ap_section].get("dtim_period").is_none());
    // A hostapd that runs another schedule fails the profile.
    let frequency = 2407 + u16::from(profile.channel) * 5;
    let mut observed = OpenWrtRadio {
        enabled: true,
        channel: profile.channel,
        geometry: format!(
            "channel {} ({frequency} MHz), width: 20 MHz, center1: {frequency} MHz",
            profile.channel
        ),
        htmode: "HT20".into(),
        ht: true,
        he: false,
        beacon_interval_tu: Some(100),
        dtim_period: Some(3),
    };
    assert!(profile.verify(&observed).is_ok());
    observed.dtim_period = Some(2);
    assert!(profile.verify(&observed).is_err());
}
