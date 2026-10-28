//! Build the chip's typed vendor scenarios and forward their arguments.
use crate::Result;
use oer_os_process::{self as process, Checkout};
use std::ffi::OsString;

pub fn run(ctx: &Checkout, chip: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let chip = oer_tool_pins::artifacts::project::Project::new(&ctx.root, chip)?;
    let scenarios = chip.scenarios();
    if !ctx.root.join(chip.scenarios_directory()).is_dir() {
        return Err(format!("no vendor scenarios for chip {}", chip.name()).into());
    }
    let Some((scenario, rest)) = args.split_first() else {
        return Err("select a scenario, for example `gain`".into());
    };
    let package = scenarios.entry.as_str();
    let binary = scenarios.binary.as_str();
    super::phase::timed("build vendor scenarios", || -> Result<()> {
        process::run(oer_os_toolchain::cargo_in(&ctx.root).args([
            "build",
            "--locked",
            "--profile",
            oer_os_toolchain::workspace::PROFILE,
            "-p",
            package,
            "--bin",
            binary,
        ]))?;
        Ok(())
    })?;
    let mut command = ctx.command(oer_os_toolchain::workspace::binary(&ctx.root, binary));
    command.arg(scenario).args(rest);
    let label = format!("scenario {}", scenario.to_string_lossy());
    super::phase::timed(&label, || {
        let mut child = process::owned::Child::spawn(&mut command)?;
        Ok(process::exit_code(child.wait_forwarding_cancellation()?))
    })
}
