//! `cargo fw hot-code`: an image's hot-code list from a PC profile
//! ([`oer_tools_image_checks::linker::hot`]).

use std::path::Path;

use oer_tools_image_build::Result;
use oer_tools_image_checks::linker::hot::{HotCode, profiled_functions};
use serde_json::Value;

/// Write the list of the functions of `elf` that hold the most samples of
/// the `measurements`' profiles in `results` (a scenario's results file),
/// every hart's ranked together, within `budget_bytes`, to `output`. Prints
/// the listed functions.
pub fn write(
    results: &Path,
    measurements: &[String],
    elf: &Path,
    budget_bytes: u64,
    output: &Path,
) -> Result<()> {
    let document: Value = serde_json::from_slice(&std::fs::read(results)?)
        .map_err(|error| format!("{}: {error}", results.display()))?;
    let mut pcs = Vec::new();
    let mut sampled = Vec::new();
    for measurement in measurements {
        let profile = profile_pcs(&document, measurement)?;
        sampled.push(format!("{measurement}, {} samples", profile.len()));
        pcs.extend(profile);
    }
    let bytes = std::fs::read(elf)?;
    let functions = profiled_functions(&oer_tools_analysis_elf::Elf::parse(&bytes)?)?;
    let list = HotCode::from_samples(&functions, pcs.iter().copied(), budget_bytes);
    let name = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let origin = format!(
        "Written by `cargo fw hot-code` from {} ({}) and its {}:\n\
         the cached or already moved functions with the most samples per byte within the budget,\n\
         densest first.\n\
         The image linker moves them into SRAM; regenerate after the hot path changes.",
        name(results),
        sampled.join("; "),
        name(elf),
    );
    std::fs::write(output, list.render(&origin)?)?;
    for function in &list.functions {
        println!("{function}");
    }
    eprintln!(
        "fw: {} functions within {budget_bytes} bytes written to {}",
        list.functions.len(),
        output.display()
    );
    Ok(())
}

/// The profiles of a measurement: hart 0's, which a sampled measurement
/// always has, and hart 1's where the image samples hart 1.
const PROFILES: [&str; 2] = ["/value/profile/samples", "/value/profile_hart1/samples"];

/// The sampled PCs of `measurement`'s profiles, every hart's: each entry of
/// a profile's `samples` is `[tick, pc, level, ra]`.
fn profile_pcs(document: &Value, measurement: &str) -> Result<Vec<u32>> {
    let entry = document
        .get("measurements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(measurement))
        .ok_or_else(|| format!("the results have no measurement `{measurement}`"))?;
    let [hart0, hart1] = PROFILES.map(|pointer| entry.pointer(pointer).and_then(Value::as_array));
    let hart0 =
        hart0.ok_or_else(|| format!("measurement `{measurement}` has no profile samples"))?;
    hart0
        .iter()
        .chain(hart1.into_iter().flatten())
        .map(|sample| {
            sample
                .get(1)
                .and_then(Value::as_u64)
                .and_then(|pc| u32::try_from(pc).ok())
                .ok_or_else(|| {
                    format!("a sample of `{measurement}` has no 32-bit PC: {sample}").into()
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measurement_profile_yields_its_sampled_pcs() {
        let document = serde_json::json!({ "measurements": [
            { "name": "rx 10 Mbit/s", "value": { "profile": { "samples": [[0, 1, 0, null]] } } },
            { "name": "rx 65 Mbit/s", "value": { "profile": { "samples": [
                [0, 1342177280_u64, 0, 1],
                [2048, 1342177284_u64, 63, null]
            ] } } },
            { "name": "idle", "value": {} },
        ] });
        assert_eq!(
            profile_pcs(&document, "rx 65 Mbit/s").unwrap(),
            [0x5000_0000, 0x5000_0004]
        );
        assert!(profile_pcs(&document, "idle").is_err());
        assert!(profile_pcs(&document, "missing").is_err());
        let wide = serde_json::json!({ "measurements": [
            { "name": "x", "value": { "profile": { "samples": [[0, 4294967296_u64, 0, null]] } } }
        ] });
        assert!(profile_pcs(&wide, "x").is_err());
    }

    #[test]
    fn hart_1_s_samples_rank_beside_hart_0_s() {
        let document = serde_json::json!({ "measurements": [
            { "name": "tx 100 Mbit/s", "value": {
                "profile": { "samples": [[0, 1342177280_u64, 0, null]] },
                "profile_hart1": { "samples": [[0, 1342177288_u64, 0, null]] }
            } },
            { "name": "hart 1 only", "value": {
                "profile_hart1": { "samples": [[0, 1342177288_u64, 0, null]] }
            } },
        ] });
        assert_eq!(
            profile_pcs(&document, "tx 100 Mbit/s").unwrap(),
            [0x5000_0000, 0x5000_0008]
        );
        assert!(profile_pcs(&document, "hart 1 only").is_err());
    }
}
