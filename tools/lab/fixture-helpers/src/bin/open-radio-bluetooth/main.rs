//! Narrow privileged entry point: no shell, arbitrary opcodes or output paths.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod dtm;
#[cfg(target_os = "linux")]
mod hci;
use oer_lab_fixture::bluetooth::model;
#[cfg(target_os = "linux")]
mod owner;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print the exact finite interface understood by this helper.
    Capabilities,
    /// Hold fixed 7.5-ms kernel ATT defaults until stdin closes (maximum 120 s).
    AttParameters {
        #[arg(long)]
        adapter: model::Adapter,
    },
    /// Restore a durable interrupted ATT-parameter snapshot, without starting a test.
    RestoreAttParameters {
        #[arg(long)]
        adapter: model::Adapter,
    },
    /// Check DTM on LE 1M, channel 0, PRBS9, with 100 ms RX/TX windows.
    Check {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long, value_enum, default_value = "v2")]
        dtm_version: model::DtmVersion,
        #[arg(long, value_enum, default_value = "receive-transmit")]
        profile: model::DtmProfile,
    },
}

fn main() {
    use clap::Parser as _;
    let command = Cli::parse().command;
    if matches!(&command, Command::Capabilities) {
        println!("{}", model::HELPER_CAPABILITIES);
        return;
    }
    #[cfg(target_os = "linux")]
    let _software = match oer_lab_fixture::launcher::enter(
        oer_lab_fixture::launcher::LaunchTarget::Bluetooth,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!("Bluetooth fixture software lease: {error}");
            std::process::exit(1);
        }
    };
    if let Command::AttParameters { adapter } | Command::RestoreAttParameters { adapter } = &command
    {
        let result = (|| -> Result<()> {
            let _signals = oer_os_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::connection_parameters::run(
                    *adapter,
                    matches!(command, Command::RestoreAttParameters { .. }),
                )
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err("Bluetooth fixture requires Linux".into())
            }
        })();
        if let Err(error) = result {
            eprintln!("ATT parameter lease: {error}");
            std::process::exit(1);
        }
        return;
    }
    let Command::Check {
        adapter,
        dtm_version,
        profile,
    } = command
    else {
        // Capabilities and the ATT operations returned above.
        return;
    };
    let mut report = model::Check::new(adapter, dtm_version, profile);
    let result = (|| -> Result<()> {
        let _signals = oer_os_process::install_signal_handlers()?;
        #[cfg(target_os = "linux")]
        {
            owner::check(adapter, &mut report)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Bluetooth fixture requires Linux".into())
        }
    })();
    if let Err(error) = result {
        report.errors.push(error.to_string());
    }
    if let Err(error) = serde_json::to_writer(std::io::stdout().lock(), &report) {
        eprintln!("cannot write Bluetooth result: {error}");
        std::process::exit(1);
    }
    if !report.passed(adapter, dtm_version, profile) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn att_parameters_do_not_accept_arbitrary_settings_or_paths() {
        for operation in ["att-parameters", "restore-att-parameters"] {
            let args = ["helper", operation, "--adapter", "hci0"];
            assert!(Cli::try_parse_from(args).is_ok());
            for tail in [
                ["--interval", "12"],
                ["--output", "/etc/test"],
                ["--opcode", "0x004c"],
                ["--timeout", "9999"],
            ] {
                assert!(Cli::try_parse_from(args.into_iter().chain(tail)).is_err());
            }
        }
    }

    #[test]
    fn dtm_profile_defaults_to_v2_and_rejects_unbounded_arguments() {
        let args = ["helper", "check", "--adapter", "hci0"];
        assert!(matches!(
            Cli::try_parse_from(args).unwrap().command,
            Command::Check {
                dtm_version: model::DtmVersion::V2,
                ..
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(args.into_iter().chain(["--dtm-version", "v1"]))
                .unwrap()
                .command,
            Command::Check {
                dtm_version: model::DtmVersion::V1,
                ..
            }
        ));
        for tail in [
            ["--dtm-version", "auto"],
            ["--dtm-version", "v3"],
            ["--timeout-ms", "5000"],
            ["--opcode", "0x201e"],
            ["--output", "/etc/test"],
        ] {
            assert!(Cli::try_parse_from(args.into_iter().chain(tail)).is_err());
        }
    }

    #[test]
    fn capabilities_is_a_parameterless_finite_command() {
        assert!(Cli::try_parse_from(["helper", "capabilities"]).is_ok());
        assert!(Cli::try_parse_from(["helper", "capabilities", "--adapter", "hci0"]).is_err());
    }
}
