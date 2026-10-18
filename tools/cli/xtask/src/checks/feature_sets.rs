//! Tests of each package under the feature sets it declares.
//!
//! A package whose tests exercise code behind optional features declares the
//! combinations in `open-radio.test-feature-sets`; `check changed` tests a
//! changed package with each and CI tests every package with each, so the
//! list lives next to the package rather than in a workflow.

use crate::Result;
use oer_process as process;
use oer_process::Checkout;

/// Test the root-workspace package `name` with each of its test feature
/// `sets`.
pub fn test(ctx: &Checkout, name: &str, sets: &[String]) -> Result<()> {
    for features in sets {
        println!("feature sets: testing {name} with {features}");
        process::run(oer_toolchain::cargo_in(&ctx.root).args([
            "test",
            "--locked",
            "--no-fail-fast",
            "-p",
            name,
            "--features",
            features,
        ]))?;
    }
    Ok(())
}

/// Test every package of the root workspace with its declared feature sets.
pub fn run(ctx: &Checkout) -> Result<()> {
    let model = super::common::model(ctx)?;
    for package in model.members("Cargo.toml") {
        let class = model.classification(package)?;
        test(ctx, &package.name, &class.test_feature_sets)?;
    }
    Ok(())
}
