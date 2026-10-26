fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("manifest")?);
    let root = dir.join("../../..");
    let profile = root.join("chips/esp32s31/platform/chip.toml");
    let clic = root.join("chips/esp32s31/registers/types/clic.toml");
    let profile: toml::Value = toml::from_str(&std::fs::read_to_string(&profile)?)?;
    let clic: toml::Value = toml::from_str(&std::fs::read_to_string(&clic)?)?;
    let number = |v: &toml::Value| v.as_integer().ok_or("integer fact");
    let base = number(&profile["memory"]["internal-sram"]["origin"])?;
    let end = number(&profile["boot-chain"]["second-stage-loader-start"])?;
    let lp = number(&profile["memory"]["retained"]["origin"])?;
    let lines = clic["registers"].as_array().ok_or("CLIC words")?.iter()
        .find(|v| v["name"].as_str() == Some("CTRL%s")).ok_or("CTRL")?;
    let count = number(&lines["dim"]["count"])? as usize;
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR")?);
    let mut asm = String::from(".macro RUNTIME_IRQ_ENTRIES\n");
    for line in 1..count { asm.push_str(&format!("IRQ_ENTRY 0, {line}\n")); }
    asm.push_str(".endm\n.macro RUNTIME_VECTOR_TABLES\n");
    asm.push_str(&format!(".balign {}\n.word 0\n", (count * 4).next_power_of_two()));
    for line in 1..count { asm.push_str(&format!(".word _runtime_irq_h0_l{line}\n")); }
    asm.push_str(".endm\n.text\n");
    for line in 1..count {
        asm.push_str(&format!(".global __oer_interrupt_h0_l{line}\n__oer_interrupt_h0_l{line}:\nli a0, {line}\ntail facts_irq\n"));
    }
    std::fs::write(out.join("vectors.s"), asm)?;
    std::fs::write(out.join("facts.x"), format!(r#"ENTRY(_start)
MEMORY {{ SRAM (rwx) : ORIGIN = {base:#x}, LENGTH = {length:#x}
LP (rw) : ORIGIN = {lp:#x}, LENGTH = 0x100 }}
SECTIONS {{
.text : ALIGN(64) {{ KEEP(*(.text._start)) *(.text .text.*) *(.trap.runtime) }} > SRAM
.trap.mtvt : ALIGN({align}) {{ KEEP(*(.trap.mtvt)) }} > SRAM
.rodata : ALIGN(4) {{ *(.rodata .rodata.*) }} > SRAM
.data : ALIGN(4) {{ __global_pointer$ = . + 0x800; *(.sdata .sdata.*) *(.data .data.*) }} > SRAM
.bss (NOLOAD) : ALIGN(4) {{ __bss_start = .; *(.sbss .sbss.*) *(.bss .bss.*) . = ALIGN(4); __bss_end = .; }} > SRAM
.stacks (NOLOAD) : ALIGN(16) {{ __runtime_cpu0_irq_stack_bottom = .; . += 8192; __runtime_cpu0_irq_stack_top = .; __runtime_cpu0_task_stack_bottom = .; . += 16384; __runtime_cpu0_task_stack_top = .; }} > SRAM
.noinit (NOLOAD) : ALIGN(4) {{ KEEP(*(.noinit.facts)) }} > LP
/DISCARD/ : {{ *(.eh_frame .eh_frame.*) }}
}}
"#, length=end-base, align=(count*4).next_power_of_two()))?;
    if std::env::var("CARGO_CFG_TARGET_ARCH")? == "riscv32" {
        println!("cargo:rustc-link-arg=-T{}", out.join("facts.x").display());
    }
    for file in ["chips/esp32s31/platform/chip.toml", "chips/esp32s31/registers/types/clic.toml", "chips/espressif/staged-runtime/src/vectors.s"] {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    Ok(())
}
