use super::*;
use std::path::Path;

/// The test thread's case: the test's own name.
fn case() -> &'static cases::Case {
    let name = std::thread::current()
        .name()
        .and_then(|name| name.rsplit("::").next())
        .expect("a test thread names its test")
        .to_owned();
    cases::case(&name).unwrap_or_else(|| panic!("{name} is no case"))
}

/// The test's case written under a temporary root. No test runs the
/// compiler: an inspection reads the case's recorded stages.
fn fixture() -> (tempfile::TempDir, Options) {
    let root = tempfile::tempdir().unwrap();
    let options = cases::materialize(case(), root.path(), "/nonexistent/clang".as_ref()).unwrap();
    (root, options)
}

/// The inspection of the case's headers, each compiler stage answered by
/// its recorded stage (`fixtures/<case>/<header>.<stage>.json`), whose text
/// and header must be the request's, with the record's `/idf` at the
/// fixture's root.
fn inspect(options: &Options, names: &[&str], output: &Path) -> headers::Findings {
    let case = case();
    assert_eq!(names, case.names, "the test asks for its case's names");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/signatures/fixtures")
        .join(case.name);
    let names: BTreeSet<String> = names.iter().map(|s| (*s).to_owned()).collect();
    let (root, headers) = headers::selection(options).unwrap();
    let mut inspection = headers::Inspection::new(
        &options.chip,
        root.clone(),
        headers,
        &names,
        "synthetic-pin",
        output,
    );
    while let Some(request) = inspection.request().unwrap() {
        let relative = request
            .header
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let path = headers::recorded_path(&fixtures, &relative, request.stage);
        let recorded: headers::RecordedStage = serde_json::from_str(
            &fs::read_to_string(&path).unwrap_or_else(|_| panic!("no record {}", path.display())),
        )
        .unwrap();
        // The record answers this request: the same unit, the same header.
        assert_eq!(
            recorded.extra, request.extra,
            "{}: {relative} {}",
            case.name, request.stage
        );
        assert_eq!(
            recorded.header_sha256,
            oer_tools_os_durable::sha256_file(&request.header).unwrap(),
            "{}: {relative} was recorded from another text",
            case.name
        );
        let tree = match (recorded.tree, recorded.refusal) {
            (Some(tree), None) => Ok(ast::parse(
                serde_json::to_string(&tree)
                    .unwrap()
                    .replace("/idf", &root.to_string_lossy())
                    .as_bytes(),
            )
            .unwrap()),
            (None, Some(reason)) => Err(reason),
            _ => panic!("{}: a record holds a tree or a refusal", path.display()),
        };
        inspection.advance(tree).unwrap();
    }
    inspection.finish()
}

#[test]
fn a_refusal_gives_the_compiler_s_first_error() {
    assert_eq!(
        headers::refusal(
            b"note: in file\n/idf/a.h:1:1: error: unknown type name 'x'\nerror: second\n"
        ),
        "unknown type name 'x'"
    );
    assert_eq!(
        headers::refusal(b"warning only\n"),
        "compiler rejected the header context"
    );
}

#[test]
fn a_location_without_a_file_takes_the_last_one_named() {
    let tree = ast::parse(
        br#"{"inner":[{"loc":{"offset":1,"col":2,"file":"/a.h"}},{"loc":{"offset":3,"col":4}},{"loc":{"offset":5,"col":6,"includedFrom":{"file":"/b.h"}}}]}"#,
    )
    .unwrap();
    let files: Vec<_> = ast::children(&tree)
        .iter()
        .map(|node| node["loc"]["actualFile"].as_str().unwrap())
        .collect();
    // includedFrom names the include parent: the cursor stays on /a.h.
    assert_eq!(files, ["/a.h", "/a.h", "/a.h"]);
}

#[test]
fn compiler_resolves_typedefs_pointers_and_rv32_wide_argument_words() {
    let (root, options) = fixture();
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
        oer_tools_os_durable::sha256_file(&root.path().join("components/api/include/api.h"))
            .unwrap()
    );
}

#[test]
fn unsupported_abis_are_reported_and_never_become_contracts() {
    let (root, options) = fixture();
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
    let (root, options) = fixture();
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
    let (root, options) = fixture();
    let r = inspect(&options, &["selected"], &root.path().join("output"));
    assert_eq!(r.signatures["selected"].arguments, 1);
    assert!(r.conflicts.is_empty());
}

#[test]
fn conflicting_header_contracts_are_not_silently_resolved() {
    let (root, options) = fixture();
    let r = inspect(&options, &["conflicting"], &root.path().join("output"));
    assert_eq!(r.conflicts["conflicting"].len(), 2);
}

#[test]
fn rejected_header_context_supplies_no_recovery_based_facts() {
    let (root, options) = fixture();
    let r = inspect(
        &options,
        &["invalid", "apparently_valid"],
        &root.path().join("output"),
    );
    assert!(r.signatures.is_empty());
    assert!(!r.failed_candidates["apparently_valid"].is_empty());
    // The context's refusal is recorded with the compiler's reason.
    assert_eq!(
        r.failed_contexts
            .get("components/api/include/api.h")
            .map(String::as_str),
        Some("unknown type name 'missing_type'")
    );
}

#[test]
fn enum_widths_come_from_the_compiler_and_unsupported_redeclarations_conflict() {
    let (root, options) = fixture();
    let r = inspect(
        &options,
        &["enumeration", "conflicting"],
        &root.path().join("output"),
    );
    assert_eq!(r.signatures["enumeration"].arguments, 3);
    assert_eq!(r.signatures["enumeration"].result, "u64");
    assert!(!r.conflicts["conflicting"].is_empty());
}
