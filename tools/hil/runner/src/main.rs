//! The HIL runner executable (`oer-tool-hil-runner`): the runner of
//! `oer-hil-run` with this executable's embedded host build record.
#![forbid(unsafe_code)]

/// This executable's host build record, embedded by its build script.
const RUNNER_BUILD: &str = include_str!(concat!(env!("OUT_DIR"), "/runner-build.json"));

fn main() {
    oer_hil_run::main(oer_hil_run::Build {
        record: RUNNER_BUILD,
        package: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn runner_identity_comes_from_executable_and_embedded_build() {
        let _ = oer_hil_runs::run::register_runner(oer_hil_runs::run::RunnerBuild {
            record: super::RUNNER_BUILD,
            package: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        });
        let provenance = oer_hil_runs::run::runner_provenance().unwrap();
        let observer = provenance.observer.unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            observer["executable_sha256"],
            oer_os_durable::sha256_file(std::path::Path::new("/proc/self/exe")).unwrap()
        );
        let embedded: serde_json::Value = serde_json::from_str(super::RUNNER_BUILD).unwrap();
        assert_eq!(observer["build"], embedded);
        assert!(observer["build"]["inputs"]["tools/hil/link/src/reboot.rs"].is_string());
    }
}
