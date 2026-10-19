//! `cargo pins`: fetch and verify a chip's pinned vendor artifacts
//! (`chips/<chip>/vendor/artifacts.toml`) into the host-wide store that
//! every checkout's `target/vendor` links to. This binary parses the command
//! line and calls `oer-vendor-artifacts`.

#![forbid(unsafe_code)]

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{CommandFactory as _, Parser, Subcommand};
use oer_tool_pins::artifacts;

#[derive(Parser)]
#[command(about = "Fetch and verify pinned vendor artifacts")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch every artifact of the chip's pins, or the named ones, verify
    /// each against its SHA-256 and print where it lies.
    Fetch {
        /// The chip whose pins to fetch.
        #[arg(long)]
        chip: String,
        /// Artifact ids; all of the chip's when none is named.
        ids: Vec<String>,
    },
    /// Fetch the inputs of Blobray's RISC-V conformance test (the
    /// riscv-arch-test checkout and Sail) at their pins and print where
    /// they lie.
    Conformance,
    /// Link `ROOT/target/vendor` to the host-wide vendor artifact store (a
    /// new worktree finds the fetched artifacts).
    LinkStore { root: PathBuf },
}

/// The repository this tool runs in.
fn root() -> artifacts::Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(std::fs::canonicalize(manifest.join("../.."))?)
}

fn main() -> ExitCode {
    if oer_os_command_tree::requested() {
        let tree = oer_os_command_tree::command_tree(&Cli::command(), &[String::from("pins")]);
        println!("{}", oer_os_command_tree::json(&tree));
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    let result = (|| -> artifacts::Result<()> {
        let root = root()?;
        match cli.command {
            Command::Fetch { chip, ids } => artifacts::run(&root, &chip, &ids),
            Command::Conformance => {
                let inputs = artifacts::conformance::fetch(&root)?;
                println!("riscv-arch-test  {}", inputs.arch_test.display());
                println!("sail             {}", inputs.sail.display());
                Ok(())
            }
            Command::LinkStore { root } => artifacts::link_store(&root, &artifacts::store()?),
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pins: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_holds_the_pins() {
        assert!(
            root()
                .unwrap()
                .join("chips/esp32s31/vendor/artifacts.toml")
                .is_file()
        );
    }

    #[test]
    fn fetch_takes_a_chip_and_ids() {
        let cli = Cli::try_parse_from(["pins", "fetch", "--chip", "esp32c5", "libphy"]).unwrap();
        let Command::Fetch { chip, ids } = cli.command else {
            panic!("not a fetch");
        };
        assert_eq!((chip.as_str(), ids), ("esp32c5", vec!["libphy".to_owned()]));
        assert!(Cli::try_parse_from(["pins", "fetch", "libphy"]).is_err());
    }
}
