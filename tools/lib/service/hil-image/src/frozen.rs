//! Build images from a frozen source snapshot.

use std::path::{Path, PathBuf};

use oer_durable::atomic_json;
use oer_hil_image_class::NETWORK;
use oer_hil_schema::image::{FeatureDelta, ImageClass};
use oer_hil_source_snapshot::FrozenSources;
use oer_image::Overrides;

use crate::{Artifacts, LayoutSeed, Result, built, chip_build_root, seed_suffix, spec};

/// The base of the host's build slots for `chip` images (`<base>-<n>`).
pub fn build_slots(chip: &str) -> Result<PathBuf> {
    Ok(chip_build_root(chip)?.join("source-build"))
}

/// Build `class` for `chip` from `frozen` through the image pipeline. The snapshot's own esp-hal, Embassy and Xarxa
/// sources, when it holds them, replace the pinned ones.
pub fn build(
    frozen: &FrozenSources,
    chip: &str,
    class: ImageClass,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    frozen.verify_unchanged()?;
    let directory = format!(
        "{}-{NETWORK}{}{}",
        class.id(),
        seed_suffix(layout_seed),
        features.suffix()
    );
    let output = chip_build_root(chip)?
        .join("snapshot-builds")
        .join(frozen.snapshot().id())
        .join(directory);
    let spec = spec(
        &frozen.repository(),
        chip,
        class,
        (layout_seed, features),
        Overrides {
            esp_hal: frozen.source_root("esp-hal"),
        },
        output.clone(),
    )?;
    let artifacts = built(&spec, features)?;
    finish(frozen, &output)?;
    Ok(artifacts)
}

/// Confirm the checkout did not change during the build and record which
/// snapshot the output came from.
fn finish(frozen: &FrozenSources, output: &Path) -> Result<()> {
    frozen.verify_unchanged()?;
    atomic_json(&output.join("source-snapshot.json"), frozen.snapshot())?;
    Ok(())
}

/// Build `class` for the chip of `session` from the sources it bound.
pub fn build_for_run(
    session: &oer_hil_run_bundle::run::RunSession,
    class: ImageClass,
    current: crate::CurrentBuild,
) -> Result<Artifacts> {
    let frozen = session
        .frozen_sources()
        .ok_or("HIL build requires a bound source snapshot")?;
    build(
        frozen,
        session.target(),
        class,
        current.layout_seed,
        &current.features,
    )
}
