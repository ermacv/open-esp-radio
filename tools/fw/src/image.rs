//! Building an image: the stage-two runtime, its packing, the bootstrap that
//! embeds it, the placement check and the flash contents, all into
//! `target/fw/<image>/`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::{Result, chip::Chip, encode, pack, placement, w1};

/// The chip every image of `firmware/apps` is built for.
pub const CHIP: &str = "esp32s31";
/// The bootstrap package that embeds a runtime.
pub const BOOTSTRAP: &str = "oer-esp32s31-platform-bootstrap";
/// The file that lists an image's segments.
pub const SEGMENTS: &str = "segments.toml";

/// One segment as `segments.toml` lists it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SegmentFile {
    pub name: String,
    pub offset: u32,
    pub file: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub image: String,
    pub chip: String,
    #[serde(rename = "segment")]
    pub segments: Vec<SegmentFile>,
}

/// An image's output directory below the repository `root`.
pub fn output(root: &Path, image: &str) -> PathBuf {
    root.join("target/fw").join(image)
}

/// The package of the image `image` (`firmware/apps/<image>`).
pub fn package(root: &Path, image: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Manifest {
        package: Package,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
    }
    let path = root.join("firmware/apps").join(image).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("no image `{image}`: {}: {error}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(manifest.package.name)
}

/// The cargo that runs this tool.
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// The image parameters' variables a cargo build of `image`'s runtime sets
/// (`Some`) or removes (`None`): only `w1`'s runtime gets them, so a value
/// left in the caller's environment reaches no other build.
pub fn runtime_env(image: &str, w1: Option<&w1::Config>) -> Vec<(&'static str, Option<String>)> {
    match w1.filter(|_| image == w1::IMAGE) {
        Some(config) => config
            .runtime_env()
            .into_iter()
            .map(|(name, value)| (name, Some(value)))
            .collect(),
        None => vec![(w1::ENV_CHANNEL, None), (w1::ENV_BSSID, None)],
    }
}

/// Build `package` of the firmware workspace for `target`, release, with
/// `env` set or removed.
fn cargo_build(
    root: &Path,
    package: &str,
    target: &str,
    runtime: Option<&Path>,
    env: &[(&str, Option<String>)],
) -> Result<PathBuf> {
    let workspace = root.join("firmware");
    let mut command = cargo();
    command
        .current_dir(&workspace)
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            target,
            "-p",
            package,
        ])
        .env("CARGO_TARGET_DIR", workspace.join("target"));
    if let Some(runtime) = runtime {
        command.env("PSRAM_RUNTIME_BIN", runtime);
    }
    for (name, value) in env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let status = command
        .status()
        .map_err(|error| format!("run cargo: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build -p {package} failed ({status})").into());
    }
    Ok(workspace
        .join("target")
        .join(target)
        .join("release")
        .join(package))
}

/// Write `bytes` to `path` unless it already holds them, so that an
/// unchanged runtime keeps the timestamp Cargo compares and the bootstrap
/// stays fresh.
fn replace_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let staged = path.with_extension("tmp");
    std::fs::write(&staged, bytes)?;
    std::fs::rename(&staged, path)?;
    Ok(())
}

/// Build `image`, with `w1`'s parameters for image `w1`, and return its
/// segments and placement warnings.
pub fn build(root: &Path, image: &str, w1: Option<&w1::Config>) -> Result<(Manifest, Vec<String>)> {
    let chip = Chip::load(root, CHIP)?;
    let package = package(root, image)?;
    let output = output(root, image);
    std::fs::create_dir_all(&output)?;

    eprintln!("fw: building the runtime {package}");
    let runtime_elf = cargo_build(
        root,
        &package,
        &chip.rust_target,
        None,
        &runtime_env(image, w1),
    )?;
    let runtime = std::fs::read(&runtime_elf)
        .map_err(|error| format!("{}: {error}", runtime_elf.display()))?;
    std::fs::write(output.join("runtime.elf"), &runtime)?;
    let packed = pack::pack_elf(&runtime).map_err(|error| format!("pack the runtime: {error}"))?;
    let runtime_bin = std::path::absolute(output.join("runtime.bin"))?;
    replace_if_changed(&runtime_bin, &packed.bytes)?;
    eprintln!(
        "fw: packed {} runtime bytes, CRC {:08x}",
        packed.bytes.len(),
        packed.header.payload_crc32
    );
    let warnings = placement::check_elf(&runtime, oer_esp32s31_platform_layout::LAYOUT.sram)?;
    std::fs::write(output.join("placement.txt"), warnings.join("\n"))?;
    for warning in &warnings {
        eprintln!("warning: placement: {warning}");
    }

    eprintln!("fw: building the bootstrap");
    let bootstrap_elf = cargo_build(
        root,
        BOOTSTRAP,
        &chip.rust_target,
        Some(&runtime_bin),
        &runtime_env(BOOTSTRAP, None),
    )?;
    let bootstrap = std::fs::read(&bootstrap_elf)
        .map_err(|error| format!("{}: {error}", bootstrap_elf.display()))?;
    std::fs::write(output.join("bootstrap.elf"), &bootstrap)?;

    let segments = encode::encode(&bootstrap, &chip, root)?;
    let mut files = Vec::new();
    for segment in &segments {
        let file = format!("{}.bin", segment.name);
        std::fs::write(output.join(&file), &segment.data)?;
        files.push(SegmentFile {
            name: segment.name.to_owned(),
            offset: segment.offset,
            file,
        });
    }
    let manifest = Manifest {
        image: image.to_owned(),
        chip: CHIP.to_owned(),
        segments: files,
    };
    std::fs::write(output.join(SEGMENTS), toml::to_string(&manifest)?)?;
    Ok((manifest, warnings))
}

/// The built segments of `image`, in write order.
pub fn segments(root: &Path, image: &str) -> Result<(Manifest, Vec<encode::Segment>)> {
    let output = output(root, image);
    let path = output.join(SEGMENTS);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "image `{image}` is not built ({}: {error}); run `cargo fw build {image}`",
            path.display()
        )
    })?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut segments = Vec::new();
    for segment in &manifest.segments {
        let name = match segment.name.as_str() {
            "bootloader" => "bootloader",
            "partition-table" => "partition-table",
            "application" => "application",
            "otadata" => "otadata",
            other => return Err(format!("{}: unknown segment `{other}`", path.display()).into()),
        };
        segments.push(encode::Segment {
            name,
            offset: segment.offset,
            data: std::fs::read(output.join(&segment.file))?,
        });
    }
    if segments.last().map(|segment| segment.name) != Some("otadata") {
        return Err(format!("{}: the OTA selection must be written last", path.display()).into());
    }
    Ok((manifest, segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_w1s_runtime_build_gets_its_parameters() {
        let config = w1::Config {
            channel: 11,
            expect: w1::Bssid::new([0x02, 0, 0, 0, 0xF6, 0xD0]),
        };
        assert_eq!(
            runtime_env("w1", Some(&config)),
            [
                (w1::ENV_CHANNEL, Some("11".to_owned())),
                (w1::ENV_BSSID, Some("02:00:00:00:F6:D0".to_owned())),
            ]
        );
        let removed = [(w1::ENV_CHANNEL, None), (w1::ENV_BSSID, None)];
        // The unconfigured w1, another image and the bootstrap remove them.
        assert_eq!(runtime_env("w1", None), removed);
        assert_eq!(runtime_env("w0", Some(&config)), removed);
        assert_eq!(runtime_env(BOOTSTRAP, None), removed);
    }
}
