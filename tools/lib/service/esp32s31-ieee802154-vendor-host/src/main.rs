//! Run one catalog scenario against the compiled vendor driver and print its
//! boundary trace, one record per line, or compare it with the production
//! engine and print the verdict; or compare every scenario, each in its own
//! process, and write the stand's evidence shard.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use std::panic::{AssertUnwindSafe, catch_unwind};

use oer_esp32s31_ieee802154_vendor_host::{Verdict, catalog, claim, compare, port};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let first = arguments.next();
    if first.as_deref() == Some("shard") {
        let index = match (
            arguments.next().as_deref(),
            arguments.next(),
            arguments.next(),
        ) {
            (Some("--index"), Some(index), None) => PathBuf::from(index),
            _ => return usage(),
        };
        return match shard(&index) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match (first.as_deref(), arguments.next()) {
        (Some("list"), None) => {
            for scenario in catalog::SCENARIOS {
                println!("{}", scenario.name);
            }
            ExitCode::SUCCESS
        }
        (Some(command @ ("run" | "compare")), Some(name)) => {
            let Some(scenario) = catalog::SCENARIOS
                .iter()
                .find(|scenario| scenario.name == name)
            else {
                eprintln!("unknown scenario: {name}");
                return ExitCode::FAILURE;
            };
            let vendor = claim(scenario);
            if command == "run" {
                for record in vendor {
                    println!("{record}");
                }
                return ExitCode::SUCCESS;
            }
            // A port panic is a vendor assertion the port adds; the panic
            // message goes to stderr and the empty trace fails the comparison.
            let port = catch_unwind(AssertUnwindSafe(|| port::run(scenario)))
                .unwrap_or_else(|_| Ok(Vec::new()));
            let verdict = compare(&vendor, port);
            println!("{verdict}");
            match verdict {
                Verdict::Match => ExitCode::SUCCESS,
                Verdict::Diff { .. } => ExitCode::from(1),
                Verdict::Incomplete(_) => ExitCode::from(2),
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: oer-esp32s31-ieee802154-vendor-host list | run <scenario> | compare <scenario> | shard --index <directory>"
    );
    ExitCode::FAILURE
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The chip, the artifact source of the compiled vendor files and the
/// production entry every scenario drives.
const CHIP: &str = "esp32s31";
const SOURCE: &str = "esp-idf";
const PRODUCTION: &str = "oer_espressif_ieee802154_engine::engine";

/// Compare every catalog scenario, each in a process of its own (the
/// vendor driver runs one scenario per process), and write the stand's
/// shard into `index` when every one compared MATCH.
fn shard(index: &Path) -> Result<()> {
    let program = std::env::current_exe()?;
    let mut matched = Vec::new();
    for scenario in catalog::SCENARIOS {
        let status = oer_process::command(&program)
            .args(["compare", scenario.name])
            .stdout(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("{} did not compare MATCH ({status})", scenario.name).into());
        }
        matched.push(scenario.name.to_owned());
    }
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = directory.join("../../../..");
    let inputs = oer_vendor_artifacts::pinned(&root, CHIP)?
        .into_iter()
        .filter(|artifact| artifact.source == SOURCE)
        .map(|artifact| Ok((artifact.id, oer_durable::sha256_file(&artifact.path)?)))
        .collect::<Result<_>>()?;
    let shard = oer_vendor_evidence::producer::host_stand::shard(
        &oer_vendor_evidence::producer::host_stand::Stand {
            chip: CHIP,
            directory,
            source: SOURCE,
            production: PRODUCTION,
        },
        &matched,
        inputs,
    )?;
    let path = oer_vendor_evidence_shard::store::write(index, &shard)?;
    println!("evidence shard {}", path.display());
    Ok(())
}
