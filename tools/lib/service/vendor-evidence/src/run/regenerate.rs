//! Compute a chip's derived vendor evidence index for the checkout.
//!
//! Each shard records the digests of the sources its verdicts depend on, so a
//! shard is stale exactly when one of them changed; the index is untracked
//! derived data, computed whole or for named scenarios.
//!
//! This module decides which producer computes which shard: the Blobray
//! scenario engine ([`super::scenario`]) or a host stand below
//! `verification/<chip>/host/` (its own `shard` command);
//! `cargo verification evidence` calls it.
use crate::producer::{Producer, host_stand};
use crate::{Result, baseline};
use oer_tool_base_process::Checkout;
use oer_tool_data_vendor_evidence_shard::{Index, store};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Shard directory of `chip`, relative to the repository root.
fn directory(root: &Path, chip: &str) -> Result<String> {
    Ok(oer_tool_data_vendor_artifacts::project::Project::new(root, chip)?.evidence_shards())
}

/// The comparison probe packages of `chip` a scenario run needs.
fn probes(chip: &str) -> (String, String) {
    (
        format!("oer-{chip}-probe-radio-elf"),
        format!("oer-{chip}-probe-bluetooth-elf"),
    )
}

/// Fail when a shard of the index `directory` records a file of a report
/// package.
fn reject_report_sources(ctx: &Checkout, directory: &Path) -> Result<()> {
    let model = oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?;
    let report = crate::policy::report_packages(&model)?;
    crate::policy::reject_report_sources(directory, &report)
}

/// The typed Blobray scenarios of a chip, run through `vendor-scenario`.
struct Blobray<'a> {
    ctx: &'a Checkout,
    chip: &'a str,
    linker: PathBuf,
    output: &'a Path,
}

impl Producer for Blobray<'_> {
    fn command(&self) -> &'static str {
        oer_tool_data_vendor_evidence_shard::BLOBRAY
    }

    fn owns(&self, _scenario: &str) -> bool {
        true
    }

    fn produce(&self, scenarios: &[String], index: &Path) -> Result<()> {
        let (ctx, chip) = (self.ctx, self.chip);
        super::probes::run(ctx, chip, false)?;
        let (radio, bluetooth) = probes(chip);
        let radio = super::probes::elf(ctx, &radio)?;
        // Only chips with Bluetooth scenarios build a Bluetooth probe.
        let bluetooth = super::probes::elf(ctx, &bluetooth).ok();
        // Several scenarios run concurrently under one budget in `all`.
        let runs: Vec<String> = if scenarios.len() > 1 {
            vec!["all".into()]
        } else {
            scenarios.to_vec()
        };
        for scenario in runs {
            let mut args: Vec<OsString> = vec![
                scenario.clone().into(),
                "--production".into(),
                radio.clone().into(),
                "--linker".into(),
                self.linker.as_os_str().to_owned(),
                "--output".into(),
                self.output.join(&scenario).into(),
                "--index".into(),
                index.into(),
            ];
            if let Some(bluetooth) = bluetooth
                .as_ref()
                .filter(|_| scenario == "all" || scenario == "bluetooth")
            {
                args.extend(["--bluetooth-production".into(), bluetooth.clone().into()]);
            }
            if super::scenario::run(ctx, chip, &args)? != ExitCode::SUCCESS {
                return Err(format!("vendor scenario {scenario} failed").into());
            }
        }
        Ok(())
    }
}

/// One host stand of a chip: its own workspace, whose `shard` command
/// compares every scenario and writes the stand's shard.
struct HostStand<'a> {
    ctx: &'a Checkout,
    scenario: String,
    manifest: String,
}

impl Producer for HostStand<'_> {
    fn command(&self) -> &'static str {
        oer_tool_data_vendor_evidence_shard::HOST_STAND
    }

    fn owns(&self, scenario: &str) -> bool {
        scenario == self.scenario
    }

    fn produce(&self, _scenarios: &[String], index: &Path) -> Result<()> {
        super::phase::timed(&format!("stand {}", self.scenario), || {
            oer_tool_base_process::run(
                oer_tool_base_toolchain::cargo_in(&self.ctx.root)
                    .args(["run", "--quiet", "--manifest-path", &self.manifest, "--"])
                    .arg("shard")
                    .arg("--index")
                    .arg(index),
            )
        })?;
        Ok(())
    }
}

