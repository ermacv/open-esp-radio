//! The compiler setup every image build shares: the single owner of the
//! image compiler flags. No `.cargo/config.toml` carries Rust flags for an
//! image; `cargo fw build` applies [`configure`].
//!
//! `RUSTC_BOOTSTRAP=1` is set here, on the image's Cargo command only, and
//! exists for the unstable `-Z` flags alone (`emit-stack-sizes`,
//! `move-size-limit`, `share-generics`, `merge-functions`) on the exact
//! stable toolchain pinned by `rust-toolchain.toml`.
//!
//! Every image links through the image linker (`oer-image-linker` of `tools/image/linker`), which
//! refuses an input section named as zero-initialized that carries bytes and
//! otherwise runs the target's `rust-lld` with the same arguments. The linker
//! is built from the sources being built, the checkout or worktree at
//! [`ImageCompiler::root`], so an image never links with another tree's
//! linker.

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Result, instrumentation::Instrumentation};
use oer_os_toolchain::Tool;

/// The image compiler setup of an image under `policy` with the classes of
/// `instrumentation`, built from the tree at `root` with the image linker's
/// build in `linker_target`.
pub fn of_policy<'a>(
    policy: &oer_chip_profile::policy::StackPolicy,
    instrumentation: &Instrumentation,
    root: &'a Path,
    linker_target: &'a Path,
    rust_target: &'a str,
) -> ImageCompiler<'a> {
    ImageCompiler {
        root,
        linker_target,
        rust_target,
        max_move_bytes: policy.max_move_bytes,
        runtime_environment: policy.runtime_environment(),
        instrumentation: instrumentation.clone(),
    }
}

/// The input section patterns (`name` or `name.*`, comma separated) the
/// image linker refuses initialized bytes in: the regions the boot zeroes.
/// Unset or empty, it checks none.
pub const ZEROED_INPUTS_ENV: &str = "OER_IMAGE_ZEROED_INPUTS";

/// Names the host build root instead of the user's cache directory.
pub const BUILD_ROOT_ENV: &str = "OER_BUILD_ROOT";

/// The host's directory of the image build slots: the one place every
/// checkout of this host shares, since the slots bound the host's load.
/// Nothing compiled lives here.
pub fn host_build_root() -> Result<PathBuf> {
    oer_os_durable::xdg::overridable(BUILD_ROOT_ENV, oer_os_durable::xdg::Base::Cache, "build")
        .map_err(|error| error.to_string().into())
}

/// The firmware compile cache (`CARGO_TARGET_DIR`) of the checkout at
/// `root` for images built with `instrumentation`, inside its own
/// `target/fw/`: every image class and chip of that checkout. Each set of
/// instrumentation classes has a cache of its own, so a plain image's cache
/// never compiles an instrumentation cfg. A cache is never shared between checkouts: Cargo decides that a
/// path package's unit is fresh by comparing modification times, so a unit
/// another checkout compiled from other sources of the same package would
/// pass for this one's. Builds uplift their binaries to the same
/// `<target>/firmware/<name>`, so each holds [`lock_compile_cache`] from its
/// Cargo run until its outputs are copied out.
pub fn compile_cache(root: &Path, instrumentation: &Instrumentation) -> PathBuf {
    root.join("target/fw").join(instrumentation.cache_name())
}

/// The target directory of the image linker of the checkout at `root`: its
/// path is part of every image's Rust flags, so it does not differ between
/// that checkout's builds.
pub fn linker_target(root: &Path) -> PathBuf {
    root.join("target/fw/image-linker")
}

/// Start the compile cache and image linker of the checkout at `to` from a
/// copy of those of `from`, cheaply where the file system can share the
/// blocks (`cp -a --reflink=auto`). A new worktree's first build then
/// recompiles only what its sources change: Cargo rebuilds a path package
/// whose sources are newer than its unit, and reuses the units of registry
/// and git dependencies. Nothing to copy is no error.
pub fn seed_build_cache(from: &Path, to: &Path) -> Result<()> {
    let source = from.join("target/fw");
    if !source.is_dir() {
        return Ok(());
    }
    let target = to.join("target");
    std::fs::create_dir_all(&target)?;
    if target.join("fw").exists() {
        return Err(format!("{} already has a build cache", to.display()).into());
    }
    oer_os_process::run(
        oer_os_process::command("cp")
            .args(["-a", "--reflink=auto"])
            .arg(&source)
            .arg(target.join("fw")),
    )
    .map_err(|error| format!("seeding the build cache of {}: {error}", to.display()).into())
}

