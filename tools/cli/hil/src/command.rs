//! The `cargo hil` command: the test commands handled here (runs, analyses,
//! experiments, the evidence archive, the firmware catalog), every other command
//! forwarded to the runner this crate builds. The stand's own commands are
//! `cargo stand`.
use crate::Result;
use oer_process::Checkout;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

pub fn run(ctx: &Checkout, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (options, args) = match LeaseOptions::split(args) {
        Ok(split) => split,
        // `cargo hil archive` keeps its exit codes (`archive::command`): a
        // usage error is 2.
        Err(error) if names_archive(args) => {
            eprintln!("hil: {error}");
            return Ok(std::process::ExitCode::from(2));
        }
        Err(error) => return Err(error),
    };
    let args = args.as_slice();
    // The archive reads the shared store itself; it needs no checkout link.
    if args.first().and_then(|argument| argument.to_str()) == Some("archive") {
        return crate::archive::command(ctx, options.owner(ctx), &args[1..]);
    }
    use_shared_store(ctx)?;
    let first = args.first().and_then(|argument| argument.to_str());
    match first {
        None | Some("help" | "--help" | "-h") => println!("{HIL_HELP}"),
        Some("__command-tree") => return command_tree(ctx),
        _ => {}
    }
    match first.and_then(HilCommand::named) {
        Some(HilCommand::Dashboard) => {
            return crate::dashboard::serve(&oer_hil_run_bundle::RunStore::shared()?, &args[1..]);
        }
        Some(HilCommand::Peer) => {
            return crate::peer::peer(ctx, options.owner(ctx)?, &args[1..]);
        }
        Some(HilCommand::Firmware) => return firmware(ctx, &args[1..]),
        Some(HilCommand::Images) => return crate::images::command(ctx, &args[1..]),
        Some(HilCommand::Observer) => return crate::images::observer(ctx),
        Some(HilCommand::Sweep) => {
            let apply = match args[1..] {
                [] => false,
                [ref flag] if flag == "--apply" => true,
                _ => return Err("usage: cargo hil sweep [--apply]".into()),
            };
            crate::sweep::run(&ctx.root, crate::sweep::Policy::default(), apply)?;
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(HilCommand::Runs) => return runs(ctx, &options, &args[1..]),
        // Dispatched before the shared store is linked.
        Some(HilCommand::Archive) => unreachable!("`archive` returns above"),
        Some(HilCommand::Perf) => return perf(ctx, &options, &args[1..]),
        Some(HilCommand::Profile) => return profile(ctx, &args[1..]),
        Some(HilCommand::Wait) => return wait(&args[1..]),
        Some(HilCommand::Ab) => return ab(ctx, &options.owner(ctx)?, args),
        Some(HilCommand::Bisect) => {
            return crate::experiments::bisect(ctx, &options.owner(ctx)?, &args[1..]);
        }
        Some(HilCommand::Revision) => {
            return crate::experiments::revision(ctx, &options.owner(ctx)?, &args[1..]);
        }
        None => {}
    }
    // The runner has no lease options: take them from after the command
    // too, before the runner is built.
    let (options, args) = options.with_late(args)?;
    let (enqueue, after, args) = crate::jobs::take(args)?;
    if (enqueue || after.is_some()) && !produces_runs(&args) {
        return Err("--enqueue and --after apply to run and run-all".into());
    }
    if enqueue {
        let frozen = freeze_enqueued(ctx, &args)?;
        let id = crate::jobs::enqueue(ctx, &options.owner(ctx)?, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let owner = match options.owner(ctx) {
        Ok(owner) => Some(owner),
        // Read-only runner commands need no registered stand owner. Invalid
        // overrides and registry/context failures must still reach the caller.
        Err(error) if error.is::<oer_stand_owners::NoOwner>() => None,
        Err(error) => return Err(error),
    };
    // Only a command that produces runs is a job; a read-only one is not.
    let mut job = if produces_runs(&args) {
        Some(oer_hil_experiment::job::Running::begin(
            &ctx.root,
            owner.as_deref().unwrap_or("unregistered"),
            &args,
            after.as_ref(),
        )?)
    } else {
        None
    };
    let args = args.as_slice();
    // An enqueued job runs the runner and sources fixed when it was
    // enqueued.
    let frozen = crate::jobs::Frozen::inherited()?;
    let runner = match &frozen {
        Some(frozen) => oer_hil_experiment::launch::Runner {
            executable: frozen.runner.clone(),
            receipt: Some(frozen.receipt.clone()),
        },
        None => oer_hil_experiment::launch::Runner::prepare(&ctx.root)?,
    };
    let mut runner_args = match frozen
        .as_ref()
        .and_then(|frozen| frozen.snapshot.as_deref())
    {
        Some(snapshot) => with_source_snapshot(args, snapshot),
        None => args.to_vec(),
    };
    apply_quarantine(&mut runner_args)?;
    let mut launch = oer_hil_experiment::launch::Launch::new(&ctx.root, &runner).args(runner_args);
    // The runner's stand requests are the job's tickets.
    let mut context = oer_process::Context::current()?.clone();
    if let Some(owner) = owner {
        launch = launch.env(oer_stand_owners::OWNER_ENV, &owner);
        context.set(oer_stand_owners::OWNER_KEY, owner);
    }
    if let Some(job) = &job {
        context.set(oer_stand_arbiter::jobs::JOB_KEY, job.id());
    }
    launch = launch.context(context);
    let launched = oer_hil_experiment::launch::launch_run(&launch)?;
    if produces_runs(args) {
        // Name the runs in the receipt of whatever invoked this command.
        oer_hil_run_bundle::receipt::record(&launched.runs)?;
        let store = oer_hil_run_bundle::RunStore::shared()?;
        if let Some(job) = job.as_mut() {
            job.finish_with(&store, &launched.runs)?;
        }
        // Ctrl-C during the pruning, such as while it waits for the pins,
        // ends the command (exit 130); any other failure is only reported.
        if let Err(error) = prune_automatically(ctx, &store) {
            if oer_process::is_cancelled(&*error) {
                return Err(error);
            }
            eprintln!("hil: automatic pruning of the run store failed: {error}");
        }
    }
    Ok(oer_process::exit_code(launched.status))
}

/// Whether `args` run `cargo hil archive`, which has its own exit codes.
fn names_archive(args: &[OsString]) -> bool {
    args.iter().any(|argument| argument == "archive")
}

/// The commands handled here, printed before the runner's own help; every
/// other command goes to the runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HilCommand {
    Dashboard,
    Peer,
    Firmware,
    Images,
    Observer,
    Sweep,
    Runs,
    Perf,
    Archive,
    Profile,
    Bisect,
    Revision,
    Ab,
    Wait,
}

impl HilCommand {
    const ALL: [Self; 14] = [
        Self::Dashboard,
        Self::Peer,
        Self::Firmware,
        Self::Images,
        Self::Observer,
        Self::Sweep,
        Self::Runs,
        Self::Perf,
        Self::Archive,
        Self::Profile,
        Self::Bisect,
        Self::Revision,
        Self::Ab,
        Self::Wait,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Peer => "peer",
            Self::Firmware => "firmware",
            Self::Images => "images",
            Self::Observer => "observer",
            Self::Sweep => "sweep",
            Self::Runs => "runs",
            Self::Perf => "perf",
            Self::Archive => "archive",
            Self::Profile => "profile",
            Self::Bisect => "bisect",
            Self::Revision => "revision",
            Self::Ab => "ab",
            Self::Wait => "wait",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.name() == name)
    }
}

pub(crate) const HIL_HELP: &str = "\
HIL test commands (the stand's own commands, leases, boards, owners and the
queue, are `cargo stand`):
  cargo hil perf report|baseline|check   gated measurements per commit, baselines, regressions
  cargo hil profile RUN [--scenario S] [--repetition N] [--top N]   symbolized program-counter profiles
  cargo hil bisect --good A --bad B --scenario S [--layout-seed N]   first commit at which S stops passing
  cargo hil revision --rev R --scenario S... [--layout-seed N]   S on revision R with the runner of its HIL wire
  cargo hil ab --a VARIANT --b VARIANT --scenario S [--repetitions N] [--layout-seeds K] [--enqueue] [--after JOB]
                                      A/B comparison with noise-aware verdicts; --enqueue makes it a job
  cargo hil run ... --enqueue [--after JOB]   start the run detached as a job and print its id
  cargo hil run S... --repetitions N  each scenario N times instead of its own count
  cargo hil wait JOB|RUN              block until the job or run ends; exit 0 passed, 1 failed, 2 interrupted, 3 blocked, 4 broken, 5 no run, 6 abandoned
  cargo hil peer send BOARD LINE... [--for 5s]                   one peer text-protocol command and its answer
  cargo hil dashboard [--port 8765]   live page of the queue, boards, runs and leases on 127.0.0.1
  cargo hil runs list [--scenario S] [--outcome O] [--image I] [--since 3d]
  cargo hil runs why RUN              why a run did not pass: failure, missed criteria, log tail
  cargo hil runs compare A B [--measurement TEXT]   measurements side by side, judged against their noise
  cargo hil runs history SCENARIO [--measurement TEXT]
  cargo hil runs pin RUN --reason TEXT | unpin RUN
  cargo hil runs prune [--days 30] [--keep-failed 5] [--apply]
  cargo hil archive ref RUN... [--selection SELECTION]   the references (run, SHA-256 of its seal) of runs; pins the store's for a selection
  cargo hil archive pack --refs FILE --out DIR [--pin FILE]   [network] the cited runs as an evidence archive version
  cargo hil archive unpin VERSION|SELECTION   drop the pins of a version never published or of a selection
  cargo hil archive holders           the archive's holders of pins (selections, versions) and their runs
  cargo hil archive publish DIR [--repository OWNER/NAME [--new-repository]] [--pin FILE]   [network] release it in the pinned repository, write the pin
  cargo hil archive fetch --out DIR [--pin FILE]   [network] the pinned version, verified, for qualification
  cargo hil firmware list             tracked ESP-IDF images (peers, vendor references)
  cargo hil firmware build IMAGE      build against the one pinned ESP-IDF
  cargo hil images check --all|--class C|--reaching PKG [--type-check]   image classes with their audits
  cargo hil images compare --base REV [--class C]   image classes at REV and here, modulo placement
  cargo hil observer                  prepare the observer configuration without running HIL
  cargo hil sweep [--apply]           list (or remove) this checkout's unused incremental and image caches

