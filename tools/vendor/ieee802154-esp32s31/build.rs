//! Compile the pinned public ESP-IDF IEEE 802.15.4 driver for the host.
//!
//! Every vendor file is read from the sources `cargo pins fetch`
//! fetched, or from the checkout `OER_ESP_IDF_DIR` names, and must match the
//! SHA-256 pinned in the repository's `artifacts.toml`. The build generates a recording
//! replacement for every `ieee802154_ll_*` accessor declared by the real
//! common LL header, so the compiled driver keeps its own control flow while
//! each register-layer call reaches the Rust recorder.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use oer_tool_pins::Manifest;

const SOURCE_ENV: &str = "OER_ESP_IDF_DIR";
const COMMON_LL: &str = "components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h";

const INCLUDE_DIRS: &[&str] = &[
    "components/ieee802154/include",
    "components/ieee802154/private_include",
    "components/esp_hal_ieee802154/include",
    "components/esp_hal_ieee802154/esp32s31/include",
    "components/soc/esp32s31/include",
    "components/soc/esp32s31/register",
    "components/esp_coex/include",
];

/// The repository's pinned-artifact manifest and the source whose artifacts
/// this stand compiles.
const ARTIFACTS: &str = "../../../chips/esp32s31/vendor/artifacts.toml";
const IDF_SOURCE: &str = "esp-idf";
/// The repository root, relative to this package.
const REPOSITORY: &str = "../../..";
/// Translation units compiled only with a Cargo feature.
const FEATURE_UNITS: &[(&str, &str)] = &[(
    "components/ieee802154/esp_ieee802154_multipan.c",
    "multipan",
)];

struct Source {
    path: String,
    requires: Option<String>,
    sha256: String,
}

/// The pinned ESP-IDF source of the manifest: its fetched directory in the
/// repository at `root` and revision, and its files.
fn ledger(text: &str, root: &Path) -> (PathBuf, String, Vec<Source>) {
    let manifest = Manifest::parse(text).expect("artifact manifest");
    let idf = manifest
        .source(IDF_SOURCE)
        .expect("the manifest pins the ESP-IDF source");
    let revision = idf
        .revision
        .clone()
        .expect("the ESP-IDF source pins a revision");
    let sources = manifest
        .artifact
        .iter()
        .filter(|a| a.source == IDF_SOURCE)
        .map(|a| Source {
            requires: FEATURE_UNITS
                .iter()
                .find(|(unit, _)| *unit == a.path)
                .map(|(_, feature)| (*feature).to_owned()),
            sha256: a
                .sha256
                .clone()
                .expect("the ESP-IDF source pins each file's SHA-256"),
            path: a.path.clone(),
        })
        .collect();
    let directory = manifest
        .source_directory(root, idf)
        .expect("the fetched ESP-IDF directory");
    (directory, revision, sources)
}

struct Accessor {
    returns: String,
    name: String,
    parameters: Vec<(String, String)>,
}

/// Parse the one-line `FORCE_INLINE_ATTR`/`static inline` accessor
/// signatures of the common LL header.
fn accessors(header: &str) -> Vec<Accessor> {
    let mut found = Vec::new();
    for line in header.lines() {
        let Some(signature) = line
            .strip_prefix("FORCE_INLINE_ATTR ")
            .or_else(|| line.strip_prefix("static inline "))
        else {
            continue;
        };
        let (Some(paren), Some(close)) = (signature.find('('), signature.rfind(')')) else {
            continue;
        };
        // The accessor name is the last token before the parameter list; the
        // return type may itself be an `ieee802154_ll_*` typedef.
        let head = signature[..paren].trim_end();
        let split = head.rfind([' ', '*']).expect("accessor return type");
        let name = head[split + 1..].to_owned();
        if !name.starts_with("ieee802154_ll_") {
            continue;
        }
        let returns = head[..=split].trim().to_owned();
        let list = signature[paren + 1..close].trim();
        let parameters = if list == "void" || list.is_empty() {
            Vec::new()
        } else {
            list.split(',')
                .map(|parameter| {
                    let parameter = parameter.trim();
                    let split = parameter.rfind([' ', '*']).expect("typed parameter");
                    (
                        parameter[..=split].trim().to_owned(),
                        parameter[split + 1..].trim().to_owned(),
                    )
                })
                .collect()
        };
        found.push(Accessor {
            returns,
            name,
            parameters,
        });
    }
    assert!(!found.is_empty(), "no LL accessors found in {COMMON_LL}");
    found
}

