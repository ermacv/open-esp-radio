//! The staged runtime's own trap entries and exit (`vectors.s`), assembled
//! by the repository's `rustc` as images assemble them: every entry passes
//! the entry and exit checks, and each way of losing the interrupted state
//! fails them.
use super::*;

/// The runtime's trap assembly.
const VECTORS: &str = include_str!("../../../../chips/espressif/staged-runtime/src/vectors.s");

/// A program that assembles `vectors` for `harts` harts, raising the
/// threshold on exit when `raise_threshold`, with every line's handler and
/// the exception handler defined.
fn runtime(vectors: &str, harts: u32, raise_threshold: bool) -> Vec<u8> {
    let build = tempfile::Builder::new()
        .prefix("vectors")
        .tempdir()
        .unwrap();
    let path = build.path().join("vectors.s");
    std::fs::write(&path, vectors).unwrap();
    let source = format!(
        r#"
#![no_std]
#![no_main]

core::arch::global_asm!(
    include_str!({path:?}),
    fs = const 0x6000,
    mie = const 8,
    mintthresh = const 0x347,
    harts = const {harts},
    raise_threshold = const {raise},
);

#[unsafe(no_mangle)]
pub extern "C" fn handler() {{}}

#[unsafe(no_mangle)]
pub extern "C" fn oer_exception_handler() -> ! {{
    loop {{}}
}}

#[unsafe(no_mangle)]
pub extern "C" fn keep() {{}}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {{
    loop {{}}
}}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {{
    loop {{}}
}}
"#,
        raise = u32::from(raise_threshold),
    );
    let mut link = vec![
        "--undefined=_runtime_mtvt".to_owned(),
        "--undefined=_start_trap".to_owned(),
        "--defsym=__runtime_cpu0_task_stack_bottom=0x48000000".to_owned(),
        "--defsym=__runtime_cpu0_task_stack_top=0x48010000".to_owned(),
        "--defsym=__runtime_cpu0_irq_stack_top=0x2f008000".to_owned(),
    ];
    for hart in 0..harts {
        for line in 1..48 {
            link.push(format!("--defsym=__oer_interrupt_h{hart}_l{line}=handler"));
        }
    }
    let link: Vec<&str> = link.iter().map(String::as_str).collect();
    compiled_linking(&source, &link)
}

/// Check every vector of `elf` with the interrupt check and the exception
/// entry with the entry check; the vectors by slot.
fn checked(elf: &[u8]) -> Result<(Vec<Option<TrapEntry>>, TrapEntry)> {
    with_facts(elf, |facts| {
        let slots = crate::vector_table(facts, "_runtime_mtvt", &mut || Ok(()))?;
        let vectors = slots
            .into_iter()
            .map(|slot| {
                slot.map(|entry| crate::interrupt_entry(facts, entry, &mut || Ok(())))
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;
        let exception = facts
            .file()
            .address("_start_trap")
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "no _start_trap"))?;
        let exception = crate::trap_entry(facts, exception as u32, &mut || Ok(()))?;
        Ok((vectors, exception))
    })
}

fn address(elf: &[u8], name: &str) -> u32 {
    oer_analysis_elf::Elf::parse(elf)
        .unwrap()
        .address(name)
        .unwrap() as u32
}

#[test]
fn every_runtime_entry_returns_through_the_checked_exit() {
    for (harts, raise) in [(1, true), (2, false)] {
        let elf = runtime(VECTORS, harts, raise);
        let (vectors, exception) = checked(&elf).unwrap();
        let handler = address(&elf, "handler");
        assert_eq!(vectors.len(), 64 * harts as usize);
        for (slot, vector) in vectors.iter().enumerate() {
            let line = slot % 64;
            match vector {
                Some(vector) => {
                    assert!((1..48).contains(&line), "slot {slot}");
                    assert_eq!((vector.frame, vector.handler), (96, handler));
                }
                None => assert!(line == 0 || line >= 48, "slot {slot}"),
            }
        }
        assert_eq!(exception.frame, 96);
        assert_eq!(exception.handler, address(&elf, "oer_exception_handler"));
    }
}

/// The runtime's assembly with `from` replaced by `to` once, which must
/// change it.
fn broken(from: &str, to: &str) -> String {
    assert!(VECTORS.contains(from), "{from}");
    VECTORS.replacen(from, to, 1)
}

/// The first failure of the checks on `vectors`.
fn failure(vectors: &str, raise: bool) -> String {
    match checked(&runtime(vectors, 1, raise)) {
        Ok(_) => panic!("the broken entry passes"),
        Err(error) => error.message,
    }
}

#[test]
fn an_exit_that_loses_mepc_or_mcause_or_reads_a_stale_slot_fails() {
    let lost = broken("    lw t0, 76(sp)\n    csrw mepc, t0\n", "");
    assert!(failure(&lost, false).contains("without the trap's mepc"));
    let lost = broken("    lw t0, 80(sp)\n    csrw mcause, t0\n", "");
    assert!(failure(&lost, false).contains("without the trap's mcause"));
    // mepc from the interrupted sp's slot.
    let stale = broken(
        "    lw t0, 76(sp)\n    csrw mepc, t0\n",
        "    lw t0, 72(sp)\n    csrw mepc, t0\n",
    );
    assert!(failure(&stale, false).contains("without the trap's mepc"));
    let lost = broken("    lw t0, 84(sp)\n    csrw {mintthresh}, t0\n", "");
    assert!(failure(&lost, true).contains("interrupt threshold"));
}

#[test]
fn an_exit_that_unmasks_early_or_returns_unmasked_fails() {
    // MIE cleared only after mepc and mcause are back.
    let late = broken(
        "    csrci mstatus, {mie}\n    lw t0, 80(sp)",
        "    lw t0, 80(sp)",
    );
    assert!(failure(&late, false).contains("with interrupts enabled"));
    let unmasked = broken(
        "    addi sp, sp, 96\n    mret",
        "    addi sp, sp, 96\n    csrsi mstatus, {mie}\n    mret",
    );
    assert!(failure(&unmasked, false).contains("returns with interrupts enabled"));
    let lost = broken("    lw t0, 68(sp)\n    csrw mstatus, t0\n", "");
    assert!(failure(&lost, false).contains("mstatus"));
}

#[test]
fn an_exit_on_another_stack_or_without_a_register_fails() {
    // The nested return leaves sp below the interrupted handler's.
    let wrong = broken(
        "    addi sp, sp, 96\n    mret",
        "    addi sp, sp, 80\n    mret",
    );
    assert!(failure(&wrong, false).contains("another stack"));
    let lost = broken("    lw a5, 52(sp)\n", "");
    assert!(failure(&lost, false).contains("without restoring a register"));
}

#[test]
fn an_entry_that_nests_before_saving_or_keeps_the_fpu_on_fails() {
    let unsaved = broken("    csrr t1, mepc\n    sw t1, 76(sp)\n", "");
    assert!(failure(&unsaved, false).contains("admits nesting before its frame holds"));
    let fpu = broken("    li t1, {fs}\n    csrc mstatus, t1\n", "");
    assert!(failure(&fpu, false).contains("with the FPU on"));
}
