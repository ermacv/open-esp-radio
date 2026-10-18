//! Executor conformance with the official RISC-V architectural tests.
//!
//! Each test of the selected RV32 suites is assembled with the caller's clang
//! against the target model in `riscv_conformance/`, executed through
//! in-process verification until it jumps to the executor's return sentinel,
//! and its signature (the memory between `begin_signature` and
//! `end_signature`) must equal the signature the RISC-V Sail formal model
//! writes for the same ELF, stopped at the same sentinel. A signature left entirely at the suite's fill
//! value fails, so a test whose body was not assembled cannot pass.
//!
//! The inputs are external and pinned in `riscv_conformance/inputs.toml`,
//! which also lists the suites: set `BLOBRAY_RISCV_ARCH_TEST` to the pinned
//! riscv-arch-test checkout, `BLOBRAY_RISCV_CC` to a clang with the riscv32
//! target and lld and `BLOBRAY_SAIL` to the pinned `sail_riscv_sim`, then run
//! `cargo test -p blobray-cli --test riscv_conformance -- --ignored`.
use blobray_application as app;
use blobray_domain::*;
use object::{Object, ObjectSymbol};
use oer_riscv_model::*;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned inputs; the test runs the suites they list.
#[derive(serde::Deserialize)]
struct Inputs {
    #[serde(rename = "arch-test")]
    arch_test: ArchTest,
}
#[derive(serde::Deserialize)]
struct ArchTest {
    suite: Vec<Suite>,
}
/// One suite of `riscv-test-suite/rv32i_m/` and the instruction set it is
/// assembled for.
#[derive(serde::Deserialize)]
struct Suite {
    name: String,
    march: String,
}
fn suites() -> Vec<Suite> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/riscv_conformance/inputs.toml");
    let inputs: Inputs = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    inputs.arch_test.suite
}
/// Value the suites fill the signature area with before a test runs.
const SIGNATURE_FILL: u32 = 0xdead_beef;
/// Stack mapped for every test; the tests do not use it before writing it.
const STACK: u32 = 0x9000_0000;
const STACK_BYTES: u32 = 4096;
/// Tests that need machine-mode state the executor does not model, with the
/// reason. Each must still fail, so an entry cannot outlive its cause.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "cebreak-01",
        "c.ebreak raises a breakpoint trap handled in machine mode, which the executor stops at",
    ),
    (
        "Fencei",
        "the test stores into its own code, and image loading rejects writable code segments",
    ),
    ("clmul-01", "Zbc is not part of the executed ISA"),
    ("clmulh-01", "Zbc is not part of the executed ISA"),
    ("clmulr-01", "Zbc is not part of the executed ISA"),
];

fn environment(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must name the conformance input"))
}

fn assemble(
    cc: &Path,
    suite: &Path,
    march: &str,
    source: &Path,
    output: &Path,
) -> std::result::Result<(), String> {
    let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/riscv_conformance");
    let status = Command::new(cc)
        .args(["--target=riscv32", "-mabi=ilp32", "-nostdlib", "-static"])
        .arg(format!("-march={march}"))
        .args(["-fuse-ld=lld", "-DXLEN=32", "-DTEST_CASE_1=True"])
        .arg("-I")
        .arg(suite.join("riscv-test-suite/env"))
        .arg("-I")
        .arg(&model)
        .arg("-T")
        .arg(model.join("link.ld"))
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|e| e.to_string())?;
    if status.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&status.stderr);
        Err(stderr
            .lines()
            .find(|l| l.contains("error"))
            .unwrap_or("assembly failed")
            .to_owned())
    }
}

fn symbol(file: &object::File<'_>, name: &str) -> u32 {
    file.symbols()
        .find(|s| s.name() == Ok(name))
        .map(|s| s.address() as u32)
        .unwrap_or_else(|| panic!("missing symbol {name}"))
}

