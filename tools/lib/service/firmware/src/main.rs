//! `cargo fw`, the dev kit: flash an image bundle or an ESP-IDF catalog
//! image to an attached board, monitor a board's console, list the attached
//! boards and compare two linked images.
//!
//! It uses the image pipeline (`oer-image`), the devices
//! library (`oer-devices`: discovery, the device lock, the flash writer,
//! consoles), the chip model and the foundation; it knows nothing of the
//! HIL stand. A board busy with another process (a HIL lease, another
//! `cargo fw`) is refused naming its holder, or waited for with `--wait`.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use clap::{Parser, Subcommand};
use oer_chip_profile::Profile;
use oer_image_bundle::ImageBundle;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
#[command(
    name = "cargo fw",
    about = "Flash and monitor firmware on attached boards"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write an image to an attached board and start it: an image bundle's
    /// directory or an ESP-IDF catalog image (built first).
    Flash {
        #[arg(value_name = "BUNDLE|CATALOG-IMAGE")]
        image: String,
        #[command(flatten)]
        device: DeviceOptions,
        /// Monitor the console after the write.
        #[arg(long)]
        monitor: bool,
    },
    /// Read a board's console.
    Monitor {
        #[command(flatten)]
        device: DeviceOptions,
        /// Reset the board into its application first.
        #[arg(long)]
        reset: bool,
        #[command(flatten)]
        capture: CaptureOptions,
    },
    /// The attached boards: port, MAC, known chip and the holder of each.
    Devices {
        /// Connect to each free board's ROM to learn its chip.
        #[arg(long)]
        probe: bool,
        #[arg(long)]
        json: bool,
    },
    /// Compare two linked ELF images function by function, modulo
    /// placement (a binary analysis: runs with the `checks` build).
    Compare {
        old: PathBuf,
        new: PathBuf,
        /// Reviewed rename applied to both images, FROM=TO (e.g. a moved path).
        #[arg(long = "alias")]
        aliases: Vec<String>,
        /// Reviewed function whose code may differ (a scheduling tie).
        #[arg(long = "allow")]
        allowed: Vec<String>,
        /// Print the instruction diff of differing functions whose name
        /// contains this.
        #[arg(long)]
        show: Vec<String>,
    },
}

#[derive(clap::Args, Clone, Default)]
struct DeviceOptions {
    /// The board: its MAC or port; default the only attached board.
    #[arg(long, value_name = "MAC|PORT")]
    device: Option<String>,
    /// Wait while another process holds the board instead of failing.
    #[arg(long)]
    wait: bool,
}

