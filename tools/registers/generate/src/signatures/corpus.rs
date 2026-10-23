//! Capture named calls with Blobray. ELF supplies only the names to ask for;
//! the existing ELF view remains the sole binary reader. Data/absolute
//! names are candidates too: only Blobray decides whether a call targets one.
use crate::Result;
use oer_analysis_elf::SymbolKind;
use oer_analysis_riscv_model::{AnalysisProfile, AnalysisResources};
use oer_os_process::Checkout;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Input {
    pub role: String,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Caller {
    sites: Vec<Site>,
}
#[derive(Deserialize)]
struct Site {
    target: String,
}
#[derive(Deserialize)]
struct Document {
    analysis_resources: oer_analysis_riscv_model::AnalysisResources,
    schema: u32,
    inputs: Vec<Input>,
    abi: Option<String>,
    call_summaries: bool,
    symbols: Vec<String>,
    callers: Vec<Caller>,
    blocked: Vec<serde_json::Value>,
    partial: u64,
    partial_causes: serde_json::Value,
    gaps: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct Corpus {
    pub analysis_resources: AnalysisResources,
    pub inputs: Vec<Input>,
    pub partial: u64,
    pub partial_causes: serde_json::Value,
    pub blocked: usize,
    pub gaps: u64,
    pub callers: usize,
    pub named_sites: usize,
    /// Externally linked named targets, including targets defined in another
    /// member/pin of this same corpus. A signature can constrain either.
    pub calls: BTreeMap<String, usize>,
    /// Targets without defined code bytes in the captured pins; absolute
    /// import symbols do not establish a captured implementation.
    pub without_definition: BTreeSet<String>,
}

pub(super) fn capture(ctx: &Checkout, chip: &str, output: &Path) -> Result<Corpus> {
    let mut inputs = Vec::new();
    let mut names = BTreeSet::new();
    let mut defined = BTreeSet::new();
    for pin in oer_tool_pins::artifacts::pinned(&ctx.root, chip)? {
        if pin.firmware {
            continue;
        }
        let bytes = fs::read(&pin.path)?;
        if !oer_analysis_elf::is_binary(&bytes) {
            continue;
        }
        inputs.push(Input {
            role: pin.id,
            sha256: oer_os_durable::sha256_bytes(&bytes),
        });
        for (_, elf) in oer_analysis_elf::objects(&bytes)? {
            let executable: BTreeMap<_, _> = elf
                .sections()
                .filter(|s| s.executable && !s.data.is_empty())
                .map(|s| (s.index, (s.address, s.data.len() as u64)))
                .collect();
            for symbol in elf.symbols().filter(|s| s.global) {
                let code =
                    symbol
                        .section
                        .and_then(|i| executable.get(&i))
                        .is_some_and(|(start, size)| {
                            symbol.address >= *start && symbol.address - start < *size
                        });
                if !symbol.name.is_empty() {
                    names.insert(symbol.name.to_owned());
                }
                // An absolute import can have a defined symbol without a
                // captured body. Bytes, not SHN_ABS, establish a definition.
                if symbol.defined
                    && code
                    && matches!(symbol.kind, SymbolKind::Text | SymbolKind::Unknown)
                {
                    defined.insert(symbol.name.to_owned());
                }
            }
        }
    }
    if inputs.is_empty() || names.is_empty() {
        return Err(format!("no pinned binary/function corpus for {chip}").into());
    }
    let names: Vec<_> = names.into_iter().collect();
    let host = oer_os_toolchain::workspace::build(&ctx.root, "oer-tool-analysis-cli", "blobray")?;
    let mut command = ctx.command(host);
    let limits = AnalysisProfile::SignatureCorpus.limits();
    command.args([
        "call-arguments",
        "--analysis-profile",
        "signature-corpus",
        "--format",
        "json",
        "--abi",
        "riscv-integer",
        "--call-summaries",
        "--timeout-secs",
        &(limits.timeout_ms / 1000).to_string(),
        "--working-memory-mib",
        &(limits.working_memory_bytes >> 20).to_string(),
        "--max-work-units",
        &limits.work_units.to_string(),
    ]);
    for input in &inputs {
        command
            .arg("--input")
            .arg(format!("{}=pin:{chip}/{}", input.role, input.role));
    }
    for name in &names {
        command.arg("--symbol").arg(name);
    }
    fs::create_dir_all(output)?;
    let partial = output.join("calls.json.partial");
    eprintln!(
        "signatures: every named call of {} pinned {chip} binaries",
        inputs.len()
    );
    if !command
        .stdout(fs::File::create(&partial)?)
        .status()?
        .success()
    {
        return Err(format!("Blobray call capture failed; see {}", partial.display()).into());
    }
    let path = output.join("calls.json");
    fs::rename(partial, &path)?;
    let document: Document = serde_json::from_slice(&fs::read(path)?)?;
    finish(document, inputs, &names, &defined)
}

fn finish(
    document: Document,
    inputs: Vec<Input>,
    names: &[String],
    defined: &BTreeSet<String>,
) -> Result<Corpus> {
    if document.schema != 7
        || document.analysis_resources.profile != AnalysisProfile::SignatureCorpus
        || document.inputs != inputs
        || document.symbols != names
        || document.abi.as_deref() != Some("riscv-integer")
        || !document.call_summaries
    {
        return Err(
            "call-arguments schema, pins, names or ABI differ from the requested full corpus"
                .into(),
        );
    }
    let mut calls = BTreeMap::new();
    for caller in &document.callers {
        for site in &caller.sites {
            *calls.entry(site.target.clone()).or_default() += 1;
        }
    }
    let without_definition = calls
        .keys()
        .filter(|name| !defined.contains(*name))
        .cloned()
        .collect();
    let named_sites = calls.values().sum();
    Ok(Corpus {
        analysis_resources: document.analysis_resources,
        inputs,
        partial: document.partial,
        partial_causes: document.partial_causes,
        blocked: document.blocked.len(),
        gaps: document.gaps,
        callers: document.callers.len(),
        named_sites,
        calls,
        without_definition,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_analysis_riscv_model::{
        AnalysisLimited, AnalysisUsage, FunctionLimits, LimitReason, LocalBudgetScope,
    };

    #[test]
    fn corpus_preserves_the_child_resource_document_without_recounting() {
        let mut function = FunctionLimits::default();
        let reason = LimitReason::LocalBudget(LocalBudgetScope::ArgumentSummary);
        function.event(reason);
        function.result(&[reason]);
        let mut limited = AnalysisLimited::default();
        limited.function(&function);
        let mut limits = AnalysisProfile::SignatureCorpus.limits();
        limits.work_units = 12345;
        let resources = AnalysisResources::new(
            AnalysisProfile::SignatureCorpus,
            limits,
            AnalysisUsage {
                work_units: 123,
                peak_admitted_bytes: 456,
            },
            limited,
        );
        let document = Document {
            analysis_resources: resources.clone(),
            schema: 7,
            inputs: vec![],
            abi: Some("riscv-integer".into()),
            call_summaries: true,
            symbols: vec![],
            callers: vec![],
            blocked: vec![],
            partial: 0,
            partial_causes: serde_json::json!({}),
            gaps: 0,
        };
        let corpus = finish(document, vec![], &[], &BTreeSet::new()).unwrap();
        assert_eq!(corpus.analysis_resources, resources);
        let json = serde_json::to_value(corpus).unwrap();
        assert_eq!(
            json["analysis_resources"],
            serde_json::to_value(resources).unwrap()
        );
    }
}
