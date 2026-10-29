//! Building an image of `firmware/apps` through the image pipeline: its
//! bundle lies in `target/fw/<image>/`,
//! described by its `bundle.json`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use oer_image_build::{Application, Gates, ImageSpec, Result};
use oer_image_bundle::{ImageBundle, Purpose};
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

/// Build `image` into its bundle.
pub fn build(
    root: &Path,
    image: &str,
    enforced: Enforced,
    binary: Option<&str>,
) -> Result<ImageBundle> {
    let mut specification = spec(root, image, enforced)?;
    if let Some(binary) = binary {
        specification.application.binary = binary.to_owned();
    }
    let bundle = oer_image_build::build(&specification)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    write_analysis_reports(&bundle, &mut std::io::stderr().lock())?;
    Ok(bundle)
}

/// Print the frame and panic inventories, including every reported location.
fn write_analysis_reports(bundle: &ImageBundle, output: &mut impl std::io::Write) -> Result<()> {
    use oer_image_checks::{panics, stack::frames};

    for report in &bundle.reports {
        if [
            frames::RUNTIME_REPORT,
            frames::BOOTSTRAP_REPORT,
            panics::RUNTIME_REPORT,
            panics::BOOTSTRAP_REPORT,
        ]
        .contains(&report.as_str())
        {
            writeln!(
                output,
                "fw: {report}\n{}",
                std::fs::read_to_string(bundle.path(report))?
            )?;
        }
    }
    Ok(())
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

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in oer_espressif_facts::tracked::FILES {
            let path = directory.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        for image in [
            "boot-smoke",
            "radio-root",
            "../hil/esp32s31",
            "../probes/esp32s31-radio",
        ] {
            let manifest = directory
                .path()
                .join("firmware/apps")
                .join(image)
                .join("Cargo.toml");
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            let suffix = image
                .strip_prefix("../")
                .map_or_else(|| format!("apps-{image}"), |path| path.replace('/', "-"));
            std::fs::write(
                manifest,
                format!("[package]\nname = \"oer-firmware-{suffix}\"\n"),
            )
            .unwrap();
        }
        directory
    }

    #[test]
    fn remaining_images_keep_the_pipeline_checks_and_configuration() {
        let directory = repository();
        for image in [
            "boot-smoke",
            "radio-root",
            "../hil/esp32s31",
            "../probes/esp32s31-radio",
        ] {
            for enforced in [
                Enforced::Only([Check::Interrupts].into()),
                Enforced::Reported,
            ] {
                let spec = spec(directory.path(), image, enforced.clone()).unwrap();
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
    fn a_saved_bundle_cannot_flash_a_removed_application() {
        let directory = repository();
        let profile = oer_chip_profile::Profile::load(directory.path(), CHIP).unwrap();
        for image in ["w0", "w1", "w2", "w3", "b0"] {
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

    #[test]
    fn frame_and_panic_reports_print_locations_for_both_elfs() {
        let directory = repository();
        let profile = oer_chip_profile::Profile::load(directory.path(), CHIP).unwrap();
        let mut bundle = ImageBundle::new(
            directory.path(),
            &profile,
            profile.facts.flash.clone().unwrap(),
        );
        for (name, text) in [
            ("runtime-frames.txt", "4097\tover limit\truntime::large"),
            ("bootstrap-frames.txt", "8192\tover limit\tboot::large"),
            (
                "runtime-panics.txt",
                "hal\t1\t0\ncompiler-async-fn-resumed\t2\texcluded\nHAL::call at 0x00001024 calls panic",
            ),
            (
                "bootstrap-panics.txt",
                "unnamed\t1\t0\ncompiler-async-fn-resumed\t0\texcluded\n_start at 0x00001028 calls panic",
            ),
        ] {
            std::fs::write(bundle.path(name), text).unwrap();
            bundle.reports.push(name.into());
        }
        // Unrelated reports are handled by their own output paths.
        bundle.reports.push("placement.txt".into());
        let mut output = Vec::new();
        write_analysis_reports(&bundle, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        for expected in [
            "runtime::large",
            "boot::large",
            "hal\t1\t0",
            "unnamed\t1\t0",
            "compiler-async-fn-resumed\t2\texcluded",
            "compiler-async-fn-resumed\t0\texcluded",
            "0x00001024",
            "0x00001028",
        ] {
            assert!(text.contains(expected), "{text}");
        }
    }

    #[test]
    fn flash_loads_reported_failures_without_enforcing_checks() {
        let directory = repository();
        let root = directory.path();
        let profile = oer_chip_profile::Profile::load(root, CHIP).unwrap();
        let output = output(root, "boot-smoke");
        let staging = oer_image_bundle::staging_directory(&output).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        let mut built = ImageBundle::new(&staging, &profile, profile.facts.flash.clone().unwrap());
        for segment in built.segments() {
            std::fs::write(segment.file, b"fixture").unwrap();
        }
        built.checks.push(oer_image_bundle::CheckRecord {
            elf: oer_image_bundle::CheckedElf::Runtime,
            check: "interrupts".into(),
            result: oer_image_bundle::CheckResult::Failed("no bound".into()),
        });
        let published = built.publish(&output).unwrap();
        let loaded = bundle(root, "boot-smoke").unwrap();
        assert_eq!(loaded.checks, published.checks);
        assert!(loaded.snapshot().is_ok());
    }

    #[test]
    fn flash_and_run_cannot_write_an_analysis_bundle_in_the_normal_directory() {
        let directory = repository();
        let root = directory.path();
        let profile = oer_chip_profile::Profile::load(root, CHIP).unwrap();
        let output = output(root, "radio-root");
        let staging = oer_image_bundle::staging_directory(&output).unwrap();
        for purpose in [Purpose::Analysis, Purpose::Flash] {
            std::fs::create_dir_all(&staging).unwrap();
            let mut built =
                ImageBundle::new(&staging, &profile, profile.facts.flash.clone().unwrap());
            built.purpose = purpose;
            for segment in built.segments() {
                std::fs::write(segment.file, b"fixture").unwrap();
            }
            built.publish(&output).unwrap();
            // Both commands' write_image uses this loader, then the common
            // board writer requests a flash snapshot before writing.
            let loaded = bundle(root, "radio-root").unwrap();
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
}
