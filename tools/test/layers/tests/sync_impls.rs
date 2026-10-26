//! Unsafe Sync/Send implementations belong to the memory core.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

// Exact lexical counts, including tests: remove or reduce an entry when its
// implementations move to oer-memory. No new exceptions belong here.
const EXCEPTIONS: &[(&str, usize)] = &[
    ("chips/esp32s31/hal/src/ieee80211/dma/rx_storage.rs", 2),
    ("chips/espressif/executor/src/task.rs", 1),
    ("chips/espressif/executor/src/time_driver.rs", 1),
    ("portable/interrupt-table/src/signal.rs", 2),
    ("portable/interrupt-table/src/signal/tests.rs", 2),
];

fn sync_or_send_trait<'a>(tokens: impl Iterator<Item = &'a TokenTree>) -> bool {
    let mut angles = 0_usize;
    let mut previous: Option<&TokenTree> = None;
    for token in tokens {
        match token {
            TokenTree::Ident(name) if angles == 0 && name == "for" => {
                return matches!(previous, Some(TokenTree::Ident(name)) if ["Sync", "Send", "r#Sync", "r#Send"].contains(&name.to_string().as_str()));
            }
            TokenTree::Group(group) if angles == 0 && group.delimiter() == Delimiter::Brace => {
                return false;
            }
            TokenTree::Punct(punct) => match punct.as_char() {
                '<' => angles += 1,
                '>' if !matches!(previous, Some(TokenTree::Punct(punct)) if punct.as_char() == '-') =>
                {
                    angles = angles.saturating_sub(1);
                }
                ';' if angles == 0 => return false,
                _ => {}
            },
            _ => {}
        }
        previous = Some(token);
    }
    false
}

fn implementations(tokens: TokenStream) -> usize {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    let mut count = 0;
    for (index, pair) in tokens.windows(2).enumerate() {
        if let [TokenTree::Ident(qualifier), TokenTree::Ident(keyword)] = pair
            && qualifier == "unsafe"
            && keyword == "impl"
            && sync_or_send_trait(tokens.iter().skip(index + 2))
        {
            count += 1;
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            count += implementations(group.stream());
        }
    }
    count
}

