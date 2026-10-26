//! Architectural instructions have one owner; entry/trap ABI assembly is separate.
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};

// Only these crates/files may own assembly outside the instruction API.
const ABI_ASSEMBLY: &[(&str, &[&str])] = &[
    (
        "chips/espressif/staged-bootstrap",
        &["src/entry.rs", "src/runtime.rs"],
    ),
    (
        "chips/espressif/staged-runtime",
        &["src/entry.rs", "src/stacks.rs"],
    ),
];

// These source inputs are compiled by the host analysis tests, not linked
// into firmware. Assembly inside string literals needs no exception.
const HOST_COMPILER_FIXTURES: &[&str] = &[
    "tools/analysis/cli/tests/fixtures/argument_summaries.rs",
    "tools/analysis/cli/tests/fixtures/call_results.rs",
    "tools/analysis/cli/tests/fixtures/read_uses.rs",
    "tools/analysis/cli/tests/fixtures/unknown_stack.rs",
];

fn allowed(package: &str, file: &str, instruction: &str) -> bool {
    if instruction == "asm" {
        return package == "portable/riscv";
    }
    ABI_ASSEMBLY
        .iter()
        .any(|(owner, files)| package == *owner && files.contains(&file))
        || package.starts_with("firmware/bootstrap/")
        || package.starts_with("firmware/probes/")
}

fn assembly(tokens: TokenStream, found: &mut Vec<String>) {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    for pair in tokens.windows(2) {
        if let [TokenTree::Ident(name), TokenTree::Punct(bang)] = pair
            && bang.as_char() == '!'
            && ["asm", "global_asm", "naked_asm"].contains(&name.to_string().as_str())
        {
            found.push(name.to_string());
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            assembly(group.stream(), found);
        }
    }
}

fn sources(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source tree is readable") {
        let entry = entry.expect("source entry is readable");
        let path = entry.path();
        if path.is_dir() {
            if !["target", ".git"].contains(&entry.file_name().to_string_lossy().as_ref()) {
                sources(&path, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn violations(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    sources(root, &mut files);
    let mut violations = Vec::new();
    for path in files {
        let relative = path.strip_prefix(root).expect("source is below root");
        if HOST_COMPILER_FIXTURES.contains(&relative.to_string_lossy().as_ref()) {
            continue;
        }
        let owner = path
            .ancestors()
            .skip(1)
            .find(|directory| directory.join("Cargo.toml").is_file())
            .expect("each source has a package manifest");
        let package = owner.strip_prefix(root).expect("package is below root");
        let file = path.strip_prefix(owner).expect("source is below package");
        let text = std::fs::read_to_string(&path).expect("source is readable");
        if !text.contains("asm") {
            continue;
        }
        let tokens = text.parse().expect("Rust source tokenizes");
        let mut found = Vec::new();
        assembly(tokens, &mut found);
        for instruction in found {
            if !allowed(
                &package.to_string_lossy(),
                &file.to_string_lossy(),
                &instruction,
            ) {
                violations.push(format!("{}: {instruction}!", path.display()));
            }
        }
    }
    violations.sort();
    violations
}

fn assert_instruction_boundary(root: &Path) {
    assert_eq!(
        violations(root),
        Vec::<String>::new(),
        "assembly instruction ownership"
    );
}

#[test]
fn architectural_instructions_stay_in_oer_riscv() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    assert_instruction_boundary(&root);
}

#[test]
#[should_panic(expected = "assembly instruction ownership")]
fn a_new_crate_with_asm_is_rejected_even_below_the_unsafe_boundary() {
    let root = tempfile::tempdir().expect("temporary source tree");
    let package = root.path().join("firmware/probes/unreviewed");
    std::fs::create_dir_all(package.join("src")).expect("fixture directory");
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = 'unreviewed'\n",
    )
    .expect("fixture manifest");
    let source = package.join("src/lib.rs");
    std::fs::write(
        &source,
        "pub fn wait() { unsafe { core::arch::asm!(\"wfi\"); } }",
    )
    .expect("fixture source");
    assert_instruction_boundary(root.path());
}

#[test]
fn inline_assembly_in_integration_tests_is_rejected() {
    for package in [
        "portable/unreviewed",
        "firmware/probes/unreviewed",
        "tools/analysis/cli",
    ] {
        let root = tempfile::tempdir().expect("temporary source tree");
        let package = root.path().join(package);
        std::fs::create_dir_all(package.join("tests")).expect("fixture directory");
        std::fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = 'unreviewed'\n",
        )
        .expect("fixture manifest");
        let source = package.join("tests/instructions.rs");
        std::fs::write(
            &source,
            "pub fn wait() { unsafe { core::arch::asm!(\"wfi\"); } }",
        )
        .expect("fixture source");
        assert_eq!(
            violations(root.path()),
            [format!("{}: asm!", source.display())]
        );
    }
}

#[test]
fn only_listed_host_compiler_fixtures_are_skipped() {
    let root = tempfile::tempdir().expect("temporary source tree");
    let package = root.path().join("tools/analysis/cli");
    std::fs::create_dir_all(package.join("tests/fixtures")).expect("fixture directory");
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = 'unreviewed'\n",
    )
    .expect("fixture manifest");
    for fixture in HOST_COMPILER_FIXTURES {
        std::fs::write(
            root.path().join(fixture),
            "core::arch::global_asm!(\"ret\");",
        )
        .expect("fixture source");
    }
    assert_instruction_boundary(root.path());
    let source = package.join("tests/fixtures/unreviewed.rs");
    std::fs::write(&source, "core::arch::global_asm!(\"ret\");").expect("fixture source");
    assert_eq!(
        violations(root.path()),
        [format!("{}: global_asm!", source.display())]
    );
}

#[test]
fn abi_exceptions_do_not_allow_inline_operations_or_other_files() {
    assert!(allowed("portable/riscv", "src/instructions.rs", "asm"));
    assert!(allowed(
        "chips/espressif/staged-runtime",
        "src/stacks.rs",
        "global_asm"
    ));
    assert!(!allowed(
        "chips/espressif/staged-runtime",
        "src/stacks.rs",
        "asm"
    ));
    assert!(!allowed(
        "chips/espressif/staged-runtime",
        "src/other.rs",
        "naked_asm"
    ));
    assert!(!allowed("portable/mmio", "src/lib.rs", "asm"));
    let mut found = Vec::new();
    assembly(
        "// asm!(\"wfi\");\nconst EXAMPLE: &str = r#\"asm!(\"wfi\");\"#;"
            .parse()
            .expect("comments and strings tokenize"),
        &mut found,
    );
    assert!(
        found.is_empty(),
        "assembly text in host fixtures is not code"
    );
}
