#![forbid(unsafe_code)]
use std::{path::PathBuf, process::ExitCode};

use clap::Parser as _;
use oer_stand_fixture_install::{Provider, apply_system};

#[derive(clap::Parser)]
#[command(name = "open-radio-fixture-install")]
struct Cli {
    #[arg(long)]
    provider: ProviderArg,
    #[arg(long, value_name = "PREPARED_BUNDLE")]
    bundle: PathBuf,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ProviderArg {
    LinuxNet,
}

impl From<ProviderArg> for Provider {
    fn from(provider: ProviderArg) -> Self {
        match provider {
            ProviderArg::LinuxNet => Self::LinuxNet,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let operator = std::env::var("SUDO_USER").unwrap_or_default();
    match apply_system(cli.provider.into(), &cli.bundle, &operator) {
        Ok(result) => {
            if let Err(error) = serde_json::to_writer_pretty(std::io::stdout().lock(), &result) {
                eprintln!("cannot publish installation result: {error}");
                return ExitCode::FAILURE;
            }
            println!();
            if result.primary_error.is_none() && result.recovery_error.is_none() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("fixture installation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_cli_has_only_finite_provider_and_bundle_inputs() {
        assert!(
            Cli::try_parse_from([
                "installer",
                "--provider",
                "linux-net",
                "--bundle",
                "/prepared/bundle"
            ])
            .is_ok()
        );
        for unexpected in ["--root", "--command", "--path", "--output", "--sudoers"] {
            assert!(
                Cli::try_parse_from([
                    "installer",
                    "--provider",
                    "linux-net",
                    "--bundle",
                    "/prepared/bundle",
                    unexpected,
                    "/tmp/escape"
                ])
                .is_err(),
                "accepted {unexpected}"
            );
        }
    }
}