/// The lock of the compile cache `cache`: one build at a time, the next
/// waiting for it.
pub fn lock_compile_cache(cache: &Path) -> Result<oer_os_process::lock::FileLock> {
    oer_os_process::lock::FileLock::wait(
        &cache.join("build.lock"),
        oer_os_process::lock::Mode::Exclusive,
        &format!("waiting for another build in {}", cache.display()),
    )
    .map_err(|error| error.to_string().into())
}

/// What an image's compiler setup depends on.
#[derive(Clone, Debug)]
pub struct ImageCompiler<'a> {
    /// The tree being built: its `oer-image-checks` builds the linker
    /// (`oer-image-linker`).
    pub root: &'a Path,
    /// The Cargo target directory of the linker's host build.
    pub linker_target: &'a Path,
    /// The image's Rust target triple.
    pub rust_target: &'a str,
    /// The move size above which the compiler warns (`-Z move-size-limit`).
    pub max_move_bytes: u64,
    /// Variables the image's runtime reads at compile time (its stack
    /// policy's reserves).
    pub runtime_environment: Vec<(String, String)>,
    /// The instrumentation classes whose cfgs the image is compiled under.
    pub instrumentation: Instrumentation,
}

/// Configure `command`, a Cargo build of an image, with the compiler flags
/// every image shares, building the image linker first.
pub fn configure(command: &mut Command, compiler: &ImageCompiler<'_>) -> Result<()> {
    let linker = build_linker(compiler.root, compiler.linker_target)?;
    crate::large_values::configure(
        command,
        compiler.root,
        &compiler.root.join("target/fw/large-values-driver"),
        compiler.rust_target,
        compiler.max_move_bytes,
    )?;
    for (key, _) in env::vars_os() {
        if key.to_str().is_some_and(|key| {
            key.starts_with("OPEN_RADIO_TASK_STACK_HART")
                || key.starts_with("OPEN_RADIO_IRQ_STACK_HART")
        }) {
            command.env_remove(key);
        }
    }
    for (variable, value) in &compiler.runtime_environment {
        command.env(variable, value);
    }
    // C and C++ that build scripts compile for the image (through the `cc`
    // and `cmake` crates) emit the same `.stack_sizes` section, so the stack
    // gate has their frames as well.
    for variable in c_flag_variables(compiler.rust_target) {
        let flags = with_stack_sizes(env::var(&variable).ok());
        command.env(variable, flags);
    }
    // The pinned stable toolchain supports these flags, but they remain
    // unstable: RUSTC_BOOTSTRAP enables them for this command alone. The
    // stack-size ELF section is consumed by a safe host-side parser.
    configure_flags(
        command,
        compiler.max_move_bytes,
        &linker,
        &compiler.instrumentation,
    )?;

    Ok(())
}

/// Build the image linker of the tree at `root` for the host into
/// `target_dir` and return its path. The build leaves the image's compiler
/// environment behind: it is a host tool.
pub fn build_linker(root: &Path, target_dir: &Path) -> Result<PathBuf> {
    build_host_tool(
        root,
        target_dir,
        "oer-tool-image-linker",
        "oer-image-linker",
    )
}

pub(crate) fn build_host_tool(
    root: &Path,
    target_dir: &Path,
    package: &str,
    binary: &str,
) -> Result<PathBuf> {
    let mut command = oer_os_toolchain::command(Tool::Cargo)?;
    command
        .current_dir(root)
        .args(["build", "--quiet", "-p", package, "--bin", binary])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir);
    for variable in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "RUSTC_BOOTSTRAP",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        command.env_remove(variable);
    }
    oer_os_process::run(&mut command)
        .map_err(|error| format!("building {binary} of {} failed: {error}", root.display()))?;
    Ok(target_dir.join("debug").join(binary))
}

