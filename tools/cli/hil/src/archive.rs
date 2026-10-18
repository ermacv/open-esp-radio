//! `cargo hil archive`: the HIL evidence archive ( Q7,
//! `oer-hil-evidence-archive`).
//!
//! `ref` prints the references of sealed runs and pins them for its
//! selection, `pack` writes the version of the runs a references file cites
//! (the pinned version's from its downloaded copy), `holders` lists who pins
//! which runs, `unpin` drops a holder's pins, `publish` makes a version a release of the archive
//! repository and writes the pin, and `fetch` downloads the pinned version
//! into a verified copy. `pack`, `publish` and `fetch` reach the network,
//! through `gh`.
//!
//! Exit status: 0 done, 1 an archive rule refused the inputs, 2 a usage
//! error, 3 the environment failed (I/O, `gh`, the network), 130 cancelled.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use oer_hil_evidence_archive::{
    Manifest, PIN, Pin, Refused, Repository, RunId, RunRef, Version, pack, release,
};
use oer_hil_run_bundle::{RunStore, store::Sidecar};
use oer_process::Checkout;
use serde::{Deserialize, Serialize};

use crate::Result;

/// `cargo hil archive`.
#[derive(Debug, Parser)]
#[command(
    name = "archive",
    about = "The HIL evidence archive: immutable versions of cited runs"
)]
pub(crate) struct ArchiveCli {
    #[command(subcommand)]
    command: ArchiveCommand,
}

