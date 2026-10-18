//! Runs a crate's `tests/ui` twins through trybuild.
//!
//! Each case `tests/ui/<case>.rs` is one twin file: as written it must
//! compile; with `ui_reject` set it must fail, and every rejected line names
//! its expected error code in a `// ui-error: E0499` marker. trybuild drops
//! `RUSTFLAGS` and passes only `--cfg trybuild`, so it cannot set `ui_reject`
//! per case. Both twins are therefore generated copies in the target
//! directory with the cfg rewritten; the case sources stay untouched. The
//! checked-in snapshot `tests/ui/<case>.stderr` belongs to the reject twin and
//! must report every marked code.
//!
//! `ui_reject` needs no check-cfg registration: case files are no target of
//! any ordinary build, and both generated copies have the cfg rewritten.
#![allow(
    clippy::panic,
    clippy::expect_used,
    reason = "a test runner reports a broken case by panicking with its name"
)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const MARKER: &str = "// ui-error:";
const CFG: &str = "ui_reject";
/// How a checked-in snapshot names the case file.
const CASE_DIR: &str = "tests/ui/";
/// rustc's location prefixes; trybuild keeps the absolute path after them for
/// a file in the target directory.
const LOCATIONS: [&str; 2] = ["--> ", "::: "];

/// Runs every case of the crate at `manifest_dir`, in file-name order.
///
/// `target_tmpdir` receives the generated twins; an integration test passes
/// `env!("CARGO_TARGET_TMPDIR")`. With `TRYBUILD=overwrite` the produced
/// snapshots are copied back to `tests/ui/`.
///
/// # Panics
///
/// When a case has no marker or no `ui_reject`, when trybuild reports a
/// failure, or when a snapshot misses a marked error code.
pub fn run(manifest_dir: &str, target_tmpdir: &str) {
    let ui = Path::new(manifest_dir).join(CASE_DIR);
    let package = env::var("CARGO_PKG_NAME").expect("cargo sets CARGO_PKG_NAME for tests");
    let generated = Path::new(target_tmpdir).join("ui-cases").join(package);
    if generated.exists() {
        fs::remove_dir_all(&generated).expect("remove the stale generated twins");
    }
    let twins = Twins {
        pass: generated.join("pass"),
        reject: generated.join("reject"),
    };
    fs::create_dir_all(&twins.pass).expect("create the pass twin directory");
    fs::create_dir_all(&twins.reject).expect("create the reject twin directory");

    let cases: Vec<Case> = case_paths(&ui)
        .into_iter()
        .map(|source| Case::prepare(source, &twins))
        .collect();

    // A set of compile_fail tests alone builds in one cargo invocation; a set
    // holding a pass test builds and runs each test on its own. Each set runs
    // when dropped and panics on a failure.
    {
        let reject = trybuild::TestCases::new();
        for case in &cases {
            reject.compile_fail(&case.reject);
        }
    }
    {
        let pass = trybuild::TestCases::new();
        for case in &cases {
            pass.pass(&case.pass);
        }
    }

    let overwrite = env::var_os("TRYBUILD").is_some_and(|value| value == "overwrite");
    for case in &cases {
        if overwrite {
            case.copy_back(&twins);
        }
        case.check_snapshot();
    }
}

/// The directories of the generated twins.
struct Twins {
    pass: PathBuf,
    reject: PathBuf,
}

impl Twins {
    /// The reject directory as rustc prints it in a location.
    fn reject_dir(&self) -> String {
        format!("{}/", self.reject.display())
    }
}

/// One case: its generated twins, checked-in snapshot and the error codes its
/// markers expect.
struct Case {
    name: String,
    pass: PathBuf,
    reject: PathBuf,
    snapshot: PathBuf,
    codes: Vec<String>,
}

impl Case {
    fn prepare(source_path: PathBuf, twins: &Twins) -> Self {
        let name = file_name(&source_path);
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("{name}: read the case: {error}"));
        let codes = marker_codes(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        if !source.contains(CFG) {
            panic!("{name}: no `{CFG}`, so the case has no reject twin");
        }

        // `cfg(any())` is always false and `cfg(all())` always true.
        let pass = twins.pass.join(&name);
        let reject = twins.reject.join(&name);
        for (path, cfg) in [(&pass, "any()"), (&reject, "all()")] {
            fs::write(path, twin(&source, cfg))
                .unwrap_or_else(|error| panic!("{name}: write {}: {error}", path.display()));
        }

        let snapshot = source_path.with_extension("stderr");
        if snapshot.exists() {
            let checked_in = read(&snapshot);
            let generated = relocate(&checked_in, CASE_DIR, &twins.reject_dir());
            write(&reject.with_extension("stderr"), &generated);
        }
        Case {
            name,
            pass,
            reject,
            snapshot,
            codes,
        }
    }

