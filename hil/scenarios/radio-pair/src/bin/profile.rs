//! `radio-pair-profile <results> <runtime.elf>`: the receiver's
//! registration profile (`tasks/448`) from a results file of the radio
//! pair, named with the image's ELF. It prints one JSON object: the
//! registration's time, waits and polls, the cache's counts, the sampler's
//! own share, and the samples split by level, by function (outermost and
//! innermost inlined), by memory (internal SRAM, PSRAM, flash, ROM by the chip's map) and
//! the share inside memory copy entries. Exit 0, or 2 when an input cannot
//! be read.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

use std::{
    collections::{BTreeMap, HashMap},
    process::ExitCode,
};

use serde_json::{Value, json};

/// The most names each table prints.
const TOP: usize = 40;
/// The chip's map (`chips/esp32s31/facts/chip.toml` `[memory]`): the
/// internal SRAM from `0x2f00_0000`, 512 KiB; the ROM from `0x2f80_0000`
/// to `0x2f85_0000`; flash XIP from `0x4000_0000`, 64 MiB; PSRAM from
/// `psram-origin` `0x5000_0000`. The facts give PSRAM's origin only: its
/// end here is the aperture's next 256 MiB, past any module's PSRAM.
const SRAM: core::ops::Range<u32> = 0x2f00_0000..0x2f08_0000;
const ROM: core::ops::Range<u32> = 0x2f80_0000..0x2f85_0000;
const FLASH: core::ops::Range<u32> = 0x4000_0000..0x4400_0000;
const PSRAM: core::ops::Range<u32> = 0x5000_0000..0x6000_0000;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [results, elf] = arguments.as_slice() else {
        eprintln!("usage: radio-pair-profile <results> <runtime.elf>");
        return ExitCode::from(2);
    };
    match run(results, elf) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(results: &str, elf: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(results).map_err(|error| format!("{results}: {error}"))?;
    let results: Value =
        serde_json::from_str(&text).map_err(|error| format!("{results}: {error}"))?;
    let measured = |name: &str| -> Result<Value, String> {
        results
            .get("measurements")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|measurement| measurement.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|measurement| measurement.get("value").cloned())
            .ok_or_else(|| format!("no measurement {name}"))
    };
    let summary = measured("receiver/register-profile")?;
    let samples = samples_of(&measured("receiver/register-samples")?)?;
    let symbolizer = oer_tools_analysis_elf::dwarf::Symbolizer::read(std::path::Path::new(elf))
        .map_err(|error| format!("{elf}: {error}"))?;
    let mut places = HashMap::new();
    let addresses = samples
        .iter()
        .flat_map(|sample| [Some(sample.pc), sample.ra.and_then(|ra| ra.checked_sub(1))])
        .flatten();
    for address in addresses {
        if let std::collections::hash_map::Entry::Vacant(entry) = places.entry(address) {
            let frames = symbolizer
                .frames(u64::from(address))
                .map_err(|error| format!("{address:#x}: {error}"))?;
            entry.insert(place(&frames));
        }
    }
    Ok(report(
        &samples,
        &places,
        &summary,
        measured("receiver/register-micros").ok(),
        measured("receiver/register-waits").ok(),
        elf,
    ))
}

/// One kept sample: its program counter, raw level byte and, in a trace
/// build, its interrupted return address.
#[derive(Clone, Copy, Debug)]
struct Sample {
    pc: u32,
    level: u8,
    ra: Option<u32>,
}

/// The samples of a results file: `[pc, level]` or `[pc, level, ra]`.
fn samples_of(value: &Value) -> Result<Vec<Sample>, String> {
    value
        .as_array()
        .ok_or("the samples are no array")?
        .iter()
        .map(|sample| {
            let field = |index: usize| sample.get(index).and_then(Value::as_u64);
            let pc = field(0).and_then(|pc| u32::try_from(pc).ok());
            let level = field(1).and_then(|level| u8::try_from(level).ok());
            let ra = field(2).and_then(|ra| u32::try_from(ra).ok());
            match (pc, level) {
                (Some(pc), Some(level)) => Ok(Sample { pc, level, ra }),
                _ => Err(format!("a sample: {sample}")),
            }
        })
        .collect()
}