#[derive(Debug, Subcommand)]
enum ArchiveCommand {
    /// Print the references of runs as a references file: a sealed run of
    /// the run store by its seal, and one the pinned version holds and the
    /// store no longer has from the pin's manifest. Pin the store's runs for
    /// a selection, named in the file (a new one, or SELECTION), so pruning
    /// keeps them until a version holding them is published or the
    /// selection is dropped.
    Ref {
        #[arg(required = true, value_parser = parse_run)]
        runs: Vec<RunId>,
        /// Add the runs to this selection instead of a new one.
        #[arg(long, value_parser = parse_selection)]
        selection: Option<String>,
        #[arg(long, default_value = PIN)]
        pin: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// [network] Pack the runs a references file cites into OUT
    /// (`evidence.tar.gz`, `manifest.json`): the runs the pinned version
    /// already holds from its verified copy, which this downloads, the
    /// others from the run store. The version takes over the selection's
    /// pins until it is published.
    Pack {
        #[arg(long)]
        refs: PathBuf,
        #[arg(long, default_value = PIN)]
        pin: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// [network] Publish the packed version in DIR as a release of the
    /// pinned version's repository, then write the pin. Before the first
    /// version, name the repository; another than the pinned one only with
    /// `--new-repository`.
    Publish {
        dir: PathBuf,
        #[arg(long, value_parser = parse_repository)]
        repository: Option<Repository>,
        /// Move the archive to `--repository`, away from the pinned one.
        #[arg(long, requires = "repository")]
        new_repository: bool,
        #[arg(long, default_value = PIN)]
        pin: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Drop the local pins HOLDER holds: a version packed but never
    /// published, or a selection `ref` made. Pins set by hand (`cargo hil
    /// runs pin`) stay.
    Unpin {
        #[arg(value_parser = parse_holder)]
        holder: String,
        #[arg(long)]
        json: bool,
    },
    /// List the archive's holders of pins in the run store (selections and
    /// versions never published) and the runs each holds.
    Holders {
        #[arg(long)]
        json: bool,
    },
    /// [network] Download the pinned version and verify it into OUT.
    Fetch {
        #[arg(long, default_value = PIN)]
        pin: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

/// A references file: the runs a version is to hold, and the selection
/// that pins them since `ref` named them.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Refs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selection: Option<String>,
    runs: Vec<RunRef>,
}

/// `cargo hil archive ...`, with its own exit status.
pub fn command(ctx: &Checkout, owner: Result<String>, args: &[OsString]) -> Result<ExitCode> {
    let parsed = match ArchiveCli::try_parse_from(
        std::iter::once(OsString::from("archive")).chain(args.iter().cloned()),
    ) {
        Ok(parsed) => parsed,
        Err(error) => {
            let usage = error.use_stderr();
            error.print()?;
            return Ok(ExitCode::from(if usage { 2 } else { 0 }));
        }
    };
    Ok(match execute(ctx, owner, parsed.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hil: archive: {error}");
            ExitCode::from(status(&*error))
        }
    })
}

/// The exit status of a failed command.
fn status(error: &(dyn std::error::Error + Send + Sync + 'static)) -> u8 {
    if oer_process::is_cancelled(error) {
        130
    } else if error.is::<Refused>() {
        1
    } else {
        3
    }
}

fn execute(ctx: &Checkout, owner: Result<String>, command: ArchiveCommand) -> Result<()> {
    let rooted = |path: &Path| ctx.root.join(path);
    match command {
        ArchiveCommand::Ref {
            runs,
            selection,
            pin,
            json,
        } => {
            let by = owner?;
            let store = RunStore::shared()?;
            let pinned = Pin::load(&rooted(&pin))?;
            let (references, local) =
                references(&store, &runs, pinned.as_ref().map(Pin::manifest))?;
            let mut refs = Refs {
                selection: None,
                runs: references,
            };
            if local.is_empty() {
                eprintln!(
                    "hil: archive: every run is in the pinned version, which keeps it: nothing to pin"
                );
            } else {
                let selection = select(&store, &local, &by, selection.as_deref())?;
                eprintln!(
                    "hil: archive: {selection} pins the run store's runs among these until a \
                     version holding them is published; `cargo hil archive unpin {selection}` drops it"
                );
                refs.selection = Some(selection);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&refs)?);
            } else {
                print!("{}", toml::to_string(&refs)?);
            }
        }
        ArchiveCommand::Pack {
            refs,
            pin,
            out,
            json,
        } => {
            let by = owner?;
            let refs = read_refs(&rooted(&refs))?;
            let store = RunStore::shared()?;
            let out = rooted(&out);
            let pinned = Pin::load(&rooted(&pin))?;
            let manifest = pack_version(
                &store,
                &refs,
                &out,
                &by,
                // The pinned version's runs come from its verified copy, so
                // the new version keeps them after pruning (owner decision
                // ).
                |staging| {
                    pinned
                        .as_ref()
                        .map(|pinned| {
                            release::fetch(
                                &Gh::new(pinned.repository().clone()),
                                pinned,
                                &staging.join("base"),
                            )
                        })
                        .transpose()
                },
            )?;
            report(
                json,
                &manifest,
                serde_json::json!({ "out": out }),
                &format!("packed {} into {}", manifest.version(), out.display()),
            )?;
        }
        ArchiveCommand::Publish {
            dir,
            repository,
            new_repository,
            pin,
            json,
        } => {
            let dir = rooted(&dir);
            let repository = publish_repository(
                repository,
                Pin::load(&rooted(&pin))?.as_ref(),
                new_repository,
            )?;
            let (manifest, published) = release::publish(&Gh::new(repository.clone()), &dir)?;
            let pin_path = rooted(&pin);
            Pin::new(repository, manifest.clone()).write(&pin_path)?;
            // A published version holds the runs: their local pins may go.
            let_go(&RunStore::shared()?, &manifest.version().to_string())?;
            report(
                json,
                &manifest,
                serde_json::json!({ "published": format!("{published:?}").to_lowercase(), "pin": pin_path }),
                &format!(
                    "{} {:?}; pinned in {}",
                    manifest.version(),
                    published,
                    pin_path.display()
                ),
            )?;
        }
        ArchiveCommand::Unpin { holder, json } => {
            let released = let_go(&RunStore::shared()?, &holder)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "holder": holder, "released": released })
                );
            } else {
                println!("hil: archive: {holder} no longer pins {released} runs");
            }
        }
        ArchiveCommand::Holders { json } => {
            let pins =
                RunStore::shared()?.read::<oer_hil_run_bundle::store::Notes>(Sidecar::Pins)?;
            let holders = crate::pins::holders_of(&pins);
            if json {
                println!("{}", serde_json::to_string_pretty(&holders)?);
            } else if holders.is_empty() {
                println!("hil: archive: no holder pins a run");
            } else {
                for (holder, runs) in &holders {
                    println!("{holder}\t{} runs\t{}", runs.len(), runs.join(" "));
                }
            }
        }
        ArchiveCommand::Fetch { pin, out, json } => {
            let pin_path = rooted(&pin);
            let pin = Pin::load(&pin_path)?
                .ok_or_else(|| Refused(format!("{} pins no version", pin_path.display())))?;
            let copy = release::fetch(&Gh::new(pin.repository().clone()), &pin, &rooted(&out))?;
            report(
                json,
                copy.manifest(),
                serde_json::json!({ "copy": copy.root() }),
                &format!(
                    "verified {} into {}",
                    copy.manifest().version(),
                    copy.root().display()
                ),
            )?;
        }
    }
    Ok(())
}

