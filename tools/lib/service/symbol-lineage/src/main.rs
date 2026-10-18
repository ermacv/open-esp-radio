//! Command-line entry for vendor symbol lineage.

#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use oer_symbol_lineage::{
    archive::{Error, Revision, read_archive},
    correspond::{Evidence, Policy},
    lineage::{Lineage, NameClass, trace},
};

/// Carry source function names from the earliest archive revision to the
/// last one.
///
/// Revisions come either from the history of one library in a Git repository
/// or from an ordered list of archive files.
#[derive(Parser)]
struct Arguments {
    /// Vendor Git repository holding the library.
    #[arg(long, requires = "library", conflicts_with = "archive")]
    repo: Option<PathBuf>,
    /// Library path inside the repository, such as `libble_app.a`.
    #[arg(long)]
    library: Option<String>,
    /// Revisions to use, oldest first. Defaults to every first-parent commit
    /// that changes the library.
    #[arg(long = "revision")]
    revisions: Vec<String>,
    /// Archive files, oldest first, when no repository is given.
    #[arg(long = "archive")]
    archive: Vec<PathBuf>,
    /// JSON lineage report.
    #[arg(long)]
    out: PathBuf,
    /// Optional `llvm-objcopy --redefine-syms` file for the last revision.
    #[arg(long)]
    redefine_syms: Option<PathBuf>,
    /// Optional compact TOML name map for the last revision.
    #[arg(long)]
    names: Option<PathBuf>,
    /// Library identity recorded in the name map, such as a repository URL.
    #[arg(long, default_value = "")]
    source: String,
    /// Treat exactly the names with this prefix as generated, instead of the
    /// token heuristic. Repeatable.
    #[arg(long = "obfuscated-prefix")]
    obfuscated_prefixes: Vec<String>,
    /// Minimum body similarity in parts per million.
    #[arg(long, default_value_t = Policy::default().minimum_ppm)]
    minimum_similarity_ppm: u32,
    /// Minimum body similarity of a call-graph pair in parts per million.
    #[arg(long, default_value_t = Policy::default().call_graph_minimum_ppm)]
    call_graph_minimum_ppm: u32,
    /// Required lead over the next candidate in parts per million.
    #[arg(long, default_value_t = Policy::default().margin_ppm)]
    similarity_margin_ppm: u32,
    /// Minimum similarity of a dominant mutual best in parts per million.
    #[arg(long, default_value_t = Policy::default().dominant_minimum_ppm)]
    dominant_minimum_ppm: u32,
    /// How many times a dominant mutual best must exceed its runner up.
    #[arg(long, default_value_t = Policy::default().dominance_ratio)]
    dominance_ratio: u32,
    /// Minimum similarity of a neighbourhood pair in parts per million.
    #[arg(long, default_value_t = Policy::default().neighbourhood_minimum_ppm)]
    neighbourhood_minimum_ppm: u32,
}

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Error> {
    let revisions = match (&arguments.repo, &arguments.library) {
        (Some(repo), Some(library)) => from_git(repo, library, &arguments.revisions)?,
        _ if !arguments.archive.is_empty() => arguments
            .archive
            .iter()
            .map(|path| read_archive(&path.display().to_string(), &fs::read(path)?))
            .collect::<Result<_, _>>()?,
        _ => return Err("select --repo with --library, or at least one --archive".into()),
    };
    let policy = Policy {
        minimum_ppm: arguments.minimum_similarity_ppm,
        margin_ppm: arguments.similarity_margin_ppm,
        call_graph_minimum_ppm: arguments.call_graph_minimum_ppm,
        dominant_minimum_ppm: arguments.dominant_minimum_ppm,
        dominance_ratio: arguments.dominance_ratio,
        neighbourhood_minimum_ppm: arguments.neighbourhood_minimum_ppm,
    };
    let class = if arguments.obfuscated_prefixes.is_empty() {
        NameClass::Heuristic
    } else {
        NameClass::Prefixes(arguments.obfuscated_prefixes.clone())
    };
    let lineage = trace(&revisions, policy, &class);
    fs::write(&arguments.out, serde_json::to_vec_pretty(&lineage)?)?;
    if let Some(path) = &arguments.redefine_syms {
        fs::write(path, redefine_syms(&lineage))?;
    }
    if let Some(path) = &arguments.names {
        let library = arguments.library.as_deref().unwrap_or_default();
        fs::write(path, name_map(&lineage, &arguments.source, library))?;
    }
    report(&lineage);
    Ok(())
}

