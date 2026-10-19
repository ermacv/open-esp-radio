//! The inventory run behind `cargo registers inventory`: Blobray's
//! `register-accesses`, under the RISC-V integer calling convention and
//! with the constants callees return taken at their calls, over
//! every pinned vendor binary inside the publication's owned MMIO ranges,
//! compared with the register model ([`super`]), with the provenance
//! registry's cited functions ranked first.
use super::{self as inventory, Region};
use crate::Result;
use oer_os_process::Checkout;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Wall-clock budget of one Blobray operation. Analyzing every pinned
/// binary takes most of it; the host's default budget is shorter.
const OPERATION_TIMEOUT_SECS: u64 = 3600;

/// One Blobray input: a pinned vendor binary, which Blobray reads by its
/// pin.
struct Input {
    id: String,
    sha256: String,
}

/// Every pinned vendor binary of `chip`, in pin order. Firmware outputs are
/// applications linked from these binaries, and sources are not code.
fn inputs(ctx: &Checkout, chip: &str) -> Result<Vec<Input>> {
    let mut inputs = vec![];
    for pinned in oer_pins::artifacts::pinned(&ctx.root, chip)? {
        if pinned.firmware {
            continue;
        }
        let bytes = fs::read(&pinned.path)?;
        if !oer_analysis_elf::is_binary(&bytes) {
            continue;
        }
        inputs.push(Input {
            id: pinned.id,
            sha256: oer_os_durable::sha256_bytes(&bytes),
        });
    }
    if inputs.is_empty() {
        return Err(format!("no pinned vendor binary for {chip}").into());
    }
    Ok(inputs)
}

/// Blobray's register accesses of every function of `inputs` inside
/// `regions`, written to `registers.json` in `directory`.
fn accesses(
    ctx: &Checkout,
    directory: &Path,
    host: &Path,
    chip: &str,
    inputs: &[Input],
    regions: &[Region],
) -> Result<PathBuf> {
    let accesses = directory.join("registers.json");
    eprintln!(
        "register-inventory: analyzing every function of {} vendor binaries",
        inputs.len()
    );
    let mut command = ctx.command(host);
    command
        .args(["--format", "json", "register-accesses"])
        // A base address the vendor keeps in a callee-saved register across
        // a call stays known: without the assumption every register is
        // unknown after each call.
        .args(["--abi", "riscv-integer", "--call-returns"])
        .args(["--timeout-secs", &OPERATION_TIMEOUT_SECS.to_string()]);
    for input in inputs {
        command
            .arg("--input")
            .arg(format!("{}=pin:{chip}/{}", input.id, input.id));
    }
    for region in regions {
        command.args([
            "--range",
            &format!("{:#x}:{:#x}", region.start, region.end - region.start),
        ]);
    }
    // The document is complete only once renamed into place.
    let partial = directory.join("registers.json.partial");
    let status = command.stdout(fs::File::create(&partial)?).status()?;
    if !status.success() {
        return Err(format!(
            "blobray register-accesses failed; its partial output is {}",
            partial.display()
        )
        .into());
    }
    fs::rename(&partial, &accesses)?;
    Ok(accesses)
}

/// Build the inventory of `chip` into `output`, by default
/// `target/register-inventory/<chip>`.
pub fn run(ctx: &Checkout, chip: &str, output: Option<PathBuf>) -> Result<()> {
    oer_pins::artifacts::project::Project::new(&ctx.root, chip)?;
    let directory = output.unwrap_or_else(|| ctx.root.join("target/register-inventory").join(chip));
    fs::create_dir_all(&directory)?;
    let inputs = inputs(ctx, chip)?;
    let (registers, regions) = inventory::model(&ctx.root, chip)?;
    let host = oer_os_toolchain::workspace::blobray_host(&ctx.root)?;
    let accesses = accesses(ctx, &directory, &host, chip, &inputs, &regions)?;
    let ids: Vec<String> = inputs.iter().map(|input| input.id.clone()).collect();
    let observations = inventory::observations(&fs::read_to_string(&accesses)?, &ids)?;
    let cited: BTreeSet<inventory::Function> =
        oer_vendor_provenance::registry::registered(&ctx.root, chip)?
            .into_iter()
            .map(|e| (e.artifact, e.symbol))
            .collect();
    let mut words = inventory::classify(
        &regions,
        &inventory::declared_words(&registers),
        &inventory::arrays(&registers),
        &observations,
        &cited,
    );
    inventory::rank(&mut words);

    let text = inventory::render(chip, &words);
    fs::write(directory.join("report.txt"), &text)?;
    fs::write(
        directory.join("report.json"),
        serde_json::to_string_pretty(&inventory::Report {
            chip,
            inputs: inputs
                .iter()
                .map(|i| (i.id.as_str(), i.sha256.as_str()))
                .collect(),
            words: &words,
        })?,
    )?;
    print!(
        "{}",
        text.lines()
            .take(5)
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    );
    println!("report: {}", directory.join("report.txt").display());
    Ok(())
}
