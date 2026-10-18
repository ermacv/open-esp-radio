//! The receipt of one prepared observer: the executable's digest, its
//! embedded build with Cargo's actual compilation units applied, those units
//! and the selected profile.
//!
//! The producer writes it beside the runner's private copy and publishes the
//! newest as the checkout's [`CURRENT`] descriptor; an invocation of the
//! runner names its own in [`ENV`]. The runner [`bind`]s its embedded build
//! to the receipt, and the evaluator reads [`CURRENT`] (or the receipt `ENV`
//! names) as the observer every current observation is compared with.
use std::path::{Path, PathBuf};

use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Names the receipt of the runner invocation it is set for.
pub const ENV: &str = "OER_OBSERVER_RECEIPT";

/// The checkout's current observer descriptor, relative to its root.
pub const CURRENT: &str = "target/hil/current-observer.json";

/// The receipt an invocation names, or else the checkout's current one.
pub fn selected(root: &Path) -> PathBuf {
    std::env::var_os(ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(CURRENT))
}

/// Bind the running executable's embedded `build` to the receipt [`ENV`]
/// names, when it names one: the receipt must identify this executable
/// (`executable_sha256`) and this embedded build, and then supplies the
/// compilation units and profile the embedded record cannot know.
pub fn bind(build: &mut Value, executable_sha256: Option<&str>) -> Result<()> {
    let Some(path) = std::env::var_os(ENV) else {
        return Ok(());
    };
    let receipt: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    if receipt["executable_sha256"].as_str() != executable_sha256 {
        return Err("observer receipt does not identify the running executable".into());
    }
    let mut embedded = receipt["build"].clone();
    embedded["resolved"] = build["resolved"].clone();
    if embedded != *build {
        return Err("observer receipt does not identify the embedded build".into());
    }
    super::artifacts::apply(
        &mut build["resolved"],
        receipt["artifacts"]
            .as_array()
            .ok_or("observer artifacts missing")?,
    )?;
    build["resolved"]["selected_profile"] = receipt["profile"].clone();
    if *build != receipt["build"] {
        return Err("invalid observer compilation receipt".into());
    }
    Ok(())
}
