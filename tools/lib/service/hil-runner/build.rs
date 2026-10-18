//! Embed this executable's host source/build identity; never recapture it at run time.
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

fn file(root: &Path, path: &Path, inputs: &mut BTreeMap<String, String>) {
    println!("cargo:rerun-if-changed={}", path.display());
    let relative = path
        .strip_prefix(root)
        .expect("host build input outside repository");
    inputs.insert(
        relative.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
    );
}
fn tree(root: &Path, path: &Path, inputs: &mut BTreeMap<String, String>) {
    println!("cargo:rerun-if-changed={}", path.display());
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            tree(root, &entry.path(), inputs);
        } else if entry.file_type().unwrap().is_file() {
            file(root, &entry.path(), inputs);
        } else {
            panic!("host source input must be a regular file or directory");
        }
    }
}
fn package(
    root: &Path,
    path: &Path,
    seen: &mut BTreeSet<PathBuf>,
    inputs: &mut BTreeMap<String, String>,
) {
    let path = path.canonicalize().unwrap();
    assert!(
        path.starts_with(root),
        "host dependency is outside repository"
    );
    if !seen.insert(path.clone()) {
        return;
    }
    let manifest = path.join("Cargo.toml");
    file(root, &manifest, inputs);
    if path.join("src").is_dir() {
        tree(root, &path.join("src"), inputs);
    }
    if path.join("build.rs").is_file() {
        file(root, &path.join("build.rs"), inputs);
    }
    let value: serde_json::Value =
        toml_edit::de::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
    // Repository files a package compiles in from outside its directory
    // (`#[path]` modules, `include_str!`) are its declared inputs.
    for input in value["package"]["metadata"]["open-radio"]["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|input| !input.contains('*'))
    {
        let input = root.join(input);
        if input.is_dir() {
            tree(root, &input, inputs);
        } else {
            file(root, &input, inputs);
        }
    }
    for section in std::iter::once(&value).chain(
        value["target"]
            .as_object()
            .into_iter()
            .flat_map(|o| o.values()),
    ) {
        for kind in ["dependencies", "build-dependencies"] {
            for dependency in section[kind]
                .as_object()
                .into_iter()
                .flat_map(|o| o.values())
            {
                if let Some(relative) = dependency["path"].as_str() {
                    package(root, &path.join(relative), seen, inputs);
                }
            }
        }
    }
}
use oer_hil_observer::resolve as observer_resolve;
use oer_hil_run_bundle_format::observer::inputs as observer_build;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../..").canonicalize().unwrap();
    let mut inputs = BTreeMap::new();
    package(&root, &manifest, &mut BTreeSet::new(), &mut inputs);
    for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        file(&root, &root.join(path), &mut inputs);
    }
    tree(&root, &root.join("hil/schema"), &mut inputs);
    if root.join(".cargo").is_dir() {
        tree(&root, &root.join(".cargo"), &mut inputs);
    }
    let compiler = oer_process::command(env::var_os("RUSTC").unwrap())
        .arg("-vV")
        .output()
        .unwrap();
    assert!(compiler.status.success());
    let mut environment = BTreeMap::new();
    for key in [
        "TARGET",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        environment.insert(key.to_owned(), env::var(key).unwrap_or_default());
    }
    environment.extend(env::vars().filter(|(key, _)| key.starts_with("CARGO_FEATURE_")));
    let resolved = observer_resolve::resolve(&root, &environment["TARGET"])
        .expect("resolve executed observer dependencies");
    let registry: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("hil/schema/observer-inputs.json")).unwrap())
            .unwrap();
    observer_build::check_registry_schema(&registry).expect("supported observer input registry");
    observer_build::validate_registry(&resolved, &registry)
        .expect("every observer dependency has a scope");
    for kind in observer_build::workloads(&registry).unwrap() {
        observer_build::projection(
            &resolved,
            &observer_build::dependencies(&registry, &kind).unwrap(),
        )
        .expect("validate observer dependency scope");
    }
    let build = serde_json::json!({"schema":oer_hil_run_bundle_format::observer::BUILD_SCHEMA,"inputs":inputs,"compiler":String::from_utf8(compiler.stdout).unwrap(),"environment":environment,"resolved":resolved});
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("runner-build.json"),
        serde_json::to_vec(&build).unwrap(),
    )
    .unwrap();
}
