//! Runs a crate's `tests/ui` twins through trybuild.
//!
//! Each case `tests/ui/<case>.rs` is one twin file: as written it must
//! compile; with `ui_reject` set it must fail, and every rejected line names
//! its expected error in a marker: a code, `// ui-error: E0499`, or for an
//! error rustc reports without one its exact message,
//! `// ui-error: lifetime may not live long enough`. The error's primary
//! location must be the marker's line; a marker on another line names the
//! offset explicitly, `// ui-error@-1: E0185`. An error rustc locates only
//! outside the case, such as a failed const evaluation, is marked
//! `// ui-error@outside: E0080: <its whole message>`: the code and message
//! then name it, as no line can. trybuild drops
//! `RUSTFLAGS` and passes only `--cfg trybuild`, so it cannot set `ui_reject`
//! per case. Both twins are therefore generated copies in the target
//! directory with the cfg rewritten; the case sources stay untouched. The
//! checked-in snapshot `tests/ui/<case>.stderr` belongs to the reject twin and
//! must report every marked error at its line.
//!
//! A case whose guarantee holds only in some feature sets declares it,
//! `// ui-requires: not(test-support)`; under any other set it is skipped and
//! named on stderr, so another CI run must check it.
//!
//! `ui_reject` needs no check-cfg registration: case files are no target of
//! any ordinary build, and both generated copies have the cfg rewritten.

#![forbid(unsafe_code)]
#![allow(
    clippy::panic,
    clippy::expect_used,
    reason = "a test runner reports a broken case by panicking with its name"
)]

use std::{
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

const MARKER: &str = "// ui-error";
const REQUIRES: &str = "// ui-requires:";
const CFG: &str = "ui_reject";
/// How a checked-in snapshot names the case file.
const CASE_DIR: &str = "tests/ui/";
/// rustc's location prefixes; trybuild keeps the absolute path after them for
/// a file in the target directory.
const LOCATIONS: [&str; 2] = ["--> ", "::: "];

/// Runs every case of the crate at `manifest_dir`, in file-name order.
///
/// `target_tmpdir` receives the generated twins; an integration test passes
/// `env!("CARGO_TARGET_TMPDIR")`. `features` names every feature a case may
/// require and whether this build has it, `("test-support",
/// cfg!(feature = "test-support"))`. With `TRYBUILD=overwrite` the produced
/// snapshots are copied back to `tests/ui/`.
///
/// # Panics
///
/// When a case has no marker or no `ui_reject`, when a case requires a
/// feature `features` does not name, when trybuild reports a
/// failure, or when a snapshot misses a marked error at its line.
pub fn run(manifest_dir: &str, target_tmpdir: &str, features: &[(&str, bool)]) {
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

    let mut cases = Vec::new();
    for source in case_paths(&ui) {
        match Case::prepare(source, &twins, features) {
            Ok(case) => cases.push(case),
            // Written past the test harness's capture, so a skip always shows.
            Err(skipped) => {
                let _ = writeln!(std::io::stderr(), "ui-cases: skipped {skipped}");
            }
        }
    }

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

/// One case: its generated twins, checked-in snapshot and the errors its
/// markers expect.
struct Case {
    name: String,
    pass: PathBuf,
    reject: PathBuf,
    snapshot: PathBuf,
    expected: Vec<Expected>,
}

/// An error a marker expects, at the place its primary location names.
#[derive(Debug, PartialEq, Eq)]
struct Expected {
    place: Place,
    error: ExpectedError,
}

/// Where an expected error's primary location lies.
#[derive(Debug, PartialEq, Eq)]
enum Place {
    /// This line of the case.
    Line(usize),
    /// Outside the case (`@outside`).
    Outside,
}

#[derive(Debug, PartialEq, Eq)]
enum ExpectedError {
    /// `error[E0499]: …`
    Code(String),
    /// `error[E0080]: <message>`, the whole header.
    CodeAndMessage(String, String),
    /// `error: <message>`, for an error rustc reports without a code.
    Message(String),
}

impl ExpectedError {
    fn matches(&self, header: &str) -> bool {
        match self {
            Self::Code(code) => header.starts_with(&format!("error[{code}]")),
            Self::CodeAndMessage(..) | Self::Message(_) => header == self.to_string(),
        }
    }
}

impl core::fmt::Display for ExpectedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Code(code) => write!(f, "error[{code}]"),
            Self::CodeAndMessage(code, message) => write!(f, "error[{code}]: {message}"),
            Self::Message(message) => write!(f, "error: {message}"),
        }
    }
}

