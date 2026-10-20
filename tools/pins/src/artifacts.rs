//! The pinned vendor artifacts of a chip: its `artifacts.toml`, the
//! host-wide store and fetching into it.
//!
//! The chip's tracked `artifacts.toml` is the only pin. Git artifacts come
//! from the upstream repository at the pinned revision, release members from
//! the pinned release asset; every file is verified against its SHA-256
//! before it enters `target/vendor/<source>/<revision>/<path>`. Firmware
//! outputs are built from their tracked recipe: neither fetched nor hashed. Downloads use `curl` and release members `tar`.
//!
//! Pinned artifacts are immutable and verified, so every checkout of the host
//! shares one store ([`store`]): each checkout's `target/vendor` is a link
//! to it, and a new checkout or worktree finds everything another one fetched.
//! A checkout's former `target/vendor` directory is merged into the store.
//!
//! `cargo pins fetch` and the vendor checks read the pins here.
//! [`project::Project`] names the fixed files of a chip's verification
//! project (its shards, provenance registry and scenarios).
use crate::{Artifact, CACHE, Manifest, Source, SourceKind, cache_directory};
use std::path::{Path, PathBuf};

pub mod conformance;
pub mod project;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Overrides the host-wide store of fetched artifacts.
pub const STORE_ENV: &str = "OER_VENDOR_CACHE";

/// The host-wide store of fetched artifacts, which every checkout, source
/// snapshot and image build shares: `$OER_VENDOR_CACHE`, else
/// `open-esp-radio/vendor` in the user's cache directory.
pub fn store() -> Result<PathBuf> {
    oer_os_durable::xdg::overridable(STORE_ENV, oer_os_durable::xdg::Base::Cache, "vendor")
}

/// Makes `root`'s `target/vendor` a link to `store`, merging a former
/// directory's artifacts into the store first.
pub fn link_store(root: &Path, store: &Path) -> Result<()> {
    std::fs::create_dir_all(store)?;
    let link = root.join(CACHE);
    match std::fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if std::fs::read_link(&link)? == store {
                return Ok(());
            }
            std::fs::remove_file(&link)?;
        }
        Ok(metadata) if metadata.is_dir() => {
            merge_into(&link, store)?;
            std::fs::remove_dir_all(&link)?;
        }
        Ok(_) => std::fs::remove_file(&link)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(store, &link)?;
    Ok(())
}

