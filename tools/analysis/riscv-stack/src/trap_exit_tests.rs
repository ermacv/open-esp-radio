//! The staged runtime's own trap entries and exit (`vectors.s`), with the
//! entries and tables its build generates from each chip's facts, assembled
//! by the repository's `rustc` as images assemble them: every entry passes
//! the entry and exit checks, and each way of losing the interrupted state
//! fails them.
use super::*;
use oer_espressif_facts::generate::{self, Vectors};

/// The runtime's hand-written trap assembly: the common entry and exit.
const VECTORS: &str = include_str!("../../../../chips/espressif/staged-runtime/src/vectors.s");

/// The vectors the runtime's build generates for chip `id`.
fn chip(id: &str) -> Vectors {
    let (facts, _) = generate::tracked(id).unwrap();
    Vectors::of(&facts).unwrap()
}

/// The ESP32-S31's vectors.
fn s31() -> Vectors {
    chip("esp32s31")
}

/// A program that assembles `chip`'s generated vectors and `vectors`, with
/// every line's handler and the exception handler defined.
fn runtime(vectors: &str, chip: &Vectors) -> Vec<u8> {
    let build = tempfile::Builder::new()
        .prefix("vectors")
        .tempdir()
        .unwrap();
    let generated = build.path().join("generated.s");
    std::fs::write(&generated, chip.assembly()).unwrap();
    let path = build.path().join("vectors.s");
    std::fs::write(&path, vectors).unwrap();
    let source = format!(
        r#"
#![no_std]
#![no_main]

core::arch::global_asm!(
    include_str!({generated:?}),
    include_str!({path:?}),
    fs = const 0x6000,
    mie = const 8,
    mintthresh = const 0x347,
    tcontrol = const 0x7a5,
    mte = const 8,
    mpte_shift = const 4,
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
"#
    );
    let mut link = vec![
        "--undefined=_runtime_mtvt".to_owned(),
        "--undefined=_start_trap".to_owned(),
        "--defsym=__runtime_cpu0_task_stack_bottom=0x48000000".to_owned(),
        "--defsym=__runtime_cpu0_task_stack_top=0x48010000".to_owned(),
        "--defsym=__runtime_cpu0_irq_stack_top=0x2f008000".to_owned(),
    ];
    for hart in 0..chip.harts {
        for line in 1..chip.lines {
            link.push(format!("--defsym={}=handler", Vectors::handler(hart, line)));
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
    for id in ["esp32s31"] {
        let chip = chip(id);
        let elf = runtime(VECTORS, &chip);
        let (vectors, exception) = checked(&elf).unwrap();
        let handler = address(&elf, "handler");
        let slots = chip.slots as usize;
        assert_eq!(vectors.len(), slots * chip.harts as usize, "{id}");
        for (slot, vector) in vectors.iter().enumerate() {
            let line = (slot % slots) as u32;
            match vector {
                Some(vector) => {
                    assert!((1..chip.lines).contains(&line), "{id} slot {slot}");
                    assert_eq!((vector.frame, vector.handler), (96, handler));
                }
                None => assert!(line == 0 || line >= chip.lines, "{id} slot {slot}"),
            }
        }
        for entry in generate::entry_symbols(&chip) {
            address(&elf, &entry);
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
fn failure(vectors: &str) -> String {
    match checked(&runtime(vectors, &s31())) {
        Ok(_) => panic!("the broken entry passes"),
        Err(error) => error.message,
    }
}

#[test]
fn an_exit_that_loses_mepc_or_mcause_or_reads_a_stale_slot_fails() {
    let lost = broken("    lw t0, 76(sp)\n    csrw mepc, t0\n", "");
    assert!(failure(&lost).contains("without the trap's mepc"));
    let lost = broken("    lw t0, 80(sp)\n    csrw mcause, t0\n", "");
    assert!(failure(&lost).contains("without the trap's mcause"));
    // mepc from the interrupted sp's slot.
    let stale = broken(
        "    lw t0, 76(sp)\n    csrw mepc, t0\n",
        "    lw t0, 72(sp)\n    csrw mepc, t0\n",
    );
    assert!(failure(&stale).contains("without the trap's mepc"));
    let lost = broken("    lw t0, 84(sp)\n    csrw {mintthresh}, t0\n", "");
    assert!(failure(&lost).contains("interrupt threshold"));
}

#[test]
fn an_exit_that_unmasks_early_or_returns_unmasked_fails() {
    // MIE cleared only after mepc and mcause are back.
    let late = broken(
        "    csrci mstatus, {mie}\n    lw t0, 80(sp)",
        "    lw t0, 80(sp)",
    );
    assert!(failure(&late).contains("with interrupts enabled"));
    let unmasked = broken(
        "    addi sp, sp, 96\n    mret",
        "    addi sp, sp, 96\n    csrsi mstatus, {mie}\n    mret",
    );
    assert!(failure(&unmasked).contains("enables interrupts again"));
    let lost = broken("    lw t0, 68(sp)\n    csrw mstatus, t0\n", "");
    assert!(failure(&lost).contains("mstatus"));
}

#[test]
fn an_exit_on_another_stack_or_without_a_register_fails() {
    // The nested return leaves sp below the interrupted handler's.
    let wrong = broken(
        "    addi sp, sp, 96\n    mret",
        "    addi sp, sp, 80\n    mret",
    );
    assert!(failure(&wrong).contains("another stack"));
    let lost = broken("    lw a5, 52(sp)\n", "");
    assert!(failure(&lost).contains("without restoring a register"));
}

#[test]
fn an_entry_that_nests_before_saving_or_keeps_the_fpu_on_fails() {
    let unsaved = broken("    csrr t1, mepc\n    sw t1, 76(sp)\n", "");
    assert!(failure(&unsaved).contains("admits nesting before its frame holds"));
    let fpu = broken("    li t1, {fs}\n    csrc mstatus, t1\n", "");
    assert!(failure(&fpu).contains("with the FPU on"));
}

#[test]
fn an_exit_that_lets_interrupts_in_after_masking_fails() {
    // A window after the threshold's restore that clears MIE again before
    // `mret`: a higher level taken in it overwrites the restored mepc and
    // mcause.
    let window = broken(
        "    lw t0, 64(sp)\n",
        "    csrsi mstatus, {mie}\n    nop\n    nop\n    nop\n    csrci mstatus, {mie}\n    lw t0, 64(sp)\n",
    );
    assert!(failure(&window).contains("enables interrupts again on its way out"));
}

#[test]
fn an_exit_that_moves_sp_below_its_frame_fails() {
    let deep = broken(
        "_runtime_irq_exit:\n",
        "_runtime_irq_exit:\n    addi sp, sp, -1024\n    sw zero, 0(sp)\n    addi sp, sp, 1024\n",
    );
    assert!(failure(&deep).contains("moves sp below its frame on its way out"));
}

#[test]
fn an_entry_deeper_before_its_call_reports_its_deepest_frame() {
    // The entry moves sp down and back before its call: the frame counts
    // the deepest sp, not the one at the call.
    let deep = broken(
        "    csrsi mstatus, {mie}\n",
        "    addi sp, sp, -64\n    sw zero, 0(sp)\n    addi sp, sp, 64\n    csrsi mstatus, {mie}\n",
    );
    let (vectors, _) = checked(&runtime(&deep, &s31())).unwrap();
    assert_eq!(vectors[16].map(|vector| vector.frame), Some(160));
}

#[test]
fn a_trap_that_touches_the_floating_point_state_or_another_csr_fails() {
    // A dirty-FS task's ft0, before the entry turns the FPU off.
    let ft0 = broken(
        "    li t1, {fs}\n",
        "    .option push\n    .option arch, +f\n    fmv.w.x ft0, zero\n    .option pop\n    li t1, {fs}\n",
    );
    assert!(failure(&ft0).contains("floating-point instruction"));
    // `fcsr` after the exit restored mstatus.
    let fcsr = broken(
        "    lw t0, 64(sp)\n",
        "    csrw 0x003, zero\n    lw t0, 64(sp)\n",
    );
    assert!(failure(&fcsr).contains("a CSR the trap check does not model"));
    let mtvec = broken(
        "    lw t0, 64(sp)\n",
        "    csrw mtvec, zero\n    lw t0, 64(sp)\n",
    );
    assert!(failure(&mtvec).contains("a CSR the trap check does not model"));
}

#[test]
fn a_trap_that_loses_tcontrol_or_changes_it_unsaved_fails() {
    // Without the restore a nested return leaves MPTE as the nested trap
    // set it.
    let lost = broken("    lw t0, 88(sp)\n    csrw {tcontrol}, t0\n", "");
    assert!(failure(&lost).contains("without the trap's tcontrol"));
    let unsaved = broken("    csrr t1, {tcontrol}\n    sw t1, 88(sp)\n", "");
    assert!(failure(&unsaved).contains("changes tcontrol before its frame holds it"));
    // Restored before the exit masks.
    let early = broken(
        "    csrci mstatus, {mie}\n    lw t0, 80(sp)",
        "    lw t0, 88(sp)\n    csrw {tcontrol}, t0\n    csrci mstatus, {mie}\n    lw t0, 80(sp)",
    );
    assert!(failure(&early).contains("tcontrol with interrupts enabled"));
}
