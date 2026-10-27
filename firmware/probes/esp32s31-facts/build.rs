fn fact<'a>(value: &'a toml::Value, path: &[&str]) -> Result<&'a toml::Value, &'static str> {
    path.iter().try_fold(value, |value, key| {
        value.get(*key).ok_or("missing chip fact")
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("manifest")?);
    let root = dir.join("../../..");
    let profile = root.join("chips/esp32s31/platform/chip.toml");
    let clic = root.join("chips/esp32s31/registers/types/clic.toml");
    let profile: toml::Value = toml::from_str(&std::fs::read_to_string(&profile)?)?;
    let clic: toml::Value = toml::from_str(&std::fs::read_to_string(&clic)?)?;
    let family: toml::Value = toml::from_str(&std::fs::read_to_string(
        root.join("chips/espressif/platform/family.toml"),
    )?)?;
    let number = |v: &toml::Value| v.as_integer().ok_or("integer fact");
    let base = number(fact(&profile, &["memory", "internal-sram", "origin"])?)?;
    let end = number(fact(
        &profile,
        &["boot-chain", "second-stage-loader-start"],
    )?)?;
    let lp = number(fact(&profile, &["memory", "retained", "origin"])?)?;
    let lines = fact(&clic, &["registers"])?
        .as_array()
        .ok_or("CLIC words")?
        .iter()
        .find(|v| v.get("name").and_then(toml::Value::as_str) == Some("CTRL%s"))
        .ok_or("CTRL")?;
    let count = number(fact(lines, &["dim", "count"])?)? as usize;
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR")?);
    let bits = number(fact(&family, &["interrupts", "control-bits"])?)? as u32;
    let levels = number(fact(&family, &["interrupts", "level-bits"])?)?;
    let first = number(fact(&family, &["interrupts", "first-source-line"])?)?;
    if !(2..=8).contains(&bits)
        || levels != i64::from(bits)
        || count == 0
        || first <= 0
        || first as usize >= count
        || end <= base
    {
        return Err("incompatible facts geometry".into());
    }
    let controls: Vec<u8> = (0..3)
        .map(|n| (((n + 1) << (8 - bits)) - 1) as u8)
        .collect();
    std::fs::write(
        out.join("geometry.rs"),
        format!(
            "const LINE: usize = {first};\nconst LEVEL_BITS: u32 = {levels};\nconst CONTROLS: [u8; 3] = {controls:?};\n"
        ),
    )?;
    let mut asm = String::from(".macro FACTS_VECTOR_TABLE\n");
    asm.push_str(&format!(
        ".balign {}\n.word 0\n",
        (count * 4).next_power_of_two()
    ));
    for _ in 1..count {
        asm.push_str(".word _facts_irq\n");
    }
    asm.push_str(".endm\n");
    std::fs::write(out.join("vectors.s"), asm)?;
    std::fs::write(
        out.join("facts.x"),
        format!(
            r#"ENTRY(_start)
MEMORY {{ SRAM (rwx) : ORIGIN = {base:#x}, LENGTH = {length:#x}
LP (rw) : ORIGIN = {lp:#x}, LENGTH = 0x100 }}
SECTIONS {{
.text : ALIGN(64) {{ KEEP(*(.text._start)) *(.text.facts_threshold_raise) *(.text .text.*) *(.trap.facts) }} > SRAM
.trap.mtvt : ALIGN({align}) {{ KEEP(*(.trap.mtvt)) }} > SRAM
.rodata : ALIGN(4) {{ *(.rodata .rodata.*) }} > SRAM
.data : ALIGN(4) {{ __global_pointer$ = . + 0x800; *(.sdata .sdata.*) *(.data .data.*) }} > SRAM
.bss (NOLOAD) : ALIGN(4) {{ __bss_start = .; *(.sbss .sbss.*) *(.bss .bss.*) . = ALIGN(4); __bss_end = .; }} > SRAM
.stacks (NOLOAD) : ALIGN(16) {{ __runtime_cpu0_irq_stack_bottom = .; . += 8192; __runtime_cpu0_irq_stack_top = .; __runtime_cpu0_task_stack_bottom = .; . += 16384; __runtime_cpu0_task_stack_top = .; }} > SRAM
.noinit (NOLOAD) : ALIGN(4) {{ KEEP(*(.noinit.facts)) }} > LP
/DISCARD/ : {{ *(.eh_frame .eh_frame.*) }}
}}
"#,
            length = end - base,
            align = (count * 4).next_power_of_two()
        ),
    )?;
    if std::env::var("CARGO_CFG_TARGET_ARCH")? == "riscv32" {
        println!("cargo:rustc-link-arg=-T{}", out.join("facts.x").display());
    }
    for file in ["src/trap.rs", "src/trap.s"] {
        println!("cargo:rerun-if-changed={}", dir.join(file).display());
    }
    for file in [
        "chips/esp32s31/platform/chip.toml",
        "chips/espressif/platform/family.toml",
        "chips/esp32s31/registers/types/clic.toml",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    Ok(())
}
