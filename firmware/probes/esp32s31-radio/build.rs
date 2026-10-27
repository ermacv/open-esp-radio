fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("riscv32") {
        return Ok(());
    }
    let directory = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").ok_or("Cargo manifest directory")?,
    );
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("Cargo output directory")?);
    oer_probe_codegen::build(
        &directory.join("src/lib.rs"),
        &std::env::var("CARGO_PKG_NAME")?,
        &out,
    )
    .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    let source = directory.join("link.x");
    println!("cargo:rerun-if-changed={}", source.display());
    // SOURCE: the chip layout generated from platform facts. Probe records
    // use the same retained region and reserved tail as staged images.
    const LENGTH: u32 = match oer_esp32s31_layout::LAYOUT
        .retained
        .length()
        .checked_sub(oer_esp32s31_layout::LAYOUT.retained_reserved_tail)
    {
        Some(length) => length,
        None => panic!("retained tail must fit the sourced region"),
    };
    let script = out.join("probe-link.x");
    std::fs::write(
        &script,
        format!(
            "PROBE_RETAINED_ORIGIN = {:#x};\nPROBE_RETAINED_LENGTH = {:#x};\n{}",
            oer_esp32s31_layout::LAYOUT.retained.origin(),
            LENGTH,
            std::fs::read_to_string(source)?
        ),
    )?;
    // The verifier supplies its stack and installs no runtime IRQ/task stack.
    // Panic classification reads explicit empty ranges for every sourced hart.
    let mut markers = String::new();
    for hart in 0..oer_esp32s31_layout::LAYOUT.harts {
        for suffix in [
            "irq_stack_bottom",
            "irq_stack_top",
            "irq_stack_guard_bytes",
            "task_stack_bottom",
            "task_stack_top",
        ] {
            use std::fmt::Write;
            writeln!(markers, "__runtime_cpu{hart}_{suffix} = 0;")?;
        }
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&script)?
        .write_all(markers.as_bytes())?;
    println!(
        "cargo:rustc-link-arg-bin=oer-firmware-probes-esp32s31-radio=-T{}",
        script.display()
    );
    oer_esp32s31_layout::build::configure_runtime("oer-firmware-probes-esp32s31-radio-hardware")?;
    Ok(())
}
