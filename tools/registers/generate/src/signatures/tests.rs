use super::*;
use std::path::Path;

fn put(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture(header: &str) -> (tempfile::TempDir, Options) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("components/soc/esp32s31")).unwrap();
    put(
        root.path(),
        "components/common/include/esp_err.h",
        "typedef int esp_err_t;\n",
    );
    put(root.path(), "components/api/include/api.h", header);
    put(
        root.path(),
        "sdkconfig.h",
        "#define CONFIG_IDF_TARGET_ESP32S31 1\n",
    );
    let options = Options {
        chip: "esp32s31".into(),
        idf: root.path().to_owned(),
        sdkconfig: root.path().join("sdkconfig.h"),
        libc_include: None,
        clang: "clang".into(),
        check: false,
        output: None,
    };
    (root, options)
}

fn inspect(options: &Options, names: &[&str], output: &Path) -> headers::Findings {
    headers::inspect(
        options,
        &names.iter().map(|s| (*s).to_owned()).collect(),
        "synthetic-pin",
        output,
    )
    .unwrap()
}

#[test]
fn compiler_resolves_typedefs_pointers_and_rv32_wide_argument_words() {
    let (root, options) = fixture(
        "typedef unsigned long long wide_t;\nvoid empty(void);\nesp_err_t scalar(unsigned char value, const void *address);\nwide_t wide(void *p, wide_t value);\nvoid many(int a,int b,int c,int d,int e,int f,int g,wide_t h);\nvoid (*callback(void))(int);\nvoid adjusted(unsigned char bytes[6], int callback(int));\n",
    );
    let r = inspect(
        &options,
        &["empty", "scalar", "wide", "many", "callback", "adjusted"],
        &root.path().join("output"),
    );
    assert!(r.conflicts.is_empty());
    let contracts: BTreeMap<_, _> = r
        .signatures
        .iter()
        .map(|(n, s)| (n.as_str(), (s.arguments, s.result.as_str())))
        .collect();
    assert_eq!(
        contracts,
        BTreeMap::from([
            ("empty", (0, "void")),
            ("scalar", (2, "u32")),
            ("wide", (3, "u64")),
            ("many", (8, "void")),
            ("callback", (0, "u32")),
            ("adjusted", (2, "void"))
        ])
    );
    assert_eq!(
        r.signatures["wide"].source,
        "esp-idf@synthetic-pin/components/api/include/api.h:4"
    );
    assert_eq!(
        r.signatures["wide"].sha256,
        oer_os_durable::sha256_file(&root.path().join("components/api/include/api.h")).unwrap()
    );
}

#[test]
fn unsupported_abis_are_reported_and_never_become_contracts() {
    let (root, options) = fixture(
        "struct pair { int a,b; };\nstruct pair aggregate(struct pair value);\nfloat floating(double value);\nvoid variadic(int first,...);\nint old_style();\nvoid supported(int value);\n",
    );
    let r = inspect(
        &options,
        &[
            "aggregate",
            "floating",
            "variadic",
            "old_style",
            "supported",
        ],
        &root.path().join("output"),
    );
    assert_eq!(
        r.signatures.keys().map(String::as_str).collect::<Vec<_>>(),
        ["supported"]
    );
    for (name, reason) in [
        ("aggregate", "aggregate"),
        ("floating", "floating"),
        ("variadic", "variadic"),
        ("old_style", "prototype"),
    ] {
        assert!(
            r.unsupported[name].iter().any(|r| r.contains(reason)),
            "{name}: {:?}",
            r.unsupported
        );
    }
}

