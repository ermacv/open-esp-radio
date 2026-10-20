//! The `cargo hil` command: the test commands handled here (runs, analyses,
//! experiments, the firmware catalog), every other command
//! forwarded to the runner this crate builds. The stand's own commands are
//! `cargo stand`.
use crate::Result;
use oer_os_process::Checkout;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use clap::{CommandFactory as _, FromArgMatches as _};

#[derive(clap::Args)]
struct HilCli {
    /// The stand owner; repeated occurrences must agree.
    #[arg(long, global = true, value_parser = OwnerParser::default(), overrides_with = "owner")]
    owner: Option<String>,
    #[command(flatten)]
    checkout: oer_os_process::CheckoutArgs,
    #[arg(long, global = true)]
    stand_file: Option<PathBuf>,
    #[arg(long, global = true)]
    board: Option<String>,
    #[arg(long, global = true)]
    peer_board: Option<String>,
    #[command(subcommand)]
    command: Option<HilCommand>,
}

#[derive(clap::Subcommand)]
enum HilCommand {
    Dashboard(crate::dashboard::DashboardCli),
    #[command(subcommand)]
    Peer(crate::peer::PeerCli),
    #[command(subcommand)]
    Firmware(FirmwareCli),
    #[command(subcommand)]
    Images(crate::images::ImagesCli),
    Observer,
    Sweep {
        #[arg(long)]
        apply: bool,
    },
    #[command(subcommand)]
    Runs(RunsCli),
    #[command(subcommand)]
    Perf(PerfCli),
    Profile(ProfileCli),
    Bisect(crate::experiments::BisectCli),
    Ab(crate::experiments::AbCli),
    Wait {
        id: OsString,
    },
    #[command(flatten)]
    Runner(oer_hil_run::cli::CliCommand),
    #[command(name = "__command-tree", hide = true)]
    CommandTree,
}

impl clap::CommandFactory for HilCli {
    fn command() -> clap::Command {
        use clap::Args as _;
        let mut command = Self::augment_args(clap::Command::new("cargo hil").no_binary_name(true));
        for child in command.get_subcommands_mut() {
            if matches!(child.get_name(), "run" | "run-all" | "ab") {
                *child = crate::jobs::JobArgs::augment_args(child.clone());
            }
        }
        command
    }
    fn command_for_update() -> clap::Command {
        Self::command()
    }
}
impl clap::Parser for HilCli {}

/// Retain the host's parser status (1) and the runner's clap status (2).
pub fn usage_status(error: &clap::Error, args: &[OsString]) -> u8 {
    let partial = HilCli::command()
        .ignore_errors(true)
        .try_get_matches_from(args)
        .ok();
    match partial.as_ref().and_then(clap::ArgMatches::subcommand_name) {
        Some(
            "dashboard" | "peer" | "firmware" | "images" | "observer" | "sweep" | "runs" | "perf"
            | "profile" | "bisect" | "ab" | "wait",
        ) => 1,
        Some(_) => error.exit_code() as u8,
        None if !error.use_stderr() => 0,
        None if error.kind() == clap::error::ErrorKind::InvalidSubcommand => 2,
        None => 1,
    }
}

/// Clap propagates a global argument's last occurrence across subcommands.
/// Validate each occurrence while clap parses it so a late, different owner
/// cannot erase the earlier owner before agreement is checked.
#[derive(Clone, Default)]
struct OwnerParser(std::sync::Arc<std::sync::Mutex<Option<String>>>);

impl clap::builder::TypedValueParser for OwnerParser {
    type Value = String;