/// Moves every entry of `from` that `into` lacks; artifacts are keyed by
/// source and revision and verified on use, so an existing entry wins.
fn merge_into(from: &Path, into: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = into.join(entry.file_name());
        if entry.file_type()?.is_dir() && target.is_dir() {
            merge_into(&entry.path(), &target)?;
        } else if std::fs::symlink_metadata(&target).is_err() {
            std::fs::rename(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// The verified file of the fetched artifact `id` of `chip` in the
/// host-wide [`store`]: an error naming `cargo pins fetch` when it is
/// missing, and naming the pin when it differs.
pub fn fetched(root: &Path, chip: &str, id: &str) -> Result<PathBuf> {
    let manifest = Manifest::load(root, chip)?;
    let artifact = manifest.artifact(id)?;
    let source = manifest.source_of(artifact);
    if source.kind == SourceKind::Firmware {
        return Err(format!("`{id}` is a firmware build output, not a fetched artifact").into());
    }
    let path = cache_directory(&store()?, source)?.join(&artifact.path);
    if !path.is_file() {
        return Err(format!(
            "the pinned `{id}` artifact {} is missing: run `cargo pins fetch --chip {chip} {id}`",
            path.display()
        )
        .into());
    }
    let pin = pinned_sha256(artifact)?;
    if !verified(&path, pin)? {
        return Err(format!("{} differs from its pin {pin}", path.display()).into());
    }
    Ok(path)
}

/// The pinned SHA-256 of the fetched `artifact`.
fn pinned_sha256(artifact: &Artifact) -> Result<&str> {
    artifact
        .sha256
        .as_deref()
        .ok_or_else(|| format!("{} pins no SHA-256", artifact.id).into())
}

/// Whether `path` is a file whose SHA-256 is `expected`.
pub fn verified(path: &Path, expected: &str) -> Result<bool> {
    Ok(path.is_file() && oer_os_durable::sha256_file(path)? == expected)
}

/// `https://github.com/<owner>/<repo>` as `<owner>/<repo>`.
fn github(repository: &str) -> Result<&str> {
    repository
        .strip_prefix("https://github.com/")
        .map(|r| r.trim_end_matches('/'))
        .ok_or_else(|| format!("unsupported repository {repository}").into())
}

pub(crate) fn download(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = destination.with_extension("partial");
    let status = oer_os_process::command("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(url)
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("download failed: {url}").into());
    }
    std::fs::rename(&partial, destination)?;
    Ok(())
}

fn fetch(root: &Path, source: &Source, artifact: &Artifact) -> Result<PathBuf> {
    let directory = cache_directory(&root.join(CACHE), source)?;
    let destination = directory.join(&artifact.path);
    let pin = pinned_sha256(artifact)?;
    if verified(&destination, pin)? {
        return Ok(destination);
    }
    let repository = source
        .repository
        .as_deref()
        .ok_or_else(|| format!("{} lacks `repository`", source.id))?;
    let revision = source.revision.as_deref().unwrap_or_default();
    match source.kind {
        SourceKind::Git => download(
            &format!(
                "https://raw.githubusercontent.com/{}/{revision}/{}",
                github(repository)?,
                artifact.path
            ),
            &destination,
        )?,
        SourceKind::Release => {
            let asset = source
                .asset
                .as_deref()
                .ok_or_else(|| format!("{} lacks `asset`", source.id))?;
            let tarball = directory.join(asset);
            let expected = source
                .sha256
                .as_deref()
                .ok_or_else(|| format!("{} lacks `sha256`", source.id))?;
            if !verified(&tarball, expected)? {
                download(
                    &format!("{repository}/releases/download/{revision}/{asset}"),
                    &tarball,
                )?;
                let actual = oer_os_durable::sha256_file(&tarball)?;
                if actual != expected {
                    return Err(format!("{asset}: sha256 {actual}, pinned {expected}").into());
                }
            }
            let status = oer_os_process::command("tar")
                .arg("-xzf")
                .arg(&tarball)
                .arg("-C")
                .arg(&directory)
                .arg(&artifact.path)
                .status()?;
            if !status.success() {
                return Err(format!("{asset} lacks member {}", artifact.path).into());
            }
        }
        SourceKind::Firmware => unreachable!("firmware outputs are not fetched"),
    }
    let actual = oer_os_durable::sha256_file(&destination)?;
    if actual != pin {
        return Err(format!("{}: fetched sha256 {actual}, pinned {pin}", artifact.id).into());
    }
    Ok(destination)
}

/// One pinned artifact at its cache or firmware build path.
#[derive(Debug)]
pub struct Pinned {
    pub id: String,
    /// The artifact's source id.
    pub source: String,
    pub path: PathBuf,
    /// Built from a tracked firmware recipe rather than fetched from a
    /// vendor source.
    pub firmware: bool,
}

/// Every pinned artifact of `chip` at its verified path; fails when one is
/// missing or differs, naming `cargo pins fetch` for fetched ones.
/// Firmware outputs are skipped when unbuilt: they are not vendor sources.
pub fn pinned(root: &Path, chip: &str) -> Result<Vec<Pinned>> {
    let resolved = resolve(root, chip)?;
    match resolved.unfetched.first() {
        None => Ok(resolved.pinned),
        Some(id) => Err(format!(
            "{id} is not fetched as pinned; run `cargo pins fetch --chip {chip}`"
        )
        .into()),
    }
}

/// The pinned firmware outputs of `chip` not built in this checkout, by
/// artifact id: [`pinned`] skips them.
pub fn unbuilt_firmware(root: &Path, chip: &str) -> Result<Vec<String>> {
    Ok(resolve(root, chip)?.unbuilt)
}

/// Fetches every pinned vendor artifact of `chip` into the shared store,
/// leaving firmware outputs alone; fails when one cannot be fetched as pinned.
pub fn fetch_vendor_sources(root: &Path, chip: &str) -> Result<()> {
    link_store(root, &store()?)?;
    let manifest = Manifest::load(root, chip)?;
    let mut failures = vec![];
    for artifact in &manifest.artifact {
        let source = manifest.source_of(artifact);
        if source.kind == SourceKind::Firmware {
            continue;
        }
        if let Err(error) = fetch(root, source, artifact) {
            failures.push(format!("{}: {error}", artifact.id));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "vendor artifacts of {chip} could not be fetched: {}",
            failures.join("; ")
        )
        .into())
    }
}

/// The pinned fetched artifact `id` of `chip`, downloaded into the store
/// first when it is missing or differs from its pin.
pub fn ensure(root: &Path, chip: &str, id: &str) -> Result<PathBuf> {
    link_store(root, &store()?)?;
    let manifest = Manifest::load(root, chip)?;
    let artifact = manifest.artifact(id)?;
    let source = manifest.source_of(artifact);
    if source.kind == SourceKind::Firmware {
        return Err(format!("`{id}` is a firmware build output, not a fetched artifact").into());
    }
    fetch(root, source, artifact)
}

/// The firmware catalog images whose outputs `chip` pins, in pin order:
/// what `cargo hil firmware build` builds before a scenario links them.
pub fn firmware_images(root: &Path, chip: &str) -> Result<Vec<String>> {
    Ok(images(Manifest::load(root, chip)?))
}

fn images(manifest: Manifest) -> Vec<String> {
    manifest
        .source
        .into_iter()
        .filter(|source| source.kind == SourceKind::Firmware)
        .filter_map(|source| source.image)
        .collect()
}

/// The fetched vendor artifacts of `chip` that are missing or differ from
/// their pin; empty when every citation can be checked.
pub fn unfetched(root: &Path, chip: &str) -> Result<Vec<String>> {
    Ok(resolve(root, chip)?.unfetched)
}

struct Resolved {
    pinned: Vec<Pinned>,
    unfetched: Vec<String>,
    /// Firmware outputs not built in this checkout, by artifact id.
    unbuilt: Vec<String>,
}

fn resolve(root: &Path, chip: &str) -> Result<Resolved> {
    resolve_manifest(root, &Manifest::load(root, chip)?)
}

fn resolve_manifest(root: &Path, manifest: &Manifest) -> Result<Resolved> {
    let mut resolved = Resolved {
        pinned: vec![],
        unfetched: vec![],
        unbuilt: vec![],
    };
    for artifact in manifest.artifact.iter().cloned() {
        let firmware = manifest.source_of(&artifact).kind == SourceKind::Firmware;
        let path = manifest.location(root, &artifact)?;
        if firmware {
            if !path.is_file() {
                resolved.unbuilt.push(artifact.id);
                continue;
            }
        } else if !verified(&path, pinned_sha256(&artifact)?)? {
            resolved.unfetched.push(artifact.id);
            continue;
        }
        resolved.pinned.push(Pinned {
            id: artifact.id,
            source: artifact.source,
            path,
            firmware,
        });
    }
    Ok(resolved)
}

/// Fetch and verify every artifact of `chip`; report firmware outputs that
/// are not built. Fails when any fetched artifact is not available as pinned.
pub fn run(root: &Path, chip: &str, only: &[String]) -> Result<()> {
    link_store(root, &store()?)?;
    let manifest = Manifest::load(root, chip)?;
    let artifacts = selected(manifest.artifact.clone(), only)?;
    let mut failures = vec![];
    for artifact in &artifacts {
        let source = manifest.source_of(artifact);
        let result = if source.kind == SourceKind::Firmware {
            let path = manifest.location(root, artifact)?;
            if !path.is_file() {
                // Firmware outputs are optional here: a host builds them
                // when a scenario needs them.
                println!(
                    "{:<16} skipped: not built; run `cargo hil firmware build {}`",
                    artifact.id,
                    source.image.as_deref().unwrap_or_default()
                );
                continue;
            }
            Ok(path)
        } else {
            fetch(root, source, artifact)
        };
        match result {
            Ok(path) => println!("{:<16} {}", artifact.id, path.display()),
            Err(error) => {
                println!("{:<16} ERROR {error}", artifact.id);
                failures.push(artifact.id.clone());
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("unavailable artifacts: {}", failures.join(", ")).into())
    }
}

/// The artifacts `only` names, or all when it names none; an id the
/// manifest lacks is an error.
fn selected(artifacts: Vec<Artifact>, only: &[String]) -> Result<Vec<Artifact>> {
    if let Some(unknown) = only
        .iter()
        .find(|id| !artifacts.iter().any(|artifact| &artifact.id == *id))
    {
        return Err(format!("no artifact `{unknown}` in artifacts.toml").into());
    }
    Ok(artifacts
        .into_iter()
        .filter(|artifact| only.is_empty() || only.contains(&artifact.id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_artifact_selection_keeps_only_the_named_artifacts() {
        let artifact = |id: &str| Artifact {
            id: id.to_owned(),
            source: "s".to_owned(),
            path: "p".to_owned(),
            sha256: Some("h".to_owned()),
        };
        let all = || vec![artifact("rom"), artifact("phy")];
        assert_eq!(selected(all(), &[]).unwrap().len(), 2);
        let rom = selected(all(), &["rom".to_owned()]).unwrap();
        assert_eq!(
            rom.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["rom"]
        );
        assert!(selected(all(), &["missing".to_owned()]).is_err());
    }

    #[test]
    fn artifacts_must_name_declared_sources() {
        let error = Manifest::parse(
            "schema = 1\nsource = []\n[[artifact]]\nid = \"a\"\nsource = \"b\"\npath = \"c\"\nsha256 = \"d\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown source"));
        assert!(Manifest::parse("schema = 2\nsource = []\nartifact = []\n").is_err());
        assert!(Manifest::parse("schema = 1\nsource = []\nartifact = []\nextra = 1\n").is_err());
    }

    #[test]
    fn unfetched_names_missing_vendor_artifacts_in_the_source_keyed_cache() {
        let root = tempfile::tempdir().unwrap();
        let cached = root.path().join(CACHE).join("vendor/0123/lib.a");
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"pinned").unwrap();
        let manifest = format!(
            "schema = 1\n\
             [[source]]\nid = \"vendor\"\nkind = \"git\"\n\
             repository = \"https://github.com/o/r\"\nrevision = \"0123\"\n\
             [[source]]\nid = \"build\"\nkind = \"firmware\"\nimage = \"img\"\n\
             [[artifact]]\nid = \"fetched\"\nsource = \"vendor\"\npath = \"lib.a\"\nsha256 = \"{}\"\n\
             [[artifact]]\nid = \"missing\"\nsource = \"vendor\"\npath = \"other.a\"\nsha256 = \"00\"\n\
             [[artifact]]\nid = \"unbuilt\"\nsource = \"build\"\npath = \"out.elf\"\n",
            oer_os_durable::sha256_file(&cached).unwrap()
        );
        let manifest = Manifest::parse(&manifest).unwrap();
        let unbuilt = manifest.artifact("unbuilt").unwrap();
        assert_eq!(
            manifest.location(root.path(), unbuilt).unwrap(),
            root.path().join("out.elf")
        );
        let resolved = resolve_manifest(root.path(), &manifest).unwrap();
        assert_eq!(resolved.unfetched, ["missing"]);
        let pinned: Vec<_> = resolved.pinned.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pinned, ["fetched"]);
    }

    #[test]
    fn the_firmware_images_are_those_of_firmware_sources_in_pin_order() {
        let manifest = Manifest::parse(
            "schema = 1\nartifact = []\n\
             [[source]]\nid = \"b\"\nkind = \"firmware\"\nimage = \"second\"\n\
             [[source]]\nid = \"vendor\"\nkind = \"git\"\n\
             repository = \"https://github.com/o/r\"\nrevision = \"0123\"\n\
             [[source]]\nid = \"a\"\nkind = \"firmware\"\nimage = \"first\"\n",
        )
        .unwrap();
        assert_eq!(images(manifest), ["second", "first"]);
    }

    #[test]
    fn an_unbuilt_firmware_output_is_listed_and_a_built_one_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(
            "schema = 1\n\
             [[source]]\nid = \"reference\"\nkind = \"firmware\"\nimage = \"reference\"\n\
             [[artifact]]\nid = \"image\"\nsource = \"reference\"\npath = \"out/image.elf\"\n\
             [[artifact]]\nid = \"hal\"\nsource = \"reference\"\npath = \"out/libhal.a\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/image.elf"), b"elf").unwrap();
        let resolved = resolve_manifest(dir.path(), &manifest).unwrap();
        assert_eq!(
            resolved
                .pinned
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["image"]
        );
        assert_eq!(resolved.unbuilt, ["hal"]);
    }

    #[test]
    fn a_checkout_links_the_shared_store_and_merges_its_former_cache() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let store = dir.path().join("store");
        std::fs::create_dir_all(checkout.join(CACHE).join("idf/abc")).unwrap();
        std::fs::write(checkout.join(CACHE).join("idf/abc/lib.a"), b"lib").unwrap();
        std::fs::create_dir_all(store.join("idf/abc")).unwrap();
        std::fs::write(store.join("idf/abc/rom.elf"), b"rom").unwrap();
        link_store(&checkout, &store).unwrap();
        assert_eq!(std::fs::read_link(checkout.join(CACHE)).unwrap(), store);
        assert_eq!(
            std::fs::read(checkout.join(CACHE).join("idf/abc/lib.a")).unwrap(),
            b"lib"
        );
        assert_eq!(
            std::fs::read(checkout.join(CACHE).join("idf/abc/rom.elf")).unwrap(),
            b"rom"
        );
        // A second checkout finds what the first fetched, and relinking is idempotent.
        let other = dir.path().join("other");
        link_store(&other, &store).unwrap();
        link_store(&other, &store).unwrap();
        assert_eq!(
            std::fs::read(other.join(CACHE).join("idf/abc/lib.a")).unwrap(),
            b"lib"
        );
    }
}