/// The run IDs of `refs`.
fn run_names(refs: &[RunRef]) -> Vec<String> {
    refs.iter()
        .map(|reference| reference.run.to_string())
        .collect()
}

/// The references of `runs` and those of them the run store holds, to pin:
/// a sealed run of the store by its seal, and a run `pinned` (the pinned
/// version's manifest) holds that the store no longer has from it, since
/// the archive keeps that one (owner decision ).
fn references(
    store: &RunStore,
    runs: &[RunId],
    pinned: Option<&Manifest>,
) -> Result<(Vec<RunRef>, Vec<String>)> {
    let mut references = Vec::new();
    let mut local = Vec::new();
    for run in runs {
        let directory = store.run(&run.to_string());
        if !directory.exists()
            && let Some(held) = pinned.and_then(|pinned| pinned.find(run))
        {
            references.push(held.clone());
            continue;
        }
        references.push(oer_hil_evidence_archive::run_reference(&directory, run)?);
        local.push(run.to_string());
    }
    Ok((references, local))
}

/// Pack `refs` into `out` (a base, the pinned version's copy, staged next to
/// `out`, from `base`) and hand the selection's pins to the version; a
/// version whose runs could not all be pinned is removed again, so a retry
/// can write it.
fn pack_version(
    store: &RunStore,
    refs: &Refs,
    out: &Path,
    by: &str,
    base: impl FnOnce(&Path) -> Result<Option<oer_hil_evidence_archive::Copy>>,
) -> Result<Manifest> {
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(Box::new(Refused(format!(
            "{} already exists",
            out.display()
        ))));
    }
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    // Next to OUT, not in a temporary file system: a version is gigabytes.
    let staging = tempfile::tempdir_in(parent)?;
    let base = base(staging.path())?;
    let manifest = pack::pack(store.root(), &refs.runs, base.as_ref(), out)?;
    if let Err(error) = hand_over(store, &manifest, refs.selection.as_deref(), by) {
        // The original error decides the exit status (a cancellation stays
        // one); a failed cleanup is reported beside it.
        if let Err(cleanup) = std::fs::remove_dir_all(out) {
            eprintln!(
                "hil: archive: the unpinned version in {} could not be removed: {cleanup}",
                out.display()
            );
        }
        return Err(error);
    }
    Ok(manifest)
}

/// The repository `publish` releases to: the pinned version's; the one
/// named only before the first version, or with `new` to move the archive.
fn publish_repository(
    named: Option<Repository>,
    pinned: Option<&Pin>,
    new: bool,
) -> Result<Repository> {
    match (named, pinned) {
        (None, Some(pinned)) => Ok(pinned.repository().clone()),
        (None, None) => Err(Box::new(Refused(String::from(
            "no version is pinned yet: name the archive repository with --repository",
        )))),
        (Some(named), Some(pinned)) if &named != pinned.repository() && !new => {
            Err(Box::new(Refused(format!(
                "--repository {named} is not the pinned {}; --new-repository moves the archive",
                pinned.repository()
            ))))
        }
        (Some(named), _) => Ok(named),
    }
}

/// Pin `runs` for a selection of `by` (`reuse`, or a new one), in one
/// locked change; the selection's holder. Each run must still exist under
/// the lock: a prune that read the pins before may have deleted it since.
fn select(store: &RunStore, runs: &[String], by: &str, reuse: Option<&str>) -> Result<String> {
    let now = oer_durable::unix_millis();
    let selection = reuse.map_or_else(|| crate::pins::new_selection(now), str::to_owned);
    store.update_notes(Sidecar::Pins, |pins| {
        if let Some(gone) = runs.iter().find(|run| !store.run(run).is_dir()) {
            return Err(Refused(format!(
                "run {gone} was deleted from the run store before it could be pinned"
            )));
        }
        for run in runs {
            crate::pins::hold(pins, run, &selection, by, now);
        }
        Ok(())
    })??;
    Ok(selection)
}

