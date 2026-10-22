//! Public-header ABI facts for Blobray's signature contract. This consumer
//! neither decodes instructions nor narrows dataflow itself. The four-field
//! function format follows #303; per-header hashes and the complete corpus
//! stay in the companion provenance document.
mod ast;
mod corpus;
mod headers;
mod source;
mod types;

use crate::Result;
use oer_os_process::Checkout;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::PathBuf,
};

/// Inputs for `cargo registers signatures`. The configuration is explicit:
/// declarations are interpreted under that sdkconfig, never the host ABI.
pub struct Options {
    pub chip: String,
    pub idf: PathBuf,
    pub sdkconfig: PathBuf,
    pub libc_include: Option<PathBuf>,
    pub clang: OsString,
    pub check: bool,
    pub output: Option<PathBuf>,
}

#[derive(Serialize)]
struct Provenance<'a> {
    schema: u32,
    chip: &'a str,
    esp_idf_revision: &'a str,
    abi: &'a str,
    sdkconfig_sha256: String,
    /// Names/paths refer to the source tree; no machine-specific absolute
    /// cache or tool paths enter the generated facts.
    headers: &'a BTreeMap<String, String>,
    corpus: &'a [corpus::Input],
}

fn write(path: &std::path::Path, content: &str, check: bool) -> Result<()> {
    if check {
        if fs::read_to_string(path)? != content {
            return Err(format!("{} is stale; regenerate signatures", path.display()).into());
        }
    } else {
        fs::write(path, content)?;
    }
    Ok(())
}

/// Capture all externally linked named call targets in every pinned binary,
/// qualify their prototypes against the pinned ESP-IDF headers and publish
/// only supported, unambiguous facts. Reports and raw captures stay ignored.
pub fn run(ctx: &Checkout, options: Options) -> Result<()> {
    if !matches!(options.chip.as_str(), "esp32s31" | "esp32c5") {
        return Err("signature generation supports esp32s31 and esp32c5".into());
    }
    let manifest = oer_tool_pins::Manifest::load(&ctx.root, &options.chip)?;
    let revision = manifest
        .source("esp-idf")?
        .revision
        .as_deref()
        .ok_or("ESP-IDF has no revision pin")?;
    let actual = oer_os_process::git::text(&options.idf, ["rev-parse", "HEAD"])?;
    if actual != revision {
        return Err(format!("ESP-IDF is {actual}, pinned revision is {revision}").into());
    }
    let output = options.output.clone().unwrap_or_else(|| {
        ctx.root
            .join("target/register-signatures")
            .join(&options.chip)
    });
    fs::create_dir_all(&output)?;
    let config_hash = oer_os_durable::sha256_file(&options.sdkconfig)?;
    let corpus = corpus::capture(ctx, &options.chip, &output)?;
    let names: BTreeSet<_> = corpus.calls.keys().cloned().collect();
    let findings = headers::inspect(&options, &names, revision, &output)?;
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut uncovered = BTreeMap::new();
    for name in &names {
        if findings.signatures.contains_key(name) {
            continue;
        }
        let (reason, details) = if let Some(reasons) = findings.unsupported.get(name) {
            (
                "unsupported-prototype",
                reasons.iter().cloned().collect::<Vec<_>>(),
            )
        } else if let Some(headers) = findings.failed_candidates.get(name) {
            (
                "header-context-unresolved",
                headers.iter().cloned().collect(),
            )
        } else if findings.mentioned.contains(name) {
            ("no-qualified-declaration", Vec::new())
        } else {
            ("no-header-identifier", Vec::new())
        };
        *reasons.entry(reason.into()).or_default() += 1;
        uncovered.insert(
            name.clone(),
            serde_json::json!({"reason":reason,"details":details}),
        );
    }
    let report = serde_json::json!({
        "chip": options.chip, "revision": revision, "corpus": corpus,
        "covered": findings.signatures.len(), "uncovered_by_reason": reasons,
        "uncovered": uncovered, "failed_header_contexts": findings.failed_contexts,
        "conflicting_declarations": findings.conflicts,
    });
    fs::write(
        output.join("coverage.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{}: {} of {} named callees have qualified header contracts; {} have no captured code bytes",
        options.chip,
        findings.signatures.len(),
        names.len(),
        corpus.without_definition.len()
    );
    println!("report: {}", output.join("coverage.json").display());
    if !findings.conflicts.is_empty() {
        return Err("header contracts disagree; see coverage.json, no facts written".into());
    }
    if findings.signatures.is_empty() {
        return Err("no qualified header contracts, no facts written".into());
    }
    let mut facts = format!(
        "# SPDX-License-Identifier: Apache-2.0\n# Generated ABI facts from Espressif ESP-IDF {revision}; no source code is copied.\n# Regenerate with cargo registers signatures; see tools/registers/generate/README.md.\n# arguments counts occupied RV32 a-register words, capped at eight (not C parameters).\n# Context: the companion provenance document records sdkconfig and header hashes.\n"
    );
    #[derive(Serialize)]
    struct One<'a> {
        function: [&'a headers::Signature; 1],
    }
    for signature in findings.signatures.values() {
        facts.push_str(&format!("\n# Header-SHA256: {}\n", signature.sha256));
        facts.push_str(&toml::to_string(&One {
            function: [signature],
        })?);
    }
    if oer_os_durable::sha256_file(&options.sdkconfig)? != config_hash
        || oer_os_process::git::text(&options.idf, ["rev-parse", "HEAD"])? != revision
    {
        return Err("source revision or sdkconfig changed during generation".into());
    }
    source::verify(&options.idf, &findings.headers, revision)?;
    let provenance = Provenance {
        schema: 1,
        chip: &options.chip,
        esp_idf_revision: revision,
        abi: "riscv32-ilp32-integer",
        sdkconfig_sha256: config_hash,
        headers: &findings.headers,
        corpus: &corpus.inputs,
    };
    let ledger = format!(
        "# SPDX-License-Identifier: Apache-2.0\n# Generated provenance of the ESP-IDF header ABI facts.\n{}",
        toml::to_string_pretty(&provenance)?
    );
    let directory = ctx
        .root
        .join("chips")
        .join(&options.chip)
        .join("vendor/facts");
    fs::create_dir_all(&directory)?;
    write(
        &directory.join("idf-function-signatures.toml"),
        &facts,
        options.check,
    )?;
    write(
        &directory.join("idf-function-signatures.provenance.toml"),
        &ledger,
        options.check,
    )?;
    println!(
        "signature facts {}",
        if options.check { "verified" } else { "written" }
    );
    Ok(())
}

#[cfg(test)]
mod tests;
