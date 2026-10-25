#![forbid(unsafe_code)]
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Validate a chip's register model and generate its PAC and linker fragment")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Load a chip's register model and ownership policy
    /// (`chips/<chip>/registers/`) and check every rule.
    Validate {
        #[arg(long)]
        chip: String,
    },
    /// Render the chip's thin PAC (`chips/<chip>/pac/src/generated.rs`) and
    /// DMA-window linker fragment (`chips/<chip>/layout/linker/dma-window.x`)
    /// from its register model; with `--check`, compare instead of writing.
    Generate {
        #[arg(long)]
        chip: String,
        #[arg(long)]
        check: bool,
    },
    /// Generate Blobray ABI contracts from pinned ESP-IDF headers.
    Signatures {
        #[arg(long)]
        chip: String,
        #[arg(long)]
        idf: PathBuf,
        #[arg(long)]
        sdkconfig: PathBuf,
        #[arg(long)]
        libc_include: Option<PathBuf>,
        #[arg(long, default_value = "clang")]
        clang: std::ffi::OsString,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// The register checks.
    #[command(subcommand)]
    Check(Check),
}

#[derive(Subcommand)]
enum Check {
    /// Reject vendor uses of absent register bits in every pinned vendor
    /// binary; report violations, unknown bits and corpus limits in
    /// `target/register-inventory/<chip>/report.txt`. Analyzes typed Blobray
    /// records in process.
    Inventory {
        #[arg(long)]
        chip: String,
        /// The report directory, by default `target/register-inventory/<chip>`.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}
fn run() -> oer_registers_generate::Result<()> {
    let _signals = oer_os_process::install_signal_handlers()?;
    let ctx = oer_os_process::Checkout::discover("registers")?;
    match Cli::parse().command {
        Command::Validate { chip } => {
            oer_registers_generate::validate(&ctx.root, &chip)?;
            println!("register model of {chip} valid");
        }
        Command::Generate { chip, check } => {
            oer_registers_generate::generate(&ctx.root, &chip, check)?;
            println!(
                "register outputs of {chip} {}",
                if check { "verified" } else { "written" }
            );
        }
        Command::Signatures {
            chip,
            idf,
            sdkconfig,
            libc_include,
            clang,
            check,
            output,
        } => {
            oer_registers_generate::signatures::run(
                &ctx,
                oer_registers_generate::signatures::Options {
                    chip,
                    idf,
                    sdkconfig,
                    libc_include,
                    clang,
                    check,
                    output,
                },
            )?;
        }
        Command::Check(Check::Inventory { chip, output }) => {
            oer_registers_generate::checks::inventory::run(&ctx, &chip, output)?;
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    if oer_os_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_os_command_tree::command_tree(&Cli::command(), &[String::from("registers")]);
        println!("{}", oer_os_command_tree::json(&tree));
        return std::process::ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("register publication failed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
