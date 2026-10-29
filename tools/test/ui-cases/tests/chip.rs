//! Exercise artifact discovery with a proc macro and several selected packages.
use std::{fs, process::Command};

#[test]
fn chip_twins_resolve_proc_macros_with_a_configured_build_directory() {
    let scratch = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("own compiler scratch");
    let root = scratch.path();
    for directory in [".cargo", "owner", "derive", "other"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    for (path, contents) in [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"owner\", \"derive\", \"other\"]\nresolver = \"3\"\n",
        ),
        (
            ".cargo/config.toml",
            "[build]\nbuild-dir = \"displaced-intermediates\"\n",
        ),
        (
            "owner/Cargo.toml",
            "[package]\nname = \"ui-owner\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[lib]\npath = \"lib.rs\"\n[features]\nchip = []\n[dependencies]\nui-derive = { path = \"../derive\" }\n",
        ),
        (
            "owner/lib.rs",
            "#[derive(ui_derive::Owned)]\npub struct Owner;\n",
        ),
        (
            "derive/Cargo.toml",
            "[package]\nname = \"ui-derive\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[lib]\npath = \"lib.rs\"\nproc-macro = true\n",
        ),
        (
            "derive/lib.rs",
            "extern crate proc_macro;\n#[proc_macro_derive(Owned)]\npub fn owned(_: proc_macro::TokenStream) -> proc_macro::TokenStream { proc_macro::TokenStream::new() }\n",
        ),
        (
            "other/Cargo.toml",
            "[package]\nname = \"ui-other\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[lib]\nname = \"selected_alias\"\npath = \"lib.rs\"\n[features]\nchip = []\n",
        ),
        ("other/lib.rs", "pub fn observe() {}\n"),
        (
            "case.rs",
            "pub fn exercise(owner: ui_owner::Owner) {\n    drop(owner);\n    #[cfg(ui_reject)]\n    drop(owner); // ui-error: E0382\n}\npub use selected_alias::observe;\n",
        ),
    ] {
        fs::write(root.join(path), contents).unwrap();
    }
    let version = Command::new("rustc").arg("-vV").output().unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc reports its host target");
    oer_test_ui_cases::run_chip(
        root,
        &root.join("twins"),
        host,
        &[("ui-owner", "chip"), ("ui-other", "chip")],
        &["case.rs"],
    );
    assert!(root.join("twins/build").is_dir());
    assert!(!root.join("displaced-intermediates").exists());
}
