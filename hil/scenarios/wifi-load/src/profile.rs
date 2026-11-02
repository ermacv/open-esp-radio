//! The image's PC samples of a level's window (`OER_WIFI_LOAD_PROFILE_US`):
//! collected page by page, named with the image's runtime ELF's DWARF
//! (`oer_analysis_elf::dwarf::Symbolizer`), and reported as function
//! histograms beside the raw samples. A sample inside a memory copy entry
//! (`memcpy`, `memmove`) with its interrupted `ra` is named after the
//! caller `ra` returns into: `memcpy ← caller`.
//!
//! Beside the tables, the copy sites the image build found in the runtime
//! ELF (`oer_image_checks::copies`, `runtime-copies.json` beside it) are
//! weighted by the samples: those in a copy entry whose `ra` is the site's
//! return address, and those in the site's caller.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use oer_hil_harness::{Cx, Failure};
use oer_image_checks::copies::Copies;
use oer_services_load::{
    CollectSamples, MAX_SAMPLE_PERIOD_US, MIN_SAMPLE_PERIOD_US, Profile, Sample, Samples,
    SamplesCollected,
};

/// The sampling period in microseconds; without it the image samples
/// nothing.
pub(crate) const PROFILE_ENV: &str = "OER_WIFI_LOAD_PROFILE_US";
/// The most functions each histogram names, and the most copy sites.
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
/// functions and innermost inlined functions by samples, the [`TOP`] copy
/// sites by weight ([`copies`]), and every raw sample as
/// `[tick, pc, level, ra]` (`ra` null when the image recorded none).
/// `symbols` is the naming and `sites` the build's copy sites, or why
/// there are none.
pub(crate) fn report(
    collected: &Collected,
    elf: &Path,
    symbols: &Result<HashMap<u32, Place>, String>,
    sites: &Result<Copies, String>,
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
    let copies = match sites {
        Ok(sites) => copies(sites, samples),
        Err(reason) => serde_json::json!({ "error": reason }),
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("symbols".to_owned(), named);
        object.insert("copies".to_owned(), copies);
    }
    value
}

/// The copy sites of the build of the runtime ELF `elf`, which it wrote
/// beside it.
pub(crate) fn copy_sites(elf: &Path) -> Result<Copies, String> {
    let path = elf.with_file_name(oer_image_checks::copies::RUNTIME_JSON);
    oer_image_checks::copies::read(&path).map_err(|error| error.to_string())
}