/// Pin the runs of `manifest` the run store holds for its version and let
/// `selection` go from them, in one locked change. A run the store no
/// longer holds came from the pinned version, which the archive keeps.
fn hand_over(
    store: &RunStore,
    manifest: &Manifest,
    selection: Option<&str>,
    by: &str,
) -> Result<()> {
    let now = oer_durable::unix_millis();
    let version = manifest.version().to_string();
    store.update_notes(Sidecar::Pins, |pins| {
        for run in run_names(&manifest.runs) {
            if !store.run(&run).is_dir() {
                continue;
            }
            crate::pins::hold(pins, &run, &version, by, now);
            if let Some(selection) = selection {
                crate::pins::release(pins, &run, selection);
            }
        }
    })
}

/// Let `holder` go from every pin, in one locked change; the runs it held.
fn let_go(store: &RunStore, holder: &str) -> Result<usize> {
    store.update_notes(Sidecar::Pins, |pins| {
        crate::pins::release_everywhere(pins, holder)
    })
}

fn report(json: bool, manifest: &Manifest, extra: serde_json::Value, text: &str) -> Result<()> {
    if json {
        let mut value = serde_json::json!({
            "version": manifest.version(),
            "manifest_sha256": manifest.digest(),
            "runs": manifest.runs,
        });
        if let (Some(value), serde_json::Value::Object(extra)) = (value.as_object_mut(), extra) {
            value.extend(extra);
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("hil: archive: {text} ({} runs)", manifest.runs.len());
    }
    Ok(())
}

/// A RUN argument of `ref`: a run ID, or a usage error.
fn parse_run(text: &str) -> std::result::Result<RunId, String> {
    RunId::parse(text).map_err(|error| error.to_string())
}

/// The `--repository` of `publish`: `owner/name`, or a usage error.
fn parse_repository(text: &str) -> std::result::Result<Repository, String> {
    Repository::parse(text).map_err(|error| error.to_string())
}

/// A selection `ref --selection` adds to: `selection-<millis>-<process>-<n>`,
/// as `ref` makes it, so it never holds the `, ` that separates holders.
fn parse_selection(text: &str) -> std::result::Result<String, String> {
    let numbers = text
        .strip_prefix(crate::pins::SELECTION)
        .map(|rest| rest.split('-').collect::<Vec<_>>());
    match numbers {
        Some(parts)
            if parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())) =>
        {
            Ok(text.to_owned())
        }
        _ => Err(format!(
            "a selection is `{}<millis>-<process>-<n>`, as `ref` prints it",
            crate::pins::SELECTION
        )),
    }
}

/// A holder `unpin` takes: a selection `ref` made, or a version.
fn parse_holder(text: &str) -> std::result::Result<String, String> {
    if text.starts_with(crate::pins::SELECTION) {
        return parse_selection(text);
    }
    Version::parse(text)
        .map(|_| text.to_owned())
        .map_err(|error| format!("{error}, nor a `{}...` selection", crate::pins::SELECTION))
}

/// A references file as `ref` prints it: TOML, or JSON with `--json`.
fn read_refs(path: &Path) -> Result<Refs> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let refs = if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).map_err(|error| error.to_string())
    } else {
        toml::from_str(&text).map_err(|error| error.to_string())
    };
    refs.map_err(|error| Refused(format!("{}: {error}", path.display())).into())
}

/// The archive repository's releases, through `gh`.
struct Gh {
    repository: Repository,
}

impl Gh {
    fn new(repository: Repository) -> Self {
        Self { repository }
    }

    fn gh(&self, args: &[&std::ffi::OsStr]) -> Result<std::process::Output> {
        let mut command = oer_process::command("gh");
        command
            .args(args)
            .arg("--repo")
            .arg(self.repository.as_str());
        oer_process::output(&mut command, None)
    }

