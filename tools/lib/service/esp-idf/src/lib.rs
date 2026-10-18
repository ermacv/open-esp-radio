//! Build ESP-IDF projects of the firmware catalog against a chip's pinned
//! ESP-IDF: peers, vendor references and each chip's second-stage
//! bootloader (`oer_image::esp_idf::catalog`). Every build uses the
//! ESP-IDF revision pinned by the chip's `artifacts.toml`, checked out once
//! per host in the user's cache (`$OER_IDF_CACHE`, default
//! `~/.cache/open-esp-radio/esp-idf`) below `<revision>-<pins>/`, where
//! `<pins>` identifies every pinned source. Each IDF submodule whose
//! upstream repository is itself a pinned source is checked out at the
//! pinned revision instead and recorded in the tree's index, so the IDF
//! submodule check keeps it; every pinned artifact in such a submodule is
//! verified. After linking, every archive of the image named like a pinned
//! artifact must be that artifact. The IDF tools live in the cache's
//! `idf-tools`, apart from any user installation. Every checkout shares the
//! cache: preparing a tree or installing tools holds its lock exclusively,
//! and a build holds it shared, so no build sees a tree or Python environment
//! change under it. A checkout's former `target/vendor-firmware/esp-idf` and
//! `idf-tools` are removed on its first build from the cache. Outputs land in `target/vendor-firmware/<chip>/<app>/` with a
//! `build.json` recording the pins and the image digest.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use oer_vendor_pins::GitPin;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Record of one project build, next to its `build/` directory.
const BUILD_RECORD: &str = "build.json";
/// Build root, relative to the repository root.
const OUTPUT: &str = "target/vendor-firmware";
/// Pinned source id of the ESP-IDF tree.
const IDF_SOURCE: &str = "esp-idf";
/// IDF tool installation below the cache.
const TOOLS: &str = "idf-tools";
/// The ESP-IDF tree that configured a project's build directory.
const CONFIGURED_TREE: &str = "configured-tree";
/// The recipe that last configured a project's outputs.
const CONFIGURED_RECIPE: &str = "configured-recipe";

/// Marker of a completed tool installation for one ESP-IDF tree and chip.
const TOOLS_MARKER: &str = ".oer-installed";
/// Extension of the archives the image links.
const ARCHIVE_EXTENSION: &str = ".a";

/// Make `directory` a checkout of `repository` at `revision`, fetching only
/// that commit. The checkout's `origin` is `repository`, so git resolves
/// relative submodule URLs against the upstream rather than the directory.
fn checkout(directory: &Path, repository: &str, revision: &str) -> Result<()> {
    if oer_process::git::text(directory, ["rev-parse", "HEAD"]).is_ok_and(|head| head == revision)
        && directory.join(".git").exists()
    {
        if oer_process::git::text(directory, ["remote", "get-url", "origin"]).is_err() {
            oer_process::git::text(directory, ["remote", "add", "origin", repository])?;
            // Submodules initialized without an origin resolved against the
            // directory; take their URLs from `.gitmodules` again.
            if directory.join(".gitmodules").is_file() {
                oer_process::git::text(directory, ["submodule", "sync", "--quiet"])?;
            }
        }
        return Ok(());
    }
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    std::fs::create_dir_all(directory)?;
    oer_process::git::text(directory, ["init", "--quiet"])?;
    oer_process::git::text(directory, ["remote", "add", "origin", repository])?;
    oer_process::git::text(
        directory,
        ["fetch", "--quiet", "--depth", "1", "origin", revision],
    )?;
    oer_process::git::text(directory, ["checkout", "--quiet", "FETCH_HEAD"])?;
    Ok(())
}

/// `url` of a submodule, resolved against the superproject `repository` as
/// git resolves relative submodule URLs, without a `.git` suffix.
fn resolve(repository: &str, url: &str) -> String {
    let mut base = repository.trim_end_matches('/').to_owned();
    let mut rest = url;
    loop {
        if let Some(next) = rest.strip_prefix("../") {
            if let Some(index) = base.rfind('/') {
                base.truncate(index);
            }
            rest = next;
        } else if let Some(next) = rest.strip_prefix("./") {
            rest = next;
        } else {
            break;
        }
    }
    if rest.len() == url.len() {
        normalize(url)
    } else {
        normalize(&format!("{base}/{rest}"))
    }
}

