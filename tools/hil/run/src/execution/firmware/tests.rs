use super::*;

#[test]
fn current_build_has_the_canonical_firmware_plan_identity() {
    assert!(matches!(
        RunFirmware::BuildCurrent(crate::image::CurrentBuild {
            source: crate::image::ImageSource::Tree(std::path::PathBuf::from("/checkout")),
            features: oer_hil_model::schema::image::FeatureDelta::default(),
            layout_seed: None,
        })
        .plan(),
        PlannedFirmware::BuildCurrent
    ));
}