/// Where one address lies: the function its code is in and the innermost
/// function inlined there.
#[derive(Clone, Debug, Default)]
struct Place {
    function: String,
    inlined: String,
}

fn place(frames: &[oer_tools_analysis_elf::dwarf::Frame]) -> Place {
    let name = |frame: Option<&oer_tools_analysis_elf::dwarf::Frame>| {
        frame
            .and_then(|frame| frame.function.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "?".to_owned())
    };
    Place {
        function: name(frames.last()),
        inlined: name(frames.first()),
    }
}

/// `counts` by count, most first, then by name.
fn sorted(counts: HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts
}

/// One more in `count`.
fn bump(count: &mut u64) {
    *count = count.saturating_add(1);
}

/// The memory `pc` runs from.
fn memory(pc: u32) -> &'static str {
    if SRAM.contains(&pc) {
        "internal sram"
    } else if PSRAM.contains(&pc) {
        "psram"
    } else if FLASH.contains(&pc) {
        "flash"
    } else if ROM.contains(&pc) {
        "rom"
    } else {
        "other"
    }
}

/// The level a raw `clicintctl`-form byte names under the family's three
/// level bits (`chips/family/facts/layers/family.toml`, level-bits = 3):
/// level `n` is `(n - 1) << 5 | 0x1f` (`oer_portable_riscv::clic`), and 0
/// is thread mode.
fn level_of(raw: u8) -> u8 {
    if raw == 0 {
        0
    } else {
        (raw >> 5).saturating_add(1)
    }
}

/// What the image's levels are (`esp32s31-rx-ring`'s table): thread mode,
/// 1 the application's executor and its handlers, 2 the radio's level
/// task, 3 the radio's alarm handler, 5 the sampler.
fn level_name(raw: u8) -> &'static str {
    match level_of(raw) {
        0 => "thread",
        1 => "application",
        2 => "radio task",
        3 => "radio alarm handler",
        5 => "sampler",
        _ => "other",
    }
}

