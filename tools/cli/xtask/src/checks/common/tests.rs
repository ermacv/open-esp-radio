use super::*;
use std::{collections::BTreeSet, fs};

/// A fixture repository: `files` as `(path, contents)`, read through the
/// repository model.
fn repository(files: &[(&str, &str)]) -> (tempfile::TempDir, oer_repo::Model) {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let model = oer_repo::Model::load(&oer_repo::Repo::from_dir(dir.path()).unwrap()).unwrap();
    (dir, model)
}

const WORKSPACE: (&str, &str) = (
    "Cargo.toml",
    "[workspace]\nmembers = ['libraries/policy', 'crates/dependency']\nexclude = ['island']\n",
);

fn package(name: &str, class: &str) -> String {
    format!(
        "[package]\nname = '{name}'\n[features]\nleft = []\nright = []\n[package.metadata.open-radio]\n{class}\n"
    )
}

#[test]
fn production_and_source_packages_span_every_workspace() {
    let (_dir, model) = repository(&[
        WORKSPACE,
        (
            "libraries/policy/Cargo.toml",
            &package("policy", "layer = 'service'\nplatform = 'portable'"),
        ),
        (
            "crates/dependency/Cargo.toml",
            &package(
                "target-library",
                "layer = 'experiment'\nplatform = 'portable'",
            ),
        ),
        (
            "island/Cargo.toml",
            &format!(
                "{}[workspace]\n",
                package("island", "layer = 'contract'\nplatform = 'portable'")
            ),
        ),
    ]);
    let names = |packages: Vec<Classified>| {
        packages
            .into_iter()
            .map(|package| package.package.name)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        names(production_packages(&model).unwrap()),
        BTreeSet::from(["island".to_owned(), "policy".to_owned()])
    );
    assert_eq!(names(source_packages(&model).unwrap()).len(), 3);
}

#[test]
fn an_unclassified_package_cannot_disappear() {
    let (_dir, model) = repository(&[
        WORKSPACE,
        (
            "libraries/policy/Cargo.toml",
            &package("policy", "layer = 'service'\nplatform = 'portable'"),
        ),
        (
            "crates/dependency/Cargo.toml",
            "[package]\nname = 'target-library'\n",
        ),
    ]);
    let error = production_packages(&model)
        .expect_err("missing classification must fail")
        .to_string();
    assert!(
        error.contains("lacks [package.metadata.open-radio]"),
        "{error}"
    );
}
