//! The image's PC samples of a level's window (`OER_WIFI_LOAD_PROFILE_US`):
//! collected page by page, named with the image's runtime ELF's DWARF
//! (`oer_analysis_elf::dwarf::Symbolizer`), and reported as function
//! histograms beside the raw samples. A sample inside a memory copy entry
//! (`memcpy`, `memmove`) with its interrupted `ra` is named after the
//! caller `ra` returns into: `memcpy ← caller`.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use oer_hil_harness::{Cx, Failure};
use oer_services_load::{
    CollectSamples, MAX_SAMPLE_PERIOD_US, MIN_SAMPLE_PERIOD_US, Profile, Sample, Samples,
    SamplesCollected,
};

/// The sampling period in microseconds; without it the image samples
/// nothing.
pub(crate) const PROFILE_ENV: &str = "OER_WIFI_LOAD_PROFILE_US";
/// The most functions each histogram names.
const TOP: usize = 40;

/// The profile each level asks for, from `OER_WIFI_LOAD_PROFILE_US`.
pub(crate) fn requested() -> Result<Option<Profile>, Failure> {
    match std::env::var(PROFILE_ENV) {
        Ok(text) => parse(&text).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(Failure::Incomplete(format!("{PROFILE_ENV}: {error}"))),
    }
}

fn parse(text: &str) -> Result<Profile, Failure> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|period| (MIN_SAMPLE_PERIOD_US..=MAX_SAMPLE_PERIOD_US).contains(period))
        .map(|period_us| Profile { period_us })
        .ok_or_else(|| {
            Failure::Incomplete(format!(
                "{PROFILE_ENV}=`{text}` is no period of {MIN_SAMPLE_PERIOD_US} to \
                 {MAX_SAMPLE_PERIOD_US} us"
            ))
        })
}

/// One run's samples: the last page's counts and every sample.
pub(crate) struct Collected {
    pub summary: Samples,
    pub samples: Vec<Sample>,
}

/// Every sample of `run`, page by page.
pub(crate) fn collect(
    cx: &mut Cx<'_>,
    run: u32,
    call: std::time::Duration,
) -> Result<Collected, Failure> {
    let mut samples = Vec::new();
    loop {
        let from = u16::try_from(samples.len())
            .map_err(|_| Failure::Fail("the image kept too many samples".to_owned()))?;
        match cx.call(CollectSamples { run, from }, call)? {
            SamplesCollected::Page(mut page) => {
                let last = page.samples.is_empty()
                    || samples.len() + page.samples.len() >= usize::from(page.kept);
                samples.extend(page.samples.iter().copied());
                if last {
                    page.samples.clear();
                    return Ok(Collected {
                        summary: page,
                        samples,
                    });
                }
            }
            other => return Err(Failure::Fail(format!("the image's samples: {other:?}"))),
        }
    }
}

/// The function a sample's address lies in, and the innermost function
/// inlined there.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Place {
    pub function: String,
    pub inlined: String,
}

/// Name every distinct address of `samples` with `elf`'s DWARF, and the
/// call before the return address of each sample in a copy entry.
pub(crate) fn symbolize(elf: &Path, samples: &[Sample]) -> Result<HashMap<u32, Place>, String> {
    let symbolizer =
        oer_analysis_elf::dwarf::Symbolizer::read(elf).map_err(|error| error.to_string())?;
    let mut places = HashMap::new();
    let name = |address: u32, places: &mut HashMap<u32, Place>| -> Result<(), String> {
        if let std::collections::hash_map::Entry::Vacant(entry) = places.entry(address) {
            let frames = symbolizer
                .frames(u64::from(address))
                .map_err(|error| error.to_string())?;
            entry.insert(place(&frames));
        }
        Ok(())
    };
    for sample in samples {
        name(sample.pc, &mut places)?;
    }
    for sample in samples {
        if let Some(call) = call_of(sample, &places) {
            name(call, &mut places)?;
        }
    }
    Ok(places)
}