fn report(
    samples: &[Sample],
    places: &HashMap<u32, Place>,
    summary: &Value,
    micros: Option<Value>,
    waits: Option<Value>,
    elf: &str,
) -> Value {
    let total = samples.len().max(1) as f64;
    let percent = |count: u64| (1000.0 * count as f64 / total).round() / 10.0;
    let place_of = |pc: u32| places.get(&pc).cloned().unwrap_or_default();
    let mut levels = BTreeMap::<u8, u64>::new();
    let mut memories = BTreeMap::<&str, u64>::new();
    let mut copying = 0_u64;
    for &Sample { pc, level, .. } in samples {
        bump(levels.entry(level).or_default());
        bump(memories.entry(memory(pc)).or_default());
        if oer_tools_image_checks::copies::copy_entry(&place_of(pc).function) {
            bump(&mut copying);
        }
    }
    // A copy's caller: the function the byte before its `ra` lies in.
    let mut callers = HashMap::<String, u64>::new();
    for sample in samples {
        if !oer_tools_image_checks::copies::copy_entry(&place_of(sample.pc).function) {
            continue;
        }
        let caller = sample
            .ra
            .and_then(|ra| ra.checked_sub(1))
            .map_or_else(|| "(no ra)".to_owned(), |call| place_of(call).function);
        bump(callers.entry(caller).or_default());
    }
    let table = |name: &dyn Fn(&Place) -> String, of_level: Option<u8>| {
        let mut counts = HashMap::<(String, &str), u64>::new();
        for &Sample { pc, level, .. } in samples {
            if of_level.is_some_and(|wanted| wanted != level) {
                continue;
            }
            bump(counts.entry((name(&place_of(pc)), memory(pc))).or_default());
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts
            .into_iter()
            .take(TOP)
            .map(|((name, memory), count)| {
                json!({ "name": name, "memory": memory, "samples": count, "percent": percent(count) })
            })
            .collect::<Vec<_>>()
    };
    let cycles = |key: &str| summary.get(key).and_then(Value::as_u64).unwrap_or(0);
    let handler_micros = cycles("handler_cycles")
        .checked_div(cycles("cycles_per_micro"))
        .unwrap_or(0);
    let register_micros = micros.as_ref().and_then(Value::as_u64).unwrap_or(0);
    json!({
        "elf": elf,
        "register_micros": micros,
        "register_waits": waits,
        "summary": summary,
        "samples": samples.len(),
        "sampler": {
            "entries": cycles("handler_entries"),
            "micros": handler_micros,
            "percent_of_registration": (register_micros > 0)
                .then(|| (1000.0 * handler_micros as f64 / register_micros as f64).round() / 10.0),
        },
        "levels": levels
            .iter()
            .map(|(level, count)| json!({
                "raw": level,
                "level": level_of(*level),
                "name": level_name(*level),
                "samples": count,
                "percent": percent(*count),
            }))
            .collect::<Vec<_>>(),
        "memory": memories
            .iter()
            .map(|(memory, count)| json!({ "memory": memory, "samples": count, "percent": percent(*count) }))
            .collect::<Vec<_>>(),
        "copies": {
            "samples": copying,
            "percent": percent(copying),
            "callers": sorted(callers)
                .into_iter()
                .map(|(caller, count)| json!({ "caller": caller, "samples": count, "percent": percent(count) }))
                .collect::<Vec<_>>(),
        },
        "functions": table(&|place| place.function.clone(), None),
        "inlined": table(&|place| place.inlined.clone(), None),
        "by_level": levels
            .keys()
            .map(|level| json!({
                "raw": level,
                "name": level_name(*level),
                "functions": table(&|place| place.function.clone(), Some(*level)),
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_level_byte_names_its_level() {
        assert_eq!(level_of(0), 0);
        assert_eq!(level_of(0x1f), 1);
        assert_eq!(level_of(0x3f), 2);
        assert_eq!(level_of(0x5f), 3);
        assert_eq!(level_of(0x9f), 5);
    }

    #[test]
    fn addresses_are_classed_by_the_chip_s_map() {
        assert_eq!(memory(0x2f00_0000), "internal sram");
        assert_eq!(memory(0x2f07_fffc), "internal sram");
        assert_eq!(memory(0x2f80_0010), "rom");
        assert_eq!(memory(0x4000_0000), "flash");
        assert_eq!(memory(0x43ff_fffc), "flash");
        assert_eq!(memory(0x5001_cc00), "psram");
        assert_eq!(memory(0x2f08_0000), "other");
    }

    #[test]
    fn the_report_splits_samples_by_level_memory_and_function() {
        let sample = |pc, level, ra| Sample { pc, level, ra };
        let samples = [
            sample(0x4000_0010, 63, None),
            sample(0x4000_0010, 63, None),
            sample(0x2f00_1000, 63, Some(0x4000_0011)),
            sample(0x2f00_1000, 95, None),
        ];
        let places = HashMap::from([
            (
                0x4000_0010,
                Place {
                    function: "step".to_owned(),
                    inlined: "read".to_owned(),
                },
            ),
            (
                0x2f00_1000,
                Place {
                    function: "memcpy".to_owned(),
                    inlined: "memcpy".to_owned(),
                },
            ),
        ]);
        let summary =
            json!({ "handler_cycles": 3_200, "handler_entries": 4, "cycles_per_micro": 320 });
        let report = report(
            &samples,
            &places,
            &summary,
            Some(json!(1_000)),
            None,
            "x.elf",
        );
        assert_eq!(report["levels"][0]["samples"], 3);
        assert_eq!(report["levels"][1]["name"], "radio alarm handler");
        assert_eq!(report["memory"][0]["memory"], "flash");
        assert_eq!(report["memory"][0]["samples"], 2);
        assert_eq!(report["memory"][1]["memory"], "internal sram");
        assert_eq!(report["memory"][1]["samples"], 2);
        assert_eq!(report["functions"][0]["name"], "memcpy");
        assert_eq!(report["functions"][0]["samples"], 2);
        assert_eq!(report["copies"]["samples"], 2);
        assert_eq!(report["sampler"]["micros"], 10);
        assert_eq!(report["sampler"]["percent_of_registration"], 1.0);
        assert_eq!(report["by_level"][1]["functions"][0]["name"], "memcpy");
        assert_eq!(report["levels"][0]["name"], "radio task");
        assert_eq!(report["copies"]["callers"][0]["caller"], "(no ra)");
        assert_eq!(report["copies"]["callers"][1]["caller"], "step");
    }
}