/// The per-target C and C++ flag variables the `cc` and `cmake` crates read.
fn c_flag_variables(target: &str) -> [String; 2] {
    let target = target.replace('-', "_");
    [format!("CFLAGS_{target}"), format!("CXXFLAGS_{target}")]
}

/// Existing C flags with Clang's frame-size section appended once.
fn with_stack_sizes(existing: Option<String>) -> String {
    const STACK_SIZES: &str = "-fstack-size-section";
    match existing {
        Some(flags) if flags.split_whitespace().any(|flag| flag == STACK_SIZES) => flags,
        Some(flags) if !flags.trim().is_empty() => format!("{flags} {STACK_SIZES}"),
        _ => STACK_SIZES.to_owned(),
    }
}

/// Resolve a Cargo command's explicit environment before inherited values.
fn environment(command: &Command, name: &str) -> Option<OsString> {
    match command.get_envs().find(|(key, _)| *key == name) {
        Some((_, value)) => value.map(OsString::from),
        None => env::var_os(name),
    }
}

fn configure_flags(
    command: &mut Command,
    max_move_bytes: u64,
    linker: &Path,
    instrumentation: &Instrumentation,
) -> Result<()> {
    let flags = rustflags(
        inherited_rustflags(
            environment(command, "RUSTFLAGS"),
            environment(command, "CARGO_ENCODED_RUSTFLAGS"),
        )?,
        max_move_bytes,
        linker,
        instrumentation,
    )?;
    command
        .env("RUSTC_BOOTSTRAP", "1")
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"));
    Ok(())
}

/// Cargo gives encoded rustflags precedence. Preserve their argument
/// boundaries, including spaces inside a single LLVM/linker option.
fn inherited_rustflags(plain: Option<OsString>, encoded: Option<OsString>) -> Result<Vec<String>> {
    let (flags, separator) = match encoded {
        Some(flags) => (flags, Some('\x1f')),
        None => (plain.unwrap_or_default(), None),
    };
    let flags = flags
        .into_string()
        .map_err(|_| "image rustflags must be UTF-8")?;
    Ok(match separator {
        Some(_) if flags.is_empty() => Vec::new(),
        Some(separator) => flags.split(separator).map(str::to_owned).collect(),
        None => flags.split_whitespace().map(str::to_owned).collect(),
    })
}