/// The producers of `chip`'s shards, host stands first: the Blobray
/// scenarios own every name no stand owns.
fn producers<'a>(
    ctx: &'a Checkout,
    chip: &'a str,
    linker: &Path,
    output: &'a Path,
) -> Result<Vec<Box<dyn Producer + 'a>>> {
    let model = oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?;
    let mut producers: Vec<Box<dyn Producer + 'a>> = host_stand::stands(&model, chip)?
        .into_iter()
        .map(|(scenario, manifest)| {
            Box::new(HostStand {
                ctx,
                scenario,
                manifest,
            }) as Box<dyn Producer>
        })
        .collect();
    producers.push(Box::new(Blobray {
        ctx,
        chip,
        linker: resolve(linker)?,
        output,
    }));
    Ok(producers)
}

/// Rerun `selected` scenarios of `chip` with their producers, writing their
/// shards into `index`.
fn regenerate(
    ctx: &Checkout,
    chip: &str,
    selected: Vec<String>,
    index: &Path,
    linker: &Path,
    output: &Path,
) -> Result<()> {
    let mut remaining = selected;
    for producer in producers(ctx, chip, linker, output)? {
        let (owned, rest): (Vec<String>, Vec<String>) =
            remaining.into_iter().partition(|name| producer.owns(name));
        remaining = rest;
        if !owned.is_empty() {
            producer.produce(&owned, index)?;
        }
    }
    Ok(())
}

/// Compute `chip`'s derived evidence index ([`directory`]) for the checkout:
/// every shard when no scenario is named, after fetching the pins and
/// building the firmware inputs from their catalog recipes, else only the
/// named shards. Fails when a scenario does not match.
pub fn run(
    ctx: &Checkout,
    chip: &str,
    scenarios: Vec<String>,
    linker: Option<PathBuf>,
    output: PathBuf,
) -> Result<ExitCode> {
    // Ubuntu's LLD 18 fails these RV32 links: the toolchain's LLD is the default.
    let linker = match linker {
        Some(linker) => linker,
        None => oer_tool_base_toolchain::lld()?,
    };
    let relative = directory(&ctx.root, chip)?;
    let directory = ctx.root.join(&relative);
    if scenarios.is_empty() {
        oer_tool_data_vendor_artifacts::fetch_vendor_sources(&ctx.root, chip)?;
        for image in oer_tool_data_vendor_artifacts::firmware_images(&ctx.root, chip)? {
            oer_tool_service_image::esp_idf::catalog::build(&ctx.root, &image)?;
        }
        if directory.exists() {
            std::fs::remove_dir_all(&directory)?;
        }
        std::fs::create_dir_all(&directory)?;
        let mut every = host_stand::stands(
            &oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?,
            chip,
        )?
        .into_keys()
        .collect::<Vec<_>>();
        // The Blobray producer runs every one of its scenarios as `all`.
        every.push("all".into());
        println!("computing the evidence index of {chip} into {relative}");
        regenerate(ctx, chip, every, &directory, &linker, &output)?;
        for name in store::names(&directory)? {
            print!(
                "{}",
                summary(&name, None, store::read(&directory, &name).as_ref())
            );
        }
    } else {
        std::fs::create_dir_all(&directory)?;
        let before: Vec<(String, Option<Index>)> = scenarios
            .iter()
            .map(|name| (name.clone(), store::read(&directory, name)))
            .collect();
        println!("regenerating evidence shards: {}", scenarios.join(", "));
        regenerate(ctx, chip, scenarios, &directory, &linker, &output)?;
        for (name, old) in before {
            let new = store::read(&directory, &name);
            print!("{}", summary(&name, old.as_ref(), new.as_ref()));
        }
    }
    reject_report_sources(ctx, &directory)?;
    print!("{}", render_untriaged(&directory, false)?);
    Ok(ExitCode::SUCCESS)
}