impl Case {
    /// The case at `source_path`, or `Err` naming it and the requirement this
    /// build does not meet.
    fn prepare(
        source_path: PathBuf,
        twins: &Twins,
        features: &[(&str, bool)],
    ) -> Result<Self, String> {
        let name = file_name(&source_path);
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("{name}: read the case: {error}"));
        let expected = markers(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        if !source.contains(CFG) {
            panic!("{name}: no `{CFG}`, so the case has no reject twin");
        }
        for requirement in requirements(&source) {
            let met = requirement
                .met(features)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            if !met {
                return Err(format!("{name}: requires {}", requirement.text));
            }
        }

        // `cfg(any())` is always false and `cfg(all())` always true.
        let directory = source_path
            .parent()
            .expect("a case lies in tests/ui")
            .to_string_lossy()
            .into_owned();
        let pass = twins.pass.join(&name);
        let reject = twins.reject.join(&name);
        for (path, cfg) in [(&pass, "any()"), (&reject, "all()")] {
            fs::write(path, twin(&source, cfg, &directory))
                .unwrap_or_else(|error| panic!("{name}: write {}: {error}", path.display()));
        }

        let snapshot = source_path.with_extension("stderr");
        if snapshot.exists() {
            let checked_in = read(&snapshot);
            let generated = relocate(&checked_in, CASE_DIR, &twins.reject_dir());
            write(&reject.with_extension("stderr"), &generated);
        }
        Ok(Case {
            name,
            pass,
            reject,
            snapshot,
            expected,
        })
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
        let reported = reported_errors(&snapshot, &format!("{CASE_DIR}{}", self.name));
        for expected in &self.expected {
            if let Err(error) = expected.check(&reported) {
                panic!("{}: {error}", self.name);
            }
        }
    }
}

/// A twin of `source` with `ui_reject` replaced by `cfg`.
///
/// The twin lies in the target directory, so a relative `include!` path is
/// made absolute under `directory`, the case's own. trybuild builds every
/// test as a binary, and the cases are library-shaped `no_std` code: the twin
/// links std for its panic handler and gains an empty `main`. Both follow the
/// last line, so the line numbers in a snapshot are those of the case.
fn twin(source: &str, cfg: &str, directory: &str) -> String {
    let mut twin = source.replace(CFG, cfg);
    for include in ["include!(\"", "include_str!(\"", "include_bytes!(\""] {
        twin = absolute_includes(&twin, include, directory);
    }
    if source.contains("#![no_std]") {
        twin.push_str("\nextern crate std;");
    }
    twin.push_str("\nfn main() {}\n");
    twin
}

/// `source` with each relative path after `include` resolved under
/// `directory`.
fn absolute_includes(source: &str, include: &str, directory: &str) -> String {
    let mut parts = source.split(include);
    let mut resolved = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        resolved.push_str(include);
        if !part.starts_with('/') {
            resolved.push_str(directory);
            resolved.push('/');
        }
        resolved.push_str(part);
    }
    resolved
}

/// A `// ui-requires: <feature>` or `// ui-requires: not(<feature>)` line.
#[derive(Debug, PartialEq, Eq)]
struct Requirement {
    text: String,
    feature: String,
    enabled: bool,
}

