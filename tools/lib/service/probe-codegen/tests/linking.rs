//! Real RV32 archive extraction and fat-LTO acceptance, without private inputs.
use oer_probe_codegen::{Declaration, collect, validate_elf};
use std::{path::Path, process::Command};

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compile(directory: &Path, declarations: &[&str]) -> Vec<u8> {
    let source = declarations
        .iter()
        .map(|source| format!("probe! {{ {source} }}\n"))
        .collect::<String>();
    std::fs::write(directory.join("declarations.rs"), source).unwrap();
    let (catalog, _) = collect(&directory.join("declarations.rs"), "fixture").unwrap();
    let mut library = String::from(
        "#![no_std]\n#[panic_handler] fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }\n",
    );
    for source in declarations {
        library.push_str(
            &syn::parse_str::<Declaration>(source)
                .unwrap()
                .expand()
                .to_string(),
        );
    }
    std::fs::write(directory.join("library.rs"), library).unwrap();
    oer_probe_codegen::build(&directory.join("declarations.rs"), "fixture", directory).unwrap();
    std::fs::write(directory.join("main.rs"), format!(
        "#![no_std]\n#![no_main]\nextern crate fixture as _;\ninclude!({:?});\n#[unsafe(no_mangle)] pub extern \"C\" fn _start() -> ! {{ loop {{core::hint::spin_loop();}} }}",
        directory.join("probe_catalog.rs"))).unwrap();
    std::fs::write(directory.join("link.x"), "ENTRY(_start)\nSECTIONS { . = 0x10000000; .text : { *(.text .text.*) } .blobray.probes : { KEEP(*(.blobray.probes)) } .rodata : { *(.rodata .rodata.*) } /DISCARD/ : { *(.eh_frame*) } }\n").unwrap();
    let rustc = oer_toolchain::program(oer_toolchain::Tool::Rustc).unwrap();
    let options = [
        "--edition=2024",
        "--target=riscv32imafc-unknown-none-elf",
        "-Copt-level=3",
        "-Clto=fat",
        "-Cembed-bitcode=yes",
        "-Cpanic=abort",
    ];
    run(Command::new(&rustc)
        .args(options)
        .args(["--crate-type=rlib", "--crate-name=fixture"])
        .arg(directory.join("library.rs"))
        .arg("-o")
        .arg(directory.join("libfixture.rlib")));
    let mut binary = Command::new(&rustc);
    binary
        .args(options)
        .arg(directory.join("main.rs"))
        .arg("--extern")
        .arg(format!(
            "fixture={}",
            directory.join("libfixture.rlib").display()
        ))
        .arg(format!(
            "-Clink-arg=-T{}",
            directory.join("link.x").display()
        ))
        .arg("-Clink-arg=--gc-sections")
        .arg("-o")
        .arg(directory.join("fixture.elf"));
    for entry in catalog.entries {
        binary.arg(format!("-Clink-arg=--undefined={}", entry.symbol));
    }
    run(&mut binary);
    std::fs::read(directory.join("fixture.elf")).unwrap()
}

#[test]
#[ignore = "requires the repository RV32 rust-std target and linker"]
fn new_declarations_survive_rlib_extraction_lto_gc_and_relocation() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    std::fs::create_dir(&original).unwrap();
    let first = "pub fn first(value: u32) -> u32 => value.wrapping_add(3);";
    let elf = compile(&original, &[first]);
    assert_eq!(validate_elf(&elf, "fixture").unwrap().entries.len(), 1);
    let moved = temporary.path().join("moved with spaces");
    std::fs::rename(original, &moved).unwrap();
    let elf = compile(
        &moved,
        &[first, "pub fn added(value: u32) -> u32 => value ^ 0xa5;"],
    );
    let catalog = validate_elf(&elf, "fixture").unwrap();
    assert_eq!(
        catalog
            .entries
            .iter()
            .map(|e| e.symbol.as_str())
            .collect::<Vec<_>>(),
        ["added", "first"]
    );
}
