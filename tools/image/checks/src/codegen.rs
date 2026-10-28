//! Code-generation conditions required by the DWARF Waker-slot proof.
//!
//! Check the assembled rustc flags and the expanded linker arguments, not
//! just the flags the image builder adds. Conflicting options are errors
//! even when a later option would override them.

use std::{ffi::OsString, fmt};

/// A build option that invalidates the source-scope identity used by the
/// stack proof, or a missing option that must establish it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodegenPrecondition {
    MergeFunctions(String),
    MachineOutliner(String),
    PickedSourceLocations(String),
    IdenticalCodeFolding(String),
    MissingFlag(&'static str),
}

impl fmt::Display for CodegenPrecondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (option, reason) = match self {
            Self::MergeFunctions(option) => (
                option.as_str(),
                "MergeFunctions must be disabled; merged functions can lose their DWARF origin",
            ),
            Self::MachineOutliner(option) => (
                option.as_str(),
                "machine outlining must be disabled; outlined code has no original source scope",
            ),
            Self::PickedSourceLocations(option) => (
                option.as_str(),
                "pick-merged-source-locations must not be set; choosing one origin can label a drop call as wake",
            ),
            Self::IdenticalCodeFolding(option) => (
                option.as_str(),
                "LLD --icf must not be passed; folded symbols share code with different source scopes",
            ),
            Self::MissingFlag(flag) => return write!(f, "firmware stack proof requires {flag}"),
        };
        write!(f, "firmware stack proof rejects `{option}`: {reason}")
    }
}

impl std::error::Error for CodegenPrecondition {}

type Result<T> = std::result::Result<T, CodegenPrecondition>;

fn option(text: &str) -> (&str, Option<&str>) {
    let text = text.trim_start_matches('-');
    match text.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (text, None),
    }
}

fn llvm_options(text: &str) -> Result<bool> {
    let mut outliner_disabled = false;
    for argument in text.split_whitespace() {
        let (name, value) = option(argument);
        match name {
            "enable-machine-outliner" if value == Some("never") => outliner_disabled = true,
            "enable-machine-outliner" => {
                return Err(CodegenPrecondition::MachineOutliner(argument.into()));
            }
            "pick-merged-source-locations" => {
                return Err(CodegenPrecondition::PickedSourceLocations(argument.into()));
            }
            _ => {}
        }
    }
    Ok(outliner_disabled)
}

fn passes(text: &str) -> Result<()> {
    for pass in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
        match pass {
            "mergefunc" => return Err(CodegenPrecondition::MergeFunctions(text.into())),
            "machine-outliner" => return Err(CodegenPrecondition::MachineOutliner(text.into())),
            _ => {}
        }
    }
    Ok(())
}

/// Validate final rustc arguments, including flags inherited from either
/// Cargo rustflags environment variable. Required flags must be present.
pub fn rustflags(arguments: &[String]) -> Result<()> {
    let mut merge_disabled = false;
    let mut outliner_disabled = false;
    let mut arguments = arguments.iter().map(String::as_str);
    while let Some(argument) = arguments.next() {
        let (kind, setting) = if argument == "-C" || argument == "--codegen" {
            ("codegen", arguments.next())
        } else if argument == "-Z" {
            ("unstable", arguments.next())
        } else if let Some(setting) = argument
            .strip_prefix("-C")
            .or_else(|| argument.strip_prefix("--codegen="))
        {
            ("codegen", Some(setting))
        } else if let Some(setting) = argument.strip_prefix("-Z") {
            ("unstable", Some(setting))
        } else {
            continue;
        };
        let Some(setting) = setting else { continue };
        let (name, value) = option(setting);
        // rustc accepts underscores and hyphens in -C/-Z option names.
        let name = name.replace('_', "-");
        match (kind, name.as_str(), value) {
            ("unstable", "merge-functions", Some("disabled")) => merge_disabled = true,
            ("unstable", "merge-functions", _) => {
                return Err(CodegenPrecondition::MergeFunctions(setting.into()));
            }
            ("codegen", "llvm-args", Some(value)) => outliner_disabled |= llvm_options(value)?,
            ("codegen", "passes", Some(value)) => passes(value)?,
            ("codegen", "link-arg", Some(value)) => linker_arguments(&[value.into()])?,
            ("codegen", "link-args", Some(value)) => linker_arguments(
                &value
                    .split_whitespace()
                    .map(OsString::from)
                    .collect::<Vec<_>>(),
            )?,
            _ => {}
        }
    }
    if !merge_disabled {
        return Err(CodegenPrecondition::MissingFlag(
            "-Zmerge-functions=disabled",
        ));
    }
    if !outliner_disabled {
        return Err(CodegenPrecondition::MissingFlag(
            "-Cllvm-args=-enable-machine-outliner=never",
        ));
    }
    Ok(())
}

