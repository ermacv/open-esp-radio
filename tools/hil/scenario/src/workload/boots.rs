//! The per-boot loop of a workload: a fresh boot of the board under test
//! per step, each boot's typed evidence an observation.

use std::path::Path;

use oer_hil_link::SerialCapture;
use serde::Serialize;

use crate::workload::{Result, context::Context};

/// The directory of boot `boot` below `output`: `boot-NNN`.
pub fn boot_directory(output: &Path, boot: u8) -> std::path::PathBuf {
    output.join(format!("boot-{boot:03}"))
}

/// Run `observe` on `boots` fresh boots of the board under test, each in
/// its own capture in `boot-NNN` below `output`; record what each boot
/// returns as the observation `boot-NNN`, then let `accept` judge it. The
/// first failure, of a boot or of its acceptance, ends the loop; the
/// observations recorded before it stay in the repetition's results.
pub fn for_each_boot<T: Serialize>(
    context: &Context<'_>,
    output: &Path,
    boots: u8,
    mut observe: impl FnMut(u8, &SerialCapture) -> Result<T>,
    mut accept: impl FnMut(u8, &T) -> Result<()>,
) -> Result<Vec<T>> {
    std::fs::create_dir_all(output)?;
    let mut observed = Vec::with_capacity(usize::from(boots));
    for boot in 1..=boots {
        let directory = boot_directory(output, boot);
        let value = context.with_capture(&directory, |capture| observe(boot, capture))?;
        context.results.observe(format!("boot-{boot:03}"), &value);
        accept(boot, &value)?;
        observed.push(value);
    }
    Ok(observed)
}
