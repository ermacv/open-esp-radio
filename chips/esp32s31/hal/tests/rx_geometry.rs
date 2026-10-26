#![forbid(unsafe_code)]

// Zero-initialized storage bypasses `new`. Its runtime entry points must still
// evaluate the geometry's inline const check when monomorphized on the host.
#[test]
fn zeroed_rx_storage_checks_geometry_before_cpu_access_or_dma_placement() {
    use std::{fs, path::Path, process::Command};

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("own compiler scratch");
    fs::write(
        scratch.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"rx-storage-geometry\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-esp32s31-hal = {{ path = {manifest:?}, features = [\"test-support\"] }}\noer-memory = {{ path = {:?} }}\n",
            manifest.join("../../../portable/memory"),
        ),
    ).expect("write isolated compiler manifest");
    // The workspace lock pins every dependency; the case adds only its own root.
    fs::copy(
        manifest.join("../../../Cargo.lock"),
        scratch.path().join("Cargo.lock"),
    )
    .expect("pin dependencies to the workspace lock");
    for (entry, action) in [
        ("place", "let _ = Storage::place(Placed::host(storage));"),
        (
            "place_rotated",
            "let _ = Storage::place_rotated(Placed::host(storage), 1);",
        ),
        ("buffer_mut", "let _ = storage.buffer_mut(0);"),
        (
            "prepare_unpublished_buffer",
            "let _ = storage.prepare_unpublished_buffer(0);",
        ),
    ] {
        for (capacity, backing, accepted) in
            [(16, 16, true), (4, 4, true), (16, 15, false), (3, 3, false)]
        {
            fs::write(
                scratch.path().join("case.rs"),
                format!(r#"
#![forbid(unsafe_code)]
use oer_esp32s31_hal::{{dma_static::Placed, ieee80211::dma::{{rx_storage::RxDmaStorage, space::HostDma}}}};
type Storage = RxDmaStorage<4, {capacity}, {backing}, HostDma>;
pub fn exercise() {{
    let storage = Box::leak(Box::new(oer_memory::zeroed::zeroed::<Storage>()));
    {action}
}}
"#),
            ).expect("write geometry case");
            let output = Command::new(env!("CARGO"))
                .args(["build", "--offline", "--lib", "--manifest-path"])
                .arg(scratch.path().join("Cargo.toml"))
                .arg("--target-dir")
                .arg(scratch.path().join("build"))
                .output()
                .expect("run pinned compiler");
            fs::write(
                scratch
                    .path()
                    .join(format!("{entry}-{capacity}-{backing}.log")),
                &output.stderr,
            )
            .expect("save compiler diagnostics");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                accepted,
                "{entry}/{capacity}/{backing}: {stderr}"
            );
            if !accepted {
                assert!(stderr.contains("error[E0080]"), "{stderr}");
                assert!(
                    stderr.contains(
                        "RX DMA capacity includes its leading guard and fits its backing"
                    ),
                    "{stderr}"
                );
            }
        }
    }
}
