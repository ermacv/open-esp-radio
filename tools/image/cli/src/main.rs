//! `cargo fw`: build, flash and run the staged images of `firmware/apps` on
//! the stand's boards (README.md). This binary parses the
//! command line; its modules hold the commands, over the image pipeline
//! (`oer-image-build`) and the stand's one board path (`oer-lab-board`).

#![forbid(unsafe_code)]

use std::{process::ExitCode, time::Duration};

use clap::{CommandFactory as _, Parser, Subcommand};
mod command;
mod diagnostics;
mod image;
mod w1;
mod w2;
mod w3;

use oer_image_build::probe;

#[derive(Parser)]
#[command(about = "Build, flash and run staged firmware images on the stand's boards")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render typed boot diagnostics from a captured console file; no board I/O.
    DecodeConsole { console: std::path::PathBuf },
    /// W3 host observations and strict report checking, independent of firmware.
    W3 {
        #[command(subcommand)]
        command: W3Command,
    },
    /// Build an image of firmware/apps into target/fw/<image>/.
    ///
    /// Without --analysis the image is unconfigured; `run` builds it for
    /// the stand.
    Build {
        image: String,
        /// Use a fixed fictitious stand configuration (w0 through w3).
        /// The bundle in target/fw/<image>-analysis/ cannot be flashed.
        #[arg(long)]
        analysis: bool,
    },
    /// Compare sizes.json files in two output directories; builds nothing.
    /// Exit 0 for same/report, 1 for stop or incompatible provenance.
    Sizes {
        base: std::path::PathBuf,
        head: std::path::PathBuf,
        /// Compare a new composition against the sum of BASE and these images.
        /// Allows different recipes; compiler, target and profile must agree.
        #[arg(long = "add-base")]
        additional: Vec<std::path::PathBuf>,
    },
    /// Reanalyse saved ELF bundles into a separate summary tree; never builds.
    SizesRefresh {
        input: std::path::PathBuf,
        output: std::path::PathBuf,
        /// Checkout supplying the original image configuration.
        #[arg(long)]
        source_root: Option<std::path::PathBuf>,
    },
    /// Build a probe image of firmware/probes (a package named
    /// `oer-firmware-probes-<chip>-…`) with `cfg(oer_probe)` into
    /// target/fw-probe/ and print its path.
    Probe {
        package: String,
        /// Parallel cargo jobs.
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// Write a built image into a board's flash and reset it.
    Flash {
        image: String,
        /// The board's stand id or MAC.
        #[arg(long)]
        board: String,
    },
    /// Build and flash an image, then read the board's console until its
    /// `OER-RESULT:` line: exit 0 on pass, 1 on fail, 2 on a timeout and,
    /// for w1, 3 on a missing, malformed or contradicted report. For w1 the
    /// stand router is first brought to the W1 profile over SSH.
    Run {
        image: String,
        #[arg(long)]
        board: String,
        /// How long to wait for the result, such as `30s` or `2m`; the
        /// image's own run time when omitted (w1 120 s, any other 30 s).
        #[arg(long, value_parser = parse_duration)]
        timeout: Option<Duration>,
    },
    /// Print a board's console without resetting it.
    Monitor {
        #[arg(long)]
        board: String,
    },
    /// Bring the stand router to W1's profile and verify it, without a
    /// board; print the stand AP's masked BSSID and channel.
    Router,
    /// List the attached boards and their stand ids.
    Boards,
    /// Switch a board's hub port off and on again.
    PowerCycle {
        #[arg(long)]
        board: String,
    },
}

#[derive(Subcommand)]
enum W3Command {
    /// Verify the W3 host path using the installed AX211 station fixture.
    #[cfg(target_os = "linux")]
    Ax211,
    /// Recompute a-d and f from a firmware console and router observations.
    Check {
        #[arg(long)]
        console: std::path::PathBuf,
        #[arg(long)]
        observations: std::path::PathBuf,
    },
}

/// `30s`, `2m` or a number of seconds.
fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let (number, unit) = match text.strip_suffix('s') {
        Some(number) => (number, 1),
        None => match text.strip_suffix('m') {
            Some(number) => (number, 60),
            None => (text, 1),
        },
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(unit))
        .map(Duration::from_secs)
        .ok_or_else(|| format!("`{text}` is no duration such as 30s or 2m"))
}

