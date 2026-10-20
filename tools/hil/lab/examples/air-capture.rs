//! Passive smoke check. pcap is never written; decoded addresses stay private.
use clap::Parser;
use oer_hil_lab::{
    air::AirCapture,
    config::{BoardChoice, LabConfig},
};

#[derive(Debug, Parser)]
#[command(
    name = "air-capture",
    about = "Passively capture management and ACK frames"
)]
struct Cli {
    channel: u8,
    // This example historically ignores arguments after the channel.
    #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
    ignored: Vec<std::ffi::OsString>,
}

fn main() -> oer_hil_lab::Result<()> {
    let cli = Cli::try_parse().unwrap_or_else(|error| {
        let status = i32::from(error.use_stderr());
        let _ = error.print();
        std::process::exit(status);
    });
    let _signals = oer_os_process::install_signal_handlers()?;
    let lab = LabConfig::load(
        &LabConfig::default_path()?,
        "esp32s31",
        &BoardChoice {
            dut: Some("s31-a".to_owned()),
            ..Default::default()
        },
    )?;
    let capture = AirCapture::start(&lab, cli.channel)?;
    oer_os_process::sleep(std::time::Duration::from_secs(2))?;
    let frames = capture.finish()?;
    println!(
        "air: decoded {} management/ACK frames; no pcap file",
        frames.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn previous_channel_and_ignored_tail_forms_parse() {
        Cli::command().debug_assert();
        let cli =
            Cli::try_parse_from(["air-capture", "11"]).expect("accepted example command line");
        assert_eq!(cli.channel, 11);
        assert!(cli.ignored.is_empty());
        let cli = Cli::try_parse_from(["air-capture", "6", "--old-extra", "value"])
            .expect("accepted example command line");
        assert_eq!(cli.channel, 6);
        assert_eq!(cli.ignored, ["--old-extra", "value"]);
        for arguments in [
            vec!["air-capture"],
            vec!["air-capture", "256"],
            vec!["air-capture", "bad-channel"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }
}