Lease options, before any HIL command, after `lease`, or among a runner command's arguments:
  --owner NAME     default: enclosing lease owner, else the checkout directory name

Leases on different boards run in parallel. There is no budget to request:
every lease charges its owner's balance the time it holds, and waiting for a
lease that blocks you credits your balance the time waited. Among
conflicting requests the owner with the highest balance goes first, the
earlier request on a tie; balances halve every 2h and stay within 1h either
way. A run of several scenarios that has held 10m yields after its current
scenario to a waiter with a higher balance and queues again; a single
scenario or lease command runs to its end. Every lease ends at 1h (lease
exit status 124). `cargo stand queue` shows every balance and who goes next.
Scenarios tagged `air-exclusive` claim the air exclusively.

Runs never write tracked files. Evidence is the runs a qualification decision
cites, packed into an immutable evidence archive version that
`qualification/evidence/archive.toml` pins (`cargo hil archive`).

Runner commands (`cargo hil run A B C` runs several scenarios under one lease):";

/// Stand lease options accepted before the HIL command, or after `lease`,
/// and after a runner command.
#[derive(Clone, Debug, Default, PartialEq)]
struct LeaseOptions {
    owner: Option<String>,
}

impl LeaseOptions {
    /// Split a leading `--owner NAME` from the remaining arguments.
    fn split(args: &[OsString]) -> Result<(Self, Vec<OsString>)> {
        let mut options = Self::default();
        let mut rest = args.iter();
        let mut remaining = Vec::new();
        while let Some(argument) = rest.next() {
            let text = argument.to_str().unwrap_or_default();
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (text, None),
            };
            let mut value = || -> Result<String> {
                inline
                    .clone()
                    .or_else(|| {
                        rest.next()
                            .and_then(|value| value.to_str().map(str::to_owned))
                    })
                    .ok_or_else(|| format!("{name} requires a value").into())
            };
            match name {
                "--owner" => {
                    options.owner =
                        Some(oer_stand_owners::Owner::new(value()?.trim())?.to_string());
                }
                _ => {
                    remaining.push(argument.clone());
                    remaining.extend(rest.cloned());
                    break;
                }
            }
        }
        Ok((options, remaining))
    }

    /// Take lease options that follow a runner command, up to a `--`, and
    /// merge them with those before it; different owners are refused.
    fn with_late(self, args: &[OsString]) -> Result<(Self, Vec<OsString>)> {
        let mut options = self;
        let mut remaining = Vec::new();
        let mut rest = args.iter();
        while let Some(argument) = rest.next() {
            let text = argument.to_str().unwrap_or_default();
            if text == "--" {
                remaining.push(argument.clone());
                remaining.extend(rest.by_ref().cloned());
                break;
            }
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (text, None),
            };
            let owner = match name {
                "--owner" => match inline {
                    Some(value) => value.to_owned(),
                    None => rest
                        .next()
                        .and_then(|value| value.to_str())
                        .ok_or("--owner requires a value")?
                        .to_owned(),
                },
                _ => {
                    remaining.push(argument.clone());
                    continue;
                }
            };
            let owner = oer_stand_owners::Owner::new(owner.trim())?.to_string();
            if let Some(earlier) = options.owner.as_ref().filter(|earlier| **earlier != owner) {
                return Err(format!("--owner is given twice, as {earlier} and {owner}").into());
            }
            options.owner = Some(owner);
        }
        Ok((options, remaining))
    }

    /// Explicit options win; otherwise an enclosing lease's owner, otherwise
    /// the owner registered for this checkout. Nothing is derived from the
    /// checkout's directory name.
    fn owner(&self, ctx: &Checkout) -> Result<String> {
        Ok(oer_stand_owners::resolve(self.owner.as_deref(), &ctx.root)?.to_string())
    }
}