/// `repository` without a trailing slash or `.git` suffix.
fn normalize(repository: &str) -> String {
    repository
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_owned()
}

/// Submodule paths of `tree` by resolved upstream repository.
fn submodules(tree: &Path, repository: &str) -> Result<BTreeMap<String, String>> {
    let listing = oer_process::git::text(
        tree,
        [
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.(path|url)$",
        ],
    )?;
    let (mut paths, mut urls) = (BTreeMap::new(), BTreeMap::new());
    for line in listing.lines() {
        let (key, value) = line.split_once(' ').ok_or("malformed .gitmodules")?;
        if let Some(name) = key.strip_suffix(".path") {
            paths.insert(name.to_owned(), value.to_owned());
        } else if let Some(name) = key.strip_suffix(".url") {
            urls.insert(name.to_owned(), resolve(repository, value));
        }
    }
    Ok(urls
        .into_iter()
        .filter_map(|(name, url)| Some((url, paths.get(&name)?.clone())))
        .collect())
}

#[derive(Clone, Serialize, serde::Deserialize)]
struct Override {
    source: String,
    submodule: String,
    revision: String,
}

/// Check out the pinned ESP-IDF with its pinned submodules; returns the tree
/// and the submodules replaced.
fn prepare_tree(cache: &Path, pins: &[GitPin]) -> Result<(PathBuf, Vec<Override>)> {
    let idf = pins
        .iter()
        .find(|p| p.id == IDF_SOURCE)
        .ok_or_else(|| format!("no `{IDF_SOURCE}` source pinned"))?;
    let tree = cache.join(tree_name(pins, &idf.revision));
    checkout(&tree, &idf.repository, &idf.revision)?;
    let modules = submodules(&tree, &idf.repository)?;
    let mut overrides = vec![];
    for pin in pins.iter().filter(|p| p.id != IDF_SOURCE) {
        let repository = normalize(&pin.repository);
        let Some(path) = modules.get(&repository) else {
            continue;
        };
        let directory = tree.join(path);
        checkout(&directory, &pin.repository, &pin.revision)?;
        oer_process::git::text(
            &tree,
            [
                "update-index",
                "--cacheinfo",
                &format!("160000,{},{path}", pin.revision),
            ],
        )?;
        oer_process::git::text(&tree, ["submodule", "init", "--quiet", path])?;
        for (artifact, sha256) in &pin.artifacts {
            let actual = oer_durable::sha256_file(&directory.join(artifact))?;
            if &actual != sha256 {
                return Err(format!("{path}/{artifact}: sha256 {actual}, pinned {sha256}").into());
            }
        }
        overrides.push(Override {
            source: pin.id.clone(),
            submodule: path.clone(),
            revision: pin.revision.clone(),
        });
    }
    Ok((tree, overrides))
}

fn bash(script: &str, env: &[(&str, &Path)], log: &Path) -> Result<()> {
    let file = std::fs::File::create(log)?;
    let status = oer_process::command("bash")
        .args(["-ec", script])
        .envs(env.iter().map(|(k, v)| (*k, v.as_os_str())))
        .stdout(file.try_clone()?)
        .stderr(file)
        .status()?;
    if !status.success() {
        return Err(format!("command failed; see {}", log.display()).into());
    }
    Ok(())
}

/// The host-wide cache of ESP-IDF trees and tools.
pub fn cache_directory() -> Result<PathBuf> {
    oer_durable::xdg::esp_idf_cache()
}

/// Directory name of the tree prepared for `pins`: the IDF revision and a
/// digest of every pinned source, so trees with different submodule pins
/// never share a directory and a prepared tree never changes.
fn tree_name(pins: &[GitPin], revision: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let mut sources = pins
        .iter()
        .map(|pin| {
            format!(
                "{} {} {}\n",
                pin.id,
                normalize(&pin.repository),
                pin.revision
            )
        })
        .collect::<Vec<_>>();
    sources.sort();
    let digest = Sha256::digest(sources.concat().as_bytes());
    let digest = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{revision}-{digest}")
}

