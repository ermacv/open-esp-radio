//! The refusal of host-model support in an image: the feature host models
//! enable would give an image the synthetic addresses and placement of a
//! host model.

use std::path::Path;

use crate::Result;

/// The feature host models enable.
pub const TEST_SUPPORT: &str = "test-support";

/// The packages of `tree` (`cargo tree --format '{p}|{f}'`) that enable
/// [`TEST_SUPPORT`].
fn packages(tree: &str) -> Vec<String> {
    let mut packages = tree
        .lines()
        .filter_map(|line| line.split_once('|'))
        .filter(|(_, features)| {
            features
                .split(',')
                .any(|feature| feature.trim() == TEST_SUPPORT)
        })
        .map(|(package, _)| package.split(" (").next().unwrap_or(package).to_owned())
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

/// Every package of `tree` that enables test support: no image may link one.
fn forbidden_packages(tree: &str) -> Vec<String> {
    packages(tree)
}

/// Refuse the image `package` for `target` at `root` when its dependencies
/// enable [`TEST_SUPPORT`].
pub fn refuse(root: &Path, package: &str, target: &str) -> Result<()> {
    let output = crate::cargo::command()
        .current_dir(root)
        .args([
            "tree", "--locked", "-p", package, "--target", target, "-e", "normal", "--prefix",
            "none", "--format", "{p}|{f}",
        ])
        .output()
        .map_err(|error| format!("run cargo tree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree -p {package} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let packages = forbidden_packages(&String::from_utf8_lossy(&output.stdout));
    if !packages.is_empty() {
        return Err(format!(
            "image {package} enables `{TEST_SUPPORT}` in {}: host-model support never goes into an image",
            packages.join(", ")
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_image_may_link_test_support_cases() {
        let tree = "oer-ieee80211-port v0.1.0 (/port)|test-support\noer-esp32s31-hal v0.1.0 (/hal)|test-support\n";
        assert_eq!(
            super::forbidden_packages(tree),
            ["oer-esp32s31-hal v0.1.0", "oer-ieee80211-port v0.1.0"]
        );
    }
    #[test]
    fn an_image_with_test_support_is_named() {
        let tree = "oer-firmware-apps-boot-smoke v0.1.0 (/boot-smoke)|\noer-esp32s31-hal v0.1.0 (/hal)|test-support\noer-memory v0.1.0 (/m)|serde,test-support\nbytemuck v1.0.0|derive\n";
        assert_eq!(
            super::packages(tree),
            ["oer-esp32s31-hal v0.1.0", "oer-memory v0.1.0"]
        );
        assert!(super::packages("a v1|derive\n").is_empty());
    }
}
