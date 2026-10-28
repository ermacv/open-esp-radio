//! Compile the adapter's actual helper and check its out-of-line call boundary.
//! The full image build checks the Waker proof in its own LTO composition.
use oer_analysis_elf::{Elf, demangle};
use oer_analysis_riscv_model::{Ledger, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{DebugMetadata, analyze, read_debug};
use oer_os_toolchain::{Tool, command, lld};
use std::{process::Command, rc::Rc};

fn run(command: &mut Command) {
    let output = command.output().expect("run the pinned toolchain");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn displaced_registration_retains_its_out_of_line_call() {
    // Keep the real attributes and body, rather than a copy that could retain
    // the boundary after production code loses it.
    let owned = include_str!("../../../../portable/embassy/net/src/owned.rs");
    let helper = owned
        .split_once(
            "/// Wake a displaced registration only after shared state has been restored.\n",
        )
        .expect("helper documentation")
        .1
        .split_once("/// Static bounded queues for one permanent logical network endpoint.")
        .expect("end of helper")
        .0;
    let directory = tempfile::tempdir().expect("isolated compiler fixture");
    let directory = directory.path();
    std::fs::write(
        directory.join("boundary.rs"),
        format!(
            "#![no_std]\nuse core::task::Waker;\n{helper}\n\
             pub fn root(previous: Option<Waker>, current: &Waker) {{\n\
                 wake_replaced(previous, current);\n\
                 core::hint::black_box(0_u32);\n\
             }}\n"
        ),
    )
    .expect("write compiler fixture");
    run(command(Tool::Rustc)
        .expect("pinned rustc")
        .current_dir(directory)
        .env("RUSTC_BOOTSTRAP", "1")
        .args([
            "--edition=2024",
            "--crate-name=boundary",
            "--crate-type=lib",
            "--emit=obj",
            "--target=riscv32imafc-unknown-none-elf",
            "-Copt-level=3",
            "-Clto=fat",
            "-Ccodegen-units=1",
            "-Cdebuginfo=2",
            "-Cpanic=abort",
            "-Zemit-stack-sizes",
            "boundary.rs",
            "-o",
            "boundary.o",
        ]));
    let object = std::fs::read(directory.join("boundary.o")).expect("compiler object");
    let object = Elf::parse(&object).expect("ELF object");
    let root = object
        .symbols()
        .find(|symbol| demangle(symbol.name) == "boundary::root")
        .expect("public fixture root");
    run(Command::new(lld().expect("pinned LLD"))
        .current_dir(directory)
        .args(["--gc-sections", "--emit-relocs", "-e", root.name])
        .args(["boundary.o", "-o", "boundary.elf"]));

    let elf: Rc<[u8]> = std::fs::read(directory.join("boundary.elf"))
        .expect("linked compiler fixture")
        .into();
    let memory = WorkingMemory::new(64 * 1024 * 1024).expect("analysis memory");
    let mut control = || Ok(());
    let mut keep = Ledger::new(&memory, &mut control).expect("analysis ledger");
    let image = ImageFacts::new(&elf, &[], &memory, &mut control).expect("ELF facts");
    let (dwarf, _) =
        read_debug(&elf, DebugMetadata::Standard, &mut keep, &mut control).expect("compiler DWARF");
    let analysis =
        analyze(&image, &[], &[], &dwarf, &mut keep, &mut control).expect("stack analysis");
    let function = |name| {
        analysis
            .functions
            .values()
            .find(|facts| {
                facts
                    .function
                    .names
                    .iter()
                    .any(|symbol| demangle(symbol) == name)
            })
            .unwrap_or_else(|| panic!("missing proof boundary {name}"))
    };
    let helper = function("boundary::wake_replaced");
    assert!(
        function("boundary::root")
            .transfers
            .iter()
            .any(|transfer| { transfer.target == Some(helper.function.address) })
    );
}