/// The signature words, or why the test did not produce one.
fn run(elf: &[u8]) -> std::result::Result<Vec<u32>, String> {
    let file = object::File::parse(elf).map_err(|e| e.to_string())?;
    let (entry, begin, end) = (
        symbol(&file, "rvtest_entry_point"),
        symbol(&file, "begin_signature"),
        symbol(&file, "end_signature"),
    );
    let executable = app::in_process::Executable::new(elf.to_vec());
    let target = ExecutionTarget {
        executables: vec![executable.id().clone()],
        abi: CallAbi::RiscvInteger,
        stack: MemorySeed {
            address: STACK,
            length: STACK_BYTES,
            fill: Some(0),
            bytes: vec![],
        },
    };
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: target,
        replacement: None,
        binding: None,
        max_events: 16,
        cases: vec![ExecutionCase {
            name: "conformance".into(),
            reset: SessionReset::Cold,
            stack_fill: None,
            relation: None,
            vendor: Invocation {
                entry,
                goal: ExecutionGoal::Return,
                arguments: vec![Some(0); 8],
                memory: vec![],
                preload: vec![],
                models: vec![],
                calls: vec![],
                observe_memory: vec![MemorySelection {
                    name: "signature".into(),
                    address: begin,
                    length: end - begin,
                }],
                observe_calls: None,
                observe_timeline: TimelineCapture::default(),
            },
            replacement: None,
        }],
    };
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let result = app::in_process::verify(
        &app::in_process::InProcessComparison {
            request: &request,
            executables: std::slice::from_ref(&executable),
            effects: &[],
            projections: &[],
            vendor_results: None,
            dependence: None,
            patches: &[],
        },
        &blobray_backend_riscv::RiscvExecutor,
        &memory,
        &mut || Ok(()),
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut bytes = vec![0u8; (end - begin) as usize];
    for record in &result.records {
        match record {
            ExecutionEvidence::Outcome { stop, .. } if !stop.completed() => {
                return Err(format!("did not complete: {stop:?}"));
            }
            ExecutionEvidence::FinalMemory { chunk, .. } => {
                let mask = chunk.mask().unwrap();
                if chunk.known & mask != mask {
                    return Err(format!("unknown signature bytes at {:#x}", chunk.offset));
                }
                let at = chunk.offset as usize;
                bytes[at..at + chunk.length as usize]
                    .copy_from_slice(&chunk.bytes[..chunk.length as usize]);
            }
            _ => {}
        }
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect())
}

/// The signature Sail writes for `elf`.
fn reference(sail: &Path, elf: &Path, directory: &Path) -> std::result::Result<Vec<u32>, String> {
    let signature = directory.join(format!(
        "{}.sail",
        elf.file_name().unwrap().to_string_lossy()
    ));
    let output = Command::new(sail)
        // Sail stops where the executor's return sentinel ends the test.
        .args([
            "--rv32",
            "--inst-limit",
            "10000000",
            "--stop-at-pc",
            "0xfffffffe",
        ])
        .arg("--test-signature")
        .arg(&signature)
        .arg(elf)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Sail failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::read_to_string(&signature)
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| u32::from_str_radix(l.trim(), 16).map_err(|e| e.to_string()))
        .collect()
}

#[test]
#[ignore = "needs the RISC-V architectural tests, a riscv32 clang and Sail"]
fn architectural_tests_match_their_reference_signatures() {
    let suite = environment("BLOBRAY_RISCV_ARCH_TEST");
    let cc = environment("BLOBRAY_RISCV_CC");
    let sail = environment("BLOBRAY_SAIL");
    let output = tempfile::tempdir().unwrap();
    let mut passed = 0;
    let mut failures = vec![];
    let mut excluded_passing = vec![];
    for Suite {
        name: extension,
        march,
    } in suites()
    {
        let root = suite.join("riscv-test-suite/rv32i_m").join(&extension);
        let mut sources: Vec<_> = std::fs::read_dir(root.join("src"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "S"))
            .collect();
        sources.sort();
        for source in sources {
            let name = source.file_stem().unwrap().to_string_lossy().into_owned();
            let elf = output.path().join(format!("{extension}-{name}.elf"));
            let outcome = assemble(&cc, &suite, &march, &source, &elf).and_then(|()| {
                let signature = run(&std::fs::read(&elf).unwrap())?;
                let expected = reference(&sail, &elf, output.path())?;
                if expected.iter().all(|&word| word == SIGNATURE_FILL) {
                    Err("the reference signature is only fill: the test body did not run".into())
                } else if signature == expected {
                    Ok(())
                } else {
                    let first = signature
                        .iter()
                        .zip(&expected)
                        .position(|(a, b)| a != b)
                        .unwrap_or(signature.len().min(expected.len()));
                    Err(format!(
                        "signature word {first} differs ({} words, {} expected)",
                        signature.len(),
                        expected.len()
                    ))
                }
            });
            let excluded = EXCLUDED.iter().find(|(test, _)| *test == name);
            match (outcome, excluded) {
                (Ok(()), None) => passed += 1,
                (Ok(()), Some(_)) => excluded_passing.push(name),
                (Err(_), Some(_)) => {}
                (Err(reason), None) => failures.push(format!("{extension}/{name}: {reason}")),
            }
        }
    }
    eprintln!("{passed} architectural tests passed");
    assert!(
        excluded_passing.is_empty(),
        "exclusions that now pass: {excluded_passing:?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