/// Compare `chip`'s computed index with `base`, the index `main` computed at
/// the change's merge base: print what each scenario's shard says
/// differently, then every claimed vendor root the index lost or narrowed
/// ([`baseline::losses`]); whether any is. What the chip's reviewed
/// acceptances name ([`baseline::Accepted`]) passes, and an acceptance the
/// index contradicts is an error.
pub fn compare(ctx: &Checkout, chip: &str, base: &Path) -> Result<bool> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    let mut names = store::names(base)?;
    names.extend(store::names(&directory)?);
    names.sort();
    names.dedup();
    for name in &names {
        let (old, new) = (store::read(base, name), store::read(&directory, name));
        let changed = match (&old, &new) {
            (Some(old), Some(new)) => !crate::diff::differences(old, new).is_empty(),
            _ => old.is_some() != new.is_some(),
        };
        if changed {
            print!("{}", summary(name, old.as_ref(), new.as_ref()));
        }
    }
    let (base_read, new) = (
        baseline::claims(base)?,
        baseline::claims(&directory)?.current()?,
    );
    for (source, symbol) in &base_read.unread_coverage {
        println!(
            "{chip}: the base records the coverage of vendor root {source}::{symbol} in another form: its narrowing is not compared, only its loss"
        );
    }
    let old = base_read.claims;
    let accepted = baseline::Accepted::load(&ctx.root, chip)?;
    accepted.check(chip, &new)?;
    let losses = baseline::losses(&old, &new, &accepted);
    for (source, symbol) in &losses.lost {
        println!(
            "{chip}: vendor root {source}::{symbol} is no longer claimed; if that is intended, retire it in {}",
            accepted.path
        );
    }
    for ((source, symbol), before, now) in &losses.narrowed {
        println!(
            "{chip}: the comparisons of vendor root {source}::{symbol} reach {} blocks and {} directions, {} and {} before; if that is intended, accept the narrowing in {}",
            now.blocks, now.directions, before.blocks, before.directions, accepted.path
        );
    }
    Ok(!losses.is_empty())
}

/// Print the chip-wide untriaged vendor locations of `chip`'s derived
/// evidence index, one per line.
pub fn untriaged(ctx: &Checkout, chip: &str) -> Result<ExitCode> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    print!("{}", render_untriaged(&directory, true)?);
    Ok(ExitCode::SUCCESS)
}

/// Per-function counts of the chip-wide untriaged locations of the shards
/// in `directory`, or every location when `all`.
fn render_untriaged(directory: &Path, all: bool) -> Result<String> {
    let shards = store::shards(directory)?;
    let locations =
        oer_tool_data_vendor_evidence_shard::untriaged(&shards.iter().collect::<Vec<_>>());
    let mut functions = std::collections::BTreeMap::<&str, usize>::new();
    for location in &locations {
        *functions.entry(&location.function).or_default() += 1;
    }
    let mut text = format!(
        "chip-wide untriaged: {} locations in {} functions\n",
        locations.len(),
        functions.len()
    );
    if all {
        for location in &locations {
            text += &format!(
                "  {}+{:#x} {:?}\n",
                location.function, location.offset, location.kind
            );
        }
    } else {
        let mut ranked: Vec<_> = functions.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (function, count) in ranked {
            text += &format!("  {count:4} {function}\n");
        }
    }
    Ok(text)
}

/// What changed between two versions of shard `name`: its claims first,
/// then the recorded sources.
fn summary(name: &str, old: Option<&Index>, new: Option<&Index>) -> String {
    match (old, new) {
        (Some(old), Some(new)) => crate::diff::render(name, &crate::diff::differences(old, new)),
        (None, Some(_)) => format!("{name}:\n  new shard\n"),
        (_, None) => format!("{name}:\n  no readable shard\n"),
    }
}

/// `program` itself when it names a path, otherwise its first match on
/// `PATH`.
fn resolve(program: &Path) -> Result<PathBuf> {
    if program.components().count() > 1 {
        return Ok(program.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("{} is not on PATH", program.display()).into())
}