impl Requirement {
    /// Whether this build meets the requirement; an error when `features`
    /// does not name its feature.
    fn met(&self, features: &[(&str, bool)]) -> Result<bool, String> {
        features
            .iter()
            .find(|(feature, _)| *feature == self.feature)
            .map(|(_, on)| *on == self.enabled)
            .ok_or_else(|| {
                format!(
                    "`{REQUIRES} {}` names a feature the crate's tests/ui.rs does not pass",
                    self.text
                )
            })
    }
}

/// Every requirement of `source`, in source order.
fn requirements(source: &str) -> Vec<Requirement> {
    source
        .lines()
        .filter_map(|line| line.split_once(REQUIRES))
        .map(|(_, text)| {
            let text = text.trim();
            let (feature, enabled) = match text
                .strip_prefix("not(")
                .and_then(|inner| inner.strip_suffix(')'))
            {
                Some(feature) => (feature, false),
                None => (text, true),
            };
            Requirement {
                text: text.to_owned(),
                feature: feature.trim().to_owned(),
                enabled,
            }
        })
        .collect()
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

impl Expected {
    /// Whether `reported` holds this error at this place.
    fn check(&self, reported: &[(String, Option<usize>)]) -> Result<(), String> {
        let mut elsewhere = Vec::new();
        for (header, line) in reported {
            if !self.error.matches(header) {
                continue;
            }
            match (&self.place, line) {
                (Place::Line(expected), Some(line)) if line == expected => return Ok(()),
                (Place::Outside, None) => return Ok(()),
                (_, Some(line)) => elsewhere.push(format!("line {line}")),
                (_, None) => elsewhere.push("outside the case".to_owned()),
            }
        }
        let place = match self.place {
            Place::Line(line) => format!("line {line}"),
            Place::Outside => "outside the case".to_owned(),
        };
        if elsewhere.is_empty() {
            Err(format!(
                "the snapshot has no {} that the marker for {place} expects",
                self.error
            ))
        } else {
            Err(format!(
                "{} is reported at {}, but the marker expects {place}",
                self.error,
                elsewhere.join(", ")
            ))
        }
    }
}

/// The error of every `// ui-error[@±N | @outside]: <CODE | message>` marker,
/// in source order.
fn markers(source: &str) -> Result<Vec<Expected>, String> {
    let mut expected = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some((_, rest)) = line.split_once(MARKER) else {
            continue;
        };
        let number = index + 1;
        let Some((offset, error)) = rest.split_once(':') else {
            return Err(format!("line {number}: `{MARKER}` needs `:` and an error"));
        };
        let place = match offset {
            "" => Some(Place::Line(number)),
            "@outside" => Some(Place::Outside),
            offset => offset
                .strip_prefix('@')
                .and_then(|offset| offset.parse::<isize>().ok())
                .and_then(|offset| number.checked_add_signed(offset))
                .filter(|&target| target != number)
                .map(Place::Line),
        }
        .ok_or_else(|| {
            format!(
                "line {number}: `{MARKER}{offset}` is no offset like `@-1`, `@+2` or `@outside`"
            )
        })?;
        let error =
            expected_error(error.trim()).map_err(|reason| format!("line {number}: {reason}"))?;
        if place == Place::Outside && matches!(error, ExpectedError::Code(_)) {
            return Err(format!(
                "line {number}: `@outside` names no line, so it needs the code and its whole \
                 message, `E0080: <message>`"
            ));
        }
        expected.push(Expected { place, error });
    }
    if expected.is_empty() {
        return Err(format!("no `{MARKER}: <CODE | message>` marker"));
    }
    Ok(expected)
}