fn generate(accessors: &[Accessor], out: &Path) -> PathBuf {
    let include = out.join("include");
    fs::create_dir_all(&include).expect("generated include directory");
    let mut rename = String::from("/* Generated: move vendor LL bodies aside. */\n");
    let mut restore = String::from("/* Generated: release the vendor LL names. */\n");
    let mut declarations = String::from(
        "/* Generated: recording LL accessors with the vendor signatures. */\n#pragma once\n",
    );
    let mut bodies = String::from(
        "/* Generated: forward every LL accessor to the Rust recorder. */\n\
         #include <stdint.h>\n#include \"hal/ieee802154_common_ll.h\"\n\
         uint64_t oer_host_record(const char *name, const uint64_t *arguments, uint32_t count,\n                        uint32_t pointers);\n",
    );
    for accessor in accessors {
        let parameters = if accessor.parameters.is_empty() {
            "void".to_owned()
        } else {
            accessor
                .parameters
                .iter()
                .map(|(ty, name)| format!("{ty} {name}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let name = &accessor.name;
        writeln!(rename, "#define {name} oer_vendor_{name}").unwrap();
        writeln!(restore, "#undef {name}").unwrap();
        writeln!(declarations, "{} {name}({parameters});", accessor.returns).unwrap();
        let arguments = accessor
            .parameters
            .iter()
            .map(|(ty, name)| {
                if ty.contains('*') {
                    format!("(uint64_t)(uintptr_t){name}")
                } else {
                    format!("(uint64_t)(int64_t){name}")
                }
            })
            .collect::<Vec<_>>();
        // Bit `n` marks argument `n` as a buffer address, which the recorder
        // labels symbolically so traces do not depend on address layout.
        let pointers = accessor
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, (ty, _))| ty.contains('*'))
            .fold(0u32, |mask, (index, _)| mask | 1 << index);
        let call = if arguments.is_empty() {
            format!("oer_host_record(\"{name}\", 0, 0, 0)")
        } else {
            writeln!(
                bodies,
                "{} {name}({parameters})\n{{\n    uint64_t arguments[{}] = {{ {} }};",
                accessor.returns,
                arguments.len(),
                arguments.join(", ")
            )
            .unwrap();
            format!(
                "oer_host_record(\"{name}\", arguments, {}, {pointers:#x})",
                arguments.len()
            )
        };
        if arguments.is_empty() {
            writeln!(bodies, "{} {name}({parameters})\n{{", accessor.returns).unwrap();
        }
        if accessor.returns == "void" {
            writeln!(bodies, "    (void){call};\n}}").unwrap();
        } else {
            writeln!(bodies, "    return ({}){call};\n}}", accessor.returns).unwrap();
        }
    }
    fs::write(include.join("oer_ll_rename.h"), rename).unwrap();
    fs::write(include.join("oer_ll_restore.h"), restore).unwrap();
    fs::write(include.join("oer_ll_recorder.h"), declarations).unwrap();
    let recorder = out.join("oer_ll_recorder.c");
    fs::write(&recorder, bodies).unwrap();
    recorder
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo::rerun-if-env-changed={SOURCE_ENV}");
    println!("cargo::rerun-if-changed={ARTIFACTS}");
    println!("cargo::rerun-if-changed=shim");

    let (fetched, revision, sources) = ledger(
        &fs::read_to_string(manifest.join(ARTIFACTS)).unwrap(),
        &manifest.join(REPOSITORY),
    );
    // The fetched pinned sources, or an ESP-IDF checkout named explicitly.
    let idf = env::var_os(SOURCE_ENV).map_or(fetched, PathBuf::from);
    for source in &sources {
        let path = idf.join(&source.path);
        println!("cargo::rerun-if-changed={}", path.display());
        let pinned = oer_tool_pins::artifacts::verified(&path, &source.sha256)
            .unwrap_or_else(|error| panic!("{}: {error} (ESP-IDF {revision})", path.display()));
        assert!(
            pinned,
            "{} is missing or does not match ESP-IDF {revision}",
            source.path
        );
    }

    let recorder = generate(
        &accessors(&fs::read_to_string(idf.join(COMMON_LL)).unwrap()),
        &out,
    );

    let mut build = cc::Build::new();
    build
        .std("gnu11")
        .warnings(false)
        .flag("-include")
        .flag("esp_intr_alloc.h")
        .include(out.join("include"))
        .include(manifest.join("shim/include"));
    for dir in INCLUDE_DIRS {
        build.include(idf.join(dir));
    }
    // `multipan` reproduces `CONFIG_IEEE802154_MULTI_PAN_ENABLE=y` with the
    // Kconfig default of two interfaces.
    let multipan = env::var_os("CARGO_FEATURE_MULTIPAN").is_some();
    if multipan {
        build
            .define("CONFIG_IEEE802154_MULTI_PAN_ENABLE", "1")
            .define("CONFIG_IEEE802154_INTERFACE_NUM", "2");
    }
    // `sw-coex` reproduces `CONFIG_ESP_COEX_SW_COEXIST_ENABLE=y`, the default
    // of a build that also enables Wi-Fi or Bluetooth.
    if env::var_os("CARGO_FEATURE_SW_COEX").is_some() {
        build.define("CONFIG_ESP_COEX_SW_COEXIST_ENABLE", "1");
    }
    for source in sources.iter().filter(|source| {
        source.path.ends_with(".c")
            && source
                .requires
                .as_deref()
                .is_none_or(|feature| feature == "multipan" && multipan)
    }) {
        build.file(idf.join(&source.path));
    }
    build
        .file(recorder)
        .file(manifest.join("shim/src/host.c"))
        .compile("oer_esp32s31_ieee802154_vendor");
}