/// Remove this checkout's own ESP-IDF tree and tools from before the cache.
fn remove_checkout_copies(root: &Path) {
    for name in [IDF_SOURCE, TOOLS] {
        let directory = root.join(OUTPUT).join(name);
        if directory.is_dir() {
            eprintln!(
                "vendor firmware: removing {} (the ESP-IDF now lives in the shared cache)",
                directory.display()
            );
            if let Err(error) = std::fs::remove_dir_all(&directory) {
                eprintln!(
                    "vendor firmware: cannot remove {}: {error}",
                    directory.display()
                );
            }
        }
    }
}

/// The marker of the tool installation of `tree` for `chip`. ESP-IDF records
/// the targets it installed per tree path, and export demands every target's
/// tools for a path it has no record of, so each tree (one per set of pins)
/// runs its own installation.
fn tools_marker(tools: &Path, tree: &Path, chip: &str) -> Result<PathBuf> {
    let tree_name = tree
        .file_name()
        .ok_or("the ESP-IDF tree has no directory name")?
        .to_string_lossy();
    Ok(tools.join(format!("{TOOLS_MARKER}-{tree_name}-{chip}")))
}

/// Install the IDF tools of `tree` for `chip` once per tree.
fn install_tools(cache: &Path, tree: &Path, chip: &str, revision: &str) -> Result<PathBuf> {
    let tools = cache.join(TOOLS);
    let marker = tools_marker(&tools, tree, chip)?;
    if !marker.is_file() {
        std::fs::create_dir_all(&tools)?;
        bash(
            r#""$IDF_PATH/install.sh" "$OER_CHIP""#,
            &[
                ("IDF_PATH", tree),
                ("IDF_TOOLS_PATH", &tools),
                ("OER_CHIP", Path::new(chip)),
            ],
            &tools.join(format!("install-{revision}.log")),
        )?;
        std::fs::write(&marker, "")?;
    }
    Ok(tools)
}

/// Archives the linker map `map` names below `tree` that a pinned artifact
/// describes but that differ from it. A linked archive is described by a pin
/// when its path ends with the pinned path relative to the pin's checkout:
/// `<chip>/libphy.a` keeps another chip's `libphy.a` apart, and a
/// pin without a directory, such as `libble_app.a`, matches by file name.
fn unpinned_archives(map: &str, tree: &Path, pins: &[GitPin]) -> Result<Vec<String>> {
    let tree = tree.to_string_lossy();
    let mut linked = std::collections::BTreeSet::new();
    for token in map.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        if token.starts_with(tree.as_ref()) && token.ends_with(ARCHIVE_EXTENSION) {
            linked.insert(token.to_owned());
        }
    }
    let mut unpinned = vec![];
    for archive in linked {
        let digests = pins
            .iter()
            .flat_map(|pin| &pin.artifacts)
            .filter(|(path, _)| archive.ends_with(&format!("/{}", path.trim_start_matches('/'))))
            .map(|(_, sha256)| sha256.as_str())
            .collect::<Vec<_>>();
        if !digests.is_empty()
            && !digests.contains(&oer_durable::sha256_file(Path::new(&archive))?.as_str())
        {
            unpinned.push(archive);
        }
    }
    Ok(unpinned)
}

/// `build.json` of one built project.
#[derive(Serialize, serde::Deserialize)]
pub struct Build {
    pub chip: String,
    pub project: String,
    pub idf_revision: String,
    /// The ESP-IDF tree the build used; absent in records before the cache.
    #[serde(default)]
    pub idf_tree: Option<PathBuf>,
    overrides: Vec<Override>,
    pub application: String,
    pub application_sha256: String,
    pub elf: String,
    /// The [`recipe`] the build followed.
    pub recipe: String,
}

/// One ESP-IDF project to build: its directory and its target chip.
pub struct Project {
    pub name: String,
    pub source: PathBuf,
    pub chip: String,
}

/// Output directory of `project`, holding `build/`, `sdkconfig` and
/// `build.json`.
pub fn output(root: &Path, project: &Project) -> PathBuf {
    root.join(OUTPUT).join(&project.chip).join(&project.name)
}