/// `cargo hil __command-tree`: every `cargo hil` command path with its
/// subcommands and long flags, as JSON, for checking documentation against
/// the real command line. The stand's own commands and the runner's are
/// merged under `hil`.
fn command_tree(ctx: &Checkout) -> Result<std::process::ExitCode> {
    use clap::CommandFactory as _;
    use oer_command_tree::{CommandNode, command_tree as walk};
    let path = |words: &[&str]| {
        words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>()
    };
    let node = |words: &[&str], subcommands: &[&str], flags: &[&str]| CommandNode {
        path: path(words),
        subcommands: subcommands.iter().map(|word| word.to_string()).collect(),
        flags: flags.iter().map(|word| word.to_string()).collect(),
        forwards: false,
    };
    let runner = oer_hil_observer::prepare::prepare(&ctx.root)?.runner;
    let output = oer_process::command(&runner)
        .arg("__command-tree")
        .output()?;
    let mut runner_nodes: Vec<CommandNode> = serde_json::from_slice(&output.stdout)?;
    let runner_top = runner_nodes
        .first()
        .map(|root| root.subcommands.clone())
        .unwrap_or_default();
    // The stand adds lease and evidence options to the runner's run commands.
    for node in &mut runner_nodes {
        if matches!(node.path.as_slice(), [one] if ["run", "run-all"].contains(&one.as_str())) {
            node.flags
                .extend(["--owner", "--enqueue", "--after", "--after-any"].map(String::from));
        }
        node.path.insert(0, String::from("hil"));
    }
    let stand = HilCommand::ALL.map(HilCommand::name);
    let mut root = node(&["hil"], &stand, &["--owner"]);
    root.subcommands.extend(runner_top);
    let mut nodes = vec![
        root,
        node(&["hil", "dashboard"], &[], &["--port"]),
        node(&["hil", "wait"], &[], &[]),
    ];
    for (name, command) in [
        ("perf", PerfCli::command()),
        ("archive", crate::archive::ArchiveCli::command()),
        ("runs", RunsCli::command()),
        ("firmware", FirmwareCli::command()),
        ("images", crate::images::ImagesCli::command()),
        ("peer", crate::peer::PeerCli::command()),
        ("profile", ProfileCli::command()),
        ("bisect", crate::experiments::BisectCli::command()),
        ("revision", crate::experiments::RevisionCli::command()),
        ("ab", crate::experiments::AbCli::command()),
    ] {
        nodes.extend(walk(&command, &path(&["hil", name])));
    }
    // The stand makes `ab` a job as it does a run.
    if let Some(ab) = nodes
        .iter_mut()
        .find(|node| node.path == path(&["hil", "ab"]))
    {
        ab.flags
            .extend(["--enqueue", "--after", "--after-any"].map(String::from));
    }
    nodes.extend(runner_nodes.into_iter().skip(1));
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil ab ...`, a job like a run: `--enqueue` starts it detached and
/// prints its id for `cargo hil wait`, and `--after JOB` orders it.
fn ab(ctx: &Checkout, owner: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (enqueue, after, args) = crate::jobs::take(args.to_vec())?;
    if enqueue {
        let frozen = crate::jobs::Frozen::capture(ctx)?;
        let id = crate::jobs::enqueue(ctx, owner, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let mut job = oer_hil_experiment::job::Running::begin(&ctx.root, owner, &args, after.as_ref())?;
    // Every round runs the runner of the experiment's start: a pull into the
    // checkout meanwhile must not change the arms' protocol.
    let frozen = match crate::jobs::Frozen::inherited()? {
        Some(frozen) => frozen,
        None => crate::jobs::Frozen::capture(ctx)?,
    };
    let runner = oer_hil_experiment::launch::Runner {
        executable: frozen.runner,
        receipt: Some(frozen.receipt),
    };
    let runs = crate::experiments::ab(ctx, owner, &runner, &args[1..])?;
    let (ids, outcomes): (Vec<_>, Vec<_>) = runs.into_iter().unzip();
    job.finish(&ids, &outcomes)?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil wait ID`: block until the job or run ID names ends, and exit
/// with its outcome. A job is waited for through its record, a run by
/// following its bundle.
fn wait(args: &[OsString]) -> Result<std::process::ExitCode> {
    let [id] = args else {
        return Err("usage: cargo hil wait JOB|RUN".into());
    };
    let text = id.to_str().ok_or("an id is text")?;
    if oer_stand_arbiter::Arbiter::open()?
        .jobs()
        .read(text)
        .is_ok()
    {
        return crate::jobs::wait_command(args);
    }
    let run = oer_hil_analysis::Run::open(&oer_hil_run_bundle::RunStore::shared()?, text)
        .map_err(|_| format!("{text} is neither a job nor a run"))?;
    Ok(std::process::ExitCode::from(oer_hil_analysis::runs::wait(
        run.directory(),
        |line| println!("{line}"),
    )?))
}

fn parse_budget(text: &str) -> std::result::Result<std::time::Duration, String> {
    oer_stand_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil runs list|why|compare|history|pin|unpin|prune` over the shared
/// run store.
/// `cargo hil perf`: gated measurements across commits and their baselines.
fn perf(
    ctx: &Checkout,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    use oer_hil_analysis::perf;
    let store = oer_hil_run_bundle::RunStore::shared()?;
    let now = oer_durable::unix_millis();
    let find = |id: &str| -> Result<perf::RunSummary> {
        perf::summary(&oer_hil_analysis::Run::open(&store, id)?)
    };
    let baselines = perf::baselines(&store)?;
    match PerfCli::try_parse_from(args)? {
        PerfCli::Report {
            scenarios,
            measurement,
            since,
        } => {
            let runs = perf::summaries_since(&store, now.saturating_sub(since.as_millis() as u64))?;
            print!(
                "{}",
                perf::report(&runs, &scenarios, measurement.as_deref(), &baselines)
            );
        }
        PerfCli::Baseline {
            run,
            scenarios,
            reason,
        } => {
            let run = find(&run)?;
            let reason = if run.networks.is_empty() {
                reason
            } else {
                format!("{reason} [network {}]", run.networks.join(","))
            };
            let set =
                perf::set_baseline(&store, &run, &scenarios, &reason, &options.owner(ctx)?, now)?;
            println!("baseline {} for {}", run.id, set.join(", "));
        }
        PerfCli::Check { run } => {
            let regressions = perf::regressions(&find(&run)?, &baselines);
            if regressions.is_empty() {
                println!("no gated measurement of {run} regressed against its baseline");
            } else {
                for line in &regressions {
                    println!("REGRESSED {line}");
                }
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil runs list|show|why|compare|history|pin|unpin|flaky|quarantine|release|prune`
/// over the shared run store.
fn runs(
    ctx: &Checkout,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    use oer_hil_analysis::{Run, retention, runs};
    use oer_hil_run_bundle::store::Note;
    use oer_hil_run_bundle::store::Notes;
    use oer_hil_run_bundle::store::Sidecar;
    let store = oer_hil_run_bundle::RunStore::shared()?;
    let parsed = RunsCli::try_parse_from(args)?;
    // Reading every run takes long; commands about one run read only it.
    let all = || Run::all(&store);
    let find = |id: &str| Run::open(&store, id);
    let now = oer_durable::unix_millis();
    let note = |reason: String| -> Result<Note> {
        Ok(Note {
            by: options.owner(ctx)?,
            reason,
            unix_millis: now,
        })
    };
    match parsed {
        RunsCli::List {
            scenario,
            outcome,
            image,
            since,
            limit,
        } => {
            let filter = runs::Filter {
                scenario,
                outcome,
                image,
                since_millis: since.map(|since| now.saturating_sub(since.as_millis() as u64)),
            };
            let all = all()?;
            let matching = all
                .iter()
                .filter(|run| filter.matches(run))
                .collect::<Vec<_>>();
            for run in &matching[matching.len().saturating_sub(limit)..] {
                println!("{}", runs::list_line(run));
            }
        }
        RunsCli::Show { run } => print!("{}", runs::show(&find(&run)?)),
        RunsCli::Why { run, tail } => print!("{}", runs::why(&find(&run)?, tail)),
        RunsCli::Compare { a, b, measurement } => print!(
            "{}",
            runs::compare(&find(&a)?, &find(&b)?, measurement.as_deref())?
        ),
        RunsCli::History {
            scenario,
            measurement,
            limit,
        } => {
            let containing = all()?
                .into_iter()
                .filter(|run| run.scenario(&scenario).is_some())
                .collect::<Vec<_>>();
            let start = containing.len().saturating_sub(limit);
            print!(
                "{}{}",
                runs::stability(&containing, &scenario),
                runs::history(&containing[start..], &scenario, measurement.as_deref())?
            );
        }
        RunsCli::Pin { run, reason } => {
            find(&run)?;
            crate::pins::pin_existing_by_hand(&store, &run, note(reason)?)?;
        }
        RunsCli::Unpin { run } => {
            let held =
                store.update_notes(Sidecar::Pins, |pins| crate::pins::unpin_by_hand(pins, &run))?;
            if !held.is_empty() {
                eprintln!(
                    "hil: run {run} stays pinned for the evidence archive by {}; \
                     `cargo hil archive unpin HOLDER` drops a holder",
                    held.join(", ")
                );
            }
        }
        RunsCli::Flaky { since, minimum } => {
            let recent = now.saturating_sub(since.as_millis() as u64);
            let runs = all()?
                .into_iter()
                .filter(|run| run.started_millis() >= recent)
                .collect::<Vec<_>>();
            print!(
                "{}",
                runs::flaky_report(
                    &runs::stabilities(&runs),
                    minimum,
                    &store.read::<Notes>(Sidecar::Quarantine)?
                )
            );
        }
        RunsCli::Quarantine { scenario, reason } => {
            store.note(Sidecar::Quarantine, &scenario, Some(note(reason)?))?;
        }
        RunsCli::Release { scenario } => store.note(Sidecar::Quarantine, &scenario, None)?,
        RunsCli::Prune {
            days,
            keep_failed,
            apply,
            unreadable,
        } => {
            let pruned = retention::prune(
                &store,
                &retention::Retention {
                    keep_days: days,
                    keep_failed,
                },
                None,
                apply,
                unreadable,
                &captures()?,
            )?;
            for (run, bytes) in &pruned.removed_unreadable {
                let schema = run
                    .schema
                    .map_or_else(|| String::from("unknown"), |schema| schema.to_string());
                println!(
                    "{} {} ({} MiB held only by it), unreadable, schema {schema}: {}",
                    if apply { "deleted" } else { "would delete" },
                    run.id,
                    bytes >> 20,
                    run.reason
                );
            }
            for (run, why) in &pruned.unreadable_kept {
                println!("kept {}, unreadable ({}): {why}", run.id, run.reason);
            }
            for (run, bytes) in &pruned.removed {
                if apply {
                    println!("deleted {} ({} MiB)", run.id(), bytes >> 20);
                } else {
                    println!("would delete {}", runs::list_line(run));
                }
            }
            if apply {
                if pruned.observers > 0 {
                    println!(
                        "deleted {} observer builds no kept run names",
                        pruned.observers
                    );
                }
                if pruned.sources.objects > 0 || pruned.sources.captures > 0 {
                    println!(
                        "deleted {} source objects ({} MiB) and {} captures nothing uses",
                        pruned.sources.objects,
                        pruned.sources.bytes >> 20,
                        pruned.sources.captures
                    );
                }
                let objects = collect_objects(ctx);
                if objects.objects > 0 {
                    println!(
                        "deleted {} firmware objects no run links to ({} MiB)",
                        objects.objects,
                        objects.bytes >> 20
                    );
                }
            }
            println!(
                "{} {} of {} runs, {} MiB held only by them; {} kept{}",
                if apply { "deleted" } else { "would delete" },
                pruned.removed.len() + pruned.removed_unreadable.len(),
                pruned.total,
                pruned.freed() >> 20,
                pruned.kept,
                if apply { "" } else { "; add --apply to delete" }
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil firmware list | build IMAGE`.
fn firmware(ctx: &Checkout, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    match FirmwareCli::try_parse_from(args)? {
        FirmwareCli::List => crate::firmware_catalog::list(ctx)?,
        FirmwareCli::Build { image } => {
            crate::firmware_catalog::build(ctx, &image)?;
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Delete the firmware objects no run links to from this checkout's object
/// store and those of the other registered checkouts; a checkout that
/// cannot be collected is reported and skipped.
fn collect_objects(ctx: &Checkout) -> oer_hil_run_bundle::build::CollectedObjects {
    let mut checkouts = vec![ctx.root.clone()];
    if let Ok(owners) = oer_stand_owners::Owners::open()
        && let Ok(registered) = owners.checkouts()
    {
        checkouts.extend(registered);
    }
    checkouts.sort();
    checkouts.dedup();
    let mut total = oer_hil_run_bundle::build::CollectedObjects::default();
    for checkout in checkouts {
        // Build objects stay per chip.
        let chips = oer_chip_profile::supported(&checkout).unwrap_or_default();
        for target in chips
            .iter()
            .map(|chip| checkout.join("target/hil").join(chip))
        {
            match oer_hil_run_bundle::build::collect_objects(&target) {
                Ok(collected) => {
                    total.objects += collected.objects;
                    total.bytes += collected.bytes;
                }
                Err(error) => eprintln!(
                    "hil: cannot collect the firmware objects of {}: {error}",
                    target.display()
                ),
            }
        }
    }
    total
}

/// Rule of the automatic pruning, which `cargo hil runs prune` defaults to.
const PRUNE_DAYS: u64 = 30;
const PRUNE_KEEP_FAILED: usize = 5;
/// Automatic pruning runs at most once per this interval.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete, at most once a day, the runs of the shared store that no rule
/// keeps; see `cargo hil runs prune`.
/// The run store's size budget: `OER_HIL_RUN_STORE_BUDGET_GIB` gibibytes,
/// 40 by default.
fn run_store_budget() -> Result<u64> {
    let gibibytes = match std::env::var(RUN_STORE_BUDGET_ENV) {
        Ok(text) => text
            .parse::<u64>()
            .map_err(|_| format!("{RUN_STORE_BUDGET_ENV} is not a number of GiB: {text}"))?,
        Err(_) => RUN_STORE_BUDGET_GIB,
    };
    Ok(gibibytes << 30)
}

const RUN_STORE_BUDGET_ENV: &str = "OER_HIL_RUN_STORE_BUDGET_GIB";
const RUN_STORE_BUDGET_GIB: u64 = 40;

/// The captures source collection weighs: the host's capture store and what
/// queued or running jobs hold.
fn captures() -> Result<oer_hil_analysis::runs::Captures> {
    Ok(oer_hil_analysis::runs::Captures {
        stores: vec![oer_hil_image::source_snapshot_store()?],
        held: crate::sweep::held_by_jobs()?,
    })
}

fn prune_automatically(ctx: &Checkout, store: &oer_hil_run_bundle::RunStore) -> Result<()> {
    let marker = store.root().join("last-prune");
    if std::fs::metadata(&marker)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < PRUNE_INTERVAL)
    {
        return Ok(());
    }
    std::fs::write(&marker, b"")?;
    let pruned = oer_hil_analysis::retention::prune(
        store,
        &oer_hil_analysis::retention::Retention {
            keep_days: PRUNE_DAYS,
            keep_failed: PRUNE_KEEP_FAILED,
        },
        Some(run_store_budget()?),
        true,
        false,
        &captures()?,
    )?;
    let objects = collect_objects(ctx);
    if objects.objects > 0 {
        eprintln!(
            "hil: deleted {} firmware objects no run links to ({} MiB)",
            objects.objects,
            objects.bytes >> 20
        );
    }
    if !pruned.removed.is_empty() || !pruned.removed_unreadable.is_empty() {
        eprintln!(
            "hil: pruned {} runs ({} MiB) no rule keeps; see `cargo hil runs prune`",
            pruned.removed.len() + pruned.removed_unreadable.len(),
            pruned.freed() >> 20
        );
    }
    Ok(())
}

/// Make this checkout's run directory a link to the shared store.
fn use_shared_store(ctx: &Checkout) -> Result<()> {
    oer_hil_run_bundle::RunStore::shared()?.link(&ctx.root)?;
    Ok(())
}

/// Check an enqueued `run`'s scenarios and options with the runner now, so
/// a mistake shows in the terminal instead of in a job that ends no-run
/// minutes later.
/// Check an enqueued command and fix what it runs with: this binary, the
/// runner and, for a `run` that builds, a source snapshot of the checkout
/// taken now with the run's own source options.
fn freeze_enqueued(ctx: &Checkout, args: &[OsString]) -> Result<crate::jobs::Frozen> {
    let mut frozen = crate::jobs::Frozen::capture(ctx)?;
    if args.first().and_then(|arg| arg.to_str()) != Some("run") {
        return Ok(frozen);
    }
    let output = oer_process::output(
        ctx.command(&frozen.runner)
            .args(args)
            .arg("--validate-only"),
        Some(std::time::Duration::from_secs(120)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "not enqueued: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    if has_flag(args, "--firmware-from") {
        return Ok(frozen);
    }
    if has_flag(args, "--source-snapshot") {
        build_before_queue(ctx, &frozen, args)?;
        return Ok(frozen);
    }
    let output = oer_process::output(
        ctx.command(&frozen.runner)
            .args(["image", "snapshot"])
            .args(source_options(args)),
        Some(std::time::Duration::from_secs(600)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "not enqueued: the source snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let directory = snapshot["directory"]
        .as_str()
        .ok_or("the source snapshot names no directory")?;
    frozen.snapshot = Some(PathBuf::from(directory));
    build_before_queue(
        ctx,
        &frozen,
        &with_source_snapshot(args, Path::new(directory)),
    )?;
    Ok(frozen)
}

/// Build and audit a run's images with its fixed runner before the job
/// waits: a build or audit that fails does so now, in the terminal, not
/// after the queue.
fn build_before_queue(
    ctx: &Checkout,
    frozen: &crate::jobs::Frozen,
    run_args: &[OsString],
) -> Result<()> {
    eprintln!("hil: building the run's images before it queues");
    let status = ctx
        .command(&frozen.runner)
        .args(run_args)
        .arg("--build-only")
        .status()?;
    if !status.success() {
        return Err("not enqueued: the run's images did not build (see above)".into());
    }
    Ok(())
}

/// Leave quarantined scenarios out of a `run-all`, and warn of a `run` that
/// names one.
fn apply_quarantine(args: &mut Vec<OsString>) -> Result<()> {
    let quarantined: oer_hil_run_bundle::store::Notes = oer_hil_run_bundle::RunStore::shared()?
        .read(oer_hil_run_bundle::store::Sidecar::Quarantine)?;
    match args.first().and_then(|arg| arg.to_str()) {
        Some("run-all") => {
            for scenario in quarantined.keys() {
                args.push("--exclude".into());
                args.push(scenario.into());
            }
        }
        Some("run") => {
            for (scenario, entry) in &quarantined {
                if args.iter().any(|arg| arg.to_str() == Some(scenario)) {
                    eprintln!(
                        "hil: `{scenario}` is quarantined ({}); running it as asked",
                        entry.reason
                    );
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether `args` hold the option `name`, alone or as `name=value`.
fn has_flag(args: &[OsString], name: &str) -> bool {
    args.iter().any(|arg| {
        arg.to_str().is_some_and(|arg| {
            arg == name
                || arg
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

/// The options of `args` that choose a run's sources: they make its snapshot.
const SOURCE_OPTIONS: [(&str, bool); 2] =
    [("--source-include", true), ("--include-untracked", false)];

/// `args`' source options, with their values.
fn source_options(args: &[OsString]) -> Vec<OsString> {
    split_source_options(args).0
}

/// `args` split into the source options and the rest.
fn split_source_options(args: &[OsString]) -> (Vec<OsString>, Vec<OsString>) {
    let (mut sources, mut rest) = (Vec::new(), Vec::new());
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        let text = argument.to_str().unwrap_or_default();
        match SOURCE_OPTIONS.iter().find(|(name, _)| {
            text == *name
                || text
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
        }) {
            Some((name, takes_value)) => {
                sources.push(argument.clone());
                if *takes_value && text == *name {
                    sources.extend(arguments.next().cloned());
                }
            }
            None => rest.push(argument.clone()),
        }
    }
    (sources, rest)
}

/// The runner arguments of a run whose sources were captured into
/// `snapshot` when it was enqueued: its source options already made it.
fn with_source_snapshot(args: &[OsString], snapshot: &Path) -> Vec<OsString> {
    let mut arguments = split_source_options(args).1;
    arguments.push("--source-snapshot".into());
    arguments.push(snapshot.into());
    arguments
}

/// Commands that execute scenarios and write run bundles.
fn produces_runs(args: &[OsString]) -> bool {
    matches!(
        args.first().and_then(|a| a.to_str()),
        Some("run" | "run-all")
    )
}

#[derive(clap::Parser)]
#[command(name = "cargo hil perf", no_binary_name = true)]
enum PerfCli {
    /// Gated measurements of clean runs per commit, against each
    /// scenario's baseline.
    Report {
        /// Scenario IDs; every scenario with a gated measurement when omitted.
        scenarios: Vec<String>,
        /// Only measurements whose name contains this text.
        #[arg(long)]
        measurement: Option<String>,
        /// Only runs this recent, e.g. 7d.
        #[arg(long, value_parser = parse_budget, default_value = "14d")]
        since: std::time::Duration,
    },
    /// Make a clean sealed run the baseline of its scenarios.
    Baseline {
        run: String,
        /// Scenarios to set; all gated scenarios of the run when omitted.
        #[arg(long = "scenario")]
        scenarios: Vec<String>,
        #[arg(long)]
        reason: String,
    },
    /// Fail when a run's gated measurements regressed against the
    /// baselines.
    Check { run: String },
}

/// `cargo hil profile RUN [--scenario S] [--repetition N] [--top N]`.
#[derive(clap::Parser)]
#[command(name = "cargo hil profile", no_binary_name = true)]
struct ProfileCli {
    /// The run whose profiled repetitions to report.
    run: String,
    /// Only this scenario's repetitions.
    #[arg(long)]
    scenario: Option<String>,
    /// Only this repetition number.
    #[arg(long)]
    repetition: Option<u8>,
    /// The most sampled functions shown per hart.
    #[arg(long, default_value_t = 15)]
    top: usize,
}

/// Where `cargo hil profile` keeps the report of the profile at `relative`
/// (its directory below the scenario) of `run`'s `scenario`: under this
/// checkout's target directory, never inside the sealed run bundle.
fn profile_report_path(root: &Path, run: &str, scenario: &str, relative: &Path) -> PathBuf {
    root.join("target/hil/profiles")
        .join(run)
        .join(scenario)
        .join(relative)
        .join("profile.txt")
}

/// Report every `profile.json` a run's repetitions left, symbolized against
/// the run's own image, and keep each report under this checkout's
/// `target/hil/profiles/`.
fn profile(ctx: &Checkout, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = ProfileCli::try_parse_from(args)?;
    let run = oer_hil_run_bundle::RunStore::shared()?
        .open(&cli.run)?
        .directory()
        .to_owned();
    let mut found = 0;
    let mut scenarios = fs::read_dir(run.join("scenarios"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    scenarios.sort();
    for scenario in scenarios {
        let id = scenario
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if cli.scenario.as_ref().is_some_and(|wanted| *wanted != id) {
            continue;
        }
        // The scenario's result names the image class every family ran.
        let image = fs::read(scenario.join("result.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|result| result["image"].as_str().map(str::to_owned));
        let elf = image
            .map(|image| run.join("firmware").join(image).join("runtime.elf"))
            .filter(|elf| oer_hil_run_bundle_format::archived::stored_path(elf).is_file())
            .map(|elf| oer_hil_run_bundle_format::archived::read_archived(&elf))
            .transpose()?;
        let mut profiles = Vec::new();
        collect_profiles(&scenario, &mut profiles)?;
        profiles.sort();
        for path in profiles {
            let repetition = path
                .strip_prefix(&scenario)
                .ok()
                .and_then(|relative| relative.components().next())
                .and_then(|first| {
                    first
                        .as_os_str()
                        .to_str()?
                        .strip_prefix("repetition-")?
                        .parse::<u8>()
                        .ok()
                });
            if cli
                .repetition
                .is_some_and(|wanted| Some(wanted) != repetition)
            {
                continue;
            }
            let report = oer_hil_analysis::profile::report(&path, elf.as_deref(), cli.top)?;
            let relative = path
                .parent()
                .unwrap_or(&path)
                .strip_prefix(&scenario)
                .unwrap_or(Path::new(""));
            println!("== {id} {}", relative.display());
            print!("{report}");
            // The run bundle is sealed: a derived report written into it
            // would fail its integrity inventory.
            let destination = profile_report_path(&ctx.root, &cli.run, &id, relative);
            fs::create_dir_all(destination.parent().ok_or("profile report has no parent")?)?;
            oer_durable::atomic_write(&destination, report.as_bytes())?;
            eprintln!("hil: report {}", destination.display());
            found += 1;
        }
    }
    if found == 0 {
        return Err(format!(
            "run {} has no profile.json in the selected repetitions",
            cli.run
        )
        .into());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn collect_profiles(
    directory: &std::path::Path,
    found: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_profiles(&path, found)?;
        } else if path.file_name().is_some_and(|name| name == "profile.json") {
            found.push(path);
        }
    }
    Ok(())
}

#[derive(clap::Parser)]
#[command(name = "cargo hil runs", no_binary_name = true)]
enum RunsCli {
    /// Runs of every checkout, newest last.
    List {
        #[arg(long)]
        scenario: Option<String>,
        /// passed, failed, broken, blocked, interrupted; with --scenario,
        /// that scenario's outcome.
        #[arg(long)]
        outcome: Option<String>,
        /// Image class or application SHA-256 prefix.
        #[arg(long)]
        image: Option<String>,
        /// Only runs this recent, e.g. 3d or 12h.
        #[arg(long, value_parser = parse_budget)]
        since: Option<std::time::Duration>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Where a run's artifacts are: its directory in the shared store and
    /// each repetition's artifact directory.
    Show {
        run: String,
    },
    /// Why a run did not pass.
    Why {
        run: String,
        /// Lines of each failed repetition's uart.log.
        #[arg(long, default_value_t = 15)]
        tail: usize,
    },
    /// Measurement means of two runs side by side.
    Compare {
        a: String,
        b: String,
        /// Only measurements whose name contains this text.
        #[arg(long)]
        measurement: Option<String>,
    },
    /// A scenario's outcomes and measurements across runs.
    History {
        scenario: String,
        #[arg(long)]
        measurement: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Scenarios whose repetitions did not always pass, least passing
    /// first, with the stand's failures counted apart from the code's.
    Flaky {
        /// Only runs this recent, e.g. 3d or 12h.
        #[arg(long, value_parser = parse_budget, default_value = "7d")]
        since: std::time::Duration,
        /// Fewest repetitions a scenario needs to be judged.
        #[arg(long, default_value_t = 3)]
        minimum: usize,
    },
    /// Leave a scenario out of every `run-all` until it is released; an
    /// explicit `run` of it still runs, with a warning.
    Quarantine {
        scenario: String,
        #[arg(long)]
        reason: String,
    },
    Release {
        scenario: String,
    },
    /// Keep a run from pruning, e.g. an A/B baseline.
    Pin {
        run: String,
        #[arg(long)]
        reason: String,
    },
    Unpin {
        run: String,
    },
    /// List, or with --apply delete, runs no rule keeps.
    Prune {
        #[arg(long, default_value_t = PRUNE_DAYS)]
        days: u64,
        #[arg(long, default_value_t = PRUNE_KEEP_FAILED)]
        keep_failed: usize,
        #[arg(long)]
        apply: bool,
        /// Also runs of this build's schema that it cannot read; a newer
        /// branch's runs of the same schema may be among them.
        #[arg(long)]
        unreadable: bool,
    },
}

#[derive(clap::Parser)]
#[command(name = "cargo hil firmware", no_binary_name = true)]
enum FirmwareCli {
    /// Catalog images: name, chip, project and last build.
    List,
    /// Build an image against the pinned ESP-IDF.
    Build { image: String },
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_advertised_stand_command_is_dispatched_by_its_name() {
        for command in super::HilCommand::ALL {
            assert_eq!(super::HilCommand::named(command.name()), Some(command));
        }
        // `status` was advertised in the command tree without a handler.
        assert_eq!(super::HilCommand::named("status"), None);
    }

    use super::{OsString, has_flag, source_options, with_source_snapshot};

    /// A lease option `cargo hil archive` cannot take is a usage error
    /// (exit 2), as its own parsing's are; other commands keep theirs.
    #[test]
    fn a_bad_lease_option_before_archive_is_a_usage_error() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "").unwrap();
        let ctx = oer_process::Checkout::new(root.path()).unwrap();
        let args = |line: &str| line.split(' ').map(OsString::from).collect::<Vec<_>>();
        let bad_owner = [
            OsString::from("--owner"),
            OsString::from(""),
            OsString::from("archive"),
            OsString::from("holders"),
        ];
        assert!(super::LeaseOptions::split(&bad_owner).is_err());
        assert_eq!(
            super::run(&ctx, &bad_owner).unwrap(),
            std::process::ExitCode::from(2)
        );
        assert_eq!(
            super::run(&ctx, &args("--owner")).unwrap_err().to_string(),
            "--owner requires a value"
        );
    }

    #[test]
    fn an_enqueued_run_builds_from_its_snapshot_instead_of_its_source_options() {
        let args = [
            "run",
            "a",
            "--source-include",
            "x.rs",
            "--source-include=y.rs",
            "--include-untracked",
            "--layout-seed",
            "3",
        ]
        .map(OsString::from);
        assert_eq!(
            source_options(&args),
            [
                "--source-include",
                "x.rs",
                "--source-include=y.rs",
                "--include-untracked"
            ]
            .map(OsString::from)
        );
        assert_eq!(
            with_source_snapshot(&args, Path::new("/snapshots/s1")),
            [
                "run",
                "a",
                "--layout-seed",
                "3",
                "--source-snapshot",
                "/snapshots/s1"
            ]
            .map(OsString::from)
        );
        assert!(has_flag(&args, "--layout-seed"));
        assert!(!has_flag(&args, "--firmware-from"));
        // A flag that only begins with the name is another flag.
        assert!(!has_flag(
            &[OsString::from("--source-includes")],
            "--source-include"
        ));
    }

    #[test]
    fn a_profile_report_stays_outside_the_sealed_run() {
        let root = Path::new("/checkout");
        let report = profile_report_path(root, "run-1", "udp", Path::new("repetition-001"));
        assert_eq!(
            report,
            Path::new("/checkout/target/hil/profiles/run-1/udp/repetition-001/profile.txt")
        );
        assert!(!report.starts_with(root.join("target/hil/runs")));
    }

    use super::*;
    #[test]
    fn only_executing_commands_produce_runs() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(produces_runs(&args(&["run", "boot-smoke"])));
        assert!(produces_runs(&args(&["run-all", "--tag", "wifi"])));
        assert!(!produces_runs(&args(&["plan", "--scenario", "x"])));
        assert!(!produces_runs(&args(&["doctor"])));
    }

    #[test]
    fn lease_options_precede_the_command_and_stop_at_the_first_other_argument() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let (options, rest) =
            LeaseOptions::split(&args(&["--owner", "phy", "run", "x", "--owner", "kept"])).unwrap();
        assert_eq!(
            options,
            LeaseOptions {
                owner: Some("phy".into()),
            }
        );
        assert_eq!(rest, args(&["run", "x", "--owner", "kept"]));
        // The runner takes no lease options: they may follow its command.
        let (options, rest) = LeaseOptions::default()
            .with_late(&args(&["run", "x", "--owner", "phy", "y", "--", "--owner"]))
            .unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        assert_eq!(rest, args(&["run", "x", "y", "--", "--owner"]));
        let (options, _) = options.with_late(&args(&["run", "--owner=phy"])).unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        assert!(
            options
                .clone()
                .with_late(&args(&["run", "--owner", "bt"]))
                .is_err()
        );
        assert!(
            LeaseOptions::default()
                .with_late(&args(&["run", "--owner"]))
                .is_err()
        );
        let (options, rest) = LeaseOptions::split(&args(&["run", "x"])).unwrap();
        assert_eq!(options, LeaseOptions::default());
        assert_eq!(rest.len(), 2);
    }
}
