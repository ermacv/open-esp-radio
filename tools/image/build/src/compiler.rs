//! The compiler setup every image build shares: the single owner of the
//! image compiler flags. No `.cargo/config.toml` carries Rust flags for an
//! image; `cargo fw build` applies [`configure`].
//!
//! `RUSTC_BOOTSTRAP=1` is set here, on the image's Cargo command only, and
//! exists for the unstable `-Z` flags alone (`emit-stack-sizes`,
//! `move-size-limit`, `share-generics`) on the exact stable toolchain that
//! `rust-toolchain.toml` pins.
//!
//! Every image links through the image linker (`oer-image-linker` of `tools/image/linker`), which
//! refuses an input section named as zero-initialized that carries bytes and
//! otherwise runs the target's `rust-lld` with the same arguments. The linker
//! is built from the sources being built, the checkout or worktree at
//! [`ImageCompiler::root`], so an image never links with another tree's
//! linker.

use std::{
    env,
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
    /// The largest move the image allows (`-Z move-size-limit`).
    pub max_move_bytes: u64,
    /// Variables the image's runtime reads at compile time (its stack
    /// policy's reserves).
    pub runtime_environment: Vec<(&'static str, String)>,
    /// The instrumentation classes whose cfgs the image is compiled under.
    pub instrumentation: Instrumentation,
}

/// Configure `command`, a Cargo build of an image, with the compiler flags
/// every image shares, building the image linker first.
pub fn configure(command: &mut Command, compiler: &ImageCompiler<'_>) -> Result<()> {
    let linker = build_linker(compiler.root, compiler.linker_target)?;
    // The pinned stable toolchain supports these flags, but they remain
    // unstable: RUSTC_BOOTSTRAP enables them for this command alone. The
    // stack-size ELF section is consumed by a safe host-side parser.
    command.env("RUSTC_BOOTSTRAP", "1").env(
        "RUSTFLAGS",
        rustflags(
            env::var("RUSTFLAGS").ok(),
            compiler.max_move_bytes,
            &linker,
            &compiler.instrumentation,
        )?,
    );
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
    Ok(())
}

/// Build the image linker of the tree at `root` for the host into
/// `target_dir` and return its path. The build leaves the image's compiler
/// environment behind: it is a host tool.
pub fn build_linker(root: &Path, target_dir: &Path) -> Result<PathBuf> {
    let mut command = oer_os_toolchain::command(Tool::Cargo)?;
    command
        .current_dir(root)
        .args([
            "build",
            "--quiet",
            "-p",
            "oer-tool-image-linker",
            "--bin",
            "oer-image-linker",
        ])
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
    ] {
        command.env_remove(variable);
    }
    oer_os_process::run(&mut command).map_err(|error| {
        format!(
            "building the image linker of {} failed: {error}",
            root.display()
        )
    })?;
    Ok(target_dir.join("debug/oer-image-linker"))
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

/// Existing Rust flags with the image flags appended, the cfgs of
/// `instrumentation` last.
///
/// Without shared generics, fat LTO keeps one copy of a generic instance per
/// crate that instantiates it: the Wi-Fi and Bluetooth compositions both
/// instantiate the same PHY registration and radio guard, which would
/// otherwise be emitted twice.
///
/// The image linker replaces the target's `rust-lld` and keeps its flavor.
/// `RUSTFLAGS` separates flags at spaces, so the linker's path must have
/// none.
fn rustflags(
    existing: Option<String>,
    max_move_bytes: u64,
    linker: &Path,
    instrumentation: &Instrumentation,
) -> Result<String> {
    let linker = linker
        .to_str()
        .filter(|path| !path.contains(char::is_whitespace))
        .ok_or_else(|| {
            format!(
                "the image linker's path must be UTF-8 without spaces: {}",
                linker.display()
            )
        })?;
    let mut rustflags = existing.unwrap_or_default();
    for required in [
        "-Z emit-stack-sizes".to_owned(),
        format!("-Z move-size-limit={max_move_bytes}"),
        "-W large-assignments".to_owned(),
        "-Z share-generics=y".to_owned(),
        format!("-C linker={linker}"),
        "-C linker-flavor=ld.lld".to_owned(),
    ]
    .into_iter()
    .chain(instrumentation.rustflags())
    {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(&required);
    }
    Ok(rustflags)
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
                Some("-C debuginfo=1".into()),
                4096,
                Path::new("/repo/target/image-linker/debug/oer-image-linker"),
                &Instrumentation::default(),
            )
            .unwrap(),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -W large-assignments -Z share-generics=y \
             -C linker=/repo/target/image-linker/debug/oer-image-linker -C linker-flavor=ld.lld"
        );
        // A path RUSTFLAGS would split is refused.
        assert!(
            rustflags(
                None,
                4096,
                Path::new("/a b/linker"),
                &Instrumentation::default()
            )
            .is_err()
        );
    }

    #[test]
    fn an_instrumented_image_compiles_its_cfgs_in_a_cache_of_its_own() {
        use crate::instrumentation::Class;
        let instrumentation: Instrumentation = [Class::Profile, Class::Trace].into_iter().collect();
        assert_eq!(
            rustflags(None, 4096, Path::new("/linker"), &instrumentation).unwrap(),
            "-Z emit-stack-sizes -Z move-size-limit=4096 -W large-assignments \
             -Z share-generics=y -C linker=/linker -C linker-flavor=ld.lld \
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

    /// A tree whose `oer-tool-image-linker` builds an `oer-image-linker` stand-in
    /// that announces its tree.
    fn tree_with_linker(marker: &str) -> tempfile::TempDir {
        let tree = tempfile::tempdir().unwrap();
        let package = tree.path().join("linker");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(
            tree.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"linker\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        std::fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"oer-tool-image-linker\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[[bin]]\nname = \"oer-image-linker\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        std::fs::write(
            package.join("src/main.rs"),
            format!("fn main() {{ print!(\"{marker}\"); }}\n"),
        )
        .unwrap();
        tree
    }

    #[test]
    fn the_linker_is_built_from_the_given_tree() {
        let first = tree_with_linker("first");
        let second = tree_with_linker("second");
        let target = tempfile::tempdir().unwrap();
        for (tree, marker) in [(&first, "first"), (&second, "second")] {
            let linker = build_linker(tree.path(), &target.path().join(marker)).unwrap();
            let output = oer_os_process::command(&linker).output().unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), marker);
        }
    }

    /// A checkout holding package `shared` whose binary prints `marker`,
    /// its sources last modified at `modified`.
    fn checkout_with(marker: &str, modified: std::time::SystemTime) -> tempfile::TempDir {
        let tree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tree.path().join("src")).unwrap();
        std::fs::write(
            tree.path().join("Cargo.toml"),
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        let main = tree.path().join("src/main.rs");
        std::fs::write(&main, format!("fn main() {{ print!(\"{marker}\"); }}\n")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&main)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        tree
    }

    #[test]
    fn two_checkouts_of_one_package_never_share_a_unit() {
        // The second checkout's sources are older than the first one's
        // build: a cache shared by both would pass the first's unit for the
        // second's as fresh.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let first = checkout_with("first", std::time::SystemTime::now());
        let second = checkout_with("second", old);
        for (checkout, marker) in [(&first, "first"), (&second, "second")] {
            let cache = compile_cache(checkout.path(), &Instrumentation::default());
            assert!(cache.starts_with(checkout.path()));
            oer_os_process::run(
                oer_os_process::command(
                    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
                )
                .current_dir(checkout.path())
                .args(["build", "--quiet"])
                .env("CARGO_TARGET_DIR", &cache),
            )
            .unwrap();
            let output = oer_os_process::command(cache.join("debug/shared"))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), marker);
        }
        assert_ne!(
            compile_cache(first.path(), &Instrumentation::default()),
            compile_cache(second.path(), &Instrumentation::default())
        );
        assert_ne!(linker_target(first.path()), linker_target(second.path()));
    }

    #[test]
    fn a_new_worktree_starts_from_a_copy_of_the_warm_cache() {
        let warm = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(compile_cache(warm.path(), &Instrumentation::default())).unwrap();
        std::fs::write(
            compile_cache(warm.path(), &Instrumentation::default()).join("unit"),
            "compiled",
        )
        .unwrap();
        let fresh = tempfile::tempdir().unwrap();
        seed_build_cache(warm.path(), fresh.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(
                compile_cache(fresh.path(), &Instrumentation::default()).join("unit")
            )
            .unwrap(),
            "compiled"
        );
        // A worktree that already has a cache keeps it.
        assert!(seed_build_cache(warm.path(), fresh.path()).is_err());
        // A checkout without a cache seeds nothing.
        let empty = tempfile::tempdir().unwrap();
        seed_build_cache(empty.path(), tempfile::tempdir().unwrap().path()).unwrap();
    }
}
