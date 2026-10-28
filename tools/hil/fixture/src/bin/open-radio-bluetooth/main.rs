//! Narrow privileged entry point: no shell, arbitrary opcodes or output paths.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod connection_reset;
#[cfg(target_os = "linux")]
mod dtm;
#[cfg(target_os = "linux")]
mod hci;
use oer_lab_fixture::bluetooth::model;
#[cfg(target_os = "linux")]
mod owner;
#[cfg(target_os = "linux")]
mod security_failure;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, clap::ValueEnum)]
enum TerminationArg {
    PeerReset,
    PeerRfkill,
    TargetDisconnect,
    TargetReset,
}

impl From<TerminationArg> for oer_hil_protocol::bluetooth::BluetoothPeripheralTermination {
    fn from(value: TerminationArg) -> Self {
        match value {
            TerminationArg::PeerReset => Self::PeerReset,
            TerminationArg::PeerRfkill => Self::PeerRfkill,
            TerminationArg::TargetDisconnect => Self::TargetDisconnect,
            TerminationArg::TargetReset => Self::TargetReset,
        }
    }
}

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
    /// Exercise one fixed initial-key failure or missing refresh key, without sending application data.
    SecurityFailure {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long)]
        peer: model::PeerAddress,
        #[arg(long)]
        failure: oer_hil_protocol::bluetooth::BluetoothSecurityFailure,
        /// After missing-key rejection, require remote-version completion before Disconnect.
        #[arg(long)]
        read_version_before_disconnect: bool,
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
    /// Connect to one public LE peer and execute one finite termination mode.
    ConnectReset {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long)]
        peer: model::PeerAddress,
        #[arg(long, default_value = "0", value_parser = clap::value_parser!(u16).range(0..=5000))]
        hold_ms: u16,
        #[arg(long, value_enum, default_value = "peer-reset")]
        termination: TerminationArg,
        /// Fixed public test LTK, encryption before either ACL echo.
        #[arg(long)]
        encrypted: bool,
        /// Replace the public LTK between the two ACL echoes, on the same handle.
        #[arg(long, requires = "encrypted")]
        key_refresh: bool,
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
    if let Command::SecurityFailure {
        adapter,
        peer,
        failure,
        read_version_before_disconnect,
    } = command
    {
        let mut report = model::security_failure::Report::new(adapter, peer, failure);
        report.read_version_before_disconnect = read_version_before_disconnect;
        let result = (|| -> Result<()> {
            if read_version_before_disconnect
                && failure != oer_hil_protocol::bluetooth::BluetoothSecurityFailure::MissingKey
            {
                return Err("remote-version diagnostic requires missing-key rejection".into());
            }
            let _signals = oer_os_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::security_failure(adapter, peer, &mut report)
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
            eprintln!("cannot write security failure result: {error}");
            std::process::exit(1);
        }
        if !report.passed(adapter, peer, failure) {
            std::process::exit(1);
        }
        return;
    }
    if let Command::ConnectReset {
        adapter,
        peer,
        hold_ms,
        termination,
        encrypted,
        key_refresh,
    } = command
    {
        let termination = termination.into();
        let mut report = model::ConnectionReset::new(adapter, peer, hold_ms, termination);
        report.encrypted = encrypted;
        report.key_refresh = key_refresh;
        let result = (|| -> Result<()> {
            let _signals = oer_os_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::connect_reset(adapter, peer, hold_ms, termination, &mut report)
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
        if !report.passed_profile(adapter, peer, hold_ms, termination, encrypted, key_refresh) {
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
        unreachable!()
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
    fn finite_reset_command_rejects_unbounded_or_arbitrary_arguments() {
        let valid = [
            "helper",
            "connect-reset",
            "--adapter",
            "hci0",
            "--peer",
            "30:ED:A0:F3:F6:D1",
        ];
        assert!(Cli::try_parse_from(valid).is_ok());
        for tail in [
            vec!["--hold-ms", "5001"],
            vec!["--hold-ms", "-1"],
            vec!["--opcode", "0x0c03"],
            vec!["--output", "/etc/test"],
        ] {
            assert!(Cli::try_parse_from(valid.into_iter().chain(tail)).is_err());
        }
        for peer in [
            "../hci0",
            "30:ED:A0:F3:F6",
            "30:ED:A0:F3:F6:D1:00",
            "30:ED:A0:F3:F6:+1",
            "30:ED:A0:F3:F6:GG",
        ] {
            assert!(
                Cli::try_parse_from([
                    "helper",
                    "connect-reset",
                    "--adapter",
                    "hci0",
                    "--peer",
                    peer
                ])
                .is_err()
            );
        }
        assert!(Cli::try_parse_from(valid.into_iter().chain(["--hold-ms", "5000"])).is_ok());
        for termination in [
            "peer-reset",
            "peer-rfkill",
            "target-disconnect",
            "target-reset",
        ] {
            assert!(
                Cli::try_parse_from(valid.into_iter().chain(["--termination", termination]))
                    .is_ok()
            );
        }
        assert!(
            Cli::try_parse_from(valid.into_iter().chain(["--termination", "disconnect-now"]))
                .is_err()
        );
    }

    #[test]
    fn capabilities_is_a_parameterless_finite_command() {
        assert!(Cli::try_parse_from(["helper", "capabilities"]).is_ok());
        assert!(Cli::try_parse_from(["helper", "capabilities", "--adapter", "hci0"]).is_err());
    }
}
