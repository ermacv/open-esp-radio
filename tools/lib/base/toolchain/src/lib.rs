//! The host tools that repository builds run, located one way.
//!
//! Every tool has an override variable ([`Tool::variable`]); a non-empty
//! value names the program. Otherwise Cargo, `rustc` and `espflash` are
//! looked up on `PATH`, and the LLVM tools are the `llvm-tools` component of
//! the active Rust toolchain (`rust-toolchain.toml` pins it): its sysroot's
//! `lib/rustlib/<host>/bin`, never an unrelated LLVM on `PATH`, whose bitcode
//! and relocation support need not match the compiler's.
//!
//! [`versions`] records the tools an image build ran for its evidence, and
//! [`image`] configures the Cargo build of an image.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

pub mod image;
pub mod workspace;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A host tool that repository builds run.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Tool {
    Cargo,
    Rustc,
    Espflash,
    LlvmNm,
    LlvmObjcopy,
    LlvmObjdump,
}

impl Tool {
    /// Every tool, in the order evidence records them.
    pub const ALL: [Self; 6] = [
        Self::Rustc,
        Self::Cargo,
        Self::LlvmObjcopy,
        Self::LlvmNm,
        Self::LlvmObjdump,
        Self::Espflash,
    ];

    /// The tool's program name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Rustc => "rustc",
            Self::Espflash => "espflash",
            Self::LlvmNm => "llvm-nm",
            Self::LlvmObjcopy => "llvm-objcopy",
            Self::LlvmObjdump => "llvm-objdump",
        }
    }

    /// The variable that overrides the tool's program.
    pub fn variable(self) -> &'static str {
        match self {
            Self::Cargo => "CARGO",
            Self::Rustc => "RUSTC",
            Self::Espflash => "ESPFLASH",
            Self::LlvmNm => "LLVM_NM",
            Self::LlvmObjcopy => "LLVM_OBJCOPY",
            Self::LlvmObjdump => "LLVM_OBJDUMP",
        }
    }

    fn in_sysroot(self) -> bool {
        matches!(self, Self::LlvmNm | Self::LlvmObjcopy | Self::LlvmObjdump)
    }

    /// The arguments with which the tool prints its version.
    fn version_arguments(self) -> &'static [&'static str] {
        match self {
            Self::Rustc => &["-vV"],
            Self::Cargo => &["-Vv"],
            _ => &["--version"],
        }
    }
}

/// The program that runs `tool` in this process's environment.
pub fn program(tool: Tool) -> Result<OsString> {
    program_in(tool, |name| std::env::var_os(name), sysroot_bin)
}

/// Cargo, as [`program`] locates it: a non-empty `CARGO` (set when Cargo
/// runs the tool), else `cargo`.
pub fn cargo_program() -> OsString {
    program(Tool::Cargo).unwrap_or_else(|_| Tool::Cargo.name().into())
}

/// Cargo started in `directory`.
pub fn cargo_in(directory: &Path) -> Command {
    let mut command = oer_process::command(cargo_program());
    command.current_dir(directory);
    command
}

/// `tool` as a command, without arguments.
pub fn command(tool: Tool) -> Result<Command> {
    Ok(oer_process::command(program(tool)?))
}

/// [`program`] with `variable` reading the environment and `bin` giving the
/// directory of the toolchain's LLVM tools.
pub fn program_in(
    tool: Tool,
    variable: impl Fn(&str) -> Option<OsString>,
    bin: impl FnOnce(&OsStr) -> Result<PathBuf>,
) -> Result<OsString> {
    if let Some(program) = variable(tool.variable()).filter(|value| !value.is_empty()) {
        return Ok(program);
    }
    if !tool.in_sysroot() {
        return Ok(tool.name().into());
    }
    let rustc = variable(Tool::Rustc.variable())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| Tool::Rustc.name().into());
    let path = bin(&rustc)?.join(format!("{}{}", tool.name(), std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path.into_os_string())
    } else {
        Err(format!(
            "the active Rust toolchain lacks {}; install its llvm-tools component, or set {}",
            path.display(),
            tool.variable()
        )
        .into())
    }
}

/// The LLD the active Rust toolchain ships,
/// `<sysroot>/lib/rustlib/<host>/bin/gcc-ld/ld.lld`, for the host tools that
/// link RV32 images themselves: an unrelated LLD on `PATH`, such as
/// Ubuntu's LLD 18, need not link them.
pub fn lld() -> Result<PathBuf> {
    let rustc = program(Tool::Rustc)?;
    let path = sysroot_bin(&rustc)?
        .join("gcc-ld")
        .join(format!("ld.lld{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("the active Rust toolchain ships no {}", path.display()).into())
    }
}

/// `<sysroot>/lib/rustlib/<host>/bin` of `rustc`.
fn sysroot_bin(rustc: &OsStr) -> Result<PathBuf> {
    let text = |arguments: &[&str]| -> Result<String> {
        let output = oer_process::capture(oer_process::command(rustc).args(arguments))?;
        Ok(String::from_utf8(output.stdout)?)
    };
    let sysroot = text(&["--print", "sysroot"])?;
    Ok(Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(host_of(&text(&["-vV"])?)?)
        .join("bin"))
}

/// The host target triple `rustc -vV` printed in `verbose`.
fn host_of(verbose: &str) -> Result<String> {
    Ok(verbose
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc -vV names no host")?
        .to_owned())
}

/// The host target triple of the active Rust toolchain, for a build of a
/// workspace whose Cargo configuration defaults to a chip target.
pub fn host_target() -> Result<String> {
    let rustc = program(Tool::Rustc)?;
    let output = oer_process::capture(oer_process::command(rustc).arg("-vV"))?;
    host_of(&String::from_utf8(output.stdout)?)
}

/// Fails unless `tool` is present and runs.
pub fn require(tool: Tool) -> Result<()> {
    require_program(&program(tool)?)
}

/// Fails unless `program` runs with `--version`: a host program a workload
/// or fixture needs besides the toolchain.
pub fn require_program(program: &OsStr) -> Result<()> {
    let mut command = oer_process::command(program);
    command.arg("--version");
    match oer_process::output(&mut command, Some(std::time::Duration::from_secs(60))) {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err(format!(
            "required program `{}` is unavailable",
            Path::new(program).display()
        )
        .into()),
    }
}

/// A tool as a build ran it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolVersion {
    pub tool: Tool,
    /// The program that ran, or the tool's name when it cannot be located.
    pub program: String,
    /// What the program printed for its version; `None` when it did not run.
    pub version: Option<String>,
}

/// Every tool of [`Tool::ALL`] with the version it reports now. Call it when
/// a build runs, so the record names the tools that produced the image.
pub fn versions() -> Vec<ToolVersion> {
    Tool::ALL
        .into_iter()
        .map(|tool| match program(tool) {
            Ok(program) => ToolVersion {
                tool,
                version: version_of(&program, tool),
                program: program.to_string_lossy().into_owned(),
            },
            Err(_) => ToolVersion {
                tool,
                program: tool.name().to_owned(),
                version: None,
            },
        })
        .collect()
}

fn version_of(program: &OsStr, tool: Tool) -> Option<String> {
    let mut command = oer_process::command(program);
    command.args(tool.version_arguments());
    let output =
        oer_process::output(&mut command, Some(std::time::Duration::from_secs(60))).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests;
