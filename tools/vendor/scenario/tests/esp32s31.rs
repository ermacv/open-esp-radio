//! The ESP32-S31 scenarios, which install their chip for the whole process:
//! a test process of its own, apart from the engine's tests with their test
//! chip.

use oer_pins_artifacts::SourceKind;
use oer_vendor_scenario::esp32s31::{
    artifacts::{manifest, sha256},
    install,
    run::Scenario,
};

#[test]
fn a_firmware_input_has_the_digest_of_its_current_build_or_none() {
    install();
    // Any input path, explicit or default, must have this digest: the
    // current build's, or the input is refused with the build command.
    match sha256("phy-sdk") {
        Ok(digest) => assert_eq!(digest.len(), 64),
        Err(error) => assert!(
            error
                .to_string()
                .contains("run `cargo fw idf build phy-tracking-reference`"),
            "{error}"
        ),
    }
}

#[test]
fn tracked_manifest_pins_every_scenario_input_once() {
    install();
    let manifest = manifest();
    for id in ["libphy", "librftest", "libpp", "rom"] {
        assert_eq!(sha256(id).unwrap().len(), 64, "{id}");
    }
    for id in ["sdk", "phy-sdk"] {
        assert_eq!(manifest.artifact(id).unwrap().sha256, None, "{id}");
    }
    let mut ids: Vec<_> = manifest.artifact.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), manifest.artifact.len());
    for source in &manifest.source {
        if source.kind != SourceKind::Firmware {
            assert!(
                source.repository.is_some() && source.revision.is_some(),
                "{}",
                source.id
            );
        }
        if source.kind == SourceKind::Release {
            assert!(
                source.asset.is_some() && source.sha256.is_some(),
                "{}",
                source.id
            );
        }
    }
}

#[test]
fn scenarios_accept_their_inputs_and_reject_index_and_mutant_options() {
    #[derive(clap::Parser)]
    struct Cli {
        #[command(subcommand)]
        scenario: Scenario,
    }
    use clap::Parser as _;
    install();
    let args = [
        "scenario",
        "i2c",
        "--production",
        "probe.elf",
        "--linker",
        "ld.lld",
        "--output",
        "runs",
    ];
    assert!(Cli::try_parse_from(args).is_ok());
    for flag in ["--index", "--patch", "--bluetooth-patch"] {
        let error = Cli::try_parse_from([&args[..], &[flag, "value"]].concat())
            .err()
            .expect("removed option rejected");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
