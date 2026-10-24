//! Building an image of `firmware/apps` through the image pipeline: its
//! bundle lies in `target/fw/<image>/` or `target/fw/<image>-analysis/`,
//! described by its `bundle.json`.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

use serde::Deserialize;

use crate::{w1, w2, w3};
use oer_image_build::{
    Application, Gates, ImageSpec, Result,
    bundle::{ImageBundle, Purpose},
};
use oer_image_checks::{Enforced, interrupts::Required};

/// The chip every image of `firmware/apps` is built for.
pub const CHIP: &str = "esp32s31";

/// An image's bundle directory below the repository `root`.
pub fn output(root: &Path, image: &str) -> PathBuf {
    root.join("target/fw").join(image)
}

/// The package of the image `image` (`firmware/apps/<image>`).
pub fn package(root: &Path, image: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Manifest {
        package: Package,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
    }
    let path = root.join("firmware/apps").join(image).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("no image `{image}`: {}: {error}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(manifest.package.name)
}

/// The runtime build's variable of image `w0`'s regulatory country.
pub const ENV_W0_COUNTRY: &str = "OER_W0_COUNTRY";

/// What an image is built for on the stand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stand {
    /// B0 BLE's explicit laboratory band and conducted-power cap.
    B0 { country: [u8; 2], cap_dbm: i8 },
    /// Image `w0`: the stand router radio's regulatory country.
    W0 { country: [u8; 2] },
    /// Image `w1`: the stand's identity.
    W1(w1::Config),
    /// Image `w2`: the stand identity with the explicit W2 laboratory cap.
    W2(w1::Config),
    /// Image w3: the open HT20 station stand.
    W3(w1::Config),
}

/// Fixed fictitious stand identities for analysis, independent of the lab.
const ANALYSIS_B0: Stand = Stand::B0 {
    country: *b"DE",
    cap_dbm: 0,
};
const ANALYSIS_W0: Stand = Stand::W0 { country: *b"DE" };
static ANALYSIS_W1: LazyLock<Stand> = LazyLock::new(|| Stand::W1(analysis_config()));
static ANALYSIS_W3: LazyLock<Stand> = LazyLock::new(|| Stand::W3(analysis_config()));
static ANALYSIS_W2: LazyLock<Stand> = LazyLock::new(|| Stand::W2(analysis_config()));

fn analysis_config() -> w1::Config {
    w1::Config {
        channel: 1,
        bssid: oer_lab_router::mac::Mac::new([2, 0, 0, 0, 0, 1]).expect("analysis BSSID"),
        ssid: w1::Ssid::new(b"oer").expect("analysis SSID"),
        country: *b"DE",
    }
}

fn analysis_stand(image: &str) -> Result<&'static Stand> {
    match image {
        "b0" => Ok(&ANALYSIS_B0),
        "w0" => Ok(&ANALYSIS_W0),
        w1::IMAGE => Ok(&ANALYSIS_W1),
        w2::IMAGE => Ok(&ANALYSIS_W2),
        w3::IMAGE => Ok(&ANALYSIS_W3),
        _ => Err(format!("image `{image}` has no stand configuration for --analysis").into()),
    }
}

/// The stand's variables a cargo build of `image`'s runtime sets (`Some`) or
/// removes (`None`): only a configured runtime of its own image gets them,
/// so a value left in the caller's environment reaches no other build, nor
/// the unconfigured one.
pub fn runtime_env(image: &str, stand: Option<&Stand>) -> Vec<(String, Option<String>)> {
    match (image, stand) {
        ("b0", Some(Stand::B0 { country, cap_dbm })) => vec![
            ("OER_B0_COUNTRY".into(), Some(w1::country_text(*country))),
            ("OER_B0_POWER_CAP_DBM".into(), Some(cap_dbm.to_string())),
        ],
        ("b0", _) => vec![
            ("OER_B0_COUNTRY".into(), None),
            ("OER_B0_POWER_CAP_DBM".into(), None),
        ],
        (w2::IMAGE, Some(Stand::W2(config))) => w2::runtime_env(config),
        (w3::IMAGE, Some(Stand::W3(config))) => w3::runtime_env(config),
        (w1::IMAGE, Some(Stand::W1(config))) => config
            .runtime_env()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), Some(value)))
            .collect(),
        ("w0", Some(Stand::W0 { country })) => {
            vec![(ENV_W0_COUNTRY.to_owned(), Some(w1::country_text(*country)))]
        }
        _ => w1::ENV
            .into_iter()
            .chain(w2::ENV)
            .chain(w3::ENV)
            .chain([ENV_W0_COUNTRY])
            .map(|name| (name.to_owned(), None))
            .collect(),
    }
}