    /// Copies the produced snapshot back to `tests/ui/`, naming the case file
    /// by its checked-in path so the snapshot is the same on every machine.
    fn copy_back(&self, twins: &Twins) {
        let produced = read(&self.reject.with_extension("stderr"));
        let snapshot = relocate(&produced, &twins.reject_dir(), CASE_DIR);
        if snapshot.contains("ui-cases/") {
            panic!(
                "{}: the snapshot names the generated twin outside a location",
                self.name
            );
        }
        write(&self.snapshot, &snapshot);
    }

    fn check_snapshot(&self) {
        let snapshot = read(&self.snapshot);
        for code in &self.codes {
            if !snapshot.contains(&format!("error[{code}]")) {
                panic!(
                    "{}: the snapshot has no error[{code}] that its marker expects",
                    self.name
                );
            }
        }
    }
}

/// A twin of `source` with `ui_reject` replaced by `cfg`.
///
/// trybuild builds every test as a binary, and the cases are library-shaped
/// `no_std` code: the twin links std for its panic handler and gains an
/// empty `main`. Both follow the last line, so the line numbers in a snapshot
/// are those of the case.
fn twin(source: &str, cfg: &str) -> String {
    let mut twin = source.replace(CFG, cfg);
    if source.contains("#![no_std]") {
        twin.push_str("\nextern crate std;");
    }
    twin.push_str("\nfn main() {}\n");
    twin
}

/// Moves every location in `stderr` from directory `from` to `to`.
fn relocate(stderr: &str, from: &str, to: &str) -> String {
    let mut relocated = stderr.to_owned();
    for location in LOCATIONS {
        relocated = relocated.replace(&format!("{location}{from}"), &format!("{location}{to}"));
    }
    relocated
}

/// The `tests/ui/*.rs` cases, sorted.
fn case_paths(ui: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(ui).unwrap_or_else(|error| panic!("read {}: {error}", ui.display()));
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read {}: {error}", ui.display()))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        panic!("{} holds no cases", ui.display());
    }
    paths
}

/// The error codes of every `// ui-error: <CODE>` marker, in source order.
fn marker_codes(source: &str) -> Result<Vec<String>, String> {
    let mut codes = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some((_, rest)) = line.split_once(MARKER) else {
            continue;
        };
        let code = rest.trim();
        let digits = code.strip_prefix('E').unwrap_or_default();
        if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "line {}: `{MARKER}` needs one code like E0499, found `{code}`",
                index + 1
            ));
        }
        codes.push(code.to_owned());
    }
    if codes.is_empty() {
        return Err(format!("no `{MARKER} <CODE>` marker"));
    }
    Ok(codes)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn write(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{marker_codes, relocate, twin};

    #[test]
    fn markers_yield_their_codes_in_order() {
        let source = "let a = x; // ui-error: E0499\nlet b = y; // ui-error: E0382\n";
        assert_eq!(
            marker_codes(source),
            Ok(vec!["E0499".into(), "E0382".into()])
        );
    }

    #[test]
    fn a_case_without_a_marker_is_an_error() {
        assert!(marker_codes("#[cfg(ui_reject)]\nlet a = x;\n").is_err());
    }

    #[test]
    fn a_marker_without_a_code_is_an_error() {
        assert!(marker_codes("let a = x; // ui-error:\n").is_err());
        assert!(marker_codes("let a = x; // ui-error: borrow\n").is_err());
    }

    #[test]
    fn twins_rewrite_the_cfg_and_keep_line_numbers() {
        let source =
            "#![no_std]\n#[cfg(ui_reject)]\nfn a() {}\n#[cfg(not(ui_reject))]\nfn b() {}\n";
        let reject = twin(source, "all()");
        assert!(reject.starts_with(
            "#![no_std]\n#[cfg(all())]\nfn a() {}\n#[cfg(not(all()))]\nfn b() {}\n\n"
        ));
        assert!(reject.ends_with("extern crate std;\nfn main() {}\n"));
        assert!(twin(source, "any()").contains("#[cfg(any())]\nfn a() {}\n"));
    }

    #[test]
    fn a_std_case_gets_no_second_std() {
        assert_eq!(
            twin("#[cfg(ui_reject)]\nfn a() {}\n", "all()"),
            "#[cfg(all())]\nfn a() {}\n\nfn main() {}\n"
        );
    }

    #[test]
    fn relocation_moves_only_locations() {
        let stderr = " --> /t/reject/a.rs:7:17\n  ::: /t/reject/b.rs:1:1\nsee /t/reject/a.rs\n";
        assert_eq!(
            relocate(stderr, "/t/reject/", "tests/ui/"),
            " --> tests/ui/a.rs:7:17\n  ::: tests/ui/b.rs:1:1\nsee /t/reject/a.rs\n"
        );
    }
}