/// For a sample in a copy entry with its `ra`, an address of the call
/// `ra` returns from: the byte before `ra`, inside the call instruction.
fn call_of(sample: &Sample, places: &HashMap<u32, Place>) -> Option<u32> {
    let place = places.get(&sample.pc)?;
    if !oer_image_checks::copies::copy_entry(&place.function) {
        return None;
    }
    sample.ra?.checked_sub(1)
}

/// The frames at one address run from the innermost inlined function out
/// to the function the code lies in.
fn place(frames: &[oer_analysis_elf::dwarf::Frame]) -> Place {
    let name = |frame: Option<&oer_analysis_elf::dwarf::Frame>| {
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

/// The run's profile: its counts, the samples by level, the [`TOP`]
/// functions and innermost inlined functions by samples, and every raw
/// sample as `[tick, pc, level, ra]` (`ra` null when the image recorded
/// none). `symbols` is the naming or why there is none.
pub(crate) fn report(
    collected: &Collected,
    elf: &Path,
    symbols: &Result<HashMap<u32, Place>, String>,
) -> serde_json::Value {
    let summary = &collected.summary;
    let samples = &collected.samples;
    let mut levels = BTreeMap::<u8, u64>::new();
    for sample in samples {
        *levels.entry(sample.level).or_default() += 1;
    }
    let mut value = serde_json::json!({
        "period_us": summary.period_us,
        "ticks": summary.ticks,
        "stride": summary.stride,
        "effective_period_us": u64::from(summary.period_us) * u64::from(summary.stride.max(1)),
        "kept": summary.kept,
        "stray": summary.stray,
        "cut": summary.cut,
        "levels": levels
            .iter()
            .map(|(level, count)| (level.to_string(), serde_json::json!(count)))
            .collect::<serde_json::Map<_, _>>(),
        "elf": elf.display().to_string(),
        "samples": samples
            .iter()
            .map(|sample| {
                serde_json::json!([sample.tick, sample.pc, sample.level, sample.ra])
            })
            .collect::<Vec<_>>(),
    });
    let named = match symbols {
        Ok(places) => serde_json::json!({
            "functions": histogram(samples, places, |place| &place.function),
            "inlined": histogram(samples, places, |place| &place.inlined),
        }),
        Err(reason) => serde_json::json!({ "error": reason }),
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("symbols".to_owned(), named);
    }
    value
}

/// The [`TOP`] names of `samples` by count, with their share, most first.
/// A sample in a copy entry with its `ra` is named `name ← caller's name`.
fn histogram(
    samples: &[Sample],
    places: &HashMap<u32, Place>,
    name: impl Fn(&Place) -> &String,
) -> Vec<serde_json::Value> {
    let mut counts = HashMap::<String, u64>::new();
    for sample in samples {
        let own = places
            .get(&sample.pc)
            .map_or("?", |place| name(place).as_str());
        let key = match call_of(sample, places).and_then(|call| places.get(&call)) {
            Some(caller) => format!("{own} ← {}", name(caller)),
            None => own.to_owned(),
        };
        *counts.entry(key).or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total = samples.len().max(1) as f64;
    counts
        .into_iter()
        .take(TOP)
        .map(|(name, count)| {
            serde_json::json!({
                "name": name,
                "samples": count,
                "percent": crate::round(100.0 * count as f64 / total),
            })
        })
        .collect()
}

/// The runtime ELF of the image the harness built and wrote for this run.
pub(crate) fn elf(image: &str) -> Result<PathBuf, String> {
    let root = oer_os_process::Checkout::discover("wifi-load")
        .map_err(|error| format!("the checkout: {error}"))?
        .root;
    Ok(oer_image_build::images::output(&root, image).join("runtime.elf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(tick: u32, pc: u32, level: u8) -> Sample {
        Sample {
            tick,
            pc,
            level,
            ra: None,
        }
    }

    #[test]
    fn a_period_lies_within_the_service_s_range() {
        assert_eq!(parse(" 1000 ").ok(), Some(Profile { period_us: 1000 }));
        for text in ["249", "10001", "", "1ms"] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_place_names_the_outer_function_and_the_innermost_inlined_one() {
        use oer_analysis_elf::dwarf::Frame;
        let frame = |name: &str| Frame {
            function: Some(name.to_owned()),
            ..Frame::default()
        };
        assert_eq!(
            place(&[frame("memcpy"), frame("copy_frame"), frame("receive")]),
            Place {
                function: "receive".into(),
                inlined: "memcpy".into()
            }
        );
        assert_eq!(place(&[]).function, "?");
        assert_eq!(place(&[Frame::default()]).inlined, "?");
    }

    #[test]
    fn the_report_counts_levels_and_ranks_functions_by_samples() {
        let samples = vec![
            sample(0, 0x10, 0),
            sample(2, 0x10, 0),
            sample(4, 0x20, 3),
            sample(6, 0x30, 0),
        ];
        let places = HashMap::from([
            (
                0x10,
                Place {
                    function: "poll".into(),
                    inlined: "memcpy".into(),
                },
            ),
            (
                0x20,
                Place {
                    function: "on_wifi".into(),
                    inlined: "on_wifi".into(),
                },
            ),
        ]);
        let collected = Collected {
            summary: Samples {
                period_us: 1000,
                ticks: 7,
                stride: 2,
                kept: 4,
                ..Samples::default()
            },
            samples,
        };
        let report = report(&collected, Path::new("runtime.elf"), &Ok(places));
        assert_eq!(report["effective_period_us"], 2000);
        assert_eq!(report["levels"]["0"], 3);
        assert_eq!(report["levels"]["3"], 1);
        assert_eq!(report["samples"][2], serde_json::json!([4, 0x20, 3, null]));
        let functions = &report["symbols"]["functions"];
        assert_eq!(functions[0]["name"], "poll");
        assert_eq!(functions[0]["samples"], 2);
        assert_eq!(functions[0]["percent"], 50.0);
        assert_eq!(functions[2]["name"], "on_wifi");
        assert_eq!(functions[1]["name"], "?");
        assert_eq!(report["symbols"]["inlined"][0]["name"], "memcpy");
        let unnamed = super::report(&collected, Path::new("x"), &Err("no ELF".into()));
        assert_eq!(unnamed["symbols"]["error"], "no ELF");
    }

    #[test]
    fn a_sample_in_a_copy_is_named_after_the_caller_its_ra_returns_into() {
        let place = |function: &str, inlined: &str| Place {
            function: function.into(),
            inlined: inlined.into(),
        };
        let with_ra = |tick, pc, ra| Sample {
            ra: Some(ra),
            ..sample(tick, pc, 0)
        };
        let samples = vec![
            with_ra(0, 0x100, 0x2004),
            with_ra(1, 0x104, 0x2004),
            // No `ra`, or `ra` outside a copy: the sample's own name.
            sample(2, 0x100, 0),
            with_ra(3, 0x2000, 0x3004),
        ];
        let places = HashMap::from([
            (0x100, place("memcpy", "copy_forward")),
            (0x104, place("memcpy", "copy_forward")),
            (0x2000, place("poll_action", "poll_action")),
            (0x2003, place("poll_action", "write_frame")),
        ]);
        assert_eq!(call_of(&samples[0], &places), Some(0x2003));
        assert_eq!(call_of(&samples[2], &places), None);
        assert_eq!(call_of(&samples[3], &places), None);
        let functions = histogram(&samples, &places, |place| &place.function);
        assert_eq!(functions[0]["name"], "memcpy ← poll_action");
        assert_eq!(functions[0]["samples"], 2);
        let inlined = histogram(&samples, &places, |place| &place.inlined);
        assert_eq!(inlined[0]["name"], "copy_forward ← write_frame");
        let names: Vec<_> = functions.iter().map(|row| row["name"].clone()).collect();
        assert!(names.contains(&"memcpy".into()) && names.contains(&"poll_action".into()));
        let collected = Collected {
            summary: Samples::default(),
            samples,
        };
        let report = report(&collected, Path::new("runtime.elf"), &Ok(places));
        assert_eq!(
            report["samples"][0],
            serde_json::json!([0, 0x100, 0, 0x2004])
        );
    }
}
