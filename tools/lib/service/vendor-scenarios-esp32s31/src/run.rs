//! Run one authenticated ESP32-S31 vendor-comparison scenario: the
//! scenario set, its inputs and the dispatch that decides its verdicts and
//! evidence shard. The binary adds the reviewer commands.
use crate::{
    ampdu_resort, ble, calibration_leaves, calibration_prefix, channel, coex, coex_hw, coverage,
    decisions,
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{Budget, Result},
    harness_edges, i2c, i2c_transport, mac, observation,
    phy::PhyOptions,
    retry, rfpll, rx_append, rx_gain, session, state, tracking, tx_dc,
};
use clap::Subcommand;
use oer_vendor_evidence_shard::Index;
use oer_vendor_scenario_engine::findings::RunReport;
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

/// One vendor-comparison scenario and its inputs.
#[derive(Subcommand)]
pub enum Scenario {
    /// Wi-Fi/BT gain arithmetic and publication, calibration storage and the
    /// RF-test power producer (`--rftest`).
    Gain {
        #[command(flatten)]
        common: Common,
        /// Authenticated `librftest.a`, by default the pinned one.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("librftest"))]
        rftest: PathBuf,
    },
    /// Channel restoration over installed ROM callbacks: full-root channel,
    /// temperature prefix over every sensor range and stuck-readiness containment.
    Channel {
        #[command(flatten)]
        common: Common,
    },
    /// Complete RX-gain root: publication guards, DC calibration and
    /// failed-channel, minimum-search and shared-budget containment.
    RxGain {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Complete TX-DC/PWDET root: Wi-Fi/BT DC rows over constant and
    /// alternating SAR samples, and PBus/SAR fault containment.
    TxDc {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Every PHY comparison scenario concurrently under one budget, each in its
    /// own output directory; any failure fails the run.
    All {
        #[command(flatten)]
        common: Common,
        /// Authenticated bootloader SDK firmware (calibration leaves and prefix).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("sdk"))]
        sdk: PathBuf,
        /// Authenticated PHY SDK firmware (RFPLL and diagnostics symbols).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
        /// Authenticated `librftest.a` (RF-test power producer).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("librftest"))]
        rftest: PathBuf,
        /// Authenticated `libpp.a` (Wi-Fi MAC HAL leaves).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libpp"))]
        libpp: PathBuf,
        /// Authenticated `libnet80211.a` (Wi-Fi MAC roots calling `libpp.a`).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libnet80211"))]
        libnet80211: PathBuf,
        /// Compiled Bluetooth probe image.
        #[arg(long)]
        bluetooth_production: PathBuf,
        /// Authenticated `libcoexist.a` (coexistence schedule).
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libcoexist"))]
        libcoexist: PathBuf,
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
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Coexistence hardware-timer and core leaves of the pinned
    /// `libcoexist.a` against the compiled radio probe image.
    CoexHw {
        #[command(flatten)]
        common: Common,
        /// Authenticated vendor firmware supplying logging symbols.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// The coexistence time-slice schedule of `libcoexist.a`: scheme
    /// selection, status bits, phase timeouts and restarts.
    Coex {
        #[command(flatten)]
        common: Common,
        /// Authenticated `libcoexist.a`.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libcoexist"))]
        libcoexist: PathBuf,
    },
    /// Wi-Fi MAC HAL leaves of `libpp.a` over every radio register fill.
    WifiMac {
        #[command(flatten)]
        common: Common,
        /// Authenticated `libpp.a`.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libpp"))]
        libpp: PathBuf,
        /// Authenticated `libnet80211.a`.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("libnet80211"))]
        libnet80211: PathBuf,
        /// Authenticated vendor Wi-Fi firmware supplying network-stack symbols.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// Combined calibration and parameter tracking parents with their real
    /// children, RFPLL corrections and failed-TX containment.
    Tracking {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
    /// PHY I2C command memory and transport, harness call edges, calibration
    /// leaves and PBus/DCODE prefix (`--sdk`) and RFPLL (`--phy-sdk`).
    I2c {
        #[command(flatten)]
        common: Common,
        /// Authenticated linked SDK firmware (crystal-clock symbol companion),
        /// by default the pinned one.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("sdk"))]
        sdk: PathBuf,
        /// Authenticated SDK firmware with the RFPLL diagnostics symbol, by
        /// default the pinned one.
        #[arg(long, default_value_os_t = crate::artifacts::default_path("phy-sdk"))]
        phy_sdk: PathBuf,
    },
}

#[derive(Clone, clap::Args)]
pub struct Common {
    /// Pinned `libphy.a`.
    #[arg(long, default_value_os_t = crate::artifacts::default_path("libphy"))]
    library: PathBuf,
    /// Pinned ROM ELF.
    #[arg(long, default_value_os_t = crate::artifacts::default_path("rom"))]
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
    /// Point mutant of the loaded radio probe image in every comparison that
    /// loads it, `TARGET[:ORIGINAL]:REPLACEMENT`: TARGET a hexadecimal
    /// address or `symbol+OFFSET`, ORIGINAL the bytes there (the instruction
    /// the probe holds when omitted), REPLACEMENT hexadecimal bytes, `nop` or
    /// `ret`; repeat for several. The probe is not rebuilt; a failing
    /// scenario kills the mutant, and a passing one reports whether it ran.
    #[arg(long = "patch", value_parser = oer_vendor_scenario_engine::mutant::parse)]
    mutants: Vec<oer_vendor_scenario_engine::mutant::Spec>,
    /// Point mutant of the Bluetooth production probe image, in the same
    /// form: the Bluetooth comparisons load that image instead of the radio
    /// probe, so they take only these.
    #[arg(long = "bluetooth-patch", value_parser = oer_vendor_scenario_engine::mutant::parse)]
    bluetooth_mutants: Vec<oer_vendor_scenario_engine::mutant::Spec>,
    /// The resolved radio and Bluetooth mutants.
    #[arg(skip)]
    patches: Vec<blobray_application::in_process::ImagePatch>,
    #[arg(skip)]
    bluetooth_patches: Vec<blobray_application::in_process::ImagePatch>,
    /// Write the scenario's shard of the native evidence index qualification
    /// reads into this directory; `all` writes every scenario's shard.
    #[arg(long)]
    index: Option<PathBuf>,
    /// Dep-info files of the libraries that decide the verdicts, from the
    /// binary's `--verdict-dep-info`.
    #[arg(skip)]
    verdict: Vec<PathBuf>,
}

/// Resolve `common`'s mutants against the radio probe and, for the
/// comparisons that load it, the Bluetooth probe.
fn resolve_mutants(common: &mut Common, bluetooth: Option<&Path>) -> Result<()> {
    use oer_vendor_scenario_engine::mutant::resolve;
    if !common.mutants.is_empty() {
        common.patches = resolve(&std::fs::read(&common.production)?, &common.mutants)?;
    }
    if !common.bluetooth_mutants.is_empty() {
        let production =
            bluetooth.ok_or("only the Bluetooth comparisons take Bluetooth mutants")?;
        common.bluetooth_patches = resolve(&std::fs::read(production)?, &common.bluetooth_mutants)?;
    }
    Ok(())
}

/// Exit code of one scenario.
/// Where one scenario's evidence shard goes: its name, the index directory
/// if one was requested, and the production probe image it compared.
struct Shard {
    scenario: &'static str,
    index: Option<PathBuf>,
    production: PathBuf,
    verdict: Vec<PathBuf>,
}

impl Shard {
    fn of(scenario: &'static str, common: &Common) -> Self {
        Self {
            scenario,
            index: common.index.clone(),
            production: common.production.clone(),
            verdict: common.verdict.clone(),
        }
    }
}

/// Run one scenario and, when it passed and an index directory was given,
/// write its shard.
fn single(shard: Shard, outcome: Result<Outcome>) -> Result<ExitCode> {
    let (code, claims) = outcome?;
    if let Some(directory) = &shard.index {
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
        let index = evidence::shard(shard.scenario, &shard.production, &claims, &shard.verdict)?;
        oer_vendor_scenario_engine::shard::record(directory, &index)?;
    }
    Ok(code)
}

/// Evidence entries and untriaged coverage of one scenario, alongside its
/// exit code.
type Outcome = (ExitCode, session::Claims);

/// Vendor roots each scenario claims against its compiled production entry.
const GAIN_CLAIMS: [(&str, &str, &str); 4] = [
    (
        "archive",
        "phy_wifi_get_tx_tab_new",
        "open_phy_channel_trace_calculate_tx_gain",
    ),
    (
        "archive",
        "phy_set_tx_gain_mem_new",
        "open_phy_channel_trace_publish_tx_gain",
    ),
    (
        "archive",
        "phy_bt_get_tx_tab_new",
        "open_phy_bluetooth_trace_calculate_gain",
    ),
    (
        "archive",
        "phy_bt_set_tx_gain_new",
        "open_phy_bluetooth_trace_tx_gain",
    ),
];
const I2C_CLAIMS: [(&str, &str, &str); 12] = [
    (
        "archive",
        "phy_i2c_master_cmd_mem_init",
        "open_phy_trace_command_memory",
    ),
    (
        "archive",
        "phy_get_i2c_hostid_new",
        "open_phy_trace_i2c_host",
    ),
    ("rom", "phy_i2c_readReg", "open_phy_trace_i2c_transfer"),
    ("rom", "phy_i2c_writeReg", "open_phy_trace_i2c_transfer"),
    ("rom", "phy_i2c_readReg_Mask", "open_phy_trace_i2c_transfer"),
    (
        "rom",
        "phy_i2c_writeReg_Mask",
        "open_phy_trace_i2c_transfer",
    ),
    ("rom", "phy_i2c_master_reset", "open_phy_trace_i2c_reset"),
    (
        "archive",
        "phy_i2c_master_mem_cfg",
        "open_phy_trace_phy_i2c_master_mem_cfg",
    ),
    (
        "archive",
        "phy_i2c_master_command_mem_cfg",
        "open_phy_trace_phy_i2c_master_command_mem_cfg",
    ),
    (
        "archive",
        "phy_get_i2c_data",
        "open_phy_trace_phy_get_i2c_data",
    ),
    (
        "archive",
        "phy_i2c_enter_critical",
        "open_phy_trace_phy_i2c_enter_critical",
    ),
    (
        "archive",
        "phy_i2c_exit_critical",
        "open_phy_trace_phy_i2c_exit_critical",
    ),
];
const I2C_SDK_CLAIMS: [(&str, &str, &str); 5] = [
    (
        "rom",
        "phy_pbus_clear_reg",
        "open_phy_calibration_trace_pbus_clear",
    ),
    (
        "archive",
        "phy_txgain_comp_pacfg_new",
        "open_phy_calibration_leaf",
    ),
    ("archive", "phy_force_dig_gain", "open_phy_calibration_leaf"),
    (
        "archive",
        "phy_temp_to_power_new",
        "open_phy_calibration_leaf",
    ),
    ("archive", "phy_reg_update_new", "open_phy_calibration_leaf"),
];
const I2C_RFPLL_CLAIMS: [(&str, &str, &str); 4] = [
    (
        "archive",
        "phy_rfpll_cap_init_cal_track",
        "open_phy_rfpll_trace_search",
    ),
    (
        "archive",
        "phy_rfpll_cap_track_new",
        "open_phy_rfpll_trace_maintain",
    ),
    (
        "archive",
        "phy_rfpll_cap_track_new",
        "open_phy_rfpll_trace_track",
    ),
    (
        "archive",
        "phy_set_rfpll_freq_new",
        "open_phy_rfpll_trace_program",
    ),
];
const CHANNEL_CLAIMS: [(&str, &str, &str); 2] = [
    (
        "archive",
        "phy_chip_set_chan",
        "open_phy_channel_trace_state",
    ),
    ("rom", channel::SENSOR_ROOT, channel::SENSOR_ENTRY),
];
const RX_GAIN_CLAIMS: [(&str, &str, &str); 1] = [(
    "archive",
    "phy_set_rx_gain_table",
    "open_phy_calibration_trace_rx_gain",
)];
const TX_DC_CLAIMS: [(&str, &str, &str); 1] = [(
    "archive",
    "phy_txdc_cal_pwdet_init",
    "open_phy_calibration_trace_tx_dc_pwdet",
)];
const TRACKING_CLAIMS: [(&str, &str, &str); 2] = [
    (
        "archive",
        "phy_cal_param_track",
        "open_phy_calibration_trace_combined",
    ),
    (
        "archive",
        "phy_param_track_tot",
        "open_phy_tracking_trace_parent",
    ),
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
        patches: common.patches,
    };
    let mut g = Gain::new(&options)?;
    let unmet = gain_state::exercise(&mut g, options.rftest.is_some())?;
    g.doc("unmet-obligations", &unmet)?;
    g.coefficient_boundaries()?;
    g.characterize()?;
    g.wifi()?;
    g.publish_wifi()?;
    g.bluetooth()?;
    g.additive()?;
    g.negative()?;
    let claims = g.session.claims(
        "gain",
        &g.roots,
        &GAIN_CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &unmet,
            "authenticated gain arithmetic/publication, gain state passed",
            &g.run,
        ),
        claims,
    ))
}

fn i2c(
    common: Common,
    sdk: Option<PathBuf>,
    phy_sdk: Option<PathBuf>,
    report: &dyn RunReport,
) -> Result<Outcome> {
    let options = i2c::Options {
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        sdk,
        phy_sdk,
        patches: common.patches,
    };
    let mut ctx = i2c::I2c::new(&options)?;
    let unmet = i2c::unmet(options.sdk.is_some(), options.phy_sdk.is_some());
    ctx.doc("unmet-obligations", &unmet)?;
    if options.phy_sdk.is_some() {
        rfpll::exercise(&mut ctx)?;
    }
    if options.sdk.is_some() {
        calibration_prefix::exercise(&mut ctx)?;
    }
    ctx.command_memory()?;
    i2c_transport::exercise(&mut ctx)?;
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
    if options.phy_sdk.is_some() {
        list.extend(I2C_RFPLL_CLAIMS);
    }
    let claims = ctx.session.claims(
        "i2c",
        &ctx.roots,
        &list,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(&unmet, "authenticated PHY comparisons passed", &ctx.run),
        claims,
    ))
}

impl Common {
    fn phy(self) -> PhyOptions {
        PhyOptions {
            library: self.library,
            rom: self.rom,
            production: self.production,
            linker: self.linker,
            output: self.output,
            budget: self.budget,
            patches: self.patches,
        }
    }
}

fn channel(common: Common, report: &dyn RunReport) -> Result<Outcome> {
    let options = common.phy();
    let mut ctx = channel::Channel::new(&options)?;
    channel::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "channel",
        &ctx.roots,
        &CHANNEL_CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &[],
            "authenticated channel restoration, temperature prefix, containment passed",
            &ctx.run,
        ),
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
        .map(|id| crate::artifacts::default_path(id));
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
        patches: common.patches,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    retry::exercise(&mut ctx)?;
    rx_append::exercise(&mut ctx)?;
    ampdu_resort::exercise(&mut ctx)?;
    ampdu_resort::exercise_timeouts(&mut ctx)?;
    let claims = ctx.session.claims(
        "wifi-mac",
        &ctx.roots,
        &mac::claims(&ctx),
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
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
            .map(|id| crate::artifacts::default_path(id))
            .collect(),
        rom: common.rom,
        firmware: Some(phy_sdk),
        production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        patches: common.bluetooth_patches,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "bluetooth",
        &ctx.roots,
        &mac::claims(&ctx),
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
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
            .map(|id| crate::artifacts::default_path(id))
            .collect(),
        rom: common.rom,
        firmware: Some(phy_sdk),
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        patches: common.patches,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "coex-hw",
        &ctx.roots,
        &mac::claims(&ctx),
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &[],
            "authenticated coexistence hardware leaves passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn coex(common: Common, libcoexist: PathBuf, report: &dyn RunReport) -> Result<Outcome> {
    let options = coex::CoexOptions {
        library: libcoexist,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        patches: common.patches,
    };
    let mut ctx = coex::Coex::new(&options)?;
    coex::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "coex",
        &ctx.roots,
        coex::CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(&[], "authenticated coexistence schedule passed", &ctx.run),
        claims,
    ))
}

