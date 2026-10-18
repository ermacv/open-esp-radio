//! Compile both sides of a contract and check structured rustc diagnostics.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Output,
};

use oer_process::Checkout;
use serde::Deserialize;
use serde_json::Value;

use crate::Result;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    suite: Vec<Suite>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    name: String,
    packages: Vec<String>,
    features: Vec<String>,
    /// `host` or a chip profile, resolved through the repository model.
    targets: Vec<String>,
    cases: Vec<String>,
}

/// The target and feature set are identical for both sides of each case.
/// `ui_reject` selects the misuse; its expected error is marked on that line
/// with `// ui-error: E0123`. A deliberately uncoded error uses
/// `// ui-error: null exact diagnostic message`. Repeat the annotation when
/// rustc reports multiple errors at one boundary. An inline-const E0080 may
/// name the call through rustc's instantiation note. Every error must match once.
pub fn run(ctx: &Checkout) -> Result<()> {
    let catalog: Catalog = toml::from_str(&fs::read_to_string(
        ctx.root.join("tools/xtask/tests/ui.toml"),
    )?)?;
    for suite in catalog.suite {
        if suite.targets.is_empty() || suite.cases.is_empty() {
            return Err(format!("UI suite {} has no targets or cases", suite.name).into());
        }
        for target in &suite.targets {
            let triple = if target == "host" {
                oer_toolchain::host_target()?
            } else {
                super::common::rust_target(&ctx.root, target)?
            };
            let artifacts = dependencies(ctx, &suite, &triple)?;
            let output = tempfile::tempdir()?;
            for case in &suite.cases {
                let source = ctx.root.join(case);
                let expected = expectations(&fs::read_to_string(&source)?)?;
                for reject in [false, true] {
                    let result = compile(&source, output.path(), &triple, &artifacts, reject)?;
                    validate(&result, &source, reject.then_some(&expected)).map_err(|error| {
                        format!(
                            "{} / {target} / {case} / reject={reject}: {error}",
                            suite.name
                        )
                    })?;
                }
                println!("UI {} / {target}: {case}", suite.name);
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Libraries {
    externs: BTreeMap<String, String>,
    search: BTreeSet<PathBuf>,
}

/// Select artifacts emitted by this exact Cargo invocation, never by globbing
/// target/debug/deps where different feature sets and stale versions coexist.
fn dependencies(ctx: &Checkout, suite: &Suite, target: &str) -> Result<Libraries> {
    if suite.packages.is_empty() {
        return Ok(Libraries::default());
    }
    let mut command = oer_toolchain::cargo_in(&ctx.root);
    command.args([
        "build",
        "--locked",
        "--lib",
        "--no-default-features",
        "--message-format=json",
        "--target",
        target,
    ]);
    for package in &suite.packages {
        command.args(["-p", package]);
    }
    if !suite.features.is_empty() {
        command.arg("--features").arg(suite.features.join(","));
    }
    let output = oer_process::capture(&mut command)?;
    let mut artifacts = Libraries::default();
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: Value = serde_json::from_slice(line)?;
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let files = message["filenames"]
            .as_array()
            .ok_or("artifact filenames missing")?;
        for file in files.iter().filter_map(Value::as_str) {
            if let Some(parent) = Path::new(file).parent() {
                artifacts.search.insert(parent.to_owned());
            }
        }
        let Some(name) = message["target"]["name"].as_str() else {
            continue;
        };
        if !suite
            .packages
            .iter()
            .any(|package| package.replace('-', "_") == name)
        {
            continue;
        }
        let library = files
            .iter()
            .filter_map(Value::as_str)
            .find(|file| file.ends_with(".rlib"))
            .ok_or_else(|| format!("no library artifact for {name}"))?;
        if artifacts
            .externs
            .insert(name.to_owned(), library.to_owned())
            .is_some()
        {
            return Err(format!("ambiguous library artifact for {name}").into());
        }
    }
    if artifacts.externs.len() != suite.packages.len() {
        return Err(format!(
            "UI suite {} did not build every declared dependency",
            suite.name
        )
        .into());
    }
    Ok(artifacts)
}

fn compile(
    source: &Path,
    output: &Path,
    target: &str,
    artifacts: &Libraries,
    reject: bool,
) -> Result<Output> {
    let mut command = oer_toolchain::command(oer_toolchain::Tool::Rustc)?;
    command
        .arg(source)
        .args([
            "--crate-name",
            "contract",
            "--crate-type=lib",
            "--edition=2024",
            "--error-format=json",
            "--emit=link",
            "--target",
            target,
            "--out-dir",
        ])
        .arg(output);
    if reject {
        command.args(["--cfg", "ui_reject"]);
    }
    for (name, artifact) in &artifacts.externs {
        command.arg("--extern").arg(format!("{name}={artifact}"));
    }
    for directory in &artifacts.search {
        command
            .arg("-L")
            .arg(format!("dependency={}", directory.display()));
    }
    oer_process::output(&mut command, None)
}

#[derive(Clone, Debug)]
struct ExpectedError {
    line: usize,
    code: Option<String>,
    message: Option<String>,
}

type Expected = Vec<ExpectedError>;

fn expectations(source: &str) -> Result<Expected> {
    let mut expected = Vec::new();
    for (index, line) in source.lines().enumerate() {
        for annotation in line.split("// ui-error:").skip(1).map(str::trim) {
            let (code, message) = if let Some(message) = annotation.strip_prefix("null ") {
                if message.trim().is_empty() {
                    return Err("an uncoded UI error requires its exact message".into());
                }
                (None, Some(message.trim().to_owned()))
            } else if annotation.len() == 5
                && annotation.starts_with('E')
                && annotation[1..].bytes().all(|b| b.is_ascii_digit())
            {
                (Some(annotation.to_owned()), None)
            } else {
                return Err(format!(
                    "invalid UI error annotation on line {}: {annotation}",
                    index + 1
                )
                .into());
            };
            expected.push(ExpectedError {
                line: index + 1,
                code,
                message,
            });
        }
    }
    if expected.is_empty() {
        return Err("UI case has no intended error annotation".into());
    }
    Ok(expected)
}

fn validate(output: &Output, source: &Path, expected: Option<&Expected>) -> Result<()> {
    let reject = expected.is_some();
    if output.status.success() == reject {
        return Err(format!(
            "unexpected compiler status {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut remaining = expected.cloned().unwrap_or_default();
    let diagnostics: Vec<Value> = output
        .stderr
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<std::result::Result<_, _>>()?;
    for (index, message) in diagnostics.iter().enumerate() {
        if message["level"] != "error" {
            continue;
        }
        let spans = message["spans"]
            .as_array()
            .ok_or("diagnostic spans missing")?;
        // rustc's final summary is not another diagnostic. Every other error,
        // including an uncoded lifetime error, requires an exact expectation.
        if spans.is_empty()
            && message["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("aborting due to "))
        {
            continue;
        }
        let code = message["code"]["code"].as_str();
        // An inline const in a dependency reports E0080 at its definition.
        // Rustc then names this exact instantiation in a separate note before
        // the next error. Only that note can move a const diagnostic's boundary.
        let instantiated = diagnostics[index + 1..]
            .iter()
            .take_while(|note| note["level"] != "error")
            .filter(|note| {
                code == Some("E0080")
                    && note["level"] == "note"
                    && note["message"].as_str().is_some_and(|text| {
                        text.starts_with("the above error was encountered while instantiating `")
                    })
            })
            .filter_map(|note| note["spans"].as_array())
            .flatten();
        let matching = spans
            .iter()
            .chain(instantiated)
            .filter(|span| span["is_primary"] == true)
            .find_map(|span| {
                if span["file_name"].as_str().map(Path::new) != Some(source) {
                    return None;
                }
                let start = usize::try_from(span["line_start"].as_u64()?).ok()?;
                let end = usize::try_from(span["line_end"].as_u64()?).ok()?;
                remaining.iter().position(|expected| {
                    (start..=end).contains(&expected.line)
                        && expected.code.as_deref() == code
                        && expected
                            .message
                            .as_deref()
                            .is_none_or(|text| message["message"].as_str() == Some(text))
                })
            });
        let Some(line) = matching else {
            return Err(format!("unintended compiler error: {}", message["rendered"]).into());
        };
        remaining.remove(line);
    }
    if !remaining.is_empty() {
        return Err(format!("missing intended diagnostics: {remaining:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