#[derive(clap::Args, Clone, Default)]
struct CaptureOptions {
    /// Stop after this long (`30s`, `2m`); default until interrupted.
    #[arg(long = "for", value_name = "DURATION", value_parser = parse_duration)]
    duration: Option<Duration>,
    /// Stop at the first line containing TEXT; fails when it never appears.
    #[arg(long, value_name = "TEXT")]
    until: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo fw: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    if oer_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("fw")]);
        println!("{}", oer_command_tree::json(&tree));
        return Ok(ExitCode::SUCCESS);
    }
    let cli = Cli::parse();
    let root = oer_process::Checkout::discover("cargo fw")?.root;
    if !cfg!(feature = "checks") && matches!(cli.command, Command::Compare { .. }) {
        return with_checks(&root);
    }
    match cli.command {
        Command::Flash {
            image,
            device,
            monitor,
        } => flash(&root, &image, &device, monitor),
        Command::Monitor {
            device,
            reset,
            capture,
        } => {
            let opened = oer_devices::device::find(device.device.as_deref())?
                .open(&command_line(), device.wait)?;
            let console = if reset {
                opened.reset()?
            } else {
                opened.console()?
            };
            monitor(&root, console, &capture)
        }
        Command::Devices { probe, json } => {
            let mut devices = oer_devices::devices();
            if probe {
                let chips = Profile::all(&root)?;
                for device in &mut devices {
                    if device.holder.is_some() {
                        continue;
                    }
                    match device.clone().open(&command_line(), false) {
                        Ok(mut opened) => match opened.probe(&chips) {
                            Ok(chip) => device.chip = Some(chip),
                            Err(error) => eprintln!("{}: {error}", device.mac),
                        },
                        Err(error) => eprintln!("{}: {error}", device.mac),
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
            } else if devices.is_empty() {
                println!("no board is attached");
            } else {
                for device in devices {
                    println!(
                        "{:<17} {:<10} {:<60} {}",
                        device.mac,
                        device.chip.as_deref().unwrap_or("?"),
                        device.port.display(),
                        device.holder.map_or_else(
                            || String::from("free"),
                            |holder| format!("busy: {holder}")
                        )
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Compare {
            old,
            new,
            aliases,
            allowed,
            show,
        } => {
            compare(&old, &new, aliases, allowed, show)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Compare `old` with `new` function by function under the reviewed
/// aliases and allowed differences; fails when they differ beyond
/// placement.
#[cfg(feature = "checks")]
fn compare(
    old: &Path,
    new: &Path,
    aliases: Vec<String>,
    allowed: Vec<String>,
    show: Vec<String>,
) -> Result<()> {
    let aliases = aliases
        .into_iter()
        .map(|alias| {
            alias
                .split_once('=')
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .ok_or_else(|| format!("alias `{alias}` is not FROM=TO").into())
        })
        .collect::<Result<Vec<_>>>()?;
    let review = oer_image_compare::Review {
        aliases: oer_image_compare::Aliases(aliases),
        allowed: allowed.into_iter().collect(),
        show,
    };
    let comparison = oer_image_compare::compare_elf(old, new, &review)?;
    if comparison.equivalent(&review.allowed) {
        Ok(())
    } else {
        Err("images differ beyond placement".into())
    }
}

/// Without the `checks` feature, `run` re-executes before a comparison.
#[cfg(not(feature = "checks"))]
fn compare(_: &Path, _: &Path, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Result<()> {
    unreachable!("an image comparison runs with the `checks` feature")
}

/// Run this command again as `oer-fw` built with the `checks` feature: the
/// image comparison (and its analyzers) is only compiled into that build.
/// Preserve this operation's explicit context through Cargo so a delegated
/// operation stays inside its enclosing lease after the feature re-exec.
fn with_checks(root: &Path) -> Result<ExitCode> {
    let mut command =
        oer_process::command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "-p",
            "oer-fw",
            "--features",
            "checks",
            "--",
        ])
        .args(std::env::args_os().skip(1));
    oer_process::Context::current()?.apply(&mut command)?;
    let status = command.status()?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// This process's command line, for the device lock's holder record.
fn command_line() -> String {
    let mut words = vec![String::from("cargo fw")];
    words.extend(std::env::args().skip(1));
    words.join(" ")
}

/// `cargo fw flash`: resolve the image to a bundle, take the board, write
/// and start it, then monitor when asked.
fn flash(
    root: &Path,
    image: &str,
    device: &DeviceOptions,
    monitor_after: bool,
) -> Result<ExitCode> {
    let found = oer_devices::device::find(device.device.as_deref())?;
    let path = Path::new(image);
    let bundle = if path.is_dir() {
        ImageBundle::load(&std::path::absolute(path)?)?
    } else {
        let entries = oer_image::esp_idf::catalog::entries(root)?;
        let entry = oer_image::esp_idf::catalog::entry(&entries, image)
            .map_err(|_| format!("`{image}` is no bundle directory or catalog image"))?;
        if let Some(reason) = &entry.hold {
            return Err(format!("`{image}` is held and not flashed: {reason}").into());
        }
        let build = oer_image::esp_idf::catalog::build(root, image)?;
        oer_image::esp_idf::catalog::bundle(
            root,
            entry,
            &build,
            &root.join("target/firmware/catalog").join(&entry.image),
        )?
    };
    let profile = Profile::load(root, &bundle.chip)?;
    let mut opened = found.open(&command_line(), device.wait)?;
    eprintln!(
        "fw: writing {} to {} ({})",
        bundle.directory.display(),
        opened.device().mac,
        opened.device().port.display()
    );
    opened.write(&bundle, image, &profile)?;
    eprintln!("fw: {} runs the image", opened.device().mac);
    if !monitor_after {
        return Ok(ExitCode::SUCCESS);
    }
    monitor(root, opened.console()?, &CaptureOptions::default())
}

/// Copy `console` to the terminal and a log until the capture ends, under
/// its device lock.
fn monitor(
    root: &Path,
    console: oer_devices::device::Console,
    capture: &CaptureOptions,
) -> Result<ExitCode> {
    let mac = console.device().mac.clone();
    let directory = root.join("target/firmware/console");
    std::fs::create_dir_all(&directory)?;
    let log = directory.join(format!(
        "{}-{}.log",
        mac.compact(),
        oer_durable::unix_millis()
    ));
    eprintln!("fw: console of {mac} (log {})", log.display());
    // Without a duration the capture lasts until the process is interrupted.
    let duration = capture
        .duration
        .unwrap_or(Duration::from_secs(u64::from(u32::MAX)));
    let seen = console.capture(duration, capture.until.as_deref(), &log)?;
    Ok(match (&capture.until, seen) {
        (Some(text), false) => {
            eprintln!("fw: `{text}` did not appear");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    })
}

/// A duration such as `90s`, `2m` or `1h30m`; a bare number is seconds.
fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let mut total = 0_u64;
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let unit = match character {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(format!("invalid duration `{text}`; use e.g. 30s, 2m or 1h")),
        };
        let value: u64 = number
            .parse()
            .map_err(|_| format!("invalid duration `{text}`"))?;
        total += value * unit;
        number.clear();
    }
    if !number.is_empty() || total == 0 {
        return Err(format!("invalid duration `{text}`; use e.g. 30s, 2m or 1h"));
    }
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_example_command_is_left() {
        for removed in ["build", "list"] {
            let error = Cli::try_parse_from(["cargo fw", removed]).err().unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
        let error = Cli::try_parse_from(["cargo fw", "flash", "bundle", "--check", "stack"])
            .err()
            .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
