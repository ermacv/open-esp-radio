//! The repository's flashable images: each is the package in a directory of
//! [`DIRECTORIES`], named by that directory, and built through the image
//! pipeline into `target/fw/<image>/`, described by its `bundle.json`.
//! `cargo fw` and the HIL harness build and load them here.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Application, Gates, ImageSpec, Result};
use oer_tools_image_bundle::{ImageBundle, Purpose};
use oer_tools_image_checks::{Enforced, interrupts::Required};

/// The chip every image is built for.
pub const CHIP: &str = "esp32s31";

/// Where image packages live, each with the prefix its images' names carry:
/// the examples and the probes by their directory, the HIL scenarios'
/// images as `hil-<directory>`. No two images share a name.
pub const DIRECTORIES: [(&str, &str); 3] =
    [("examples", ""), ("hil/images", "hil-"), ("hil/probes", "")];

/// The application entry: an image that depends on it gets the chip's
/// feature from the build, never from its own manifest.
const ENTRY: &str = "oer";

/// An image's bundle directory below the repository `root`.
pub fn output(root: &Path, image: &str) -> PathBuf {
    root.join("target/fw").join(image)
}

/// The package directory of `image`: its directory of [`DIRECTORIES`]. An
/// image is named by its directory alone, so its bundle directory stays
/// below `target/fw/`.
fn directory(root: &Path, image: &str) -> Result<PathBuf> {
    let roots = || {
        DIRECTORIES
            .iter()
            .map(|(directory, prefix)| format!("{directory}/{prefix}*"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if image.is_empty() || image.starts_with('.') || image.contains(['/', '\\']) {
        return Err(format!(
            "`{image}` is not an image name: name an image by its directory ({})",
            roots()
        )
        .into());
    }
    let found = DIRECTORIES
        .iter()
        .filter_map(|(directory, prefix)| {
            let name = image.strip_prefix(prefix)?;
            (!name.is_empty()).then(|| root.join(directory).join(name))
        })
        .filter(|directory| directory.join("Cargo.toml").is_file())
        .collect::<Vec<_>>();
    match found.as_slice() {
        [directory] => Ok(directory.clone()),
        [] => Err(format!("no image `{image}` in {}", roots()).into()),
        _ => Err(format!("image `{image}` is in more than one of {}", roots()).into()),
    }
}

/// The package of the image `image`.
pub fn package(root: &Path, image: &str) -> Result<String> {
    Ok(manifest(root, image)?.package.name)
}

#[derive(Deserialize)]
struct Manifest {
    package: Package,
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default)]
    target: std::collections::BTreeMap<String, Target>,
}

#[derive(Deserialize)]
struct Target {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

fn manifest(root: &Path, image: &str) -> Result<Manifest> {
    let path = directory(root, image)?.join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("no image `{image}`: {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()).into())
}

/// The features the build gives `image`: the chip's feature of the
/// application entry when the image depends on it.
fn chip_features(manifest: &Manifest, chip: &str) -> Vec<String> {
    if manifest.dependencies.contains_key(ENTRY)
        || manifest
            .target
            .values()
            .any(|target| target.dependencies.contains_key(ENTRY))
    {
        vec![format!("{ENTRY}/{chip}")]
    } else {
        Vec::new()
    }
}

/// The pipeline's spec of `image`, with the caller's enforcement policy. Every check runs.
pub fn spec(root: &Path, image: &str, enforced: Enforced) -> Result<ImageSpec> {
    let profile = oer_tools_chip_profile::Profile::load(root, CHIP)?;
    let manifest = manifest(root, image)?;
    let features = chip_features(&manifest, CHIP);
    let package = manifest.package.name;
    Ok(ImageSpec {
        purpose: Purpose::Flash,
        root: root.to_owned(),
        chip: CHIP.to_owned(),
        application: Application {
            binary: package.clone(),
            package,
            features,
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
    use oer_tools_image_checks::Check;

    /// A repository with the chip facts and image manifests at `images`.
    fn repository(images: &[&str]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in oer_chips_family_facts::tracked::FILES {
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
            "hil/probes/esp32s31-boot-smoke",
            "hil/images/aes",
            "hil/probes/esp32s31-radio",
        ]);
        for (image, package) in [
            ("esp32s31-boot-smoke", "oer-hil-probes-esp32s31-boot-smoke"),
            ("hil-aes", "oer-hil-images-aes"),
            ("esp32s31-radio", "oer-hil-probes-esp32s31-radio"),
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
        let directory = repository(&["examples/twin", "hil/probes/twin"]);
        let twin = package(directory.path(), "twin").unwrap_err().to_string();
        assert!(twin.contains("more than one"), "{twin}");
        let none = package(directory.path(), "absent").unwrap_err().to_string();
        assert!(none.contains("no image"), "{none}");
    }

    #[test]
    fn an_image_is_named_by_its_directory_and_never_by_a_path() {
        let directory = repository(&["hil/images/wifi-station"]);
        for path in [
            "../../hil/images/station",
            "hil/images/wifi-station",
            ".station",
            "",
        ] {
            let refused = package(directory.path(), path).unwrap_err().to_string();
            assert!(refused.contains("not an image name"), "{path}: {refused}");
        }
        assert_eq!(
            package(directory.path(), "hil-wifi-station").unwrap(),
            "oer-hil-images-wifi-station"
        );
        // A HIL image is named with its root's prefix, never bare.
        assert!(package(directory.path(), "wifi-station").is_err());
    }

    #[test]
    fn an_image_on_the_entry_gets_the_chip_feature_from_the_build() {
        let directory = repository(&["examples/plain"]);
        let manifest = directory.path().join("examples/entry/Cargo.toml");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            manifest,
            "[package]\nname = \"oer-examples-entry\"\n[dependencies]\noer = { path = \"../../oer\" }\n",
        )
        .unwrap();
        let entry = spec(directory.path(), "entry", Enforced::Reported).unwrap();
        assert_eq!(entry.application.features, [format!("oer/{CHIP}")]);
        let targeted = directory.path().join("examples/targeted/Cargo.toml");
        std::fs::create_dir_all(targeted.parent().unwrap()).unwrap();
        std::fs::write(
            targeted,
            "[package]\nname = \"oer-examples-targeted\"\n[target.'cfg(target_arch = \"riscv32\")'.dependencies]\noer = { path = \"../../oer\" }\n",
        )
        .unwrap();
        let targeted = spec(directory.path(), "targeted", Enforced::Reported).unwrap();
        assert_eq!(targeted.application.features, [format!("oer/{CHIP}")]);
        let plain = spec(directory.path(), "plain", Enforced::Reported).unwrap();
        assert!(plain.application.features.is_empty());
    }

    #[test]
    fn a_saved_bundle_cannot_flash_a_removed_application() {
        let directory = repository(&[]);
        let profile = oer_tools_chip_profile::Profile::load(directory.path(), CHIP).unwrap();
        for image in ["retired-wifi-station", "retired-bluetooth-controller"] {
            assert!(package(directory.path(), image).is_err());
            let output = output(directory.path(), image);
            let staging = oer_tools_image_bundle::staging_directory(&output).unwrap();
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
