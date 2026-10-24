#![forbid(unsafe_code)]
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Validate reviewed registers and publish SVD/PAC source")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize empty peripheral geometry from an explicit source request.
    InitModel {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        directory: PathBuf,
    },
    /// Capture SVD into a native editable model without accepting its claims.
    ImportSvd {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        chip: String,
        #[arg(long)]
        address_space: String,
    },
    Validate {
        #[arg(long)]
        manifest: PathBuf,
    },
    Generate {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        check: bool,
    },
    /// Query native analog-I2C geometry, routes, bit states and provenance.
    InspectAnalog {
        #[arg(long)]
        manifest: PathBuf,
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
    /// Handwritten MMIO below DIRECTORY (Git-tracked Rust files) uses one
    /// PAC transaction per register access.
    PacTransactions { directory: PathBuf },
    /// Every MMIO word both the radio and esp-hal write is reviewed.
    SharedWords {
        #[arg(long)]
        chip: String,
        /// esp-hal's `src` directory, as the firmware workspace resolves it.
        #[arg(long)]
        esp_hal_src: PathBuf,
        /// `esp-metadata-generated`'s `src` directory, whose chip file holds
        /// the clock and reset writers esp-hal expands and its peripheral
        /// singleton aliases.
        #[arg(long)]
        esp_metadata_generated_src: PathBuf,
        /// The chip PAC's `src` directory.
        #[arg(long)]
        pac_src: PathBuf,
    },
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
        Command::InitModel { request, directory } => {
            let count = oer_registers_generate::initialize_model(&request, &directory)?;
            println!("initialized {count} unreviewed peripherals");
        }
        Command::ImportSvd {
            source,
            directory,
            chip,
            address_space,
        } => {
            let count =
                oer_registers_generate::import_svd(&source, &directory, &chip, &address_space)?;
            println!("imported {count} unreviewed peripherals; original XML retained");
        }
        Command::Validate { manifest } => {
            oer_registers_generate::Publication::load(&manifest)?;
            println!("register publication inputs valid");
        }
        Command::Generate { manifest, check } => {
            oer_registers_generate::Publication::load(&manifest)?.generate(check)?;
            println!(
                "register outputs {}",
                if check { "verified" } else { "written" }
            );
        }
        Command::InspectAnalog { manifest } => {
            println!(
                "{:#}",
                oer_registers_generate::Publication::load(&manifest)?.analog_inventory()?
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
        Command::Check(Check::PacTransactions { directory }) => {
            let listed = oer_os_process::capture(
                oer_os_process::git::command(&ctx.root)
                    .args(["ls-files", "--"])
                    .arg(&directory),
            )?;
            let files: Vec<String> = String::from_utf8(listed.stdout)?
                .lines()
                .map(str::to_owned)
                .collect();
            oer_registers_generate::checks::pac_transactions::check(&ctx.root, &files)?;
        }
        Command::Check(Check::SharedWords {
            chip,
            esp_hal_src,
            esp_metadata_generated_src,
            pac_src,
        }) => {
            let shared = oer_registers_generate::checks::shared_words::check(
                &ctx.root,
                &chip,
                &oer_registers_generate::checks::shared_words::EspHalSources {
                    esp_hal: &esp_hal_src,
                    metadata_generated: &esp_metadata_generated_src,
                },
                &pac_src,
            )?;
            println!("shared MMIO words of {chip}: {shared} reviewed");
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
