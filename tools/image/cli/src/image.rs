//! Building an image through the image pipeline and printing its
//! analysis reports; where images live and what their bundle is belong to
//! `oer_image_build::images`.

use std::path::Path;

pub use oer_image_build::images::{CHIP, bundle, package};
use oer_image_build::{Enforced, Result, images};
use oer_image_bundle::ImageBundle;

/// Build `image` into its bundle.
pub fn build(
    root: &Path,
    image: &str,
    enforced: Enforced,
    binary: Option<&str>,
    features: &[String],
) -> Result<ImageBundle> {
    let bundle = oer_image_build::build(&specification(root, image, enforced, binary, features)?)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    write_analysis_reports(&bundle, &mut std::io::stderr().lock())?;
    Ok(bundle)
}

/// The pipeline's spec of `image`: its package's `binary` instead of the
/// default one, and its default features plus `features`.
fn specification(
    root: &Path,
    image: &str,
    enforced: Enforced,
    binary: Option<&str>,
    features: &[String],
) -> Result<oer_image_build::ImageSpec> {
    let mut specification = images::spec(root, image, enforced)?;
    if let Some(binary) = binary {
        specification.application.binary = binary.to_owned();
    }
    specification.application.features = features.to_vec();
    Ok(specification)
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

#[cfg(test)]
mod tests {
    use super::*;
    use oer_image_build::images::output;
    use oer_image_bundle::Purpose;

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in oer_espressif_facts::tracked::FILES {
            let path = directory.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        for image in ["boot-smoke", "radio-root"] {
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
    fn the_features_join_the_package_defaults() {
        let directory = repository();
        let features = ["station".to_owned()];
        let spec = specification(
            directory.path(),
            "boot-smoke",
            Enforced::Reported,
            None,
            &features,
        )
        .unwrap();
        assert_eq!(spec.application.features, features);
        assert!(spec.application.default_features);
        let plain = specification(
            directory.path(),
            "boot-smoke",
            Enforced::Reported,
            None,
            &[],
        )
        .unwrap();
        assert!(plain.application.features.is_empty());
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