fn rx_gain(common: Common, phy_sdk: PathBuf, report: &dyn RunReport) -> Result<Outcome> {
    let mut ctx = rx_gain::RxGain::new(&common.phy(), &phy_sdk)?;
    rx_gain::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "rx-gain",
        &ctx.roots,
        &RX_GAIN_CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &[],
            "authenticated RX gain publication, calibration, containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn tx_dc(common: Common, phy_sdk: PathBuf, report: &dyn RunReport) -> Result<Outcome> {
    let mut ctx = tx_dc::TxDc::new(&common.phy(), &phy_sdk)?;
    tx_dc::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "tx-dc",
        &ctx.roots,
        &TX_DC_CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &[],
            "authenticated TX-DC/PWDET calibration, fault containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn tracking(common: Common, phy_sdk: PathBuf, report: &dyn RunReport) -> Result<Outcome> {
    let mut ctx = tracking::Tracking::new(&common.phy(), &phy_sdk)?;
    tracking::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "tracking",
        &ctx.roots,
        &TRACKING_CLAIMS,
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    Ok((
        finish(
            &[],
            "authenticated tracking parents, failed-TX containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

/// Evidence index builders over the repository sources the verdicts depend on.
mod evidence {
    use super::*;

    use crate::{BLUETOOTH_PROBES_PACKAGE, PROBES_MANIFEST, PROBES_PACKAGE, PROBES_TARGET};
    use oer_vendor_scenario_engine::shard::ProbeImages;

    /// The probe images of the ESP32-S31 scenarios.
    const PROBES: ProbeImages = ProbeImages {
        manifest: PROBES_MANIFEST,
        packages: &[PROBES_PACKAGE, BLUETOOTH_PROBES_PACKAGE],
        target: PROBES_TARGET,
    };

    pub fn shard(
        scenario: &str,
        production: &Path,
        claims: &session::Claims,
        verdict: &[PathBuf],
    ) -> Result<Index> {
        oer_vendor_scenario_engine::shard::shard(
            scenario,
            production,
            claims,
            &PROBES,
            env!("CARGO_PKG_NAME"),
            verdict,
        )
    }
}

/// Every vendor input and extra production image `all` requires.
struct AllInputs {
    sdk: PathBuf,
    phy_sdk: PathBuf,
    rftest: PathBuf,
    libpp: PathBuf,
    libnet80211: PathBuf,
    bluetooth_production: PathBuf,
    libcoexist: PathBuf,
}

/// Run every PHY comparison scenario; each must pass with no unmet obligation.
fn all(common: Common, inputs: AllInputs, report: &dyn RunReport) -> Result<ExitCode> {
    let AllInputs {
        sdk,
        phy_sdk,
        rftest,
        libpp,
        libnet80211,
        bluetooth_production,
        libcoexist,
    } = inputs;
    if !common.patches.is_empty() || !common.bluetooth_patches.is_empty() {
        if common.index.is_some() {
            return Err("a point-mutant run writes no evidence index".into());
        }
        println!(
            "{} point-mutant patches applied to every radio comparison, {} to every Bluetooth comparison",
            common.patches.len(),
            common.bluetooth_patches.len()
        );
    }
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
            Box::new(|| {
                i2c(
                    within("i2c"),
                    Some(sdk.clone()),
                    Some(phy_sdk.clone()),
                    report,
                )
            }),
        ),
        ("channel", Box::new(|| channel(within("channel"), report))),
        (
            "rx-gain",
            Box::new(|| rx_gain(within("rx-gain"), phy_sdk.clone(), report)),
        ),
        (
            "tx-dc",
            Box::new(|| tx_dc(within("tx-dc"), phy_sdk.clone(), report)),
        ),
        (
            "tracking",
            Box::new(|| tracking(within("tracking"), phy_sdk.clone(), report)),
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
            "coex",
            Box::new(|| coex(within("coex"), libcoexist.clone(), report)),
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
    let write = |directory: &Path, name: &str, claims: &session::Claims| -> Result<()> {
        let production = if name == "bluetooth" {
            &bluetooth_production
        } else {
            &common.production
        };
        oer_vendor_scenario_engine::shard::record(
            directory,
            &evidence::shard(name, production, claims, &common.verdict)?,
        )
    };
    if let Some(failed) = conclude_failed(failed, common.index.as_deref(), &passed, write) {
        return failed;
    }
    let mut closures = vec![];
    let mut lines = observation::Lines::default();
    let mut unprojected = std::collections::BTreeSet::new();
    for (_, claims) in &passed {
        closures.extend(claims.closures.iter().cloned());
        lines.extend(&claims.lines);
        unprojected.extend(claims.unprojected.iter().cloned());
    }
    // A line is unobserved when no scenario observes it; every decision must
    // still review one.
    let root = oer_process::built_root();
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
    // Decisions are shared by every scenario, so one is stale only when no
    // scenario's closures leave a location it excludes uncovered.
    coverage::Observed::of(&closures)
        .check("all", oer_vendor_scenario_engine::coverage::decisions()?)?;
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
    record_passed(common.index.as_deref(), &passed, write)?;
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

/// Write the shard of every passed scenario into `index`, when one is given.
fn record_passed<T>(
    index: Option<&Path>,
    passed: &[(&str, T)],
    mut write: impl FnMut(&Path, &str, &T) -> Result<()>,
) -> Result<()> {
    let Some(index) = index else {
        return Ok(());
    };
    for (name, claims) in passed {
        write(index, name, claims)?;
    }
    Ok(())
}

/// What `all` ends with when a scenario failed: each passed scenario's shard
/// rests on its own claims alone, so it is still written and one DIFF
/// withholds only its own scenario's evidence; then the failure. The decision
/// checks that judge every scenario's closures, lines and state together
/// cannot run on a partial set and are skipped. `None` when every scenario
/// passed.
fn conclude_failed<T>(
    failed: Option<Result<ExitCode>>,
    index: Option<&Path>,
    passed: &[(&str, T)],
    write: impl FnMut(&Path, &str, &T) -> Result<()>,
) -> Option<Result<ExitCode>> {
    let failed = failed?;
    println!("cross-scenario decision checks skipped: a scenario did not pass");
    Some(record_passed(index, passed, write).and(failed))
}

impl Scenario {
    /// The inputs every comparison scenario shares.
    fn common_mut(&mut self) -> &mut Common {
        match self {
            Scenario::Gain { common, .. }
            | Scenario::Channel { common }
            | Scenario::RxGain { common, .. }
            | Scenario::TxDc { common, .. }
            | Scenario::CoexHw { common, .. }
            | Scenario::Coex { common, .. }
            | Scenario::WifiMac { common, .. }
            | Scenario::Tracking { common, .. }
            | Scenario::I2c { common, .. }
            | Scenario::Bluetooth { common, .. }
            | Scenario::All { common, .. } => common,
        }
    }

    /// Resolve the mutants of the scenario's inputs against their probes.
    fn resolve_mutants(&mut self) -> Result<()> {
        match self {
            Scenario::Bluetooth {
                common,
                bluetooth_production,
                ..
            }
            | Scenario::All {
                common,
                bluetooth_production,
                ..
            } => resolve_mutants(common, Some(bluetooth_production)),
            Scenario::Gain { common, .. }
            | Scenario::Channel { common }
            | Scenario::RxGain { common, .. }
            | Scenario::TxDc { common, .. }
            | Scenario::CoexHw { common, .. }
            | Scenario::Coex { common, .. }
            | Scenario::WifiMac { common, .. }
            | Scenario::Tracking { common, .. }
            | Scenario::I2c { common, .. } => resolve_mutants(common, None),
        }
    }
}

/// Run `scenario`, handing each suite's findings to `report`; `verdict`
/// names the dep-info files of the libraries that decide its verdicts.
pub fn run(mut scenario: Scenario, report: &dyn RunReport, verdict: Vec<PathBuf>) -> ExitCode {
    scenario.common_mut().verdict = verdict;
    if let Err(error) = scenario.resolve_mutants() {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    let result = match scenario {
        Scenario::Gain { common, rftest } => {
            let shard = Shard::of("gain", &common);
            single(shard, gain(common, Some(rftest), report))
        }
        Scenario::Channel { common } => {
            let shard = Shard::of("channel", &common);
            single(shard, channel(common, report))
        }
        Scenario::Coex { common, libcoexist } => {
            let shard = Shard::of("coex", &common);
            single(shard, coex(common, libcoexist, report))
        }
        Scenario::RxGain { common, phy_sdk } => {
            let shard = Shard::of("rx-gain", &common);
            single(shard, rx_gain(common, phy_sdk, report))
        }
        Scenario::TxDc { common, phy_sdk } => {
            let shard = Shard::of("tx-dc", &common);
            single(shard, tx_dc(common, phy_sdk, report))
        }
        Scenario::Tracking { common, phy_sdk } => {
            let shard = Shard::of("tracking", &common);
            single(shard, tracking(common, phy_sdk, report))
        }
        Scenario::WifiMac {
            common,
            libpp,
            libnet80211,
            phy_sdk,
        } => {
            let shard = Shard::of("wifi-mac", &common);
            single(shard, wifi_mac(common, libpp, libnet80211, phy_sdk, report))
        }
        Scenario::Bluetooth {
            common,
            bluetooth_production,
            phy_sdk,
        } => {
            let shard = Shard {
                production: bluetooth_production.clone(),
                ..Shard::of("bluetooth", &common)
            };
            single(
                shard,
                bluetooth(common, bluetooth_production, phy_sdk, report),
            )
        }
        Scenario::CoexHw { common, phy_sdk } => {
            let shard = Shard::of("coex-hw", &common);
            single(shard, coex_hw(common, phy_sdk, report))
        }
        Scenario::All {
            common,
            sdk,
            phy_sdk,
            rftest,
            libpp,
            libnet80211,
            bluetooth_production,
            libcoexist,
        } => all(
            common,
            AllInputs {
                sdk,
                phy_sdk,
                rftest,
                libpp,
                libnet80211,
                bluetooth_production,
                libcoexist,
            },
            report,
        ),
        Scenario::I2c {
            common,
            sdk,
            phy_sdk,
        } => {
            let shard = Shard::of("i2c", &common);
            single(shard, i2c(common, Some(sdk), Some(phy_sdk), report))
        }
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcomes() -> Outcomes<'static, u8> {
        vec![
            ("gain", 1.0, Ok((ExitCode::SUCCESS, 1))),
            ("channel", 2.0, Err("scenario channel panicked".into())),
            ("wifi-mac", 3.0, Ok((ExitCode::FAILURE, 3))),
            ("bluetooth", 4.0, Ok((ExitCode::SUCCESS, 4))),
        ]
    }

    #[test]
    fn a_failed_scenario_still_records_every_passed_scenario_and_fails() {
        let (elapsed, passed, failed) = split(outcomes());
        assert_eq!(elapsed.len(), 4);
        assert_eq!(passed, [("gain", 1), ("bluetooth", 4)]);
        let mut written = vec![];
        let result = conclude_failed(failed, Some(Path::new("index")), &passed, |_, name, _| {
            written.push(name.to_owned());
            Ok(())
        });
        assert_eq!(written, ["gain", "bluetooth"]);
        // The first failure is the result.
        let error = result.unwrap().unwrap_err().to_string();
        assert!(error.contains("channel"), "{error}");
    }

    #[test]
    fn without_an_index_or_a_failure_nothing_is_written_early() {
        let (_, passed, failed) = split(outcomes());
        let mut written = 0;
        let result = conclude_failed(failed, None, &passed, |_, _, _| {
            written += 1;
            Ok(())
        });
        assert!(result.is_some() && written == 0);
        let (_, passed, failed) = split(vec![("gain", 1.0, Ok((ExitCode::SUCCESS, 1u8)))]);
        let result = conclude_failed(failed, Some(Path::new("index")), &passed, |_, _, _| {
            written += 1;
            Ok(())
        });
        // Every scenario passed: the decision checks run first, recording after.
        assert!(result.is_none() && written == 0);
    }
}
