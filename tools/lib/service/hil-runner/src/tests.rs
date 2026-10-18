//! Tests that need this executable's embedded build record.
/// Register this test executable's build identity, as `main` does.
pub(crate) fn register() {
    let _ = oer_tool_service_hil_run_bundle::run::register_runner(
        oer_tool_service_hil_run_bundle::run::RunnerBuild {
            record: crate::RUNNER_BUILD,
            package: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        },
    );
}

#[test]
fn runner_identity_comes_from_executable_and_embedded_build() {
    register();
    let provenance = oer_tool_service_hil_run_bundle::run::runner_provenance().unwrap();
    let observer = provenance.observer.unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(
        observer["executable_sha256"],
        oer_tool_base_durable::sha256_file(std::path::Path::new("/proc/self/exe")).unwrap()
    );
    let embedded: serde_json::Value = serde_json::from_str(crate::RUNNER_BUILD).unwrap();
    assert_eq!(observer["build"], embedded);
    assert!(observer["build"]["inputs"]["hil/host/link/src/reboot.rs"].is_string());
}