/// How long `cargo fw run` waits for `image`'s result without `--timeout`:
/// W3's bidirectional echo workload takes up to four minutes, W1's hold
/// cycles up to two minutes, and other images report within 30 s.
pub fn run_timeout(image: &str) -> std::time::Duration {
    if image == w3::IMAGE {
        std::time::Duration::from_secs(240)
    } else if image == w1::IMAGE {
        std::time::Duration::from_secs(120)
    } else {
        std::time::Duration::from_secs(30)
    }
}

/// The pipeline's spec of `image`, with the stand's parameters and the
/// caller's enforcement policy. Every check runs.
pub fn spec(
    root: &Path,
    image: &str,
    stand: Option<&Stand>,
    analysis: bool,
    enforced: Enforced,
) -> Result<ImageSpec> {
    let stand = if analysis {
        Some(analysis_stand(image)?)
    } else {
        stand
    };
    let profile = oer_chip_profile::Profile::load(root, CHIP)?;
    let package = package(root, image)?;
    Ok(ImageSpec {
        purpose: if analysis {
            Purpose::Analysis
        } else {
            Purpose::Flash
        },
        root: root.to_owned(),
        chip: CHIP.to_owned(),
        application: Application {
            binary: package.clone(),
            package,
            features: Vec::new(),
            default_features: true,
        },
        stack_policy: match image {
            w1::IMAGE => "firmware/apps/w1/stack.toml".into(),
            w3::IMAGE if stand.is_some() => "firmware/apps/w3/stack-configured.toml".into(),
            w3::IMAGE => "firmware/apps/w3/stack.toml".into(),
            _ => profile.stack_policy(),
        },
        layout_seed: None,
        runtime_environment: runtime_env(image, stand),
        output: if analysis {
            output(root, &format!("{image}-analysis"))
        } else {
            output(root, image)
        },
        checks: Some(Gates {
            enforced,
            ..Gates::all(Required::Product)
        }),
    })
}

/// Build `image`, with the stand's parameters, into its bundle.
pub fn build(
    root: &Path,
    image: &str,
    stand: Option<&Stand>,
    analysis: bool,
    enforced: Enforced,
) -> Result<ImageBundle> {
    let bundle = oer_image_build::build(&spec(root, image, stand, analysis, enforced)?)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(bundle)
}

