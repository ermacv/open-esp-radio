use super::*;

#[test]
fn only_the_missing_package_is_invalidated() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let release = directory.path();
    let fingerprints = release.join(".fingerprint");
    for name in [
        "fixture-0123456789abcdef",
        "fixture-1111111111111111",
        "fixture-other-0123456789abcdef",
        "external-0123456789abcdef",
    ] {
        fs::create_dir_all(fingerprints.join(name)).expect("fingerprint");
    }
    let collector = Collector {
        packages: [(
            "fixture".into(),
            Package {
                id: "fixture".into(),
                name: "fixture".into(),
                manifest_path: release.join("Cargo.toml"),
            },
        )]
        .into(),
        release: release.into(),
        driver_id: "current".into(),
        limit: 4096,
    };
    collector
        .invalidate(&[("fixture".into(), "0123456789abcdef".into())].into())
        .expect("invalidate one package");
    assert!(!fingerprints.join("fixture-0123456789abcdef").exists());
    assert!(fingerprints.join("fixture-other-0123456789abcdef").exists());
    assert!(fingerprints.join("external-0123456789abcdef").exists());
    assert!(fingerprints.join("fixture-1111111111111111").exists());
}

#[test]
fn bin_reports_follow_the_elf_when_cargo_uplifts_a_cached_feature_variant() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let root = directory.path();
    let manifest = root.join("Cargo.toml");
    fs::write(&manifest, "fixture").expect("manifest");
    let release = root.join("firmware");
    fs::create_dir_all(release.join("deps")).expect("artifact directory");
    let binary = release.join("fixture");
    let collector = Collector {
        packages: BTreeMap::new(),
        release: release.clone(),
        driver_id: "current".into(),
        limit: 4096,
    };
    for (hash, bytes) in [
        ("aaaaaaaaaaaaaaaa", b"same-data"),
        ("bbbbbbbbbbbbbbbb", b"same-data"),
    ] {
        let hashed = release.join(format!("deps/fixture-{hash}"));
        fs::write(&hashed, bytes).expect("ELF stand-in");
        if binary.exists() {
            fs::remove_file(&binary).expect("remove prior uplift");
        }
        fs::hard_link(hashed, &binary).expect("Cargo uplift");
        let facts = serde_json::json!({"schema":2,"driver_id":"current","manifest":manifest,
            "metadata_hash":hash,"max_move_bytes":4096,"records":[],"binary_sha256":oer_os_durable::sha256_file(&binary).expect("digest")});
        fs::write(
            release.join(format!("deps/fixture-{hash}.large-values.json")),
            facts.to_string(),
        )
        .expect("variant facts");
    }
    let artifact: Artifact = serde_json::from_value(serde_json::json!({"reason":"compiler-artifact", "target":{"name":"fixture","kind":["bin"]},"executable":binary})).expect("artifact");
    for (hash, bytes) in [
        ("aaaaaaaaaaaaaaaa", b"same-data"),
        ("bbbbbbbbbbbbbbbb", b"same-data"),
    ] {
        fs::remove_file(&binary).expect("remove prior uplift");
        fs::hard_link(release.join(format!("deps/fixture-{hash}")), &binary).expect("Cargo uplift");
        assert_eq!(fs::read(&binary).expect("uplift bytes"), bytes);
        assert_eq!(
            collector
                .facts(&artifact, &manifest)
                .expect("matching facts")["metadata_hash"],
            hash
        );
    }
}