/// SHA-256 of what a build of `project` against the `artifacts.toml` of
/// `pins` follows from: the pinned ESP-IDF tree (the IDF
/// revision and every pinned source) and each file below the project
/// directory with its contents. Files the project names outside its
/// directory, such as a shared partition table, are not part of it.
pub fn recipe(root: &Path, pins: &str, project: &Project) -> Result<String> {
    project_recipe(&oer_vendor_pins::git_pins(root, pins)?, &project.source)
}

fn project_recipe(pins: &[GitPin], source: &Path) -> Result<String> {
    let revision = &pins
        .iter()
        .find(|pin| pin.id == IDF_SOURCE)
        .ok_or("the pins name no ESP-IDF source")?
        .revision;
    let mut files = vec![];
    project_files(source, Path::new(""), &mut files)?;
    files.sort();
    let mut text = format!("tree {}\n", tree_name(pins, revision));
    for file in files {
        let digest = oer_durable::sha256_file(&source.join(&file))?;
        text.push_str(&format!("{digest} {}\n", file.display()));
    }
    Ok(oer_durable::sha256_bytes(text.as_bytes()))
}

/// Every file below `directory`, relative to the project root.
fn project_files(base: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(base.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            project_files(base, &path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// The record of the last build of `project` when it followed the current
/// [`recipe`]; an error when the project is not built or its build followed
/// another recipe.
pub fn current(root: &Path, pins: &str, project: &Project) -> Result<Build> {
    let record = output(root, project).join(BUILD_RECORD);
    let Ok(bytes) = std::fs::read(&record) else {
        return Err(format!("{} is not built", project.name).into());
    };
    let build: Build = serde_json::from_slice(&bytes)?;
    if build.recipe != recipe(root, pins, project)? {
        return Err(format!("the build of {} follows another recipe", project.name).into());
    }
    Ok(build)
}

/// Build `projects` against the ESP-IDF and vendor archives pinned by the
/// `artifacts.toml` of `pins`, each for its own target chip, and write their
/// `build.json`. An image linking an archive named like a pinned artifact but
/// differing from it is refused.
/// The ESP-IDF revision and tree of the previous build in `output`, if any.
/// Whether the build directory below `output` was configured by another
/// ESP-IDF tree than `tree` at `revision`. CMake rejects such a cache, and the
/// generated sdkconfig follows that tree. The configuring tree is recorded
/// before each build, so a failed build is recognised too.
fn configured_by_another_tree(output: &Path, revision: &str, tree: &Path) -> bool {
    built_tree(output).is_some_and(|built| built != (revision.to_owned(), Some(tree.to_owned())))
        || std::fs::read_to_string(output.join(CONFIGURED_TREE))
            .is_ok_and(|previous| Path::new(&previous) != tree)
}

fn built_tree(output: &Path) -> Option<(String, Option<PathBuf>)> {
    let record: Build =
        serde_json::from_slice(&std::fs::read(output.join(BUILD_RECORD)).ok()?).ok()?;
    Some((record.idf_revision, record.idf_tree))
}

/// Prepare `output` for a build of `recipe` before it starts. The previous
/// record is removed, so a failed build leaves none that a later rollback of
/// the sources could take for this one. The recipe the outputs were last
/// configured by is kept apart from the record: when it differs, the
/// sdkconfig starts afresh, since ESP-IDF applies `sdkconfig.defaults` only
/// to options an existing sdkconfig lacks.
fn configure_recipe(output: &Path, recipe: &str) -> Result<()> {
    std::fs::create_dir_all(output)?;
    remove_if_present(&output.join(BUILD_RECORD))?;
    let marker = output.join(CONFIGURED_RECIPE);
    if std::fs::read_to_string(&marker).ok().as_deref() != Some(recipe) {
        remove_if_present(&output.join("sdkconfig"))?;
    }
    std::fs::write(marker, recipe)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

pub fn build(root: &Path, pins: &str, projects: &[Project]) -> Result<Vec<Build>> {
    let recipes = projects
        .iter()
        .map(|project| recipe(root, pins, project))
        .collect::<Result<Vec<_>>>()?;
    let pins = oer_vendor_pins::git_pins(root, pins)?;
    let cache = cache_directory()?;
    std::fs::create_dir_all(&cache)?;
    use oer_process::lock::{FileLock, Mode};
    // Exclusive while the tree and tools may change, then shared for the
    // builds; the lock converts in place.
    let mut lock = FileLock::acquire(&cache.join(".lock"), Mode::Exclusive)?;
    let (tree, overrides) = prepare_tree(&cache, &pins)?;
    let revision = oer_process::git::text(&tree, ["rev-parse", "HEAD"])?;
    let mut tools = PathBuf::new();
    for project in projects {
        tools = install_tools(&cache, &tree, &project.chip, &revision)?;
    }
    lock.convert(Mode::Shared)?;
    remove_checkout_copies(root);
    let mut builds = Vec::with_capacity(projects.len());
    for (project, recipe) in projects.iter().zip(recipes) {
        let name = &project.name;
        let chip = &project.chip;
        let output = output(root, project);
        let build = output.join("build");
        if configured_by_another_tree(&output, &revision, &tree) {
            remove_if_present(&build)?;
            remove_if_present(&output.join("sdkconfig"))?;
        }
        configure_recipe(&output, &recipe)?;
        std::fs::write(
            output.join(CONFIGURED_TREE),
            tree.to_string_lossy().as_bytes(),
        )?;
        // `--preview`: the pinned IDF lists the chip as a preview target.
        bash(
            r#". "$IDF_PATH/export.sh" >/dev/null
idf.py --preview -C "$OER_PROJECT" -B "$OER_BUILD" -DIDF_TARGET="$OER_CHIP" -DSDKCONFIG="$OER_SDKCONFIG" build"#,
            &[
                ("IDF_PATH", &tree),
                ("IDF_TOOLS_PATH", &tools),
                ("OER_PROJECT", &project.source),
                ("OER_BUILD", &build),
                ("OER_CHIP", Path::new(chip)),
                ("OER_SDKCONFIG", &output.join("sdkconfig")),
            ],
            &output.join("build.log"),
        )?;
        let description: serde_json::Value =
            serde_json::from_slice(&std::fs::read(build.join("project_description.json"))?)?;
        let field = |key: &str| -> Result<String> {
            description[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("project description lacks `{key}`").into())
        };
        let application = build.join(field("app_bin")?);
        let elf = build.join(field("app_elf")?);
        let map = std::fs::read_to_string(elf.with_extension("map"))?;
        let unpinned = unpinned_archives(&map, &tree, &pins)?;
        if !unpinned.is_empty() {
            return Err(format!("{name} links unpinned archives: {}", unpinned.join(", ")).into());
        }
        let record = Build {
            chip: chip.to_owned(),
            project: name.to_owned(),
            idf_revision: revision.clone(),
            idf_tree: Some(tree.clone()),
            overrides: overrides.clone(),
            application_sha256: oer_durable::sha256_file(&application)?,
            application: application.display().to_string(),
            elf: elf.display().to_string(),
            recipe,
        };
        let mut bytes = serde_json::to_vec_pretty(&record)?;
        bytes.push(b'\n');
        std::fs::write(output.join(BUILD_RECORD), bytes)?;
        builds.push(record);
    }
    Ok(builds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_resolves_relative_submodules_against_its_upstream() {
        let upstream = tempfile::tempdir().unwrap();
        let run =
            |directory: &Path, args: &[&str]| oer_process::git::text(directory, args).unwrap();
        run(upstream.path(), &["init", "--quiet"]);
        run(
            upstream.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        run(upstream.path(), &["config", "user.name", "test"]);
        run(
            upstream.path(),
            &["config", "uploadpack.allowReachableSHA1InWant", "true"],
        );
        std::fs::write(upstream.path().join("file"), "x").unwrap();
        run(upstream.path(), &["add", "file"]);
        run(upstream.path(), &["commit", "--quiet", "-m", "one"]);
        let revision = run(upstream.path(), &["rev-parse", "HEAD"]);
        let repository = upstream.path().to_string_lossy().into_owned();
        let target = tempfile::tempdir().unwrap();
        let tree = target.path().join("tree");
        checkout(&tree, &repository, &revision).unwrap();
        assert_eq!(run(&tree, &["remote", "get-url", "origin"]), repository);
        // An earlier checkout without an origin gains one.
        run(&tree, &["remote", "remove", "origin"]);
        checkout(&tree, &repository, &revision).unwrap();
        assert_eq!(run(&tree, &["remote", "get-url", "origin"]), repository);
    }

    #[test]
    fn relative_submodule_urls_resolve_against_the_superproject() {
        let idf = "https://github.com/espressif/esp-idf";
        assert_eq!(
            resolve(idf, "../../espressif/esp-phy-lib.git"),
            "https://github.com/espressif/esp-phy-lib"
        );
        assert_eq!(
            resolve(idf, "https://github.com/espressif/esp-coex-lib.git"),
            "https://github.com/espressif/esp-coex-lib"
        );
        assert_eq!(
            normalize("https://github.com/espressif/esp-phy-lib/"),
            "https://github.com/espressif/esp-phy-lib"
        );
    }

    #[test]
    fn each_tree_installs_its_own_tools() {
        let tools = Path::new("/cache/idf-tools");
        let marker = |tree: &str| tools_marker(tools, Path::new(tree), "chip-b").unwrap();
        assert_ne!(marker("/cache/abc-01"), marker("/cache/abc-02"));
        assert_eq!(marker("/cache/abc-01"), marker("/cache/abc-01"));
        assert_ne!(
            marker("/cache/abc-01"),
            tools_marker(tools, Path::new("/cache/abc-01"), "chip-a").unwrap()
        );
    }

    #[test]
    fn a_failed_build_of_another_tree_is_reconfigured() {
        let directory = tempfile::tempdir().unwrap();
        let tree = Path::new("/cache/abc-02");
        assert!(!configured_by_another_tree(directory.path(), "abc", tree));
        // A build that failed left no record, only the tree that configured it.
        std::fs::write(directory.path().join(CONFIGURED_TREE), "/cache/abc-01").unwrap();
        assert!(configured_by_another_tree(directory.path(), "abc", tree));
        std::fs::write(directory.path().join(CONFIGURED_TREE), "/cache/abc-02").unwrap();
        assert!(!configured_by_another_tree(directory.path(), "abc", tree));
    }

    #[test]
    fn a_previous_build_reports_its_idf_revision_and_tree() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(built_tree(directory.path()), None);
        let record = Build {
            chip: "chip-a".into(),
            project: "calibration".into(),
            idf_revision: "abc".into(),
            idf_tree: Some("/cache/abc-01".into()),
            overrides: vec![],
            application: String::new(),
            application_sha256: String::new(),
            elf: String::new(),
            recipe: String::new(),
        };
        std::fs::write(
            directory.path().join(BUILD_RECORD),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert_eq!(
            built_tree(directory.path()),
            Some(("abc".into(), Some("/cache/abc-01".into())))
        );
        // A record from before the cache names no tree, so its build is redone.
        std::fs::write(
            directory.path().join(BUILD_RECORD),
            r#"{"chip":"chip-a","project":"p","idf_revision":"abc","overrides":[],"application":"","application_sha256":"","elf":"","recipe":"r"}"#,
        )
        .unwrap();
        assert_eq!(built_tree(directory.path()), Some(("abc".into(), None)));
    }

    #[test]
    fn a_build_of_another_recipe_drops_the_record_and_the_sdkconfig_first() {
        let output = tempfile::tempdir().unwrap();
        let path = |name: &str| output.path().join(name);
        configure_recipe(output.path(), "old").unwrap();
        std::fs::write(path("sdkconfig"), "old").unwrap();
        std::fs::write(path(BUILD_RECORD), "{}").unwrap();
        // The same recipe keeps the sdkconfig; its record is rewritten after the build.
        configure_recipe(output.path(), "old").unwrap();
        assert!(path("sdkconfig").is_file());
        assert!(!path(BUILD_RECORD).exists());
        // Another recipe starts the sdkconfig afresh, and when its build
        // fails and the sources roll back, no record confirms the outputs.
        std::fs::write(path(BUILD_RECORD), "{}").unwrap();
        configure_recipe(output.path(), "new").unwrap();
        assert!(!path("sdkconfig").exists() && !path(BUILD_RECORD).exists());
        std::fs::write(path("sdkconfig"), "new").unwrap();
        configure_recipe(output.path(), "old").unwrap();
        assert!(!path("sdkconfig").exists());
    }

    #[test]
    fn trees_with_different_pins_never_share_a_directory() {
        let pin = |id: &str, revision: &str| GitPin {
            id: id.into(),
            repository: format!("https://example.invalid/{id}"),
            revision: revision.into(),
            artifacts: vec![],
        };
        let base = [pin("esp-idf", "aa"), pin("esp32-wifi-lib", "bb")];
        let name = tree_name(&base, "aa");
        assert!(name.starts_with("aa-"));
        let reordered = [pin("esp32-wifi-lib", "bb"), pin("esp-idf", "aa")];
        assert_eq!(tree_name(&reordered, "aa"), name);
        let changed = [pin("esp-idf", "aa"), pin("esp32-wifi-lib", "cc")];
        assert_ne!(tree_name(&changed, "aa"), name);
    }

    #[test]
    fn a_recipe_follows_the_project_files_and_the_pinned_tree() {
        let pin = |id: &str, revision: &str| GitPin {
            id: id.into(),
            repository: format!("https://example.invalid/{id}"),
            revision: revision.into(),
            artifacts: vec![],
        };
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("main")).unwrap();
        std::fs::write(project.path().join("main/main.c"), "a").unwrap();
        let pins = [pin("esp-idf", "aa")];
        let recipe = project_recipe(&pins, project.path()).unwrap();
        assert_eq!(project_recipe(&pins, project.path()).unwrap(), recipe);
        assert_ne!(
            project_recipe(&[pin("esp-idf", "bb")], project.path()).unwrap(),
            recipe
        );
        std::fs::write(project.path().join("main/main.c"), "b").unwrap();
        assert_ne!(project_recipe(&pins, project.path()).unwrap(), recipe);
        assert!(project_recipe(&[pin("esp32-wifi-lib", "cc")], project.path()).is_err());
    }

    #[test]
    fn only_differing_archives_named_like_pins_are_unpinned() {
        let directory = tempfile::tempdir().unwrap();
        let tree = directory.path();
        for chip in ["chip-a", "chip-b"] {
            std::fs::create_dir_all(tree.join(chip)).unwrap();
        }
        std::fs::write(tree.join("chip-a/libphy.a"), b"pinned").unwrap();
        std::fs::write(tree.join("chip-a/libbtbb.a"), b"other").unwrap();
        std::fs::write(tree.join("chip-b/libphy.a"), b"another chip").unwrap();
        std::fs::write(tree.join("libmain.a"), b"local").unwrap();
        std::fs::create_dir_all(tree.join("chip-a-bt-lib")).unwrap();
        std::fs::write(tree.join("chip-a-bt-lib/libble_app.a"), b"substituted").unwrap();
        let digest = |bytes: &[u8]| {
            let path = tree.join("digest");
            std::fs::write(&path, bytes).unwrap();
            oer_durable::sha256_file(&path).unwrap()
        };
        let pins = [GitPin {
            id: "esp-phy-lib".into(),
            repository: String::new(),
            revision: String::new(),
            artifacts: vec![
                ("chip-a/libphy.a".into(), digest(b"pinned")),
                ("chip-a/libbtbb.a".into(), digest(b"pinned")),
                // A pin without a directory matches by file name.
                ("libble_app.a".into(), digest(b"pinned")),
            ],
        }];
        let map = format!(
            "LOAD {0}/chip-a/libphy.a\n {0}/chip-a/libbtbb.a(x.o)\n{0}/chip-b/libphy.a\n\
             {0}/libmain.a\n{0}/chip-a-bt-lib/libble_app.a\n",
            tree.display()
        );
        let unpinned = unpinned_archives(&map, tree, &pins).unwrap();
        assert_eq!(
            unpinned,
            [
                format!("{}/chip-a-bt-lib/libble_app.a", tree.display()),
                format!("{}/chip-a/libbtbb.a", tree.display()),
            ]
        );
    }
}
