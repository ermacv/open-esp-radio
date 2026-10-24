//! `Boundary` polls one concrete future; a `dyn Future` behind it is rejected.
//! The hardware-in-the-loop image keeps a byte-identical wrapper that cannot
//! build on the host, so this test checks both sources.
use std::process::Command;

const SOURCES: [&str; 2] = [
    include_str!("../src/future.rs"),
    include_str!("../../../hil/esp32s31/src/wifi_station/future.rs"),
];

fn compile(future: &str, use_site: &str) -> (bool, String) {
    let directory = tempfile::tempdir().expect("create the fixture directory");
    let source = directory.path().join("fixture.rs");
    std::fs::write(
        &source,
        format!("mod future {{\n{future}\n}}\n#[allow(dead_code)]\n{use_site}\n"),
    )
    .expect("write the fixture");
    let output = Command::new("rustc")
        .args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
        .arg("--out-dir")
        .arg(directory.path())
        .arg(&source)
        .output()
        .expect("run the pinned rustc");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn boundary_accepts_a_concrete_future() {
    for source in SOURCES {
        let (ok, stderr) = compile(
            source,
            "fn run(f: core::pin::Pin<&mut impl core::future::Future<Output = ()>>) \
         -> impl core::future::Future<Output = ()> + '_ { future::Boundary(f) }",
        );
        assert!(ok, "{stderr}");
    }
}

#[test]
fn boundary_rejects_a_dyn_future() {
    for source in SOURCES {
        let (ok, stderr) = compile(
            source,
            "fn run(f: core::pin::Pin<&mut dyn core::future::Future<Output = ()>>) \
         -> impl core::future::Future<Output = ()> + '_ { future::Boundary(f) }",
        );
        assert!(!ok, "a dyn Future behind Boundary compiled");
        assert!(stderr.contains("E0277"), "{stderr}");
    }
}
