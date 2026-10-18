//! Build the typed vendor scenarios, then run one scenario. Blobray runs
//! inside the scenario process.
//!
//! Private inputs stay explicit arguments of the scenario.
use crate::Result;
use oer_tool_base_process as process;
use oer_tool_base_process::Checkout;
use std::ffi::OsString;
use std::path::PathBuf;

/// The engine library behind every chip's verdicts; with the chip's
/// scenario library it decides what a shard records.
const ENGINE_PACKAGE: &str = "oer-tool-service-vendor-scenario-engine";
/// Cargo's machine-readable build messages, with human diagnostics kept.
const MESSAGE_FORMAT: &str = "--message-format=json-render-diagnostics";
/// The scenario binary's argument naming one verdict library's dep-info.
const VERDICT_DEP_INFO: &str = "--verdict-dep-info";

pub fn run(ctx: &Checkout, chip: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let chip = oer_tool_data_vendor_artifacts::project::Project::new(&ctx.root, chip)?;
    let scenarios = chip.scenarios();
    if !ctx
        .root
        .join(chip.verification("scenarios/Cargo.toml"))
        .is_file()
    {
        return Err(format!("no vendor scenarios for chip {}", chip.name()).into());
    }
    // The verdict library decides the verdicts; the entry package's binary
    // of the same name runs the scenario.
    let (library, package, binary) = (
        scenarios.library.as_str(),
        scenarios.entry.as_str(),
        scenarios.entry.as_str(),
    );
    let Some((scenario, rest)) = args.split_first() else {
        return Err("select a scenario, for example `gain`".into());
    };
    // Cargo keeps a `.d` only beside a root unit's output, so the verdict
    // libraries are built as roots first; the binary links the same units.
    let verdict = super::phase::timed("build vendor scenarios", || -> Result<Vec<PathBuf>> {
        let libraries = oer_tool_base_toolchain::workspace::VERIFICATION
            .cargo(&ctx.root, "build")
            .args([
                "--profile",
                oer_tool_base_toolchain::workspace::VERIFICATION.profile,
                "-p",
                ENGINE_PACKAGE,
                "-p",
                library,
                "--lib",
                MESSAGE_FORMAT,
            ])
            .stderr(std::process::Stdio::inherit())
            .output()?;
        if !libraries.status.success() {
            return Err(format!("building {ENGINE_PACKAGE} and {library} failed").into());
        }
        let verdict = verdict_dep_info(&libraries.stdout, &[ENGINE_PACKAGE, library])?;
        process::run(
            oer_tool_base_toolchain::workspace::VERIFICATION
                .cargo(&ctx.root, "build")
                .args([
                    "--profile",
                    oer_tool_base_toolchain::workspace::VERIFICATION.profile,
                    "-p",
                    package,
                    "--bin",
                    binary,
                ]),
        )?;
        Ok(verdict)
    })?;
    let mut command =
        ctx.command(oer_tool_base_toolchain::workspace::VERIFICATION.binary(&ctx.root, binary));
    command.arg(scenario).args(rest);
    for file in verdict {
        command.arg(VERDICT_DEP_INFO).arg(file);
    }
    let label = format!("scenario {}", scenario.to_string_lossy());
    super::phase::timed(&label, || {
        let mut child = oer_tool_base_process::owned::Child::spawn(&mut command)?;
        Ok(oer_tool_base_process::exit_code(
            child.wait_forwarding_cancellation()?,
        ))
    })
}

/// The dep-info files of the library targets of `packages` in Cargo's JSON
/// build `messages`: each sits beside the `.rlib` a root build writes.
fn verdict_dep_info(messages: &[u8], packages: &[&str]) -> Result<Vec<PathBuf>> {
    let mut files = vec![];
    for package in packages {
        let library = package.replace('-', "_");
        let rlib = messages
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
            .filter(|message| {
                message["reason"] == "compiler-artifact"
                    && message["target"]["name"] == library.as_str()
                    && message["target"]["kind"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
            })
            .flat_map(|message| {
                message["filenames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|name| name.as_str().map(PathBuf::from))
                    .collect::<Vec<_>>()
            })
            .find(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "rlib")
            })
            .ok_or_else(|| format!("the build reported no library of {package}"))?;
        files.push(rlib.with_extension("d"));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_dep_info_sits_beside_each_library() {
        let messages = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"oer_tool_service_vendor_scenario_engine","kind":["lib"]},"filenames":["/t/deps/liboer_vendor_scenario_engine-1.rlib","/t/deps/liboer_vendor_scenario_engine-1.rmeta"]}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"oer_tool_service_vendor_scenario_report","kind":["lib"]},"filenames":["/t/deps/liboer_vendor_scenario_report-2.rlib"]}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
        );
        assert_eq!(
            verdict_dep_info(
                messages.as_bytes(),
                &["oer-tool-service-vendor-scenario-engine"]
            )
            .unwrap(),
            [PathBuf::from("/t/deps/liboer_vendor_scenario_engine-1.d")]
        );
        assert!(verdict_dep_info(messages.as_bytes(), &["oer-absent"]).is_err());
    }
}
