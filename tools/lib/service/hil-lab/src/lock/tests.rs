use super::*;

#[test]
fn a_run_claims_its_boards_fixtures_and_the_air() {
    use oer_stand_claims::{AIR, Claim};
    let lab = crate::config::LabConfig::for_test();
    let measured = Spectrum::new(BAND_2G4, Need::Strict, Emits::Noisy);
    let request = LeaseRequest {
        air: vec![measured],
        ..LeaseRequest::device(Default::default())
    };
    let keys = [String::from("openwrt-host-boot:x")];
    let claims = claims(&lab, &request, &keys);
    let [range, coarse] = measured.claims();
    assert_eq!(
        claims,
        [
            Claim::board(&lab.dut.mac),
            Claim::exclusive("openwrt-host-boot:x"),
            range,
            coarse,
        ]
    );
    assert_eq!(claims.last(), Some(&Claim::exclusive(AIR)));
    // Radio-free work claims no air at all.
    let quiet = LeaseRequest {
        device: false,
        air: Vec::new(),
        ..LeaseRequest::device(Default::default())
    };
    assert!(claims_of(&lab, &quiet).is_empty());
}

#[test]
fn a_run_shares_its_fixture_software_with_other_runs_but_not_an_installation() {
    use oer_stand_claims::Claim;
    let lab = crate::config::LabConfig::for_test();
    let required = oer_hil_scenario_catalog::requirements::Requirements {
        laptop_client: true,
        ..Default::default()
    };
    let claims = claims_of(
        &lab,
        &LeaseRequest {
            device: false,
            ..LeaseRequest::device(required)
        },
    );
    let software =
        oer_stand_fixture_install::resource(oer_stand_fixture_install::Provider::LinuxNet);
    assert!(claims.contains(&Claim::shared(&software)), "{claims:?}");
    assert!(!claims.contains(&Claim::exclusive(&software)));
}

fn claims_of(
    lab: &crate::config::LabConfig,
    request: &LeaseRequest,
) -> Vec<oer_stand_claims::Claim> {
    claims(lab, request, &[])
}

#[cfg(unix)]
#[test]
fn different_interfaces_on_the_same_radio_share_one_key() {
    let root = tempfile::tempdir().unwrap();
    let radio = root.path().join("radio");
    std::fs::create_dir_all(&radio).unwrap();
    for name in ["client", "monitor"] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
        std::os::unix::fs::symlink(&radio, root.path().join(name).join("phy80211")).unwrap();
    }
    assert_eq!(
        local_radio_key(&root.path().join("client")).unwrap(),
        local_radio_key(&root.path().join("monitor")).unwrap()
    );
}