/// The built bundle of `image`, its flash files checked against its
/// manifest.
pub fn bundle(root: &Path, image: &str) -> Result<ImageBundle> {
    let directory = output(root, image);
    ImageBundle::load(&directory).map_err(|error| {
        format!(
            "image `{image}` is not built ({}: {error}); run `cargo fw build {image}`",
            directory.display()
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_image_checks::Check;

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("chips/esp32s31/platform/chip.toml");
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(
            &profile,
            include_str!("../../../../chips/esp32s31/platform/chip.toml"),
        )
        .unwrap();
        for image in ["b0", "w0", "w1", "w2", "w3", "boot-smoke"] {
            let manifest = directory
                .path()
                .join("firmware/apps")
                .join(image)
                .join("Cargo.toml");
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            std::fs::write(
                manifest,
                format!("[package]\nname = \"oer-firmware-apps-{image}\"\n"),
            )
            .unwrap();
        }
        directory
    }

    #[test]
    fn analysis_uses_the_fixed_stands_through_the_runtime_environment() {
        let directory = repository();
        for (image, stand) in [
            ("b0", &ANALYSIS_B0),
            ("w0", &ANALYSIS_W0),
            ("w1", &*ANALYSIS_W1),
            ("w2", &*ANALYSIS_W2),
            ("w3", &*ANALYSIS_W3),
        ] {
            let spec = spec(
                directory.path(),
                image,
                None,
                true,
                Enforced::Only([Check::Frames, Check::Interrupts, Check::Panics].into()),
            )
            .unwrap();
            assert_eq!(spec.runtime_environment, runtime_env(image, Some(stand)));
        }
    }

    #[test]
    fn analysis_configuration_is_fictitious_and_keeps_the_lab_power_cap() {
        assert_eq!(ANALYSIS_W0, Stand::W0 { country: *b"DE" });
        for stand in [&*ANALYSIS_W1, &*ANALYSIS_W2, &*ANALYSIS_W3] {
            let config = match stand {
                Stand::W1(config) | Stand::W2(config) | Stand::W3(config) => config,
                _ => unreachable!(),
            };
            assert_eq!(config.channel, 1);
            assert_eq!(config.bssid.octets(), [2, 0, 0, 0, 0, 1]);
            assert_eq!(config.ssid.bytes(), b"oer");
            assert_eq!(config.country, *b"DE");
        }
        assert!(runtime_env("w2", Some(&ANALYSIS_W2)).contains(&(
            w2::ENV[4].to_owned(),
            Some(w2::LAB_POWER_CAP_DBM.to_string())
        )));
    }

    #[test]
    fn build_and_analysis_enforce_frames_and_interrupts_with_separate_outputs() {
        let directory = repository();
        for image in ["w0", "w1", "w2", "w3"] {
            for analysis in [false, true] {
                let enforced =
                    Enforced::Only([Check::Frames, Check::Interrupts, Check::Panics].into());
                let spec = spec(directory.path(), image, None, analysis, enforced.clone()).unwrap();
                assert_eq!(
                    spec.output,
                    directory.path().join("target/fw").join(if analysis {
                        format!("{image}-analysis")
                    } else {
                        image.to_owned()
                    })
                );
                assert_eq!(
                    spec.purpose,
                    if analysis {
                        Purpose::Analysis
                    } else {
                        Purpose::Flash
                    }
                );
                let gates = spec.checks.unwrap();
                assert_eq!(gates.enforced, enforced);
                let expected_policy = match (image, analysis) {
                    ("w1", _) => "firmware/apps/w1/stack.toml",
                    ("w3", true) => "firmware/apps/w3/stack-configured.toml",
                    ("w3", false) => "firmware/apps/w3/stack.toml",
                    _ => "chips/esp32s31/platform/stack.toml",
                };
                assert_eq!(spec.stack_policy, Path::new(expected_policy));
                assert!(gates.frames && gates.stack && gates.placement);
                assert_eq!(gates.interrupts, Some(Required::Product));
                if !analysis {
                    assert_eq!(spec.runtime_environment, runtime_env(image, None));
                }
            }
        }
    }

    #[test]
    fn stand_builds_report_every_check() {
        let directory = repository();
        for (image, stand) in [
            ("boot-smoke", None),
            ("w0", Some(&ANALYSIS_W0)),
            ("w1", Some(&*ANALYSIS_W1)),
            ("w2", Some(&*ANALYSIS_W2)),
            ("w3", Some(&*ANALYSIS_W3)),
        ] {
            let spec = spec(directory.path(), image, stand, false, Enforced::Reported).unwrap();
            let gates = spec.checks.unwrap();
            assert_eq!(gates.enforced, Enforced::Reported);
            assert!(gates.frames && gates.stack && gates.placement);
            assert_eq!(gates.interrupts, Some(Required::Product));
            assert_eq!(spec.runtime_environment, runtime_env(image, stand));
        }
    }

    #[test]
    fn images_without_a_stand_refuse_analysis() {
        let directory = repository();
        assert!(
            spec(
                directory.path(),
                "boot-smoke",
                None,
                false,
                Enforced::Only([Check::Frames, Check::Interrupts, Check::Panics].into())
            )
            .is_ok()
        );
        let error = spec(
            directory.path(),
            "boot-smoke",
            None,
            true,
            Enforced::Only([Check::Frames, Check::Interrupts, Check::Panics].into()),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(
            error.contains("image `boot-smoke` has no stand configuration"),
            "{error}"
        );
    }

    #[test]
    fn flash_loads_reported_failures_without_enforcing_checks() {
        let directory = repository();
        let root = directory.path();
        let profile = oer_chip_profile::Profile::load(root, CHIP).unwrap();
        let output = output(root, "w0");
        let staging = oer_image_build::bundle::staging_directory(&output).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        let mut built = ImageBundle::new(&staging, &profile, profile.flash.clone().unwrap());
        for segment in built.segments() {
            std::fs::write(segment.file, b"fixture").unwrap();
        }
        built.checks.push(oer_image_checks::CheckRecord {
            elf: oer_image_checks::CheckedElf::Runtime,
            check: "interrupts".into(),
            result: oer_image_checks::CheckResult::Failed("no bound".into()),
        });
        let published = built.publish(&output).unwrap();
        let loaded = bundle(root, "w0").unwrap();
        assert_eq!(loaded.checks, published.checks);
        assert!(loaded.snapshot().is_ok());
    }

    #[test]
    fn flash_and_run_cannot_write_an_analysis_bundle_in_the_normal_directory() {
        let directory = repository();
        let root = directory.path();
        let profile = oer_chip_profile::Profile::load(root, CHIP).unwrap();
        let output = output(root, "w2");
        let staging = oer_image_build::bundle::staging_directory(&output).unwrap();
        for purpose in [Purpose::Analysis, Purpose::Flash] {
            std::fs::create_dir_all(&staging).unwrap();
            let mut built = ImageBundle::new(&staging, &profile, profile.flash.clone().unwrap());
            built.purpose = purpose;
            for segment in built.segments() {
                std::fs::write(segment.file, b"fixture").unwrap();
            }
            built.publish(&output).unwrap();
            // Both commands' write_image uses this loader, then the common
            // board writer requests a flash snapshot before writing.
            let loaded = bundle(root, "w2").unwrap();
            let snapshot = loaded.snapshot();
            if purpose == Purpose::Analysis {
                assert!(
                    snapshot
                        .unwrap_err()
                        .to_string()
                        .contains("purpose Analysis")
                );
            } else {
                assert!(snapshot.is_ok());
            }
        }
    }

    #[test]
    fn w1_waits_long_enough_for_its_report_by_default() {
        assert_eq!(run_timeout("w1"), std::time::Duration::from_secs(120));
        assert_eq!(run_timeout("w0"), std::time::Duration::from_secs(30));
    }

    #[test]
    fn only_w1s_runtime_build_gets_its_parameters() {
        let config = w1::Config {
            channel: 11,
            bssid: oer_lab_router::mac::Mac::new([0x02, 0, 0, 0, 0xF6, 0xD0]).unwrap(),
            ssid: w1::Ssid::new(b"Lab").unwrap(),
            country: *b"DE",
        };
        let stand = Stand::W1(config);
        let set = |name: &str, value: &str| (name.to_owned(), Some(value.to_owned()));
        assert_eq!(
            runtime_env("w1", Some(&stand)),
            [
                set(w1::ENV_CHANNEL, "11"),
                set(w1::ENV_BSSID, "02:00:00:00:F6:D0"),
                set(w1::ENV_SSID, "4c6162"),
                set(w1::ENV_COUNTRY, "DE"),
            ]
        );
        let removed = [
            w1::ENV_CHANNEL,
            w1::ENV_BSSID,
            w1::ENV_SSID,
            w1::ENV_COUNTRY,
        ]
        .into_iter()
        .chain(w2::ENV)
        .chain(w3::ENV)
        .chain([ENV_W0_COUNTRY])
        .map(|name| (name.to_owned(), None))
        .collect::<Vec<_>>();
        // The unconfigured w1 and another image remove them.
        assert_eq!(runtime_env("w1", None), removed);
        assert_eq!(runtime_env("w0", Some(&stand)), removed);
        // w0 gets only its country.
        assert_eq!(
            runtime_env("w0", Some(&Stand::W0 { country: *b"DE" })),
            [set(ENV_W0_COUNTRY, "DE")]
        );
    }
}