#[test]
fn a_plain_cargo_cache_is_repaired_and_then_produces_identical_reports() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let root = directory.path();
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let binary =
        crate::compiler::build_host_tool(&source_root, &root.join("driver"), BINARY, BINARY)
            .expect("build the pinned compiler driver in the test's own directory");
    for (path, contents) in [
        (
            "Cargo.toml",
            "[workspace]\nmembers=['app','helper','macros']\nexclude=['external']\nresolver='3'\n[profile.firmware]\ninherits='release'\npanic='abort'\n",
        ),
        (
            "app/Cargo.toml",
            "[package]\nname='app'\nversion='0.1.0'\nedition='2024'\n[dependencies]\nhelper={path='../helper'}\nexternal={path='../external'}\nmacros={path='../macros'}\n",
        ),
        (
            "app/src/lib.rs",
            "#![no_std]\n#[inline(never)] pub fn run(value: &[u8;8192]) { helper::small(); external::generic(value); }",
        ),
        (
            "helper/Cargo.toml",
            "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "helper/src/lib.rs",
            "#![no_std]\n#[inline(never)] pub fn small() { core::hint::black_box(7); }",
        ),
        ("helper/build.rs", "fn main() {}"),
        (
            "external/Cargo.toml",
            "[package]\nname='external'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "external/src/lib.rs",
            "#![no_std]\n#[inline(never)] pub fn generic<T:Copy>(value: &T) { let copied=*value; core::hint::black_box(&copied); }",
        ),
        (
            "macros/Cargo.toml",
            "[package]\nname='macros'\nversion='0.1.0'\nedition='2024'\n[lib]\nproc-macro=true\n",
        ),
        ("macros/src/lib.rs", "extern crate proc_macro;"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        fs::write(path, contents).expect("fixture source");
    }
    let cache = root.join("cache");
    let target = "riscv32imafc-unknown-none-elf";
    let mut command = crate::cargo::command();
    command
        .current_dir(root)
        .args([
            "build",
            "--offline",
            "-p",
            "app",
            "--profile",
            "firmware",
            "--target",
            target,
        ])
        .env("CARGO_TARGET_DIR", &cache)
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    let log = BuildLog::create(&root.join("build.log")).expect("fixture log");
    log.run(&mut command, "plain compiler cache")
        .expect("ordinary rustc build");
    let manifests: Vec<_> = workspace(root)
        .expect("metadata")
        .into_iter()
        .map(|package| package.manifest_path)
        .collect();
    command
        .env("RUSTC_WRAPPER", &binary)
        .env("OER_LARGE_VALUES_TARGET", target)
        .env("OER_LARGE_VALUES_LIMIT", "4096")
        .env(
            "OER_LARGE_VALUES_WORKSPACE",
            serde_json::to_string(&manifests).expect("workspace JSON"),
        )
        .env(
            "OER_LARGE_VALUES_DRIVER_ID",
            oer_os_durable::sha256_file(&binary).expect("driver digest"),
        );
    let mut first = build(
        &mut command,
        &log,
        root,
        &cache,
        target,
        "firmware",
        "runtime",
        root,
    )
    .expect("recover missing reports");
    let checked: Vec<_> = first
        .crates
        .iter()
        .filter(|coverage| coverage.status == "checked")
        .collect();
    assert_eq!(checked.len(), 2);
    assert!(
        first
            .crates
            .iter()
            .any(|coverage| coverage.status == "outside workspace (ordinary rustc)")
    );
    assert!(
        first
            .crates
            .iter()
            .any(|coverage| coverage.status == "host (ordinary rustc)")
    );
    assert!(checked.iter().any(|coverage| {
        coverage.facts.as_ref().expect("checked facts")["records"]
            .as_array()
            .expect("records")
            .iter()
            .any(|record| record["instance"] == "external::generic::<[u8; 8192]>")
    }));
    let mut second = build(
        &mut command,
        &log,
        root,
        &cache,
        target,
        "firmware",
        "runtime",
        root,
    )
    .expect("Fresh reports");
    assert_eq!(
        serde_json::to_value(&first).expect("first JSON"),
        serde_json::to_value(&second).expect("second JSON")
    );
    first.publish(root).expect("image reports");
    let second_output = root.join("second-report");
    fs::create_dir(&second_output).expect("second output");
    second.publish(&second_output).expect("Fresh image reports");
    for name in ["large-values.json", "large-values.txt"] {
        assert_eq!(
            fs::read(root.join(name)).expect("first report"),
            fs::read(second_output.join(name)).expect("Fresh report")
        );
    }
    fn report_files(directory: &Path) -> usize {
        fs::read_dir(directory)
            .expect("cache entries")
            .map(|entry| {
                let path = entry.expect("cache entry").path();
                if path.is_dir() {
                    report_files(&path)
                } else {
                    usize::from(path.to_string_lossy().ends_with(SUFFIX))
                }
            })
            .sum()
    }
    // The explicit metadata filter excludes the external dependency even
    // though its source is below the same root, and excludes host units.
    assert_eq!(report_files(&cache), 2);
    let messages = fs::read(root.join("runtime-compiler-artifacts.jsonl")).expect("Cargo messages");
    for line in messages
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: Value = serde_json::from_slice(line).expect("Cargo JSON");
        if value["reason"] == "compiler-artifact" {
            assert_eq!(value["fresh"], true);
        }
    }
    // A later ordinary build can reuse the same metadata hash after the
    // source changes. Existing sidecars must not certify the newer unit.
    fs::write(
        root.join("app/src/lib.rs"),
        "#![no_std]\n#[inline(never)] pub fn run() { helper::small(); }",
    )
    .expect("changed source");
    let mut plain = crate::cargo::command();
    plain
        .current_dir(root)
        .args([
            "build",
            "--offline",
            "-p",
            "app",
            "--profile",
            "firmware",
            "--target",
            target,
        ])
        .env("CARGO_TARGET_DIR", &cache)
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    log.run(&mut plain, "ordinary rebuild after source change")
        .expect("ordinary rebuild");
    let repaired = build(
        &mut command,
        &log,
        root,
        &cache,
        target,
        "firmware",
        "runtime",
        root,
    )
    .expect("repair older facts");
    assert!(
        repaired
            .crates
            .iter()
            .filter_map(|coverage| coverage.facts.as_ref())
            .all(|facts| facts["records"].as_array().expect("records").is_empty())
    );
}

#[test]
fn a_primary_rlib_and_staticlib_uplift_needs_no_separate_rmeta() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let release = directory.path();
    let deps = release.join("deps");
    fs::create_dir(&deps).expect("deps");
    let manifest = release.join("Cargo.toml");
    fs::write(&manifest, "fixture").expect("manifest");
    for extension in ["rlib", "a"] {
        let hashed = deps.join(format!("libfixture-0123456789abcdef.{extension}"));
        fs::write(&hashed, b"library").expect("library");
        fs::hard_link(hashed, release.join(format!("libfixture.{extension}"))).expect("uplift");
    }
    let facts = serde_json::json!({"schema":2,"driver_id":"current","manifest":manifest,
        "metadata_hash":"0123456789abcdef","max_move_bytes":4096,"records":[]});
    fs::write(
        deps.join("fixture-0123456789abcdef.large-values.json"),
        facts.to_string(),
    )
    .expect("facts");
    let artifact:Artifact = serde_json::from_value(serde_json::json!({"reason":"compiler-artifact",
        "target":{"name":"fixture","kind":["rlib","staticlib"]},"filenames":[release.join("libfixture.rlib"),release.join("libfixture.a")]})).expect("artifact");
    let collector = Collector {
        packages: BTreeMap::new(),
        release: release.into(),
        driver_id: "current".into(),
        limit: 4096,
    };
    assert_eq!(
        collector
            .facts(&artifact, &manifest)
            .expect("uplifted facts"),
        facts
    );
}
