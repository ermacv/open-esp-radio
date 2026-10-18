//! `cargo metadata` of a workspace, as `cargo xtask doc` reads it.

use crate::{Result, graph};
use oer_process as process;
use oer_process::Checkout;
use serde_json::Value;
use std::path::Path;

fn document(
    context: &Checkout,
    manifest: &Path,
    features: &[String],
    target: Option<&str>,
    locked: bool,
    no_deps: bool,
) -> Result<Value> {
    let mut command = oer_toolchain::cargo_in(&context.root);
    command
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .args(features);
    command.arg("--offline");
    if locked {
        command.arg("--locked");
    }
    if let Some(target) = target {
        command.args(["--filter-platform", target]);
    }
    if no_deps {
        command.arg("--no-deps");
    }
    Ok(serde_json::from_slice(
        &process::capture(&mut command)?.stdout,
    )?)
}

pub fn metadata_no_deps(context: &Checkout, manifest: &Path) -> Result<cargo_metadata::Metadata> {
    let value = document(context, manifest, &[], None, true, true)?;
    graph::validate_packages(&value)?;
    Ok(serde_json::from_value(value)?)
}
