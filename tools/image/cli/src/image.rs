//! Building an image of `firmware/apps` through the image pipeline: its
//! bundle lies in `target/fw/<image>/`, described by its `bundle.json`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{w1, w2};
use oer_image_build::{Application, Gates, ImageSpec, Result, bundle::ImageBundle};
use oer_image_checks::interrupts::Required;

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
    /// Image `w0`: the stand router radio's regulatory country.
    W0 { country: [u8; 2] },
    /// Image `w1`: the stand's identity.
    W1(w1::Config),
    /// Image `w2`: the stand identity with the explicit W2 laboratory cap.
    W2(w1::Config),
}

/// The stand's variables a cargo build of `image`'s runtime sets (`Some`) or
/// removes (`None`): only a configured runtime of its own image gets them,
/// so a value left in the caller's environment reaches no other build, nor
/// the unconfigured one.
pub fn runtime_env(image: &str, stand: Option<&Stand>) -> Vec<(String, Option<String>)> {
    match (image, stand) {
        (w2::IMAGE, Some(Stand::W2(config))) => w2::runtime_env(config),
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
            .chain([ENV_W0_COUNTRY])
            .map(|name| (name.to_owned(), None))
            .collect(),
    }
}

/// How long `cargo fw run` waits for `image`'s result without `--timeout`:
/// w1's hold cycles and report take up to two minutes, any other image
/// reports within 30 s.
pub fn run_timeout(image: &str) -> std::time::Duration {
    if image == w1::IMAGE {
        std::time::Duration::from_secs(120)
    } else {
        std::time::Duration::from_secs(30)
    }
}

/// The pipeline's spec of `image`, with the stand's parameters:
/// every check, reported and never failing the build.
pub fn spec(root: &Path, image: &str, stand: Option<&Stand>) -> Result<ImageSpec> {
    let profile = oer_chip_profile::Profile::load(root, CHIP)?;
    let package = package(root, image)?;
    Ok(ImageSpec {
        root: root.to_owned(),
        chip: CHIP.to_owned(),
        application: Application {
            binary: package.clone(),
            package,
            features: Vec::new(),
            default_features: true,
        },
        stack_policy: profile.stack_policy(),
        layout_seed: None,
        runtime_environment: runtime_env(image, stand),
        output: output(root, image),
        checks: Some(Gates {
            enforced: false,
            ..Gates::all(Required::Proven)
        }),
    })
}

/// Build `image`, with the stand's parameters, into its bundle.
pub fn build(root: &Path, image: &str, stand: Option<&Stand>) -> Result<ImageBundle> {
    let bundle = oer_image_build::build(&spec(root, image, stand)?)?;
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
        ].into_iter().chain(w2::ENV).chain([ENV_W0_COUNTRY])
        .map(|name| (name.to_owned(), None)).collect::<Vec<_>>();
        // The unconfigured w1 and another image remove them.
        assert_eq!(runtime_env("w1", None), removed);
        assert_eq!(runtime_env("w0", Some(&stand)), removed);
        // w0 gets only its country.
        assert_eq!(
            runtime_env("w0", Some(&Stand::W0 { country: *b"DE" })),
            [set(ENV_W0_COUNTRY, "DE")]
        );
    }

    #[test]
    fn an_app_image_reports_every_check() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let spec = spec(&root, "w0", None).unwrap();
        assert_eq!(spec.application.package, "oer-firmware-apps-w0");
        assert_eq!(spec.output, root.join("target/fw/w0"));
        let checks = spec.checks.unwrap();
        assert!(!checks.enforced);
        assert!(checks.stack && checks.placement && checks.interrupts.is_some());
    }
}
