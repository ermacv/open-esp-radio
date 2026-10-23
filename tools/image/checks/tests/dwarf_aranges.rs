use oer_analysis_elf::{
    Elf,
    dwarf::{Symbolizer, load},
};
use oer_os_toolchain::{Tool, command};
use std::{path::Path, process::Command, rc::Rc};

fn run(command: &mut Command) {
    let output = command.output().expect("run the pinned compiler or linker");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn staged_links_keep_cu_ranges_without_the_malformed_lto_accelerator() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests-fixtures/dwarf-aranges");
    let scripts =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/espressif/staged-layout/linker");
    for target in [
        "riscv32imafc-unknown-none-elf",
        "riscv32imac-unknown-none-elf",
    ] {
        let directory = tempfile::tempdir().expect("isolated compiler fixture");
        let directory = directory.path();
        for file in ["main.rs", "shared.rs", "leaf.rs", "link.x"] {
            std::fs::copy(fixture.join(file), directory.join(file)).expect("copy fixture");
        }
        let compiler = || {
            let mut compiler = command(Tool::Rustc).expect("pinned rustc");
            compiler.current_dir(directory).args([
                "--edition=2024",
                "--target",
                target,
                "-Copt-level=3",
                "-Cdebuginfo=2",
                "-Cpanic=abort",
                "-Ccodegen-units=1",
                "-Ldependency=.",
            ]);
            compiler
        };
        run(compiler().args([
            "--crate-type=rlib",
            "--crate-name=leaf",
            "-Cembed-bitcode=yes",
            "leaf.rs",
            "-o",
            "libleaf.rlib",
        ]));
        run(compiler().args([
            "--crate-type=rlib",
            "--crate-name=shared",
            "-Cembed-bitcode=yes",
            "--extern=leaf=libleaf.rlib",
            "shared.rs",
            "-o",
            "libshared.rlib",
        ]));
        let link = |name: &str, staged: bool| {
            let mut compiler = compiler();
            compiler.args([
                "--crate-type=bin",
                "--crate-name=fixture",
                "--extern=shared=libshared.rlib",
                "-Clto=fat",
                "-Clink-arg=--emit-relocs",
            ]);
            if staged {
                compiler.arg(format!("-Clink-arg=-L{}", scripts.display()));
                compiler.arg("-Clink-arg=-Tdwarf.x");
            }
            run(compiler.args(["-Clink-arg=-Tlink.x", "main.rs", "-o", name]));
            std::fs::read(directory.join(name)).expect("linked fixture")
        };
        let before = link("before.elf", false);
        let after = link("after.elf", true);
        let original = Elf::parse(&before).expect("original ELF");
        let fixed = Elf::parse(&after).expect("staged ELF");

        let dwarf = load(&original).expect("original DWARF");
        let mut headers = dwarf.debug_aranges.headers();
        let mut overflow = false;
        while let Some(header) = headers.next().expect("aranges header") {
            let mut entries = header.entries();
            while let Some(entry) = entries.next().transpose() {
                overflow |= entry.is_err();
            }
        }
        assert!(overflow, "the pinned LTO must reproduce the backward span");
        assert!(fixed.section_by_name(".debug_aranges").is_none());
        assert!(fixed.section_by_name(".rela.debug_aranges").is_none());
        let allocated = |elf: &Elf<'_>| {
            elf.sections()
                .filter(|section| section.allocated)
                .map(|section| {
                    (
                        section.name.to_owned(),
                        section.address,
                        section.size,
                        section.data.to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(allocated(&original), allocated(&fixed));
        let original = Symbolizer::new(&before).expect("original symbolizer");
        let copied = Symbolizer::new(&after).expect("staged symbolizer");
        let shared = Symbolizer::shared(Rc::from(&after[..])).expect("shared symbolizer");
        let leaf = fixed.symbol("leaf").expect("leaf symbol");
        let frames = copied.frames(leaf.address).expect("leaf frames");
        assert!(frames.iter().any(|frame| {
            frame
                .file
                .as_deref()
                .is_some_and(|file| file.ends_with("leaf.rs"))
                && frame.line == Some(5)
        }));
        for function in fixed.functions().expect("fixture functions") {
            for address in (function.address..function.address + function.size).step_by(2) {
                let frames = copied.frames(u64::from(address)).expect("staged frames");
                assert_eq!(
                    frames,
                    original
                        .frames(u64::from(address))
                        .expect("original frames")
                );
                assert_eq!(
                    frames,
                    shared.frames(u64::from(address)).expect("shared frames")
                );
            }
        }
    }
}
