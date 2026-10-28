//! Run one authenticated ESP32-S31 vendor-comparison scenario: the
//! scenario set, its inputs and the dispatch that prints its verdicts.
//! The binary adds the reviewer commands.
use crate::esp32s31::{
    ble, calibration_leaves, coex_hw, decisions,
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{Budget, Result},
    harness_edges, i2c, mac, observation, session, state,
};
use crate::findings::RunReport;
use clap::Subcommand;
use std::{path::PathBuf, process::ExitCode};

/// One vendor-comparison scenario and its inputs.
#[derive(Subcommand)]
pub enum Scenario {
    /// Vendor gain characterization, calibration storage and the RF-test
    /// power producer (`--rftest`).
    Gain {
        #[command(flatten)]
        common: Common,
        /// Authenticated `librftest.a`, by default the pinned one.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("librftest"))]
        rftest: PathBuf,
    },
    /// Every comparison scenario concurrently under one budget, each in its
    /// own output directory; any failure fails the run.
    All {
        #[command(flatten)]
        common: Common,
        /// Authenticated bootloader SDK firmware (calibration leaves).
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("sdk"))]
        sdk: PathBuf,
        /// Authenticated PHY SDK firmware (logging and network-stack symbols).
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
        /// Authenticated `librftest.a` (RF-test power producer).
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("librftest"))]
        rftest: PathBuf,
        /// Authenticated `libpp.a` (Wi-Fi MAC HAL leaves).
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("libpp"))]
        libpp: PathBuf,
        /// Authenticated `libnet80211.a` (Wi-Fi MAC roots calling `libpp.a`).
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("libnet80211"))]
        libnet80211: PathBuf,
        /// Compiled Bluetooth probe image.
        #[arg(long)]
        bluetooth_production: PathBuf,
    },
    /// Bluetooth LE controller leaves of the pinned Bluetooth archives
    /// against the compiled Bluetooth probe image.
    Bluetooth {
        #[command(flatten)]
        common: Common,
        /// Compiled Bluetooth probe image.
        #[arg(long)]
        bluetooth_production: PathBuf,
        /// Authenticated vendor firmware supplying logging symbols.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Coexistence hardware-timer and core leaves of the pinned
    /// `libcoexist.a` against the compiled radio probe image.
    CoexHw {
        #[command(flatten)]
        common: Common,
        /// Authenticated vendor firmware supplying logging symbols.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Wi-Fi MAC HAL leaves of `libpp.a` over every radio register fill.
    WifiMac {
        #[command(flatten)]
        common: Common,
        /// Authenticated `libpp.a`.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("libpp"))]
        libpp: PathBuf,
        /// Authenticated `libnet80211.a`.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("libnet80211"))]
        libnet80211: PathBuf,
        /// Authenticated vendor Wi-Fi firmware supplying network-stack symbols.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// PHY I2C command memory, harness call edges and calibration leaves
    /// (`--sdk`).
    I2c {
        #[command(flatten)]
        common: Common,
        /// Authenticated linked SDK firmware (crystal-clock symbol companion),
        /// by default the pinned one.
        #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("sdk"))]
        sdk: PathBuf,
    },
}

#[derive(Clone, clap::Args)]
pub struct Common {
    /// Pinned `libphy.a`.
    #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("libphy"))]
    library: PathBuf,
    /// Pinned ROM ELF.
    #[arg(long, default_value_os_t = crate::esp32s31::artifacts::default_path("rom"))]
    rom: PathBuf,
    /// Freshly built production probe ELF.
    #[arg(long)]
    production: PathBuf,
    /// External linker selected for image preparation.
    #[arg(long)]
    linker: PathBuf,
    /// Ignored output root; each run creates a new `run-*` directory.
    #[arg(long)]
    output: PathBuf,
    #[command(flatten)]
    budget: Budget,
}

fn single(outcome: Result<Outcome>) -> Result<ExitCode> {
    outcome.map(|(code, _)| code)
}

/// Exit code, observation and state collected by one scenario.
type Outcome = (ExitCode, session::Claims);

/// Vendor roots each scenario claims against its compiled production entry.
const I2C_CLAIMS: [(&str, &str, &str); 1] = [(
    "archive",
    "phy_i2c_master_cmd_mem_init",
    "open_phy_trace_command_memory",
)];
const I2C_SDK_CLAIMS: [(&str, &str, &str); 3] = [
    (
        "archive",
        "phy_txgain_comp_pacfg_new",
        "open_phy_calibration_leaf",
    ),
    ("archive", "phy_force_dig_gain", "open_phy_calibration_leaf"),
    ("archive", "phy_reg_update_new", "open_phy_calibration_leaf"),
];

fn finish(unmet: &[Unmet], message: &str, run: &std::path::Path) -> ExitCode {
    if unmet.is_empty() {
        println!("{message} {}", run.display());
        ExitCode::SUCCESS
    } else {
        let ids: Vec<_> = unmet.iter().map(|o| o.id).collect();
        println!(
            "INCOMPLETE: unmet obligations: {} {}",
            ids.join(", "),
            run.display()
        );
        ExitCode::from(2)
    }
}

fn gain(common: Common, rftest: Option<PathBuf>, report: &dyn RunReport) -> Result<Outcome> {
    let options = Options {
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        rftest,
    };
    let mut g = Gain::new(&options)?;
    let unmet = gain_state::exercise(&mut g, options.rftest.is_some())?;
    g.doc("unmet-obligations", &unmet)?;
    g.characterize()?;
    let claims = g.session.claims("gain", &g.roots, &[], report)?;
    Ok((
        finish(
            &unmet,
            "authenticated gain characterization, gain state passed",
            &g.run,
        ),
        claims,
    ))
}

fn i2c(common: Common, sdk: Option<PathBuf>, report: &dyn RunReport) -> Result<Outcome> {
    let options = i2c::Options {
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        sdk,
    };
    let mut ctx = i2c::I2c::new(&options)?;
    let unmet = i2c::unmet(options.sdk.is_some());
    ctx.doc("unmet-obligations", &unmet)?;
    ctx.command_memory()?;
    if options.sdk.is_some() {
        calibration_leaves::exercise(&mut ctx)?;
    }
    harness_edges::exercise(&mut ctx)?;
    // All original source copies were deleted before linking/execution. Preserve
    // the full project closure, including probe/ROM bytes and negative evidence.
    let mut list = I2C_CLAIMS.to_vec();
    if options.sdk.is_some() {
        list.extend(I2C_SDK_CLAIMS);
    }
    let claims = ctx.session.claims("i2c", &ctx.roots, &list, report)?;
    Ok((
        finish(&unmet, "authenticated PHY comparisons passed", &ctx.run),
        claims,
    ))
}

fn wifi_mac(
    common: Common,
    libpp: PathBuf,
    libnet80211: PathBuf,
    phy_sdk: PathBuf,
    report: &dyn RunReport,
) -> Result<Outcome> {
    // The command line overrides the archives it names; the modem clock
    // archives the suite also links are the pinned ones.
    let named = [libpp, libnet80211];
    let pinned = mac::WIFI_MAC.archives[named.len()..]
        .iter()
        .map(|id| crate::esp32s31::artifacts::default_path(id));
    let archives = named.into_iter().chain(pinned).collect();
    let options = mac::MacOptions {
        suite: &mac::WIFI_MAC,
        archives,
        rom: common.rom,
        firmware: Some(phy_sdk),
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("wifi-mac", &ctx.roots, &mac::claims(&ctx), report)?;
    Ok((
        finish(&[], "authenticated Wi-Fi MAC HAL leaves passed", &ctx.run),
        claims,
    ))
}

fn bluetooth(
    common: Common,
    production: PathBuf,
    phy_sdk: PathBuf,
    report: &dyn RunReport,
) -> Result<Outcome> {
    let options = mac::MacOptions {
        suite: &ble::BLUETOOTH,
        archives: ble::BLUETOOTH
            .archives
            .iter()
            .map(|id| crate::esp32s31::artifacts::default_path(id))
            .collect(),
        rom: common.rom,
        firmware: Some(phy_sdk),
        production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("bluetooth", &ctx.roots, &mac::claims(&ctx), report)?;
    Ok((
        finish(
            &[],
            "authenticated Bluetooth controller leaves passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn coex_hw(common: Common, phy_sdk: PathBuf, report: &dyn RunReport) -> Result<Outcome> {
    let options = mac::MacOptions {
        suite: &coex_hw::COEX_HW,
        archives: coex_hw::COEX_HW
            .archives
            .iter()
            .map(|id| crate::esp32s31::artifacts::default_path(id))
            .collect(),
        rom: common.rom,
        firmware: Some(phy_sdk),
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("coex-hw", &ctx.roots, &mac::claims(&ctx), report)?;
    Ok((
        finish(
            &[],
            "authenticated coexistence hardware leaves passed",
            &ctx.run,
        ),
        claims,
    ))
}

/// Every vendor input and extra production image `all` requires.
struct AllInputs {
    sdk: PathBuf,
    phy_sdk: PathBuf,
    rftest: PathBuf,
    libpp: PathBuf,
    libnet80211: PathBuf,
    bluetooth_production: PathBuf,
}

/// Run every comparison scenario; each must pass with no unmet obligation.
fn all(common: Common, inputs: AllInputs, report: &dyn RunReport) -> Result<ExitCode> {
    let AllInputs {
        sdk,
        phy_sdk,
        rftest,
        libpp,
        libnet80211,
        bluetooth_production,
    } = inputs;
    let within = |name: &str| Common {
        output: common.output.join(name),
        ..common.clone()
    };
    type Run<'a> = Box<dyn FnOnce() -> Result<Outcome> + Send + 'a>;
    let scenarios: Vec<(&str, Run<'_>)> = vec![
        (
            "gain",
            Box::new(|| gain(within("gain"), Some(rftest.clone()), report)),
        ),
        (
            "i2c",
            Box::new(|| i2c(within("i2c"), Some(sdk.clone()), report)),
        ),
        (
            "wifi-mac",
            Box::new(|| {
                wifi_mac(
                    within("wifi-mac"),
                    libpp.clone(),
                    libnet80211.clone(),
                    phy_sdk.clone(),
                    report,
                )
            }),
        ),
        (
            "bluetooth",
            Box::new(|| {
                bluetooth(
                    within("bluetooth"),
                    bluetooth_production.clone(),
                    phy_sdk.clone(),
                    report,
                )
            }),
        ),
        (
            "coex-hw",
            Box::new(|| coex_hw(within("coex-hw"), phy_sdk.clone(), report)),
        ),
    ];
    // Scenarios share no state: each owns its session and output directory.
    // They run concurrently and their results join in the declared order.
    let outcomes: Vec<(&str, f64, Result<Outcome>)> = std::thread::scope(|scope| {
        let running: Vec<_> = scenarios
            .into_iter()
            .map(|(name, run)| {
                (
                    name,
                    scope.spawn(move || {
                        let start = std::time::Instant::now();
                        let outcome = run();
                        (start.elapsed().as_secs_f64(), outcome)
                    }),
                )
            })
            .collect();
        running
            .into_iter()
            .map(|(name, handle)| {
                let (seconds, outcome) = handle
                    .join()
                    .unwrap_or_else(|_| (0.0, Err(format!("scenario {name} panicked").into())));
                (name, seconds, outcome)
            })
            .collect()
    });
    let (elapsed, passed, failed) = split(outcomes);
    if let Some(failed) = failed {
        println!("cross-scenario decision checks skipped: a scenario did not pass");
        return failed;
    }
    let mut lines = observation::Lines::default();
    let mut unprojected = std::collections::BTreeSet::new();
    for (_, claims) in &passed {
        lines.extend(&claims.lines);
        unprojected.extend(claims.unprojected.iter().cloned());
    }
    // A line is unobserved when no scenario observes it; every decision must
    // still review one.
    let root = oer_os_process::built_root();
    let unobserved_lines = lines.unobserved();
    let mut sources = observation::Sources::default();
    sources.check(&root, decisions::observation::DECISIONS, &unobserved_lines)?;
    let (reviewed, unobserved) =
        sources.classify(&root, decisions::observation::DECISIONS, &unobserved_lines)?;
    let effect = unobserved
        .iter()
        .filter(|l| lines.effect.contains(*l))
        .count();
    let state = unobserved
        .iter()
        .filter(|l| !lines.effect.contains(*l) && lines.state.contains(*l))
        .count();
    println!(
        "{} of {} executed production hardware lines observed; {} unobserved reviewed, {} untriaged \
         ({effect} reach uncompared effects, {state} only final state, {} nothing)",
        lines.observed.len(),
        lines.executed.len(),
        reviewed.len(),
        unobserved.len(),
        unobserved.len() - effect - state,
    );
    for line in &unobserved {
        let kind = if lines.effect.contains(line) {
            "reaches uncompared effects"
        } else if lines.state.contains(line) {
            "only final state"
        } else {
            "nothing"
        };
        println!("  untriaged line {}:{} ({kind})", line.0.display(), line.1);
    }
    for (name, seconds) in &elapsed {
        println!("{name} {seconds:.1}s");
    }
    // A state decision is stale when no claim writes a byte it reviews
    // without comparing it.
    state::check(decisions::state::DECISIONS, &unprojected)?;
    let (mut reviewed_state, mut untriaged_state) = (0, std::collections::BTreeSet::new());
    let mut untriaged_claims = std::collections::BTreeMap::<_, Vec<_>>::new();
    for (root, production, byte) in &unprojected {
        let (reviewed, untriaged) = state::classify(
            decisions::state::DECISIONS,
            (root, production),
            &std::collections::BTreeSet::from([byte.clone()]),
        );
        reviewed_state += reviewed.len();
        for byte in &untriaged {
            untriaged_claims
                .entry(byte.clone())
                .or_default()
                .push(format!("{root} -> {production}"));
        }
        untriaged_state.extend(untriaged);
    }
    let unprojected = untriaged_state;
    println!(
        "{reviewed_state} unprojected vendor state bytes reviewed, {} untriaged",
        unprojected.len()
    );
    for (byte, claims) in &untriaged_claims {
        println!(
            "  untriaged state {}+{} in {}",
            byte.symbol,
            byte.offset,
            claims.join(", ")
        );
    }
    println!("all PHY comparison scenarios passed");
    Ok(ExitCode::SUCCESS)
}

/// Each scenario's duration, the scenarios that passed with their claims, and
/// the first failure.
type Split<'a, T> = (
    Vec<(&'a str, f64)>,
    Vec<(&'a str, T)>,
    Option<Result<ExitCode>>,
);

/// Each scenario's name, duration and outcome, in the declared order.
type Outcomes<'a, T> = Vec<(&'a str, f64, Result<(ExitCode, T)>)>;

/// `all`'s scenario outcomes, split ([`Split`]).
fn split<T>(outcomes: Outcomes<'_, T>) -> Split<'_, T> {
    let (mut elapsed, mut passed, mut failed) = (vec![], vec![], None);
    for (name, seconds, outcome) in outcomes {
        elapsed.push((name, seconds));
        match outcome {
            Ok((code, claims)) if code == ExitCode::SUCCESS => passed.push((name, claims)),
            Ok((code, _)) => {
                println!("scenario {name} did not pass");
                failed.get_or_insert(Ok(code));
            }
            Err(error) => {
                println!("scenario {name} failed: {error}");
                failed.get_or_insert(Err(error));
            }
        }
    }
    (elapsed, passed, failed)
}

/// Run `scenario`, handing each suite's findings to `report`.
pub fn run(scenario: Scenario, report: &dyn RunReport) -> ExitCode {
    let result = match scenario {
        Scenario::Gain { common, rftest } => single(gain(common, Some(rftest), report)),
        Scenario::WifiMac {
            common,
            libpp,
            libnet80211,
            phy_sdk,
        } => single(wifi_mac(common, libpp, libnet80211, phy_sdk, report)),
        Scenario::Bluetooth {
            common,
            bluetooth_production,
            phy_sdk,
        } => single(bluetooth(common, bluetooth_production, phy_sdk, report)),
        Scenario::CoexHw { common, phy_sdk } => single(coex_hw(common, phy_sdk, report)),
        Scenario::All {
            common,
            sdk,
            phy_sdk,
            rftest,
            libpp,
            libnet80211,
            bluetooth_production,
        } => all(
            common,
            AllInputs {
                sdk,
                phy_sdk,
                rftest,
                libpp,
                libnet80211,
                bluetooth_production,
            },
            report,
        ),
        Scenario::I2c { common, sdk } => single(i2c(common, Some(sdk), report)),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_outcome_is_collected_and_the_first_error_is_returned() {
        let (elapsed, passed, failed) = split(vec![
            ("gain", 1.0, Ok((ExitCode::SUCCESS, 1))),
            ("i2c", 2.0, Err("scenario i2c panicked".into())),
            ("wifi-mac", 3.0, Ok((ExitCode::FAILURE, 3))),
            ("bluetooth", 4.0, Ok((ExitCode::SUCCESS, 4))),
        ]);
        assert_eq!(elapsed.len(), 4);
        assert_eq!(passed, [("gain", 1), ("bluetooth", 4)]);
        assert!(
            failed
                .expect("a scenario failed")
                .expect_err("the first failure is an error")
                .to_string()
                .contains("i2c")
        );
    }
    #[test]
    fn an_incomplete_scenario_keeps_its_exit_code() {
        let (_, passed, failed) = split(vec![("gain", 1.0, Ok((ExitCode::from(2), 1)))]);
        assert!(passed.is_empty());
        assert_eq!(
            failed.expect("a scenario failed").expect("an exit code"),
            ExitCode::from(2)
        );
        let (_, passed, failed) = split(vec![("gain", 1.0, Ok((ExitCode::SUCCESS, 1)))]);
        assert_eq!(passed, [("gain", 1)]);
        assert!(failed.is_none());
    }
}
