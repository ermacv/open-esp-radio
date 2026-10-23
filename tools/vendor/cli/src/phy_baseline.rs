//! Generate measured PHY references and compare archived production captures.
use crate::{Checkout, Result};
use clap::{Args, Subcommand};
use oer_vendor_phy_esp32s31::{
    baseline::{self, Check, Reference, Signature},
    compare::Verdict,
};
use std::{path::PathBuf, process::ExitCode};

#[derive(Args)]
pub struct Arguments {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Generate the S31 Wi-Fi cold reference from complete primary run captures.
    Generate {
        /// Board label and archived run directory; repeat for every source series.
        #[arg(long, required = true, value_name = "LABEL=RUN")]
        source: Vec<String>,
        /// Measured silicon revision of the selected boards (rev0 = 0).
        #[arg(long, default_value_t = 0)]
        revision: u16,
        /// Defaults to the chip's versioned Wi-Fi cold reference path.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Compare a recorded production run without building or flashing vendor code.
    Check {
        #[arg(long)]
        board: String,
        #[arg(long, default_value_t = 0)]
        revision: u16,
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        reference: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}
pub fn run(ctx: &Checkout, args: Arguments) -> Result<ExitCode> {
    match args.command {
        Command::Generate {
            source,
            revision,
            output,
        } => {
            let signature = Signature::current(&ctx.root)?;
            let mut inputs = vec![];
            for source in source {
                let (label, path) = source.split_once('=').ok_or("expected LABEL=RUN")?;
                inputs.extend(baseline::runs::vendor(
                    std::path::Path::new(path),
                    label,
                    revision,
                )?);
            }
            let output = output.unwrap_or_else(|| signature.path(&ctx.root));
            let reference = baseline::generate(signature, inputs, &baseline::images(&ctx.root)?)?;
            reference.validate(
                &reference.signature,
                &baseline::images(&ctx.root)?,
                reference.boards.keys().next().ok_or("no boards")?,
                revision,
            )?;
            reference.write(&output)?;
            println!(
                "{}: {} boards, {} entries",
                output.display(),
                reference.boards.len(),
                reference.entries.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            board,
            revision,
            run,
            reference,
            output,
        } => {
            let checked = (|| -> Result<Check> {
                let signature = Signature::current(&ctx.root)?;
                let reference =
                    Reference::read(&reference.unwrap_or_else(|| signature.path(&ctx.root)))?;
                let series = baseline::runs::production(&run, &board, reference.channel)?;
                Ok(reference.check(
                    &signature,
                    &baseline::images(&ctx.root)?,
                    &board,
                    revision,
                    baseline::Captures {
                        identity: &series.identity,
                        production: &series.production,
                        control: series.control.as_ref(),
                    },
                ))
            })()
            .unwrap_or_else(Check::incomplete);
            if let Some(output) = output {
                oer_os_durable::atomic_json(&output, &checked)?;
            }
            println!("{}", serde_json::to_string_pretty(&checked)?);
            Ok(if checked.verdict == Verdict::Match {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        args: Arguments,
    }
    #[test]
    fn explicit_sources_and_offline_check_are_distinct_commands() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["phy-baseline", "generate"]).is_err());
        assert!(
            Cli::try_parse_from([
                "phy-baseline",
                "generate",
                "--source",
                "s31-a=run-a",
                "--source",
                "s31-b=run-b"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "phy-baseline",
                "check",
                "--board",
                "s31-b",
                "--run",
                "capture"
            ])
            .is_ok()
        );
    }
}
