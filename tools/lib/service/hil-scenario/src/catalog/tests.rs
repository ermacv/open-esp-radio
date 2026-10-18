use super::super::test_family::TestFamily;

type Catalog = super::Catalog<TestFamily>;
use std::{fs, path::PathBuf};

const ALPHA: &str = include_str!("../../../../tests/fixtures/catalog/z-system/alpha-system.toml");
const BETA: &str = include_str!("../../../../tests/fixtures/catalog/a-system/beta-system.toml");
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("oer-runner-catalog-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("catalog")).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, contents: &str) {
        let path = self.0.join("catalog").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn nested_catalog_consumes_shared_serialized_documents() {
    let tree = Tree::new();
    tree.write("z-system/alpha-system.toml", ALPHA);
    tree.write("a-system/beta-system.toml", BETA);
    tree.write("README.md", "catalog documentation");
    tree.write("a-system/README.md", "domain documentation");
    let catalog = Catalog::load(&tree.0.join("catalog")).unwrap();
    assert_eq!(
        catalog
            .all()
            .iter()
            .map(|scenario| scenario.id())
            .collect::<Vec<_>>(),
        ["alpha-system", "beta-system"]
    );
    assert!(
        catalog
            .all()
            .iter()
            .all(|scenario| scenario.repetitions() == 3)
    );
}

#[test]
fn catalog_rejects_ambiguous_or_unsupported_inputs() {
    for (name, contents) in [
        ("other/alpha-system.toml", ALPHA.to_owned()),
        ("wrong-name.toml", BETA.to_owned()),
        ("hidden.txt", "unexpected file".to_owned()),
        ("beta-system.toml", BETA.replace("schema = 5", "schema = 4")),
        (
            "beta-system.toml",
            BETA.replace("repetitions = 3", "repetitions = 0"),
        ),
    ] {
        let tree = Tree::new();
        tree.write("domain/alpha-system.toml", ALPHA);
        tree.write(name, &contents);
        assert!(
            Catalog::load(&tree.0.join("catalog")).is_err(),
            "accepted {name}"
        );
    }
    let tree = Tree::new();
    tree.write("README.md", "no scenarios");
    assert!(Catalog::load(&tree.0.join("catalog")).is_err());
}

#[cfg(unix)]
#[test]
fn catalog_rejects_symlink_files_directories_and_roots() {
    use std::os::unix::fs::symlink;
    for directory in [false, true] {
        let tree = Tree::new();
        tree.write("domain/alpha-system.toml", ALPHA);
        if directory {
            symlink(tree.0.join("catalog/domain"), tree.0.join("catalog/alias")).unwrap();
        } else {
            symlink(
                tree.0.join("catalog/domain/alpha-system.toml"),
                tree.0.join("catalog/alias.toml"),
            )
            .unwrap();
        }
        assert!(Catalog::load(&tree.0.join("catalog")).is_err());
    }
    let tree = Tree::new();
    tree.write("alpha-system.toml", ALPHA);
    fs::rename(tree.0.join("catalog"), tree.0.join("actual")).unwrap();
    symlink(tree.0.join("actual"), tree.0.join("catalog")).unwrap();
    assert!(Catalog::load(&tree.0.join("catalog")).is_err());
}

#[cfg(unix)]
#[test]
fn catalog_rejects_symlink_in_supplied_path_ancestors() {
    let tree = Tree::new();
    let outside = Tree::new();
    outside.write("alpha-system.toml", ALPHA);
    std::os::unix::fs::symlink(&outside.0, tree.0.join("alias")).unwrap();
    assert!(Catalog::load(&tree.0.join("alias/catalog")).is_err());
}

#[test]
fn a_scenario_names_no_chip() {
    let tree = Tree::new();
    tree.write("a-system/beta-system.toml", BETA);
    tree.write(
        "z-system/alpha-system.toml",
        &ALPHA.replacen("\n", "\ntargets = [\"chip-b\"]\n", 1),
    );
    assert!(Catalog::load(&tree.0.join("catalog")).is_err());
}

#[test]
fn a_diagnostic_image_serves_only_an_investigation_scenario() {
    let scenario = |image: &str, role: &str, tags: &str| {
        crate::Scenario::<TestFamily>::from_toml(
            &format!(
                "schema = 5\nid = \"exit\"\ndescription = \"exit\"\nrole = \"{role}\"\n\
                 tags = [{tags}]\n\n[wifi]\nimage = \"{image}\"\n"
            ),
            std::path::Path::new("exit.toml"),
        )
    };
    let image = "diagnostic-usb-jtag-off";
    assert!(scenario(image, "investigation", "").is_ok());
    assert!(scenario(image, "qualification", "").is_err());
    assert!(scenario(image, "investigation", "\"performance\"").is_err());
    assert!(scenario("system-watchdog", "qualification", "").is_ok());
    assert!(scenario("system-watchdog", "investigation", "").is_ok());
    assert!(
        scenario("system-watchdog", "triage", "").is_err(),
        "an unknown role"
    );
}

#[test]
fn a_profile_needs_a_sampling_image_a_period_in_range_and_a_diagnostic_scenario() {
    let document = |image: &str, tags: &str, period: u32| {
        let role = if tags.contains("qualification") {
            "qualification"
        } else {
            "investigation"
        };
        let tags = tags.replace("\"qualification\"", "");
        format!(
            "schema = 5\nid = \"profiled\"\ndescription = \"profiled\"\nrole = \"{role}\"\ntags = [{tags}]\n\
             [profile]\nharts = \"both\"\nperiod-us = {period}\n\
             [wifi]\nimage = \"{image}\"\nworkload = {{ kind = \"x\" }}\n"
        )
    };
    let parse = |text: String| {
        crate::Scenario::<super::super::test_family::TestFamily>::from_toml(
            &text,
            std::path::Path::new("profiled.toml"),
        )
    };
    // No image class samples the program counter, so every profile
    // request is refused.
    assert!(parse(document("diagnostic-usb-jtag-off", "\"diagnostic\"", 1999)).is_err());
    assert!(parse(document("system-watchdog", "\"diagnostic\"", 1999)).is_err());
    assert!(parse(document("diagnostic-usb-jtag-off", "\"diagnostic\"", 50)).is_err());
    for gated in ["\"performance\"", "\"qualification\""] {
        assert!(parse(document("diagnostic-usb-jtag-off", gated, 1999)).is_err());
    }
}
