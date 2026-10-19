//! The runner build identity the library's tests run with: the executable
//! embeds the real one (`tools/hil-runner`), whose own tests check it.

/// Register a build identity for this test process, as the executable's
/// `main` registers its embedded one.
pub(crate) fn register() {
    let _ = oer_hil_runs::run::register_runner(oer_hil_runs::run::RunnerBuild {
        record: "{}",
        package: "oer-tool-hil-runner",
        version: env!("CARGO_PKG_VERSION"),
    });
}
