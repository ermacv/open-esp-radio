//! A repetition's observed interrupt-stack use against the image's static
//! bound.
//!
//! Every image build of a chip with an interrupt contract proves a bound on
//! each hart's interrupt stack (`interrupt-stack gate`); a device watermark above it means the analysis
//! missed a path, so the repetition fails. The bound is recomputed from the
//! run's archived runtime ELF by the current analyzer, so a replayed image
//! is checked too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;

/// Each hart's bound in bytes, by hart; `None` for a hart the analysis left
/// `partial + ?` (a diagnostic image), which no observation can be held to.
type Bounds = BTreeMap<u32, Option<u64>>;

/// A watermark's capacity and free bytes, as far as recorded.
#[derive(Default)]
struct Watermark {
    capacity: Option<u64>,
    free: Option<u64>,
}

/// Each hart's peak observed interrupt-stack use in `measurements`: the
/// largest `capacity - free` of its `stack.cpuN-irq` watermarks.
pub(crate) fn observed_peaks(measurements: &[Measurement]) -> BTreeMap<u32, u64> {
    let mut values: BTreeMap<(&str, u32), Watermark> = BTreeMap::new();
    for measurement in measurements {
        let Some((prefix, rest)) = measurement.name.rsplit_once(".stack.cpu") else {
            continue;
        };
        let Some((core, field)) = rest.split_once("-irq.") else {
            continue;
        };
        let Ok(core) = core.parse::<u32>() else {
            continue;
        };
        let entry = values.entry((prefix, core)).or_default();
        match field {
            "capacity" => entry.capacity = Some(measurement.value),
            "free" => entry.free = Some(measurement.value),
            _ => {}
        }
    }
    let mut peaks = BTreeMap::new();
    for ((_, core), watermark) in values {
        if let (Some(capacity), Some(free)) = (watermark.capacity, watermark.free) {
            let used = capacity.saturating_sub(free);
            let peak = peaks.entry(core).or_insert(0);
            *peak = (*peak).max(used);
        }
    }
    peaks
}

/// The `stack.cpuN-irq.used` of each observed hart: evaluated against its
/// bound, or only observed where the bound is `partial + ?`; an error names
/// a hart the image's analysis does not know.
pub(crate) fn evaluate(
    peaks: &BTreeMap<u32, u64>,
    bounds: &Bounds,
) -> Result<Vec<Measurement>, String> {
    peaks
        .iter()
        .map(|(&core, &used)| {
            let bound = bounds.get(&core).ok_or_else(|| {
                format!("hart {core}'s interrupt stack was observed but the image has no such hart")
            })?;
            let used = Measurement::observed(
                format!("stack.cpu{core}-irq.used"),
                used,
                MeasurementUnit::Bytes,
            );
            Ok(match bound {
                Some(bound) => used.evaluated(Comparison::AtMost, *bound),
                None => used,
            })
        })
        .collect()
}

/// Each hart's static interrupt-stack bound of `chip`'s runtime ELF archived
/// at `elf`, computed once per ELF.
pub(crate) fn bounds(chip: &str, elf: &Path) -> Result<Bounds, String> {
    static BOUNDS: Mutex<BTreeMap<PathBuf, Result<Bounds, String>>> = Mutex::new(BTreeMap::new());
    let mut cache = BOUNDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(elf.to_owned())
        .or_insert_with(|| {
            let root = oer_process::built_root();
            let stacks = oer_chip_profile::Profile::load(&root, chip)
                .and_then(|profile| {
                    let bytes = oer_hil_run_bundle_format::archived::read_archived(elf)
                        .map_err(|error| error.to_string())?;
                    oer_image_check_interrupts::interrupt_stacks(&root, &profile, bytes)
                })
                .map_err(|error| format!("interrupt-stack bound of {}: {error}", elf.display()))?;
            stacks
                .harts
                .iter()
                .map(|hart| Ok((hart.core, hart.bytes)))
                .collect()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_run_bundle_format::run::MeasurementVerdict;

    fn bytes(name: &str, value: u64) -> Measurement {
        Measurement::observed(name, value, MeasurementUnit::Bytes)
    }

    #[test]
    fn a_hart_s_peak_is_its_largest_watermark_use() {
        let measurements = [
            bytes("target.a.request-1.stack.cpu0-irq.capacity", 32768),
            bytes("target.a.request-1.stack.cpu0-irq.free", 30000),
            bytes("target.a.request-1.stack.cpu0-irq.minimum-free", 4096),
            bytes("target.a.session-2.stack.cpu0-irq.capacity", 32768),
            bytes("target.a.session-2.stack.cpu0-irq.free", 29000),
            bytes("target.a.request-1.stack.cpu1-irq.capacity", 32768),
            bytes("target.a.request-1.stack.cpu1-irq.free", 31768),
            // A task stack is not an interrupt stack.
            bytes("target.a.request-1.stack.cpu0.capacity", 8192),
            bytes("target.a.request-1.stack.cpu0.free", 10),
        ];
        assert_eq!(
            observed_peaks(&measurements),
            BTreeMap::from([(0, 3768), (1, 1000)])
        );
    }

    #[test]
    fn a_use_above_the_static_bound_fails_and_an_unbounded_hart_is_an_error() {
        let peaks = BTreeMap::from([(0, 3768), (1, 1000)]);
        let evaluated =
            evaluate(&peaks, &BTreeMap::from([(0, Some(3500)), (1, Some(1000))])).unwrap();
        let verdicts: Vec<_> = evaluated
            .iter()
            .map(|measurement| (measurement.name.as_str(), measurement.verdict))
            .collect();
        assert_eq!(
            verdicts,
            [
                ("stack.cpu0-irq.used", Some(MeasurementVerdict::Failed)),
                ("stack.cpu1-irq.used", Some(MeasurementVerdict::Passed)),
            ]
        );
        assert!(evaluate(&peaks, &BTreeMap::from([(0, Some(4000))])).is_err());
        // A `partial + ?` hart is observed, never held to a number.
        let observed = evaluate(&peaks, &BTreeMap::from([(0, None), (1, Some(1000))])).unwrap();
        assert_eq!(observed[0].threshold, None);
        assert_eq!(observed[0].verdict, None);
    }
}