/// A code `E` and four digits, optionally followed by `: <message>`, or else
/// an error message.
fn expected_error(error: &str) -> Result<ExpectedError, String> {
    if error.is_empty() {
        return Err(format!("`{MARKER}` names no error"));
    }
    let (head, message) = match error.split_once(':') {
        Some((head, message)) => (head, Some(message.trim())),
        None => (error, None),
    };
    let code = head.strip_prefix('E').is_some_and(|digits| {
        digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    Ok(match (code, message) {
        (true, None) => ExpectedError::Code(head.to_owned()),
        (true, Some("")) => return Err(format!("`{head}:` names no message")),
        (true, Some(message)) => ExpectedError::CodeAndMessage(head.to_owned(), message.to_owned()),
        (false, _) => ExpectedError::Message(error.to_owned()),
    })
}

/// Each error of `stderr` with the line of its primary location in `case`,
/// if that location is in the case.
fn reported_errors(stderr: &str, case: &str) -> Vec<(String, Option<usize>)> {
    let prefix = format!("--> {case}:");
    let mut reported: Vec<(String, Option<usize>)> = Vec::new();
    let mut open = false;
    for line in stderr.lines() {
        if line.starts_with("error") && !line.starts_with("error: aborting due to") {
            reported.push((line.to_owned(), None));
            open = true;
        } else if line.starts_with("warning") {
            open = false;
        } else if open && let Some(location) = line.trim_start().strip_prefix(&prefix) {
            if let Some(last) = reported.last_mut() {
                last.1 = location
                    .split(':')
                    .next()
                    .and_then(|number| number.parse().ok());
            }
            open = false;
        } else if open && line.trim_start().starts_with("--> ") {
            open = false;
        }
    }
    reported
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
    use super::{
        Expected, ExpectedError, Place, absolute_includes, markers, relocate, reported_errors,
        requirements, twin,
    };

    fn code(line: usize, code: &str) -> Expected {
        Expected {
            place: Place::Line(line),
            error: ExpectedError::Code(code.into()),
        }
    }

    fn message(line: usize, message: &str) -> Expected {
        Expected {
            place: Place::Line(line),
            error: ExpectedError::Message(message.into()),
        }
    }

    fn outside(code: &str, message: &str) -> Expected {
        Expected {
            place: Place::Outside,
            error: ExpectedError::CodeAndMessage(code.into(), message.into()),
        }
    }

    #[test]
    fn markers_yield_their_errors_and_places_in_order() {
        let source = "let a = x; // ui-error: E0499\nlet b = y; // ui-error: E0382\n\
                      fn f() {} // ui-error: lifetime may not live long enough\n\
                      const A: u8 = f(); // ui-error@outside: E0080: evaluation panicked: no\n";
        assert_eq!(
            markers(source),
            Ok(vec![
                code(1, "E0499"),
                code(2, "E0382"),
                message(3, "lifetime may not live long enough"),
                outside("E0080", "evaluation panicked: no"),
            ])
        );
    }

    #[test]
    fn an_explicit_offset_moves_the_expected_line() {
        let source = "\n\nfn f( // ui-error@-1: E0185\n// ui-error@+2: E0382\n";
        assert_eq!(
            markers(source),
            Ok(vec![code(2, "E0185"), code(6, "E0382")])
        );
    }

    #[test]
    fn a_case_without_a_marker_is_an_error() {
        assert!(markers("#[cfg(ui_reject)]\nlet a = x;\n").is_err());
    }

    #[test]
    fn a_malformed_marker_is_an_error() {
        for source in [
            "let a = x; // ui-error:\n",
            "let a = x; // ui-error E0499\n",
            "let a = x; // ui-error@x: E0499\n",
            "let a = x; // ui-error@+0: E0499\n",
            "let a = x; // ui-error@-5: E0499\n",
            "let a = x; // ui-error: E0499:\n",
            // Outside the case no line names the error, so the code alone
            // does not.
            "const A: u8 = f(); // ui-error@outside: E0080\n",
        ] {
            assert!(markers(source).is_err(), "{source}");
        }
    }

    #[test]
    fn twins_rewrite_the_cfg_and_keep_line_numbers() {
        let source =
            "#![no_std]\n#[cfg(ui_reject)]\nfn a() {}\n#[cfg(not(ui_reject))]\nfn b() {}\n";
        let reject = twin(source, "all()", "/c");
        assert!(reject.starts_with(
            "#![no_std]\n#[cfg(all())]\nfn a() {}\n#[cfg(not(all()))]\nfn b() {}\n\n"
        ));
        assert!(reject.ends_with("extern crate std;\nfn main() {}\n"));
        assert!(twin(source, "any()", "/c").contains("#[cfg(any())]\nfn a() {}\n"));
    }

    #[test]
    fn a_std_case_gets_no_second_std() {
        assert_eq!(
            twin("#[cfg(ui_reject)]\nfn a() {}\n", "all()", "/c"),
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

    const SNAPSHOT: &str = "\
warning: unused variable: `a`
 --> tests/ui/c.rs:2:9
  |
error[E0499]: cannot borrow `x` as mutable more than once at a time
 --> tests/ui/c.rs:7:18
  |
error: lifetime may not live long enough
  --> tests/ui/c.rs:12:5
   |
error[E0308]: mismatched types
 --> src/lib.rs:3:1
  |
error: aborting due to 3 previous errors
";

    #[test]
    fn errors_pair_with_their_primary_location_in_the_case() {
        assert_eq!(
            reported_errors(SNAPSHOT, "tests/ui/c.rs"),
            vec![
                (
                    "error[E0499]: cannot borrow `x` as mutable more than once at a time".into(),
                    Some(7)
                ),
                ("error: lifetime may not live long enough".into(), Some(12)),
                ("error[E0308]: mismatched types".into(), None),
            ]
        );
    }

    #[test]
    fn an_expected_error_must_sit_at_its_place() {
        let reported = reported_errors(SNAPSHOT, "tests/ui/c.rs");
        assert_eq!(code(7, "E0499").check(&reported), Ok(()));
        assert_eq!(
            message(12, "lifetime may not live long enough").check(&reported),
            Ok(())
        );
        assert_eq!(
            outside("E0308", "mismatched types").check(&reported),
            Ok(())
        );
        assert!(
            code(8, "E0499")
                .check(&reported)
                .unwrap_err()
                .contains("at line 7")
        );
        assert!(
            code(7, "E0382")
                .check(&reported)
                .unwrap_err()
                .contains("no error[E0382]")
        );
        assert!(
            code(3, "E0308")
                .check(&reported)
                .unwrap_err()
                .contains("outside the case")
        );
        // A message names the whole header, not a prefix of it.
        assert!(message(12, "lifetime").check(&reported).is_err());
        assert!(outside("E0308", "mismatched").check(&reported).is_err());
        let inside = outside(
            "E0499",
            "cannot borrow `x` as mutable more than once at a time",
        );
        assert!(inside.check(&reported).unwrap_err().contains("at line 7"));
    }

    #[test]
    fn relative_includes_resolve_under_the_case_directory() {
        let source = "include!(\"../a.rs\");\ninclude!(\"/abs/b.rs\");\n";
        assert_eq!(
            absolute_includes(source, "include!(\"", "/x/tests/ui"),
            "include!(\"/x/tests/ui/../a.rs\");\ninclude!(\"/abs/b.rs\");\n"
        );
    }

    #[test]
    fn requirements_name_a_feature_and_whether_it_must_be_on() {
        let found = requirements("// ui-requires: not(test-support)\n// ui-requires: serde\n");
        let features = [("test-support", true), ("serde", true)];
        assert_eq!(found[0].met(&features), Ok(false));
        assert_eq!(found[1].met(&features), Ok(true));
        assert_eq!(found[0].met(&[("test-support", false)]), Ok(true));
        assert!(found[1].met(&[("test-support", false)]).is_err());
    }
}

mod chip;
pub use chip::run_chip;