fn main() -> ExitCode {
    if oer_os_command_tree::requested() {
        let tree = oer_os_command_tree::command_tree(&Cli::command(), &[String::from("fw")]);
        println!("{}", oer_os_command_tree::json(&tree));
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    let result = (|| -> oer_image_build::Result<ExitCode> {
        let root = command::root()?;
        match cli.command {
            Command::DecodeConsole { console } => diagnostics::decode_file(&console)?,
            #[cfg(target_os = "linux")]
            Command::W3 {
                command: W3Command::Ax211,
            } => return w3::ax211::run(&root),
            Command::W3 {
                command:
                    W3Command::Check {
                        console,
                        observations,
                    },
            } => return w3::check_files(&console, &observations),
            Command::Build { image, analysis } => command::build(&root, &image, analysis)?,
            Command::Sizes {
                base,
                head,
                additional,
            } => {
                let result = if additional.is_empty() {
                    oer_image_build::sizes::compare_directories(&base, &head)?
                } else {
                    oer_image_build::sizes::compare_composition(&base, &additional, &head)?
                };
                print!("{}", result.output);
                return Ok(if result.verdict == oer_image_build::sizes::Verdict::Stop {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                });
            }
            Command::SizesRefresh {
                input,
                output,
                source_root,
            } => {
                let count = oer_image_build::sizes::refresh(
                    &input,
                    &output,
                    source_root.as_deref().unwrap_or(&root),
                )?;
                println!("refreshed {count} image summaries");
            }
            Command::Probe { package, jobs } => {
                println!("{}", probe::build(&root, &package, jobs)?.display());
            }
            Command::Flash { image, board } => command::flash_image(&root, &image, &board)?,
            Command::Run {
                image,
                board,
                timeout,
            } => {
                let timeout = timeout.unwrap_or_else(|| image::run_timeout(&image));
                return command::run(&root, &image, &board, timeout);
            }
            Command::Monitor { board } => command::monitor(&root, &board)?,
            Command::Router => command::router_ready()?,
            Command::Boards => command::list_boards()?,
            Command::PowerCycle { board } => command::power_cycle(&root, &board)?,
        }
        Ok(ExitCode::SUCCESS)
    })();
    result.unwrap_or_else(|error| {
        eprintln!("fw: {error}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(line.split(' '))
    }
    #[test]
    fn only_build_accepts_analysis() {
        for image in ["w0", "w1", "w2"] {
            assert!(matches!(
                parse(&format!("fw build {image} --analysis"))
                    .unwrap()
                    .command,
                Command::Build { analysis: true, .. }
            ));
        }
        assert!(matches!(
            parse("fw build w2").unwrap().command,
            Command::Build {
                analysis: false,
                ..
            }
        ));
        for command in ["flash", "run"] {
            assert!(parse(&format!("fw {command} w2 --board b1 --analysis")).is_err());
        }
    }

    #[test]
    fn no_image_takes_stand_flags() {
        for line in [
            "fw build w1 --expect-bssid 02:00:00:00:F6:D0",
            "fw build w1 --channel 6",
            "fw run w1 --board b1 --channel 6",
            "fw run w1 --board b1 --expect-bssid 02:00:00:00:F6:D0",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
        assert!(matches!(
            parse("fw build w1").unwrap().command,
            Command::Build { .. }
        ));
        assert!(matches!(
            parse("fw run w1 --board b1 --timeout 90s").unwrap().command,
            Command::Run { .. }
        ));
        assert!(matches!(
            parse("fw router").unwrap().command,
            Command::Router
        ));
        assert!(parse("fw router --channel 6").is_err());
    }

    #[test]
    fn saved_size_refresh_has_an_explicit_output_and_configuration_root() {
        assert!(matches!(
            parse("fw sizes-refresh saved output --source-root configuration")
                .unwrap()
                .command,
            Command::SizesRefresh {
                source_root: Some(_),
                ..
            }
        ));
        assert!(parse("fw sizes-refresh saved").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("5"), Ok(Duration::from_secs(5)));
        assert!(parse_duration("soon").is_err());
    }
}

#[cfg(test)]
#[path = "../../../../firmware/apps/w1/src/own/crc32.rs"]
mod ownership_crc;
