#![forbid(unsafe_code)]

// Compile the actual C shim with a recorder owned by this one test process.
#[test]
fn interrupt_shims_preserve_handles_and_recorder_results() {
    use std::{env, path::Path};

    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = package.join("../../..");
    let manifest = oer_tool_pins::Manifest::load(&root, "esp32s31").expect("pinned manifest");
    let source = manifest.source("esp-idf").expect("pinned IDF source");
    let idf = env::var_os("OER_ESP_IDF_DIR").map_or_else(
        || {
            manifest
                .source_directory(&root, source)
                .expect("fetched IDF")
        },
        Into::into,
    );
    let scratch =
        tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("own compiler directory");
    let program = scratch.path().join("shim-interrupts");
    let mut compiler = cc::Build::new()
        .host(env!("OER_SHIM_HOST"))
        .target(env!("OER_SHIM_HOST"))
        .opt_level(0)
        .debug(false)
        .cargo_metadata(false)
        .get_compiler()
        .to_command();
    compiler
        .args(["-std=gnu11", "-ffunction-sections", "-fdata-sections"])
        .args(["-include", "esp_intr_alloc.h"])
        .arg("-I")
        .arg(Path::new(env!("OUT_DIR")).join("include"))
        .arg("-I")
        .arg(package.join("shim/include"));
    for include in [
        "components/ieee802154/include",
        "components/ieee802154/private_include",
        "components/esp_hal_ieee802154/include",
        "components/esp_hal_ieee802154/esp32s31/include",
        "components/soc/esp32s31/include",
        "components/soc/esp32s31/register",
        "components/esp_coex/include",
    ] {
        compiler.arg("-I").arg(idf.join(include));
    }
    let compiled = compiler
        .arg(package.join("shim/src/host.c"))
        .arg(package.join("tests/shim_interrupts.c"))
        .arg("-Wl,--gc-sections")
        .arg("-o")
        .arg(&program)
        .output()
        .expect("compile shim regression");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let checked = std::process::Command::new(&program)
        .output()
        .expect("run isolated C regression");
    assert!(checked.status.success(), "{checked:?}");
}
