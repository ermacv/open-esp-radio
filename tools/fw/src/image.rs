//! Building an image of `firmware/apps` through the image pipeline: its
//! bundle lies in `target/fw/<image>/`, described by its `bundle.json`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::w1;
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

/// The stand identity's variables a cargo build of `image`'s runtime sets
/// (`Some`) or removes (`None`): only a configured `w1` runtime gets them,
/// so a value left in the caller's environment reaches no other build, nor
/// the unconfigured `w1`.
pub fn runtime_env(image: &str, w1: Option<&w1::Config>) -> Vec<(String, Option<String>)> {
    match w1.filter(|_| image == w1::IMAGE) {
        Some(config) => config
            .runtime_env()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), Some(value)))
            .collect(),
        None => w1::ENV
            .into_iter()
            .map(|name| (name.to_owned(), None))
            .collect(),
    }
}

/// The pipeline's spec of `image`, with `w1`'s parameters for image `w1`:
/// every check, reported and never failing the build.
pub fn spec(root: &Path, image: &str, w1: Option<&w1::Config>) -> Result<ImageSpec> {
    let profile = oer_image_profile::Profile::load(root, CHIP)?;
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
        runtime_environment: runtime_env(image, w1),
        output: output(root, image),
        checks: Some(Gates {
            enforced: false,
            ..Gates::all(Required::Proven)
        }),
    })
}

/// Build `image`, with `w1`'s parameters for image `w1`, into its bundle.
pub fn build(root: &Path, image: &str, w1: Option<&w1::Config>) -> Result<ImageBundle> {
    let bundle = oer_image_build::build(&spec(root, image, w1)?)?;
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
    fn only_w1s_runtime_build_gets_its_parameters() {
        let config = w1::Config {
            channel: 11,
            bssid: oer_lab_router::mac::Mac::new([0x02, 0, 0, 0, 0xF6, 0xD0]).unwrap(),
            ssid: w1::Ssid::new(b"Lab").unwrap(),
        };
        let set = |name: &str, value: &str| (name.to_owned(), Some(value.to_owned()));
        assert_eq!(
            runtime_env("w1", Some(&config)),
            [
                set(w1::ENV_CHANNEL, "11"),
                set(w1::ENV_BSSID, "02:00:00:00:F6:D0"),
                set(w1::ENV_SSID, "4c6162"),
            ]
        );
        let removed =
            [w1::ENV_CHANNEL, w1::ENV_BSSID, w1::ENV_SSID].map(|name| (name.to_owned(), None));
        // The unconfigured w1 and another image remove them.
        assert_eq!(runtime_env("w1", None), removed);
        assert_eq!(runtime_env("w0", Some(&config)), removed);
    }

    #[test]
    fn an_app_image_reports_every_check() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let spec = spec(&root, "w0", None).unwrap();
        assert_eq!(spec.application.package, "oer-firmware-apps-w0");
        assert_eq!(spec.output, root.join("target/fw/w0"));
        let checks = spec.checks.unwrap();
        assert!(!checks.enforced);
        assert!(checks.stack && checks.placement && checks.interrupts.is_some());
    }
}
