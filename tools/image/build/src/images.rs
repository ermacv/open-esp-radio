//! The repository's flashable images: each is the package in a directory of
//! [`DIRECTORIES`], named by that directory, and built through the image
//! pipeline into `target/fw/<image>/`, described by its `bundle.json`.
//! `cargo fw` and the HIL harness build and load them here.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Application, Gates, ImageSpec, Result};
use oer_image_bundle::{ImageBundle, Purpose};
use oer_image_checks::{Enforced, interrupts::Required};

/// The chip every image is built for.
pub const CHIP: &str = "esp32s31";

/// Where image packages live: the applications and the HIL scenarios'
/// images. An image's name is its directory; no name may be in both.
pub const DIRECTORIES: [&str; 2] = ["firmware/apps", "hil-scenarios/images"];

/// An image's bundle directory below the repository `root`.
pub fn output(root: &Path, image: &str) -> PathBuf {
    root.join("target/fw").join(image)
}

/// The package directory of `image`: its directory of [`DIRECTORIES`]. A
/// relative path (`../probes/<probe>`) names a package beside the
/// applications.
fn directory(root: &Path, image: &str) -> Result<PathBuf> {
    if image.contains('/') {
        return Ok(root.join(DIRECTORIES[0]).join(image));
    }
    let found = DIRECTORIES
        .iter()
        .map(|directory| root.join(directory).join(image))
        .filter(|directory| directory.join("Cargo.toml").is_file())
        .collect::<Vec<_>>();
    match found.as_slice() {
        [directory] => Ok(directory.clone()),
        [] => Err(format!("no image `{image}` in {}", DIRECTORIES.join(" or ")).into()),
        _ => Err(format!("image `{image}` is in more than one of {}", DIRECTORIES.join(", ")).into()),
    }
}

/// The package of the image `image`.
pub fn package(root: &Path, image: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Manifest {
        package: Package,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
    }
    let path = directory(root, image)?.join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("no image `{image}`: {}: {error}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(manifest.package.name)
}

/// The pipeline's spec of `image`, with the caller's enforcement policy. Every check runs.
pub fn spec(root: &Path, image: &str, enforced: Enforced) -> Result<ImageSpec> {
    let profile = oer_chip_profile::Profile::load(root, CHIP)?;
    let package = package(root, image)?;
    Ok(ImageSpec {
        purpose: Purpose::Flash,
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
        runtime_environment: Vec::new(),
        output: output(root, image),
        checks: Some(Gates {
            enforced,
            ..Gates::all(Required::Product)
        }),
    })
}

/// The built bundle of `image`, its flash files checked against its
/// manifest.
pub fn bundle(root: &Path, image: &str) -> Result<ImageBundle> {
    package(root, image)?;
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

    /// A repository with the chip facts and image manifests at `images`.
    fn repository(images: &[&str]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in oer_espressif_facts::tracked::FILES {
            let path = directory.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        for image in images {
            let manifest = directory.path().join(image).join("Cargo.toml");
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            let name = image.replace('/', "-");
            std::fs::write(manifest, format!("[package]\nname = \"oer-{name}\"\n")).unwrap();
        }
        directory
    }

    #[test]
    fn applications_scenario_images_and_probes_keep_the_pipeline_checks_and_configuration() {
        let directory = repository(&[
            "firmware/apps/boot-smoke",
            "hil-scenarios/images/esp32s31-rpc",
            "firmware/probes/esp32s31-radio",
        ]);
        for (image, package) in [
            ("boot-smoke", "oer-firmware-apps-boot-smoke"),
            ("esp32s31-rpc", "oer-hil-scenarios-images-esp32s31-rpc"),
            ("../probes/esp32s31-radio", "oer-firmware-probes-esp32s31-radio"),
        ] {
            for enforced in [
                Enforced::Only([Check::Interrupts].into()),
                Enforced::Reported,
            ] {
                let spec = spec(directory.path(), image, enforced.clone()).unwrap();
                assert_eq!(spec.application.package, package);
                assert_eq!(spec.purpose, Purpose::Flash);
                assert_eq!(spec.output, output(directory.path(), image));
                assert_eq!(
                    spec.stack_policy,
                    Path::new("chips/esp32s31/platform/stack.toml")
                );
                assert!(spec.runtime_environment.is_empty());
                let gates = spec.checks.unwrap();
                assert_eq!(gates.enforced, enforced);
                assert!(gates.frames && gates.stack && gates.placement);
                assert_eq!(gates.interrupts, Some(Required::Product));
            }
        }
    }

    #[test]
    fn a_name_in_both_directories_or_in_none_is_refused() {
        let directory = repository(&["firmware/apps/twin", "hil-scenarios/images/twin"]);
        let twin = package(directory.path(), "twin").unwrap_err().to_string();
        assert!(twin.contains("more than one"), "{twin}");
        let none = package(directory.path(), "absent").unwrap_err().to_string();
        assert!(none.contains("no image"), "{none}");
    }

    #[test]
    fn a_saved_bundle_cannot_flash_a_removed_application() {
        let directory = repository(&[]);
        let profile = oer_chip_profile::Profile::load(directory.path(), CHIP).unwrap();
        for image in ["retired-wifi-station", "retired-bluetooth-controller"] {
            assert!(package(directory.path(), image).is_err());
            let output = output(directory.path(), image);
            let staging = oer_image_bundle::staging_directory(&output).unwrap();
            std::fs::create_dir_all(&staging).unwrap();
            let built = ImageBundle::new(&staging, &profile, profile.facts.flash.clone().unwrap());
            for segment in built.segments() {
                std::fs::write(segment.file, b"fixture").unwrap();
            }
            built.publish(&output).unwrap();
            let error = bundle(directory.path(), image).unwrap_err().to_string();
            assert!(error.contains("no image"), "{error}");
        }
    }
}
