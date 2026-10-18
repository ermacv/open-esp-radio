//! `cargo hil`: the alias runs this binary with the command's arguments.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use oer_hil_cli::Result;
use oer_process::Checkout;

/// A leading `--root PATH` (or `--root=PATH`) names the checkout to act on,
/// as a job's detached process passes it; every
/// other argument is the `cargo hil` command.
fn split_root(mut args: Vec<OsString>) -> Result<(Option<PathBuf>, Vec<OsString>)> {
    let Some(first) = args.first().and_then(|first| first.to_str()) else {
        return Ok((None, args));
    };
    if first == "--root" {
        if args.len() < 2 {
            return Err("--root requires a value".into());
        }
        let rest = args.split_off(2);
        return Ok((Some(PathBuf::from(&args[1])), rest));
    }
    if let Some(root) = first.strip_prefix("--root=") {
        let root = PathBuf::from(root);
        return Ok((Some(root), args.split_off(1)));
    }
    Ok((None, args))
}

fn run() -> Result<ExitCode> {
    let (root, args) = split_root(std::env::args_os().skip(1).collect())?;
    let ctx = match root {
        Some(root) => Checkout::new(root)?,
        None => Checkout::discover("cargo hil")?,
    };
    let _signals = oer_process::install_signal_handlers()?;
    oer_hil_cli::command::run(&ctx, &args)
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("hil: {error}");
            ExitCode::from(failure_status(&*error))
        }
    }
}

/// The exit status of a failed command: 130 when Ctrl-C cancelled it, such
/// as a wait for the run store's pins, else 1.
fn failure_status(error: &(dyn std::error::Error + 'static)) -> u8 {
    if oer_process::is_cancelled(error) {
        130
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn a_leading_root_names_the_checkout_and_the_rest_is_the_command() {
        assert_eq!(
            split_root(words("--root /c queue --json")).unwrap(),
            (Some(PathBuf::from("/c")), words("queue --json"))
        );
        assert_eq!(
            split_root(words("--root=/c run a")).unwrap(),
            (Some(PathBuf::from("/c")), words("run a"))
        );
        // Only a leading --root is this binary's; a later one is the command's.
        assert_eq!(
            split_root(words("lease --root /c")).unwrap(),
            (None, words("lease --root /c"))
        );
        assert!(split_root(words("--root")).is_err());
    }

    #[test]
    fn a_cancelled_command_exits_130_and_any_other_failure_1() {
        assert_eq!(failure_status(&oer_process::Cancelled), 130);
        let failed: Box<dyn std::error::Error> = "no such run".into();
        assert_eq!(failure_status(&*failed), 1);
    }
}
