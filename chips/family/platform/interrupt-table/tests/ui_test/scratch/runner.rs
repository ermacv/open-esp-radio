//! Trial (tasks/h003): the package's converted cases under `ui_test`.
//!
//! `UI_TEST_CASES` names the directory of the cases; `-- --bless` writes the
//! snapshots beside them. `UI_TEST_TWIN=runner` makes every case a twin by
//! the runner's defaults, for cases converted without a header;
//! `UI_TEST_SNAPSHOTS=none` checks the annotations alone.

use std::path::{Path, PathBuf};

use ui_test::{
    Args, Config, Match, custom_flags::edition::Edition, dependencies::DependencyBuilder,
    ignore_output_conflict, run_tests, spanned::Spanned,
};

fn main() -> ui_test::color_eyre::Result<()> {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = std::env::var_os("UI_TEST_CASES")
        .map_or_else(
            || scratch.join("../../chips/family/platform/interrupt-table/tests/ui_test/cases"),
            PathBuf::from,
        )
        .canonicalize()?;
    let workspace = scratch.join("../..").canonicalize()?;
    let package = workspace.join("chips/family/platform/interrupt-table");

    let mut config = Config::rustc(&cases);
    // What `include!` reads is no case of its own.
    config.skip_files.push("/support/".into());
    // The accept revision is the pass twin: `ui-cases` does not read its
    // warnings, and a library-shaped case leaves its constants unused.
    config
        .comment_defaults
        .revisioned
        .entry(vec!["accept".into()])
        .or_default()
        .compile_flags
        .push("-Aunused".into());
    if std::env::var_os("UI_TEST_TWIN").is_some_and(|twin| twin == "runner") {
        // No line in a case: both revisions and what `check-pass` sets.
        config.comment_defaults.revisions = Some(vec!["accept".into(), "ui_reject".into()]);
        let accept = config
            .comment_defaults
            .revisioned
            .entry(vec!["accept".into()])
            .or_default();
        accept.exit_status = Spanned::dummy(0).into();
        accept.require_annotations = Spanned::dummy(false).into();
    }
    if std::env::var_os("UI_TEST_SNAPSHOTS").is_some_and(|snapshots| snapshots == "none") {
        config.output_conflict_handling = ignore_output_conflict;
    }

    let defaults = config.comment_defaults.base();
    defaults.set_custom("edition", Edition("2024".into()));
    defaults.set_custom(
        "dependencies",
        DependencyBuilder {
            crate_manifest_path: scratch.join("deps/Cargo.toml"),
            ..DependencyBuilder::default()
        },
    );
    // A snapshot names no machine and no line of a file that is not the
    // case: the case's directory, the package, the workspace and the
    // toolchain's sources by a name, every gutter's number as `LL` at one
    // width.
    let exact = |path: &Path, name: &str| {
        (
            Match::Exact(path.display().to_string().into_bytes()),
            name.as_bytes().to_vec(),
        )
    };
    defaults.normalize_stderr.push(exact(&cases, "$DIR"));
    defaults.normalize_stderr.push(exact(&package, "$PACKAGE"));
    defaults.normalize_stderr.push(exact(&workspace, "$WORKSPACE"));
    config.stderr_filter(r"[^ ]*/lib/rustlib/src/rust/library", "$$RUST");
    config.stderr_filter(r"(\$(?:RUST|PACKAGE|WORKSPACE)/[^ :]+):\d+:\d+", "$1");
    // rustc writes a long type's name into the output directory and says where.
    config.stderr_filter(r"has been written to '[^']*'", "has been written to '$$LONG_TYPE'");
    config.stderr_filter(r"(?m)^ *\d+ \|", "LL |");
    config.stderr_filter(r"(?m)^\.\.\. *\|", "...");
    config.stderr_filter(r"(?m)^ +\|", "   |");
    config.stderr_filter(r"(?m)^ +(-->|:::) ", "  $1 ");
    config.stderr_filter(r"(?m)^ += ", "   = ");

    config.with_args(&Args::test()?);
    run_tests(config)
}