    fn run(&self, args: &[&std::ffi::OsStr]) -> Result<()> {
        let output = self.gh(args)?;
        if !output.status.success() {
            return Err(format!(
                "gh {}: {}",
                args.first()
                    .map_or_else(String::new, |a| a.to_string_lossy().into_owned()),
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

impl release::Releases for Gh {
    fn state(&self, version: &Version) -> Result<Option<release::State>> {
        let tag = version.to_string();
        let output =
            self.gh(&["release", "view", &tag, "--json", "isDraft"].map(std::ffi::OsStr::new))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("release not found") {
                return Ok(None);
            }
            return Err(format!("gh release view {tag}: {}", stderr.trim()).into());
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct View {
            is_draft: bool,
        }
        let view: View = serde_json::from_slice(&output.stdout)?;
        Ok(Some(if view.is_draft {
            release::State::Draft
        } else {
            release::State::Published
        }))
    }

    fn location(&self) -> String {
        self.repository.to_string()
    }

    fn assets(&self, version: &Version) -> Result<Vec<String>> {
        let tag = version.to_string();
        let output =
            self.gh(&["release", "view", &tag, "--json", "assets"].map(std::ffi::OsStr::new))?;
        if !output.status.success() {
            return Err(format!(
                "gh release view {tag}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        #[derive(Deserialize)]
        struct Asset {
            name: String,
        }
        #[derive(Deserialize)]
        struct View {
            assets: Vec<Asset>,
        }
        let view: View = serde_json::from_slice(&output.stdout)?;
        Ok(view.assets.into_iter().map(|asset| asset.name).collect())
    }

    fn create_draft(&self, version: &Version, assets: &[PathBuf]) -> Result<()> {
        let tag = version.to_string();
        let notes = format!(
            "HIL evidence archive version {tag} (#761 Q7). Immutable: never edit or delete."
        );
        let mut args = [
            "release", "create", &tag, "--draft", "--title", &tag, "--notes", &notes,
        ]
        .map(std::ffi::OsStr::new)
        .to_vec();
        args.extend(assets.iter().map(|asset| asset.as_os_str()));
        self.run(&args)
    }

    fn download(&self, version: &Version, name: &str, directory: &Path) -> Result<()> {
        let tag = version.to_string();
        let mut args = ["release", "download", &tag, "--pattern", name, "--dir"]
            .map(std::ffi::OsStr::new)
            .to_vec();
        args.push(directory.as_os_str());
        self.run(&args)
    }

    fn publish(&self, version: &Version) -> Result<()> {
        let tag = version.to_string();
        self.run(&["release", "edit", &tag, "--draft=false"].map(std::ffi::OsStr::new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_evidence_archive::Sha256;
    use oer_hil_run_bundle::store::Notes;

    #[test]
    fn the_commands_parse_with_the_pin_as_default() {
        let parse = |line: &str| {
            ArchiveCli::try_parse_from(std::iter::once("archive").chain(line.split_whitespace()))
        };
        assert!(matches!(
            parse("fetch --out c").unwrap().command,
            ArchiveCommand::Fetch { pin, .. } if pin == Path::new(PIN)
        ));
        assert!(matches!(
            parse("publish d --repository o/n --json").unwrap().command,
            ArchiveCommand::Publish { json: true, .. }
        ));
        assert!(parse("pack --out d").is_err());
        // A value that cannot be one is a usage error (exit 2), not a refusal.
        for line in [
            "ref not-a-run",
            "publish d --repository no-slash",
            "unpin someone",
        ] {
            let error = parse(line).unwrap_err();
            assert!(error.use_stderr(), "{line}");
        }
        assert!(parse("unpin selection-1-2-0").is_ok());
        assert!(parse("ref").is_err());
    }

    /// Two sessions select runs at once: each selection holds its own, and
    /// packing or dropping one never touches the other.
    #[test]
    fn two_selections_hold_and_hand_over_only_their_own_runs() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        for run in ["1-a", "1-b", "1-c"] {
            std::fs::create_dir_all(store.run(run)).unwrap();
        }
        let (first, second) = std::thread::scope(|scope| {
            let store = &store;
            let first =
                scope.spawn(move || select(store, &["1-a", "1-b"].map(String::from), "a", None));
            let second =
                scope.spawn(move || select(store, &["1-b", "1-c"].map(String::from), "b", None));
            (
                first.join().unwrap().unwrap(),
                second.join().unwrap().unwrap(),
            )
        });
        assert_ne!(first, second);
        let pins = || store.read::<Notes>(Sidecar::Pins).unwrap();
        assert_eq!(crate::pins::holders(&pins()["1-b"]).len(), 2);
        let run = |id: &str| RunRef {
            run: RunId::parse(id).unwrap(),
            sha256: Sha256::of(id.as_bytes()),
        };
        let manifest = Manifest::new(
            oer_hil_evidence_archive::Payload {
                sha256: Sha256::of(b"payload"),
                bytes: 1,
            },
            vec![run("1-a"), run("1-b")],
        )
        .unwrap();
        let version = manifest.version().to_string();
        hand_over(&store, &manifest, Some(&first), "a").unwrap();
        assert_eq!(crate::pins::holders(&pins()["1-a"]), [version.as_str()]);
        assert_eq!(
            crate::pins::holders(&pins()["1-b"]),
            [second.as_str(), version.as_str()]
        );
        assert_eq!(let_go(&store, &second).unwrap(), 2);
        assert!(!pins().contains_key("1-c"));
        assert_eq!(let_go(&store, &version).unwrap(), 2);
        assert!(pins().is_empty());
    }

    /// A pin taken while a prune holds the pins, which then deletes the run,
    /// waits and ends in a refusal: the run is not pinned.
    #[test]
    fn a_selection_during_a_prune_that_deletes_its_run_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        std::fs::create_dir_all(store.run("1-a")).unwrap();
        // The prune: it holds the pins shared from reading them.
        let pruning = store
            .lock_notes(Sidecar::Pins, oer_process::lock::Mode::Shared)
            .unwrap();
        std::thread::scope(|scope| {
            let store = &store;
            let selecting = scope.spawn(move || select(store, &[String::from("1-a")], "a", None));
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(
                !selecting.is_finished(),
                "the pin landed while the prune held the pins"
            );
            // The prune deletes the run, then lets the pins go.
            std::fs::remove_dir_all(store.run("1-a")).unwrap();
            drop(pruning);
            let error = selecting.join().unwrap().unwrap_err();
            assert!(error.is::<Refused>(), "{error}");
        });
        assert!(store.read::<Notes>(Sidecar::Pins).unwrap().is_empty());
    }

    /// A run deleted before its pin lands is refused, not pinned.
    #[test]
    fn a_selection_refuses_a_run_pruned_meanwhile() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        std::fs::create_dir_all(store.run("1-a")).unwrap();
        let error = select(&store, &["1-a", "1-b"].map(String::from), "a", None).unwrap_err();
        assert!(error.is::<Refused>(), "{error}");
        assert!(store.read::<Notes>(Sidecar::Pins).unwrap().is_empty());
    }

    /// A sealed run `id` in `store`: a manifest, a result and its seal.
    fn sealed(store: &RunStore, id: &str) -> RunRef {
        let directory = store.run(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::json!({ "run_id": id }).to_string(),
        )
        .unwrap();
        std::fs::write(directory.join("suite.json"), id).unwrap();
        oer_hil_run_bundle_format::run::write_integrity_index(&directory, id).unwrap();
        oer_hil_evidence_archive::run_reference(&directory, &RunId::parse(id).unwrap()).unwrap()
    }

    fn ids(runs: &[&str]) -> Vec<RunId> {
        runs.iter().map(|run| RunId::parse(run).unwrap()).collect()
    }

    /// Decision B through the CLI's steps: after pruning, `ref` takes the
    /// pinned version's runs from its manifest, and `pack` takes them from
    /// its copy; only the new run is pinned.
    #[test]
    fn a_new_version_after_pruning_through_the_cli() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path().join("store"));
        let out = directory.path().join("out");
        sealed(&store, "1-a");
        sealed(&store, "1-b");
        let (first, local) = references(&store, &ids(&["1-a", "1-b"]), None).unwrap();
        assert_eq!(local, ["1-a", "1-b"]);
        let selection = select(&store, &local, "a", None).unwrap();
        let refs = Refs {
            selection: Some(selection),
            runs: first,
        };
        let pinned = pack_version(&store, &refs, &out.join("v1"), "a", |_| Ok(None)).unwrap();
        let base = oer_hil_evidence_archive::copy::unpack(
            &out.join("v1")
                .join(oer_hil_evidence_archive::manifest::ARCHIVE),
            &pinned,
            &out.join("base"),
        )
        .unwrap();
        // Published, then pruned from the store.
        let_go(&store, &pinned.version().to_string()).unwrap();
        for run in ["1-a", "1-b"] {
            std::fs::remove_dir_all(store.run(run)).unwrap();
        }
        let third = sealed(&store, "1-c");
        let (second, local) =
            references(&store, &ids(&["1-a", "1-b", "1-c"]), Some(&pinned)).unwrap();
        assert_eq!(local, ["1-c"]);
        assert_eq!(second[..2], pinned.runs[..]);
        assert_eq!(second[2], third);
        let selection = select(&store, &local, "a", None).unwrap();
        let refs = Refs {
            selection: Some(selection),
            runs: second,
        };
        let manifest =
            pack_version(&store, &refs, &out.join("v2"), "a", move |_| Ok(Some(base))).unwrap();
        assert_eq!(manifest.runs.len(), 3);
        let pins = store.read::<Notes>(Sidecar::Pins).unwrap();
        assert_eq!(pins.keys().collect::<Vec<_>>(), ["1-c"]);
        assert_eq!(
            crate::pins::holders(&pins["1-c"]),
            [manifest.version().to_string().as_str()]
        );
        // Without the pinned version, a pruned run has no reference.
        assert!(references(&store, &ids(&["1-a"]), None).is_err());
    }

    /// `ref --selection` adds to a selection, and `holders` lists each
    /// holder with its runs, so a selection never packed is found.
    #[test]
    fn a_selection_is_reused_and_listed_with_its_runs() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        for run in ["1-a", "1-b"] {
            std::fs::create_dir_all(store.run(run)).unwrap();
        }
        let selection = select(&store, &[String::from("1-a")], "a", None).unwrap();
        let again = select(&store, &[String::from("1-b")], "a", Some(&selection)).unwrap();
        assert_eq!(again, selection);
        let holders = crate::pins::holders_of(&store.read::<Notes>(Sidecar::Pins).unwrap());
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[&selection], ["1-a", "1-b"]);
        assert!(parse_selection("evidence-0").is_err());
        assert!(parse_selection("selection-1-2-3").is_ok());
        for bad in [
            "selection-",
            "selection-1-2",
            "selection-1-2-3, evidence-0",
            "selection-a-b-c",
        ] {
            assert!(parse_selection(bad).is_err(), "{bad}");
        }
    }

    /// `publish` releases to the pinned repository; another one only before
    /// the first version or when the archive moves explicitly.
    #[test]
    fn a_version_is_published_to_the_pinned_repository() {
        let manifest = Manifest::new(
            oer_hil_evidence_archive::Payload {
                sha256: Sha256::of(b"payload"),
                bytes: 1,
            },
            vec![RunRef {
                run: RunId::parse("1-a").unwrap(),
                sha256: Sha256::of(b"seal"),
            }],
        )
        .unwrap();
        let repository = |name: &str| Repository::parse(name).unwrap();
        let pin = Pin::new(repository("o/evidence"), manifest);
        assert_eq!(
            publish_repository(None, Some(&pin), false).unwrap(),
            repository("o/evidence")
        );
        assert!(
            publish_repository(None, None, false)
                .unwrap_err()
                .is::<Refused>()
        );
        let other = publish_repository(Some(repository("x/typo")), Some(&pin), false);
        assert!(other.unwrap_err().is::<Refused>());
        assert_eq!(
            publish_repository(Some(repository("x/new")), Some(&pin), true).unwrap(),
            repository("x/new")
        );
        assert_eq!(
            publish_repository(Some(repository("o/first")), None, false).unwrap(),
            repository("o/first")
        );
    }

    #[test]
    fn a_refusal_is_one_the_environment_three() {
        assert_eq!(status(&Refused(String::from("digest"))), 1);
        assert_eq!(
            status(&*Box::<dyn std::error::Error + Send + Sync>::from("io")),
            3
        );
        assert_eq!(status(&oer_process::Cancelled), 130);
    }

    #[test]
    fn a_references_file_is_the_runs_list_and_nothing_else() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("refs.toml");
        let refs = Refs {
            selection: Some(String::from("selection-1-2-0")),
            runs: vec![RunRef {
                run: RunId::parse("1700000000000-ab").unwrap(),
                sha256: Sha256::of(b"seal"),
            }],
        };
        std::fs::write(&path, toml::to_string(&refs).unwrap()).unwrap();
        let read = read_refs(&path).unwrap();
        assert_eq!(
            (read.selection, read.runs),
            (refs.selection.clone(), refs.runs.clone())
        );
        std::fs::write(&path, "other = 1\n").unwrap();
        assert!(read_refs(&path).unwrap_err().is::<Refused>());
        // What `ref --json` prints reads back too.
        std::fs::write(&path, serde_json::to_string_pretty(&refs).unwrap()).unwrap();
        assert_eq!(read_refs(&path).unwrap().runs, refs.runs);
    }
}