/// Validate the actual LLD arguments after expanding response files. This
/// also covers options emitted by dependency build scripts and LTO options.
pub fn linker_arguments(arguments: &[OsString]) -> Result<()> {
    let mut arguments = arguments.iter().filter_map(|argument| argument.to_str());
    while let Some(argument) = arguments.next() {
        if !argument.starts_with('-') {
            continue;
        }
        let (name, value) = option(argument);
        match name {
            "icf" => return Err(CodegenPrecondition::IdenticalCodeFolding(argument.into())),
            "mllvm" | "plugin-opt" => {
                if let Some(value) = value.or_else(|| arguments.next()) {
                    llvm_options(value)?;
                }
            }
            "lto-newpm-passes" => {
                if let Some(value) = value.or_else(|| arguments.next()) {
                    passes(value)?;
                }
            }
            _ => {
                llvm_options(argument)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(extra: &str) -> Vec<String> {
        format!("-Zmerge-functions=disabled -Cllvm-args=-enable-machine-outliner=never {extra}")
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn pinned_flags_allow_riscv_features_and_size_optimization() {
        for extra in [
            "",
            "-C target-feature=+m,+a,+c,+f -Copt-level=z",
            "--codegen=debuginfo=2 -Cllvm-args=--enable-machine-outliner=never",
        ] {
            assert_eq!(rustflags(&flags(extra)), Ok(()));
        }
    }

    #[test]
    fn flags_must_actually_disable_both_function_transformations() {
        assert_eq!(
            rustflags(&[]),
            Err(CodegenPrecondition::MissingFlag(
                "-Zmerge-functions=disabled"
            ))
        );
        assert_eq!(
            rustflags(&["-Zmerge-functions=disabled".into()]),
            Err(CodegenPrecondition::MissingFlag(
                "-Cllvm-args=-enable-machine-outliner=never"
            ))
        );
    }

    #[test]
    fn unsafe_codegen_options_fail_even_before_a_disabling_override() {
        for (extra, expected) in [
            (
                "-Z merge-functions=aliases",
                CodegenPrecondition::MergeFunctions("merge-functions=aliases".into()),
            ),
            (
                "-Zmerge_functions=aliases",
                CodegenPrecondition::MergeFunctions("merge_functions=aliases".into()),
            ),
            (
                "-Cllvm_args=-enable-machine-outliner=always",
                CodegenPrecondition::MachineOutliner("-enable-machine-outliner=always".into()),
            ),
            (
                "-Zmerge-functions=trampolines",
                CodegenPrecondition::MergeFunctions("merge-functions=trampolines".into()),
            ),
            (
                "-C llvm-args=-enable-machine-outliner",
                CodegenPrecondition::MachineOutliner("-enable-machine-outliner".into()),
            ),
            (
                "-Cllvm-args=--enable-machine-outliner=always",
                CodegenPrecondition::MachineOutliner("--enable-machine-outliner=always".into()),
            ),
            (
                "--codegen=llvm-args=-pick-merged-source-locations",
                CodegenPrecondition::PickedSourceLocations("-pick-merged-source-locations".into()),
            ),
            (
                "-C llvm-args=--pick-merged-source-locations=false",
                CodegenPrecondition::PickedSourceLocations(
                    "--pick-merged-source-locations=false".into(),
                ),
            ),
            (
                "-C passes=mergefunc",
                CodegenPrecondition::MergeFunctions("mergefunc".into()),
            ),
            (
                "-Clink-arg=--icf=all",
                CodegenPrecondition::IdenticalCodeFolding("--icf=all".into()),
            ),
            (
                "-Clink-args=--icf=none",
                CodegenPrecondition::IdenticalCodeFolding("--icf=none".into()),
            ),
        ] {
            assert_eq!(rustflags(&flags(extra)), Err(expected.clone()), "{extra}");
            let mut before: Vec<String> = extra.split_whitespace().map(str::to_owned).collect();
            before.extend(flags(""));
            assert_eq!(rustflags(&before), Err(expected), "{extra}");
        }
    }

    #[test]
    fn final_linker_options_reject_folding_and_llvm_overrides() {
        for text in [
            "--icf=all",
            "--icf safe",
            "-icf=none",
            "-mllvm -enable-machine-outliner=always",
            "--mllvm=-pick-merged-source-locations",
            "--lto-newpm-passes=module(mergefunc)",
            "--lto-newpm-passes module(mergefunc)",
            "-mllvm=-enable-machine-outliner=always",
            "--plugin-opt=-enable-machine-outliner=always",
            "-plugin-opt=-pick-merged-source-locations",
        ] {
            let arguments = text
                .split_whitespace()
                .map(OsString::from)
                .collect::<Vec<_>>();
            let error = linker_arguments(&arguments).unwrap_err();
            assert!(error.to_string().contains("firmware stack proof"));
        }
        assert_eq!(
            linker_arguments(&[
                "--gc-sections".into(),
                "-mllvm=-enable-machine-outliner=never".into()
            ]),
            Ok(())
        );
    }
}