fn sources(directory: &Path, excluded: &[PathBuf], files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source tree is readable") {
        let entry = entry.expect("source entry is readable");
        let path = entry.path();
        if path.is_dir() {
            if !["target", ".git"].contains(&entry.file_name().to_string_lossy().as_ref())
                && !excluded.contains(&path)
            {
                sources(&path, excluded, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn violations(root: &Path, exceptions: &[(&str, usize)]) -> Vec<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
    let manifest: toml::Value = toml::from_str(&manifest).expect("manifest parses");
    let excluded = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("exclude"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|path| root.join(path.as_str().expect("excluded package path")))
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    sources(root, &excluded, &mut files);
    let mut actual = BTreeMap::new();
    for path in files {
        let owner = path
            .ancestors()
            .skip(1)
            .find(|directory| directory.join("Cargo.toml").is_file())
            .expect("each source has a package manifest");
        if owner.strip_prefix(root).expect("package is below root") == Path::new("portable/memory")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source is readable");
        if !text.contains("unsafe") {
            continue;
        }
        let count = implementations(text.parse().expect("Rust source tokenizes"));
        if count != 0 {
            let relative = path.strip_prefix(root).expect("source is below root");
            actual.insert(relative.to_string_lossy().into_owned(), count);
        }
    }
    let mut violations = Vec::new();
    for &(file, expected) in exceptions {
        let count = actual.remove(file).unwrap_or_default();
        if count != expected || expected == 0 {
            violations.push(format!(
                "{file}: expected {expected} unsafe Sync/Send impls, found {count}"
            ));
        }
    }
    for (file, count) in actual {
        violations.push(format!("{file}: {count} unlisted unsafe Sync/Send impls"));
    }
    violations.sort();
    violations
}

fn assert_sync_impl_boundary(root: &Path, exceptions: &[(&str, usize)]) {
    assert_eq!(
        violations(root, exceptions),
        Vec::<String>::new(),
        "unsafe Sync/Send implementation ownership"
    );
}

fn fixture(package: &str, file: &str, source: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary source tree");
    std::fs::write(root.path().join("Cargo.toml"), "[workspace]\n").expect("workspace manifest");
    let package = root.path().join(package);
    std::fs::create_dir_all(&package).expect("fixture package");
    std::fs::write(package.join("Cargo.toml"), "[package]\nname = 'fixture'\n")
        .expect("fixture manifest");
    let path = package.join(file);
    std::fs::create_dir_all(path.parent().expect("source directory")).expect("fixture directory");
    std::fs::write(path, source).expect("fixture source");
    root
}

#[test]
fn unsafe_sync_and_send_impls_stay_in_oer_memory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    assert_sync_impl_boundary(&root, EXCEPTIONS);
}

#[test]
#[should_panic(expected = "unsafe Sync/Send implementation ownership")]
fn a_new_crate_with_unsafe_sync_is_rejected_even_below_the_unsafe_boundary() {
    let root = fixture(
        "firmware/probes/unreviewed",
        "src/lib.rs",
        "struct Cell; unsafe impl Sync for Cell {}",
    );
    assert_sync_impl_boundary(root.path(), &[]);
}

#[test]
fn exception_counts_must_match_including_removed_files_and_zero_entries() {
    let file = "portable/interrupt-table/src/signal.rs";
    let root = fixture(
        "portable/interrupt-table",
        "src/signal.rs",
        "struct Cell; unsafe impl Sync for Cell {}",
    );
    assert_sync_impl_boundary(root.path(), &[(file, 1)]);
    for expected in [0, 2] {
        assert_eq!(
            violations(root.path(), &[(file, expected)]),
            [format!(
                "{file}: expected {expected} unsafe Sync/Send impls, found 1"
            )]
        );
    }
    std::fs::write(root.path().join(file), "struct Cell;").expect("remove implementation");
    assert_eq!(
        violations(root.path(), &[(file, 1)]),
        [format!(
            "{file}: expected 1 unsafe Sync/Send impls, found 0"
        )]
    );
    std::fs::remove_file(root.path().join(file)).expect("remove source");
    assert_eq!(
        violations(root.path(), &[(file, 1)]),
        [format!(
            "{file}: expected 1 unsafe Sync/Send impls, found 0"
        )]
    );
    assert!(!violations(root.path(), &[(file, 0)]).is_empty());
}

#[test]
fn an_exception_does_not_allow_more_impls_or_another_file() {
    let file = "portable/interrupt-table/src/signal.rs";
    let root = fixture(
        "portable/interrupt-table",
        "src/signal.rs",
        "struct Cell; unsafe impl Sync for Cell {} unsafe impl Send for Cell {}",
    );
    assert_eq!(
        violations(root.path(), &[(file, 1)]),
        [format!(
            "{file}: expected 1 unsafe Sync/Send impls, found 2"
        )]
    );
    std::fs::rename(
        root.path().join(file),
        root.path().join("portable/interrupt-table/src/other.rs"),
    )
    .expect("move source");
    assert_eq!(violations(root.path(), &[(file, 2)]).len(), 2);
}

#[test]
fn memory_ownership_does_not_extend_to_other_packages_even_in_tests() {
    for package in [
        "portable/memory",
        "portable/memory-extra",
        "portable/memory/nested",
    ] {
        for file in ["src/lib.rs", "tests/sharing.rs"] {
            let root = fixture(package, file, "struct Cell; unsafe impl Send for Cell {}");
            assert_eq!(
                violations(root.path(), &[]).is_empty(),
                package == "portable/memory"
            );
        }
    }
}

#[test]
fn excluded_c5_packages_are_not_scanned() {
    let root = fixture(
        "chips/esp32c5/hal",
        "src/lib.rs",
        "unsafe impl Sync for Cell {}",
    );
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nexclude = ['chips/esp32c5/hal']\n",
    )
    .expect("workspace excludes");
    assert_sync_impl_boundary(root.path(), &[]);
}

#[test]
fn tokens_find_qualified_multiline_and_macro_impls_without_counting_text_or_bounds() {
    let source = r##"
        // unsafe impl Sync for Comment {}
        /* unsafe impl Send for Comment {} */
        const EXAMPLE: &str = r#"unsafe impl Sync for String {}"#;
        const EXAMPLE2: &str = "unsafe impl Send for String {}";
        unsafe impl<T: for<'a> Fn(&'a ()) -> &'a (), const N: usize>
            ::core::marker::Sync for Cell<T, N> {}
        unsafe impl<T: Bound<{ 1 }>> std::marker::Send for Cell<T> {}
        macro_rules! cell {
            () => { unsafe impl<T> Sync for Cell<T> {} };
        }
        #[cfg(test)] mod tests { unsafe impl Send for TestCell {} }
        unsafe impl core::marker::r#Sync for RawIdentCell {}
        unsafe impl r#Send for RawIdentCell {}
        unsafe impl<T: Sync + Send> Other for Cell<T> {}
        impl<T: Sync> Other for Cell<T> {}
        impl Sync for Cell {}
    "##;
    assert_eq!(
        implementations(source.parse().expect("fixture tokenizes")),
        6
    );
}