fn from_git(repo: &Path, library: &str, selected: &[String]) -> Result<Vec<Revision>, Error> {
    let revisions = if selected.is_empty() {
        let log = oer_process::git::output(
            repo,
            [
                "log",
                "--first-parent",
                "--reverse",
                "--format=%H",
                "--",
                library,
            ],
        )?;
        String::from_utf8(log)?
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    } else {
        selected.to_vec()
    };
    let mut archives: Vec<Revision> = Vec::new();
    for revision in revisions {
        let commit = oer_process::git::output(
            repo,
            ["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
        )?;
        let revision = String::from_utf8(commit)?.trim().to_owned();
        let bytes = oer_process::git::output(repo, ["show", &format!("{revision}:{library}")])?;
        let archive = read_archive(&revision, &bytes)?;
        // A commit that does not change the bytes adds no evidence.
        if archives
            .last()
            .is_some_and(|last| last.sha256 == archive.sha256)
        {
            continue;
        }
        archives.push(archive);
    }
    Ok(archives)
}

fn redefine_syms(lineage: &Lineage) -> String {
    lineage
        .functions
        .iter()
        .filter_map(|function| {
            let source = function.source_name.as_ref()?;
            (source != &function.name).then(|| format!("{} {source}\n", function.name))
        })
        .collect()
}

/// One entry per recovered generated name, with the weakest evidence of its
/// chain. Names contain only symbol characters and need no TOML escaping.
fn name_map(lineage: &Lineage, source: &str, library: &str) -> String {
    let mut out = format!(
        "# Generated by oer-symbol-lineage. Evidence grades each chain by its weakest step.\n\
         schema = 1\nsource = {source:?}\nlibrary = {library:?}\n"
    );
    for revision in &lineage.revisions {
        out.push_str(&format!(
            "\n[[revisions]]\nlabel = {:?}\nsha256 = {:?}\nfunctions = {}\n",
            revision.label, revision.sha256, revision.functions
        ));
    }
    out.push_str("\n[names]\n");
    for function in &lineage.functions {
        let Some(name) = function
            .source_name
            .as_ref()
            .filter(|name| **name != function.name)
        else {
            continue;
        };
        let weakest = function
            .steps
            .iter()
            .map(|step| step.evidence)
            .max_by_key(|evidence| strength_rank(*evidence))
            .map_or("none", evidence_name);
        let similarity = function
            .steps
            .iter()
            .map(|step| step.similarity_ppm)
            .min()
            .unwrap_or(1_000_000);
        out.push_str(&format!(
            "{:?} = {{ name = {name:?}, origin = {:?}, weakest = {weakest:?}, similarity-ppm = {similarity} }}\n",
            function.name,
            function.origin.as_deref().unwrap_or_default(),
        ));
    }
    out
}

/// Higher is weaker.
fn strength_rank(evidence: Evidence) -> u8 {
    match evidence {
        Evidence::SameName => 0,
        Evidence::ExactBody => 1,
        Evidence::CallGraph => 2,
        Evidence::Similar { .. } => 3,
        Evidence::Neighbourhood { .. } => 4,
        Evidence::Dominant { .. } => 5,
    }
}

fn evidence_name(evidence: Evidence) -> &'static str {
    match evidence {
        Evidence::SameName => "same-name",
        Evidence::ExactBody => "exact-body",
        Evidence::CallGraph => "call-graph",
        Evidence::Similar { .. } => "similar",
        Evidence::Dominant { .. } => "dominant",
        Evidence::Neighbourhood { .. } => "neighbourhood",
    }
}

fn report(lineage: &Lineage) {
    for step in &lineage.steps {
        println!(
            "STEP {} -> {}: same-name={} exact-body={} call-graph={} similar={} dominant={} neighbourhood={} changed={} dissimilar={} unpaired={}/{}",
            step.from,
            step.to,
            step.same_name,
            step.exact_body,
            step.call_graph,
            step.similar,
            step.dominant,
            step.neighbourhood,
            step.changed_bodies,
            step.dissimilar,
            step.unpaired_left,
            step.unpaired_right
        );
    }
    let total = lineage.functions.len();
    let named = lineage
        .functions
        .iter()
        .filter(|function| function.source_name.is_some())
        .count();
    let recovered = lineage
        .functions
        .iter()
        .filter(|function| function.source_name.as_ref() != Some(&function.name))
        .filter(|function| function.source_name.is_some())
        .count();
    println!(
        "FUNCTIONS total={total} named={named} recovered={recovered} unknown={}",
        total - named
    );
}