/// Existing Rust flags with required image flags appended, checked as a
/// whole and passed through Cargo's encoded form. The machine outliner is
/// explicitly disabled regardless of CPU, target features or opt-level.
///
/// Shared generics avoid duplicate instances after fat LTO. MergeFunctions
/// must not conflate functions whose DWARF scopes identify different calls.
fn rustflags(
    mut flags: Vec<String>,
    max_move_bytes: u64,
    linker: &Path,
    instrumentation: &Instrumentation,
) -> Result<Vec<String>> {
    let linker = linker.to_str().ok_or_else(|| {
        format!(
            "the image linker's path must be UTF-8: {}",
            linker.display()
        )
    })?;
    flags.extend([
        "-Z".to_owned(),
        "emit-stack-sizes".to_owned(),
        "-Z".to_owned(),
        format!("move-size-limit={max_move_bytes}"),
        "-W".to_owned(),
        "large-assignments".to_owned(),
        "-Z".to_owned(),
        "share-generics=y".to_owned(),
        "-Zmerge-functions=disabled".to_owned(),
        "-Cllvm-args=-enable-machine-outliner=never".to_owned(),
        "-C".to_owned(),
        format!("linker={linker}"),
        "-C".to_owned(),
        "linker-flavor=ld.lld".to_owned(),
    ]);
    flags.extend(instrumentation.rustflags().flat_map(|flag| {
        flag.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }));
    oer_image_checks::codegen::rustflags(&flags)?;
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_builds_emit_frame_sizes_for_the_image_target() {
        assert_eq!(
            c_flag_variables("riscv32imafc-unknown-none-elf"),
            [
                "CFLAGS_riscv32imafc_unknown_none_elf".to_owned(),
                "CXXFLAGS_riscv32imafc_unknown_none_elf".to_owned(),
            ]
        );
        assert_eq!(with_stack_sizes(None), "-fstack-size-section");
        assert_eq!(
            with_stack_sizes(Some("-O2".into())),
            "-O2 -fstack-size-section"
        );
        assert_eq!(
            with_stack_sizes(Some("-fstack-size-section".into())),
            "-fstack-size-section"
        );
    }

    #[test]
    fn images_share_generic_instances_after_the_callers_flags() {
        assert_eq!(
            rustflags(
                vec!["-C".into(), "debuginfo=1".into()],
                4096,
                Path::new("/repo/target/image-linker/debug/oer-image-linker"),
                &Instrumentation::default(),
            )
            .unwrap()
            .join(" "),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -W large-assignments -Z share-generics=y \
             -Zmerge-functions=disabled -Cllvm-args=-enable-machine-outliner=never \
             -C linker=/repo/target/image-linker/debug/oer-image-linker -C linker-flavor=ld.lld"
        );
        // Encoded flags preserve a linker path containing spaces.
        assert!(
            rustflags(
                Vec::new(),
                4096,
                Path::new("/a b/linker"),
                &Instrumentation::default()
            )
            .unwrap()
            .contains(&"linker=/a b/linker".to_owned())
        );
    }

    #[test]
    fn image_moves_above_the_policy_limit_warn_and_compile() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("move_limit.rs");
        let rustc = oer_os_toolchain::program(oer_os_toolchain::Tool::Rustc).unwrap();
        for limit in [64, 4096] {
            let flags = rustflags(
                vec!["-A".into(), "large-assignments".into()],
                limit,
                Path::new("/unused-image-linker"),
                &Instrumentation::default(),
            )
            .unwrap();
            for bytes in [limit, limit + 1] {
                std::fs::write(
                    &source,
                    format!(
                        "#![no_std]\n\
                         #[unsafe(no_mangle)]\n\
                         pub fn move_value(value: &[u8; {bytes}]) -> u8 {{\n\
                             let copied = core::hint::black_box(*value);\n\
                             copied[core::hint::black_box(1)]\n\
                         }}\n"
                    ),
                )
                .unwrap();
                // Object emission runs code generation's move lint without
                // invoking the image linker or depending on a chip's inputs.
                let output = oer_os_process::command(&rustc)
                    .env("RUSTC_BOOTSTRAP", "1")
                    .args(["--edition=2024", "--crate-type=lib", "--emit=obj"])
                    .args(["-C", "opt-level=3"])
                    .args(&flags)
                    .arg(&source)
                    .arg("--out-dir")
                    .arg(directory.path())
                    .output()
                    .unwrap();
                let diagnostics = String::from_utf8_lossy(&output.stderr);
                assert!(
                    output.status.success(),
                    "{bytes}-byte move with limit {limit}: {diagnostics}"
                );
                if bytes > limit {
                    assert!(
                        diagnostics.contains(&format!("warning: moving {bytes} bytes")),
                        "{diagnostics}"
                    );
                    assert!(
                        diagnostics.contains(&format!("current maximum size is {limit}")),
                        "{diagnostics}"
                    );
                } else {
                    assert!(!diagnostics.contains("warning: moving"), "{diagnostics}");
                }
            }
        }
    }

    #[test]
    fn an_instrumented_image_compiles_its_cfgs_in_a_cache_of_its_own() {
        use crate::instrumentation::Class;
        let instrumentation: Instrumentation = [Class::Profile, Class::Trace].into_iter().collect();
        assert_eq!(
            rustflags(Vec::new(), 4096, Path::new("/linker"), &instrumentation)
                .unwrap()
                .join(" "),
            "-Z emit-stack-sizes -Z move-size-limit=4096 -W large-assignments \
             -Z share-generics=y -Zmerge-functions=disabled \
             -Cllvm-args=-enable-machine-outliner=never -C linker=/linker -C linker-flavor=ld.lld \
             --cfg oer_trace --cfg oer_profile"
        );
        let root = Path::new("/checkout");
        assert_eq!(
            compile_cache(root, &Instrumentation::default()),
            root.join("target/fw/cargo")
        );
        assert_eq!(
            compile_cache(root, &instrumentation),
            root.join("target/fw/cargo-trace-profile")
        );
    }

    #[test]
    fn cargo_command_pins_encoded_flags_without_losing_argument_boundaries() {
        use oer_image_checks::codegen::CodegenPrecondition;
        let mut command = Command::new("not-executed");
        command
            .env("RUSTFLAGS", "-C llvm-args=-pick-merged-source-locations")
            .env(
                "CARGO_ENCODED_RUSTFLAGS",
                "-C\x1fdebuginfo=2\x1f--cfg\x1fvalue=\"two words\"",
            );
        configure_flags(
            &mut command,
            4096,
            Path::new("/a b/linker"),
            &Instrumentation::default(),
        )
        .unwrap();
        let encoded = environment(&command, "CARGO_ENCODED_RUSTFLAGS")
            .unwrap()
            .into_string()
            .unwrap();
        let arguments: Vec<_> = encoded.split('\x1f').map(str::to_owned).collect();
        assert!(arguments.contains(&"value=\"two words\"".to_owned()));
        assert!(arguments.contains(&"linker=/a b/linker".to_owned()));
        assert!(environment(&command, "RUSTFLAGS").is_none());
        assert_eq!(environment(&command, "RUSTC_BOOTSTRAP").unwrap(), "1");
        assert_eq!(oer_image_checks::codegen::rustflags(&arguments), Ok(()));
        for (variable, value) in [
            ("RUSTFLAGS", "-C llvm-args=-pick-merged-source-locations"),
            (
                "CARGO_ENCODED_RUSTFLAGS",
                "-C\x1fllvm-args=-enable-machine-outliner=always -pick-merged-source-locations",
            ),
            ("CARGO_ENCODED_RUSTFLAGS", "-C\x1flink-args=--icf all"),
            ("CARGO_ENCODED_RUSTFLAGS", "-Z\x1fmerge-functions=aliases"),
        ] {
            let mut command = Command::new("not-executed");
            command
                .env_remove("RUSTFLAGS")
                .env_remove("CARGO_ENCODED_RUSTFLAGS")
                .env(variable, value);
            let error = configure_flags(
                &mut command,
                4096,
                Path::new("/linker"),
                &Instrumentation::default(),
            )
            .unwrap_err();
            assert!(
                error.downcast_ref::<CodegenPrecondition>().is_some(),
                "{variable}: {error}"
            );
        }
    }

    #[test]
    fn pinned_codegen_keeps_different_callback_origins_in_distinct_functions() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("origins.rs");
        std::fs::write(
            &source,
            r#"
            #![no_std]
            pub trait A { fn m(&self) -> u32; }
            pub trait B { fn n(&self) -> u32; }
            #[inline(never)] pub fn call_a(x: &dyn A) -> u32 { x.m().wrapping_add(5) }
            #[inline(never)] pub fn call_b(x: &dyn B) -> u32 { x.n().wrapping_add(5) }
        "#,
        )
        .unwrap();
        let rustc = oer_os_toolchain::program(Tool::Rustc).unwrap();
        for merge_disabled in [false, true] {
            let mut flags = rustflags(
                Vec::new(),
                4096,
                Path::new("/unused-linker"),
                &Instrumentation::default(),
            )
            .unwrap();
            if !merge_disabled {
                flags.retain(|flag| flag != "-Zmerge-functions=disabled");
            }
            let object = directory.path().join(format!("origins-{merge_disabled}.o"));
            let output = oer_os_process::command(&rustc)
                .env("RUSTC_BOOTSTRAP", "1")
                .args([
                    "--edition=2024",
                    "--crate-type=lib",
                    "--emit=obj",
                    "--target",
                    "riscv32imafc-unknown-none-elf",
                    "-Copt-level=3",
                    "-Cdebuginfo=2",
                ])
                .args(flags)
                .arg(&source)
                .arg("-o")
                .arg(&object)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let bytes = std::fs::read(object).unwrap();
            let elf = oer_analysis_elf::Elf::parse(&bytes).unwrap();
            let origins = elf
                .symbols()
                .filter(|symbol| {
                    symbol.defined
                        && (symbol.demangled().ends_with("::call_a")
                            || symbol.demangled().ends_with("::call_b"))
                })
                .map(|symbol| (symbol.section, symbol.address))
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                origins.len(),
                if merge_disabled { 2 } else { 1 },
                "MergeFunctions disabled: {merge_disabled}"
            );
        }
    }
}
