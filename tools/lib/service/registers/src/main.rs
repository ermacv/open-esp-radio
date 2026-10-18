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
    /// Inventory the vendor's radio MMIO accesses against the register model.
    Inventory {
        #[arg(long)]
        chip: String,
        /// Report directory; `target/register-inventory/<chip>` by default.
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
        /// esp-hal's `src` directory, as the HIL target workspace resolves it.
        #[arg(long)]
        esp_hal_src: PathBuf,
        /// The chip PAC's `src` directory.
        #[arg(long)]
        pac_src: PathBuf,
    },
}
fn run() -> oer_register_tool::Result<()> {
    let _signals = oer_process::install_signal_handlers()?;
    let ctx = oer_process::Checkout::discover("registers")?;
    match Cli::parse().command {
        Command::InitModel { request, directory } => {
            let count = oer_register_tool::initialize_model(&request, &directory)?;
            println!("initialized {count} unreviewed peripherals");
        }
        Command::ImportSvd {
            source,
            directory,
            chip,
            address_space,
        } => {
            let count = oer_register_tool::import_svd(&source, &directory, &chip, &address_space)?;
            println!("imported {count} unreviewed peripherals; original XML retained");
        }
        Command::Validate { manifest } => {
            oer_register_tool::Publication::load(&manifest)?;
            println!("register publication inputs valid");
        }
        Command::Generate { manifest, check } => {
            oer_register_tool::Publication::load(&manifest)?.generate(check)?;
            println!(
                "register outputs {}",
                if check { "verified" } else { "written" }
            );
        }
        Command::Inventory { chip, output } => {
            oer_register_tool::checks::inventory::run(&ctx, &chip, output)?;
        }
        Command::Check(Check::PacTransactions { directory }) => {
            let listed = oer_process::capture(
                oer_process::git::command(&ctx.root)
                    .args(["ls-files", "--"])
                    .arg(&directory),
            )?;
            let files: Vec<String> = String::from_utf8(listed.stdout)?
                .lines()
                .map(str::to_owned)
                .collect();
            oer_register_tool::checks::pac_transactions::check(&ctx.root, &files)?;
        }
        Command::Check(Check::SharedWords {
            chip,
            esp_hal_src,
            pac_src,
        }) => {
            let shared = oer_register_tool::checks::shared_words::check(
                &ctx.root,
                &chip,
                &esp_hal_src,
                &pac_src,
            )?;
            println!("shared MMIO words of {chip}: {shared} reviewed");
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    if oer_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("registers")]);
        println!("{}", oer_command_tree::json(&tree));
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