/// The copy sites of `sites` weighted by `samples`, the [`TOP`] most
/// first: `in_copy` counts the samples in a copy entry whose `ra` is the
/// site's return address (time spent copying for this site), `in_caller`
/// the samples in the site's caller (how hot the code around it is). The
/// totals give the samples in copy entries, those a site's return address
/// names, those without an `ra` and those whose `ra` names no site (a tail
/// jump's, or a stale one).
fn copies(sites: &Copies, samples: &[Sample]) -> serde_json::Value {
    let in_entry = |pc: u32| sites.entries.iter().any(|entry| entry.contains(pc));
    let mut returns = HashMap::<u32, u64>::new();
    let (mut copying, mut without_ra) = (0_u64, 0_u64);
    let mut callers = BTreeMap::<u32, (u32, u64)>::new();
    for site in &sites.sites {
        callers.insert(site.caller.address, (site.caller.size, 0));
    }
    for sample in samples {
        if in_entry(sample.pc) {
            copying += 1;
            match sample.ra {
                Some(ra) => *returns.entry(ra).or_default() += 1,
                None => without_ra += 1,
            }
        }
        if let Some((&start, (size, count))) = callers.range_mut(..=sample.pc).next_back()
            && sample.pc - start < *size
        {
            *count += 1;
        }
    }
    let total = samples.len().max(1) as f64;
    let percent = |count: u64| crate::round(100.0 * count as f64 / total);
    let mut weighted: Vec<_> = sites
        .sites
        .iter()
        .map(|site| {
            let in_copy = site
                .returns
                .and_then(|at| returns.get(&at))
                .copied()
                .unwrap_or(0);
            let in_caller = callers.get(&site.caller.address).map_or(0, |entry| entry.1);
            (site, in_copy, in_caller)
        })
        .filter(|(_, in_copy, in_caller)| in_copy + in_caller > 0)
        .collect();
    weighted.sort_by(|a, b| {
        (b.1, b.2, b.0.bytes)
            .cmp(&(a.1, a.2, a.0.bytes))
            .then(a.0.site.cmp(&b.0.site))
    });
    let attributed: u64 = sites
        .sites
        .iter()
        .filter_map(|site| site.returns)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|at| returns.get(&at))
        .sum();
    serde_json::json!({
        "samples_in_copies": copying,
        "percent_in_copies": percent(copying),
        "attributed": attributed,
        "without_ra": without_ra,
        "unattributed": copying - attributed - without_ra,
        "roots": sites.roots,
        "sites": weighted
            .into_iter()
            .take(TOP)
            .map(|(site, in_copy, in_caller)| serde_json::json!({
                "site": format!("{:#010x}", site.site),
                "entry": site.entry,
                "bytes": site.bytes,
                "caller": site.caller.name,
                "source": site.source.join(" <- "),
                "roots": site.roots,
                "in_copy": in_copy,
                "in_copy_percent": percent(in_copy),
                "in_caller": in_caller,
                "in_caller_percent": percent(in_caller),
            }))
            .collect::<Vec<_>>(),
    })
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
        let report = report(
            &collected,
            Path::new("runtime.elf"),
            &Ok(places),
            &Err("no copies".into()),
        );
        assert_eq!(report["copies"]["error"], "no copies");
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
        let unnamed = super::report(
            &collected,
            Path::new("x"),
            &Err("no ELF".into()),
            &Err("no ELF".into()),
        );
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
        let report = report(
            &collected,
            Path::new("runtime.elf"),
            &Ok(places),
            &Err("none".into()),
        );
        assert_eq!(
            report["samples"][0],
            serde_json::json!([0, 0x100, 0, 0x2004])
        );
    }

    /// A build's copy sites: `memcpy` at 0x100..0x140, called from
    /// `poll_action` (0x2000..0x2040) at 0x2010 and 0x2020 and tail-jumped
    /// into from `send` (0x3000..0x3010).
    fn fixture() -> Copies {
        use oer_image_checks::copies::{Code, CopySite, SCHEMA};
        let code = |name: &str, address, size| Code {
            name: name.into(),
            address,
            size,
        };
        let site = |at: u32, returns, caller: Code, bytes| CopySite {
            site: at,
            returns,
            entry: "memcpy".into(),
            caller,
            bytes,
            source: vec!["src/frame.rs:10".into(), "src/poll.rs:20".into()],
            roots: vec!["hart0-P2".into()],
        };
        Copies {
            schema: SCHEMA,
            entries: vec![code("memcpy", 0x100, 0x40)],
            roots: Some(vec!["hart0-P2".into()]),
            sites: vec![
                site(
                    0x2010,
                    Some(0x2014),
                    code("poll_action", 0x2000, 0x40),
                    Some(1240),
                ),
                site(
                    0x2020,
                    Some(0x2024),
                    code("poll_action", 0x2000, 0x40),
                    None,
                ),
                site(0x3008, None, code("send", 0x3000, 0x10), Some(64)),
            ],
        }
    }

    #[test]
    fn copy_sites_are_weighted_by_their_return_address_and_their_caller() {
        let with_ra = |tick, pc, ra| Sample {
            ra,
            ..sample(tick, pc, 0)
        };
        let samples = vec![
            // Three samples copy for 0x2010, one for 0x2020.
            with_ra(0, 0x100, Some(0x2014)),
            with_ra(1, 0x13c, Some(0x2014)),
            with_ra(2, 0x120, Some(0x2014)),
            with_ra(3, 0x120, Some(0x2024)),
            // In a copy without an `ra`, and with one naming no site.
            with_ra(4, 0x120, None),
            with_ra(5, 0x120, Some(0x9000)),
            // In the caller, twice; past `send`'s code; elsewhere.
            with_ra(6, 0x2000, None),
            with_ra(7, 0x203c, Some(0x2014)),
            with_ra(8, 0x3010, None),
            with_ra(9, 0x5000, None),
        ];
        let report = copies(&fixture(), &samples);
        assert_eq!(report["samples_in_copies"], 6);
        assert_eq!(report["percent_in_copies"], 60.0);
        assert_eq!(report["attributed"], 4);
        assert_eq!(report["without_ra"], 1);
        assert_eq!(report["unattributed"], 1);
        let sites = report["sites"].as_array().unwrap();
        // `send` has no weight and is left out.
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0]["site"], "0x00002010");
        assert_eq!(sites[0]["bytes"], 1240);
        assert_eq!(sites[0]["caller"], "poll_action");
        assert_eq!(sites[0]["source"], "src/frame.rs:10 <- src/poll.rs:20");
        assert_eq!(sites[0]["roots"][0], "hart0-P2");
        assert_eq!(sites[0]["in_copy"], 3);
        assert_eq!(sites[0]["in_caller"], 2);
        assert_eq!(sites[0]["in_copy_percent"], 30.0);
        assert_eq!(sites[1]["site"], "0x00002020");
        assert_eq!(sites[1]["bytes"], serde_json::Value::Null);
        assert_eq!(sites[1]["in_copy"], 1);
    }

    #[test]
    fn the_copy_sites_come_from_the_build_beside_the_elf() {
        let directory = tempfile::tempdir().unwrap();
        let elf = directory.path().join("runtime.elf");
        assert!(copy_sites(&elf).is_err());
        std::fs::write(
            directory
                .path()
                .join(oer_image_checks::copies::RUNTIME_JSON),
            serde_json::to_vec(&fixture()).unwrap(),
        )
        .unwrap();
        assert_eq!(copy_sites(&elf).unwrap(), fixture());
    }
}
