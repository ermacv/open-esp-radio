//! The compiler setup every image build shares: the single owner of the
//! image compiler flags. No `.cargo/config.toml` carries Rust flags for an
//! image; `cargo xtask build firmware`, the HIL image builder and their type
//! checks all apply [`configure`].
//!
//! `RUSTC_BOOTSTRAP=1` is set here, on the image's Cargo command only, and
//! exists for the unstable `-Z` flags alone (`emit-stack-sizes`,
//! `move-size-limit`, `share-generics`) on the exact stable toolchain that
//! `rust-toolchain.toml` pins.
//!
//! Every image links through the image linker (`tools/image/linker`), which
//! refuses an input section named as zero-initialized that carries bytes and
//! otherwise runs the target's `rust-lld` with the same arguments. The linker
//! is built from the sources being built, the checkout or source snapshot at
//! [`ImageCompiler::root`], so an image never links with another tree's
//! linker.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Result, Tool};

/// The input section patterns (`name` or `name.*`, comma separated) the
/// image linker refuses initialized bytes in: the regions the boot zeroes.
/// Unset or empty, it checks none.
pub const ZEROED_INPUTS_ENV: &str = "OER_IMAGE_ZEROED_INPUTS";

/// Names the host build root instead of the user's cache directory.
pub const BUILD_ROOT_ENV: &str = "OER_BUILD_ROOT";

/// The directory every checkout of this host builds images in: the shared
/// compile cache, the image linker, the build slots and, for HIL, the
/// source snapshots and their build workspaces. One per host, so each
/// agent's build of the same sources reuses the same compiled units.
pub fn host_build_root() -> Result<PathBuf> {
    oer_durable::xdg::overridable(BUILD_ROOT_ENV, oer_durable::xdg::Base::Cache, "build")
        .map_err(|error| error.to_string().into())
}

/// The one compile cache (`CARGO_TARGET_DIR`) below `build_root` of every
/// firmware build of the host: every image class, example and chip, and
/// the vendor comparison probes, from every checkout and source snapshot.
/// Cargo keeps the units apart by target, package path, features and flags,
/// and the flags are the same for every build of a chip. Builds uplift
/// their binaries to the same `<target>/release/<name>`, so each holds
/// [`lock_compile_cache`] from its Cargo run until its outputs are copied
/// out.
pub fn compile_cache(build_root: &Path) -> PathBuf {
    build_root.join("cargo")
}

/// The target directory below `build_root` of the image linker, built once
/// for every firmware build of the host: its path is part of every image's
/// and probe's Rust flags, so it must not differ between builds.
pub fn linker_target(build_root: &Path) -> PathBuf {
    build_root.join("image-linker")
}

/// The lock of the shared compile cache `cache`: one build at a time, the
/// next waiting for it.
pub fn lock_compile_cache(cache: &Path) -> Result<oer_process::lock::FileLock> {
    oer_process::lock::FileLock::wait(
        &cache.join("build.lock"),
        oer_process::lock::Mode::Exclusive,
        &format!("waiting for another build in {}", cache.display()),
    )
    .map_err(|error| error.to_string().into())
}

/// What an image's compiler setup depends on.
#[derive(Clone, Debug)]
pub struct ImageCompiler<'a> {
    /// The tree being built: its `tools/image/linker` becomes the linker.
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
        rustflags(env::var("RUSTFLAGS").ok(), compiler.max_move_bytes, &linker)?,
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
    let mut command = crate::command(Tool::Cargo)?;
    command
        .current_dir(root)
        .args([
            "build",
            "--quiet",
            "-p",
            "oer-image-linker",
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
    oer_process::run(&mut command).map_err(|error| {
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

/// Existing Rust flags with the image flags appended.
///
/// Without shared generics, fat LTO keeps one copy of a generic instance per
/// crate that instantiates it: the Wi-Fi and Bluetooth compositions both
/// instantiate the same PHY registration and radio guard, which would
/// otherwise be emitted twice.
///
/// The image linker replaces the target's `rust-lld` and keeps its flavor.
/// `RUSTFLAGS` separates flags at spaces, so the linker's path must have
/// none.
fn rustflags(existing: Option<String>, max_move_bytes: u64, linker: &Path) -> Result<String> {
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
        "-D large-assignments".to_owned(),
        "-Z share-generics=y".to_owned(),
        format!("-C linker={linker}"),
        "-C linker-flavor=ld.lld".to_owned(),
    ] {
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
            )
            .unwrap(),
            "-C debuginfo=1 -Z emit-stack-sizes -Z move-size-limit=4096 \
             -D large-assignments -Z share-generics=y \
             -C linker=/repo/target/image-linker/debug/oer-image-linker -C linker-flavor=ld.lld"
        );
        // A path RUSTFLAGS would split is refused.
        assert!(rustflags(None, 4096, Path::new("/a b/linker")).is_err());
    }

    /// A tree whose `oer-image-linker` is a stand-in that announces its tree.
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
            "[package]\nname = \"oer-image-linker\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
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
            let output = oer_process::command(&linker).output().unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), marker);
        }
    }
}