#[test]
fn source_locations_survive_includes_and_macro_expansions() {
    let (root, options) = fixture(
        "#include \"dependency.h\"\n#define DECLARE(name) void name(unsigned value)\nDECLARE(expanded);\nvoid after(void);\n",
    );
    put(
        root.path(),
        "components/api/include/dependency.h",
        "typedef unsigned short small;\nvoid dependency(small value);\n",
    );
    let r = inspect(
        &options,
        &["dependency", "expanded", "after"],
        &root.path().join("output"),
    );
    assert_eq!(
        r.signatures["dependency"].source,
        "esp-idf@synthetic-pin/components/api/include/dependency.h:2"
    );
    assert_eq!(
        r.signatures["expanded"].source,
        "esp-idf@synthetic-pin/components/api/include/api.h:3"
    );
    assert_eq!(
        r.signatures["after"].source,
        "esp-idf@synthetic-pin/components/api/include/api.h:4"
    );
}

#[test]
fn chip_configuration_selects_the_declaration_and_other_chip_headers_are_excluded() {
    let (root, options) = fixture(
        "#if CONFIG_IDF_TARGET_ESP32S31\nvoid selected(int value);\n#else\nvoid selected(void);\n#endif\n",
    );
    put(
        root.path(),
        "components/soc/esp32c5/include/other.h",
        "void selected(void);\n",
    );
    let r = inspect(&options, &["selected"], &root.path().join("output"));
    assert_eq!(r.signatures["selected"].arguments, 1);
    assert!(r.conflicts.is_empty());
}

#[test]
fn conflicting_header_contracts_are_not_silently_resolved() {
    let (root, options) = fixture("void conflicting(void);\n");
    put(
        root.path(),
        "components/api/include/other.h",
        "void conflicting(int value);\n",
    );
    let r = inspect(&options, &["conflicting"], &root.path().join("output"));
    assert_eq!(r.conflicts["conflicting"].len(), 2);
}

#[test]
fn rejected_header_context_supplies_no_recovery_based_facts() {
    let (root, options) =
        fixture("missing_type invalid(missing_type value);\nvoid apparently_valid(void);\n");
    let r = inspect(
        &options,
        &["invalid", "apparently_valid"],
        &root.path().join("output"),
    );
    assert!(r.signatures.is_empty());
    assert!(!r.failed_candidates["apparently_valid"].is_empty());
}

#[test]
fn enum_widths_come_from_the_compiler_and_unsupported_redeclarations_conflict() {
    let (root, options) = fixture(
        "enum small { SMALL = 1 };\nenum wide { WIDE = 0x100000000ULL };\nenum wide enumeration(enum small a, enum wide b);\nvoid conflicting(int a);\n",
    );
    put(
        root.path(),
        "components/api/include/other.h",
        "void conflicting(float a);\n",
    );
    let r = inspect(
        &options,
        &["enumeration", "conflicting"],
        &root.path().join("output"),
    );
    assert_eq!(r.signatures["enumeration"].arguments, 3);
    assert_eq!(r.signatures["enumeration"].result, "u64");
    assert!(!r.conflicts["conflicting"].is_empty());
}

#[test]
fn modified_header_cannot_be_published_under_its_old_pin() {
    let (root, _) = fixture("void original(void);\n");
    let repo = root.path();
    oer_os_process::git::run(repo, ["init", "--quiet"]).unwrap();
    oer_os_process::git::run(repo, ["add", "."]).unwrap();
    oer_os_process::git::run(
        repo,
        [
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    )
    .unwrap();
    let pin = oer_os_process::git::text(repo, ["rev-parse", "HEAD"]).unwrap();
    let header = "components/api/include/api.h";
    let hashes = BTreeMap::from([(
        header.to_owned(),
        oer_os_durable::sha256_file(&repo.join(header)).unwrap(),
    )]);
    source::verify(repo, &hashes, &pin).unwrap();
    put(repo, header, "void changed(int a);\n");
    let dirty = BTreeMap::from([(
        header.to_owned(),
        oer_os_durable::sha256_file(&repo.join(header)).unwrap(),
    )]);
    assert!(
        source::verify(repo, &dirty, &pin)
            .unwrap_err()
            .to_string()
            .contains("pinned committed bytes")
    );
}