    fn parse_ref(
        &self,
        command: &clap::Command,
        _argument: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> std::result::Result<String, clap::Error> {
        let owner = value.to_str().ok_or_else(|| {
            command
                .clone()
                .error(clap::error::ErrorKind::InvalidUtf8, "an owner name is text")
        })?;
        let owner = oer_lab_arbiter::owner::Owner::new(owner.trim())
            .map_err(|error| {
                command
                    .clone()
                    .error(clap::error::ErrorKind::ValueValidation, error.to_string())
            })?
            .to_string();
        let mut previous = self.0.lock().map_err(|_| {
            command.clone().error(
                clap::error::ErrorKind::ValueValidation,
                "owner validation lock poisoned",
            )
        })?;
        if command.get_name() != "cargo hil"
            && let Some(previous) = previous.as_ref().filter(|previous| **previous != owner)
        {
            return Err(command.clone().error(
                clap::error::ErrorKind::ValueValidation,
                format!("--owner is given twice, as {previous} and {owner}"),
            ));
        }
        *previous = Some(owner.clone());
        Ok(owner)
    }
}

pub fn run(args: &[OsString]) -> Result<std::process::ExitCode> {
    let matches = HilCli::command().try_get_matches_from(args)?;
    let cli = HilCli::from_arg_matches(&matches)?;
    let ctx = cli.checkout.checkout("cargo hil")?;
    let _signals = oer_os_process::install_signal_handlers()?;
    let options = LeaseOptions { owner: cli.owner };
    use_shared_store(&ctx)?;
    let ctx = &ctx;
    let Some(command) = cli.command else {
        HilCli::command().print_help()?;
        println!();
        return Ok(std::process::ExitCode::SUCCESS);
    };
    match command {
        HilCommand::CommandTree => command_tree(ctx),
        HilCommand::Dashboard(cli) => {
            crate::dashboard::serve(&oer_hil_runs::RunStore::shared()?, cli)
        }
        HilCommand::Peer(cli) => crate::peer::peer(ctx, options.owner(ctx)?, cli),
        HilCommand::Firmware(cli) => firmware(ctx, cli),
        HilCommand::Images(cli) => crate::images::command(ctx, cli),
        HilCommand::Observer => crate::images::observer(ctx),
        HilCommand::Sweep { apply } => {
            crate::sweep::run(&ctx.root, crate::sweep::Policy::default(), apply)?;
            Ok(std::process::ExitCode::SUCCESS)
        }
        HilCommand::Runs(cli) => runs(ctx, &options, cli),
        HilCommand::Perf(cli) => perf(ctx, &options, cli),
        HilCommand::Profile(cli) => profile(ctx, cli),
        HilCommand::Wait { id } => wait(&[id]),
        HilCommand::Ab(cli_ab) => {
            let (_, submatches) = matches.subcommand().ok_or("missing ab command")?;
            let mut args = vec![OsString::from("ab")];
            args.extend(crate::arguments::encode(
                &crate::experiments::AbCli::command(),
                submatches,
                &[],
            ));
            ab(
                ctx,
                &options.owner(ctx)?,
                cli_ab,
                crate::jobs::JobArgs::from_arg_matches(submatches)?,
                args,
            )
        }
        HilCommand::Bisect(cli) => crate::experiments::bisect(ctx, &options.owner(ctx)?, cli),
        HilCommand::Runner(command) => {
            let scheduling = match matches.subcommand() {
                Some(("run" | "run-all", submatches)) => {
                    crate::jobs::JobArgs::from_arg_matches(submatches)?
                }
                _ => crate::jobs::JobArgs::default(),
            };
            runner(ctx, &options, scheduling, command, &matches)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct LeaseOptions {
    owner: Option<String>,
}

impl LeaseOptions {
    fn owner(&self, ctx: &Checkout) -> Result<String> {
        Ok(oer_lab_arbiter::owner::resolve(self.owner.as_deref(), &ctx.root)?.to_string())
    }
}

fn runner(
    ctx: &Checkout,
    options: &LeaseOptions,
    scheduling: crate::jobs::JobArgs,
    command: oer_hil_run::cli::CliCommand,
    matches: &clap::ArgMatches,
) -> Result<std::process::ExitCode> {
    use oer_hil_run::cli::CliCommand;
    let produces_runs = matches!(command, CliCommand::Run { .. } | CliCommand::RunAll { .. });
    let after = scheduling.after();
    if (scheduling.enqueue || after.is_some()) && !produces_runs {
        return Err("--enqueue and --after apply to run and run-all".into());
    }
    let grammar = oer_hil_run::cli::command();
    let args = crate::arguments::encode(&grammar, matches, &[]);
    if scheduling.enqueue {
        let frozen = freeze_enqueued(ctx, &command, &args)?;
        let id = crate::jobs::enqueue(ctx, &options.owner(ctx)?, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let owner = match options.owner(ctx) {
        Ok(owner) => Some(owner),
        Err(error) if error.is::<oer_lab_arbiter::owner::NoOwner>() => None,
        Err(error) => return Err(error),
    };
    let mut job = if produces_runs {
        Some(crate::experiment::job::Running::begin(
            &ctx.root,
            owner.as_deref().unwrap_or("unregistered"),
            &args,
            after.as_ref(),
        )?)
    } else {
        None
    };
    let frozen = crate::jobs::Frozen::inherited()?;
    let runner = match &frozen {
        Some(frozen) => crate::experiment::launch::Runner {
            executable: frozen.runner.clone(),
        },
        None => crate::experiment::launch::Runner::prepare(&ctx.root)?,
    };
    let mut runner_args = match frozen.as_ref().and_then(|frozen| frozen.images.as_deref()) {
        Some(images) => {
            let mut args =
                crate::arguments::encode(&grammar, matches, &["source", "layout_seed", "features"]);
            // Build flags are replaced in parsed arguments, before positional values.
            let position = args
                .iter()
                .position(|arg| arg == "--")
                .unwrap_or(args.len());
            args.splice(
                position..position,
                [OsString::from("--images"), images.into()],
            );
            args
        }
        None => args.clone(),
    };
    let quarantined: oer_hil_runs::store::Notes =
        oer_hil_runs::RunStore::shared()?.read(oer_hil_runs::store::Sidecar::Quarantine)?;
    match &command {
        CliCommand::RunAll { .. } => {
            for scenario in quarantined.keys() {
                runner_args.extend([OsString::from("--exclude"), scenario.into()]);
            }
        }
        CliCommand::Run { scenarios, .. } => {
            for (scenario, entry) in &quarantined {
                if scenarios.contains(scenario) {
                    eprintln!(
                        "hil: `{scenario}` is quarantined ({}); running it as asked",
                        entry.reason
                    );
                }
            }
        }
        _ => {}
    }
    let mut launch = crate::experiment::launch::Launch::new(&ctx.root, &runner).args(runner_args);
    let mut context = oer_os_process::Context::current()?.clone();
    if let Some(owner) = owner {
        launch = launch.env(oer_lab_arbiter::owner::OWNER_ENV, &owner);
        context.set(oer_lab_arbiter::owner::OWNER_KEY, owner);
    }
    if let Some(job) = &job {
        context.set(oer_lab_arbiter::jobs::JOB_KEY, job.id());
    }
    launch = launch.context(context);
    let launched = crate::experiment::launch::launch_run(&launch)?;
    if produces_runs {
        oer_hil_runs::launch_runs::record(&launched.runs)?;
        let store = oer_hil_runs::RunStore::shared()?;
        if let Some(job) = job.as_mut() {
            job.finish_with(&store, &launched.runs)?;
        }
        if let Err(error) = prune_automatically(ctx, &store) {
            if oer_os_process::is_cancelled(&*error) {
                return Err(error);
            }
            eprintln!("hil: automatic pruning of the run store failed: {error}");
        }
    }
    Ok(oer_os_process::exit_code(launched.status))
}

/// `cargo hil __command-tree`: every `cargo hil` command path with its
/// subcommands and long flags, as JSON, for checking documentation against
/// the real command line. The stand's own commands and the runner's are
/// merged under `hil`.
fn command_tree(_ctx: &Checkout) -> Result<std::process::ExitCode> {
    let nodes = oer_os_command_tree::command_tree(&HilCli::command(), &[String::from("hil")]);
    println!("{}", oer_os_command_tree::json(&nodes));
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil ab ...`, a job like a run: `--enqueue` starts it detached and
/// prints its id for `cargo hil wait`, and `--after JOB` orders it.
fn ab(
    ctx: &Checkout,
    owner: &str,
    cli: crate::experiments::AbCli,
    scheduling: crate::jobs::JobArgs,
    args: Vec<OsString>,
) -> Result<std::process::ExitCode> {
    let after = scheduling.after();
    if scheduling.enqueue {
        let frozen = crate::jobs::Frozen::capture(ctx)?;
        let id = crate::jobs::enqueue(ctx, owner, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let mut job = crate::experiment::job::Running::begin(&ctx.root, owner, &args, after.as_ref())?;
    // Every round runs the runner of the experiment's start: a pull into the
    // checkout meanwhile must not change the arms' protocol.
    let frozen = match crate::jobs::Frozen::inherited()? {
        Some(frozen) => frozen,
        None => crate::jobs::Frozen::capture(ctx)?,
    };
    let runner = crate::experiment::launch::Runner {
        executable: frozen.runner,
    };
    let runs = crate::experiments::ab(ctx, owner, &runner, cli)?;
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
    if oer_lab_arbiter::Arbiter::open()?.jobs().read(text).is_ok() {
        return crate::jobs::wait_command(args);
    }
    let run = oer_hil_analysis::Run::open(&oer_hil_runs::RunStore::shared()?, text)
        .map_err(|_| format!("{text} is neither a job nor a run"))?;
    Ok(std::process::ExitCode::from(oer_hil_analysis::runs::wait(
        run.directory(),
        |line| println!("{line}"),
    )?))
}

fn parse_budget(text: &str) -> std::result::Result<std::time::Duration, String> {
    oer_lab_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil runs list|why|compare|history|pin|unpin|prune` over the shared
/// run store.
/// `cargo hil perf`: gated measurements across commits and their baselines.
fn perf(ctx: &Checkout, options: &LeaseOptions, cli: PerfCli) -> Result<std::process::ExitCode> {
    use oer_hil_analysis::perf;
    let store = oer_hil_runs::RunStore::shared()?;
    let now = oer_os_durable::unix_millis();
    let find = |id: &str| -> Result<perf::RunSummary> {
        perf::summary(&oer_hil_analysis::Run::open(&store, id)?)
    };
    let baselines = perf::baselines(&store)?;
    match cli {
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
fn runs(ctx: &Checkout, options: &LeaseOptions, parsed: RunsCli) -> Result<std::process::ExitCode> {
    use oer_hil_analysis::{Run, retention, runs};
    use oer_hil_runs::store::Note;
    use oer_hil_runs::store::Notes;
    use oer_hil_runs::store::Sidecar;
    let store = oer_hil_runs::RunStore::shared()?;
    // Reading every run takes long; commands about one run read only it.
    let all = || Run::all(&store);
    let find = |id: &str| Run::open(&store, id);
    let now = oer_os_durable::unix_millis();
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
            let pinned =
                store.update_notes(Sidecar::Pins, |pins| crate::pins::unpin_by_hand(pins, &run))?;
            if !pinned {
                eprintln!("hil: run {run} was not pinned");
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
fn firmware(ctx: &Checkout, cli: FirmwareCli) -> Result<std::process::ExitCode> {
    match cli {
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
fn collect_objects(ctx: &Checkout) -> oer_hil_runs::build::CollectedObjects {
    let mut checkouts = vec![ctx.root.clone()];
    if let Ok(owners) = oer_lab_arbiter::owner::Owners::open()
        && let Ok(registered) = owners.checkouts()
    {
        checkouts.extend(registered);
    }
    checkouts.sort();
    checkouts.dedup();
    let mut total = oer_hil_runs::build::CollectedObjects::default();
    for checkout in checkouts {
        // Build objects stay per chip.
        let chips = oer_chip_profile::supported(&checkout).unwrap_or_default();
        for target in chips
            .iter()
            .map(|chip| checkout.join("target/hil").join(chip))
        {
            match oer_hil_runs::build::collect_objects(&target) {
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

fn prune_automatically(ctx: &Checkout, store: &oer_hil_runs::RunStore) -> Result<()> {
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
    oer_hil_runs::RunStore::shared()?.link(&ctx.root)?;
    Ok(())
}

/// Check an enqueued `run`'s scenarios and options with the runner now, so
/// a mistake shows in the terminal instead of in a job that ends no-run
/// minutes later, and fix what it runs with: this binary, the runner and,
/// for a `run` that builds, its images, built now into the job's own
/// directory of images.
fn freeze_enqueued(
    ctx: &Checkout,
    command: &oer_hil_run::cli::CliCommand,
    args: &[OsString],
) -> Result<crate::jobs::Frozen> {
    let mut frozen = crate::jobs::Frozen::capture(ctx)?;
    if !matches!(command, oer_hil_run::cli::CliCommand::Run { .. }) {
        return Ok(frozen);
    }
    let output = oer_os_process::output(
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
    if matches!(
        command,
        oer_hil_run::cli::CliCommand::Run {
            firmware_from: Some(_),
            ..
        } | oer_hil_run::cli::CliCommand::Run {
            images: Some(_),
            ..
        }
    ) {
        return Ok(frozen);
    }
    let images = crate::jobs::images_directory(ctx)?;
    build_before_queue(ctx, &frozen, args, &images)?;
    frozen.images = Some(images);
    Ok(frozen)
}

/// Build and audit a run's images with its fixed runner into `images`
/// before the job waits: a build or audit that fails does so now, in the
/// terminal, not after the queue, and the job flashes exactly these.
fn build_before_queue(
    ctx: &Checkout,
    frozen: &crate::jobs::Frozen,
    run_args: &[OsString],
    images: &Path,
) -> Result<()> {
    eprintln!("hil: building the run's images before it queues");
    let status = ctx
        .command(&frozen.runner)
        .args(run_args)
        .arg("--build-only")
        .arg(images)
        .status()?;
    if !status.success() {
        return Err("not enqueued: the run's images did not build (see above)".into());
    }
    Ok(())
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
    /// Make a clean finished run the baseline of its scenarios.
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
/// checkout's target directory, never inside the finished run bundle.
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
fn profile(ctx: &Checkout, cli: ProfileCli) -> Result<std::process::ExitCode> {
    let run = oer_hil_runs::RunStore::shared()?
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
            .filter(|elf| oer_hil_runs::format::archived::stored_path(elf).is_file())
            .map(|elf| oer_hil_runs::format::archived::read_archived(&elf))
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
            // The run bundle is finished: a derived report written into it
            // would fail its integrity inventory.
            let destination = profile_report_path(&ctx.root, &cli.run, &id, relative);
            fs::create_dir_all(destination.parent().ok_or("profile report has no parent")?)?;
            oer_os_durable::atomic_write(&destination, report.as_bytes())?;
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
        /// Also the runs of this build's or an older schema that it cannot
        /// read; a newer branch's runs of the same schema may be among them.
        /// Nothing else deletes an older-schema run.
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
mod tests;
