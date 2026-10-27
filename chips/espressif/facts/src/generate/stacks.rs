//! Per-hart stack placement, symbol tables and exact assembly coverage.

use super::*;

impl Vectors {
    /// Linker output sections and assertions, indexed by the complete geometry.
    /// Unused stacks have equal markers and contribute no alignment padding.
    pub fn stack_linker(&self) -> String {
        let mut text = String::from(
            "/* Generated from the chip hart geometry. */\nSECTIONS {\n.irq_stacks ORIGIN(INTERNAL_LOW) (NOLOAD) : {\n",
        );
        for hart in 0..self.harts {
            let _ = writeln!(
                text,
                "__runtime_cpu{hart}_irq_stack_bottom = ABSOLUTE(.);\n. += IRQ_STACK_HART{hart}_BYTES;\n__runtime_cpu{hart}_irq_stack_top = ABSOLUTE(.);"
            );
        }
        text.push_str("} > INTERNAL_LOW\n} INSERT BEFORE .isr.text;\nSECTIONS {\n.psram.task_stacks ALIGN(__runtime_psram_noinit_end, 64) (NOLOAD) : {\n");
        for hart in 0..self.harts {
            let _ = writeln!(
                text,
                ". = TASK_STACK_HART{hart}_BYTES > 0 ? ALIGN(64) : .;\n__runtime_cpu{hart}_task_stack_bottom = ABSOLUTE(.);\n. += TASK_STACK_HART{hart}_BYTES;\n__runtime_cpu{hart}_task_stack_top = ABSOLUTE(.);"
            );
        }
        text.push_str(". = ALIGN(64);\n__runtime_task_stacks_end = ABSOLUTE(.);\n} > PSRAM_RUNTIME\n} INSERT BEFORE .bss;\n");
        for hart in 0..self.harts {
            for (kind, input, region) in [
                ("irq", "IRQ", "INTERNAL_LOW"),
                ("task", "TASK", "PSRAM_RUNTIME"),
            ] {
                let bottom = format!("__runtime_cpu{hart}_{kind}_stack_bottom");
                let top = format!("__runtime_cpu{hart}_{kind}_stack_top");
                let _ = writeln!(
                    text,
                    "ASSERT({top} - {bottom} == {input}_STACK_HART{hart}_BYTES, \"hart {hart} exact {kind} stack allocation\")\nASSERT({bottom} >= ORIGIN({region}) && {top} <= ORIGIN({region}) + LENGTH({region}), \"hart {hart} {kind} stack outside memory\")\nASSERT(({input}_STACK_HART{hart}_BYTES % 16) == 0, \"hart {hart} {kind} stack alignment\")"
                );
            }
            let _ = writeln!(
                text,
                "ASSERT(IRQ_STACK_HART{hart}_BYTES == 0 || (IRQ_STACK_HART{hart}_BYTES > __runtime_cpu{hart}_irq_stack_guard_bytes && (__runtime_cpu{hart}_irq_stack_bottom % __runtime_cpu{hart}_irq_stack_guard_bytes) == 0), \"hart {hart} IRQ stack bottom must be guard aligned with usable storage\")"
            );
            if hart != self.boot_hart {
                let _ = writeln!(
                    text,
                    "ASSERT((TASK_STACK_HART{hart}_BYTES == 0) == (IRQ_STACK_HART{hart}_BYTES == 0), \"hart {hart} task and IRQ use must agree\")"
                );
            }
        }
        let boot = self.boot_hart;
        let _ = writeln!(
            text,
            "ASSERT(HART_COUNT == {}, \"stack geometry disagrees with facts\")\nASSERT(TASK_STACK_HART{boot}_BYTES > 0 && IRQ_STACK_HART{boot}_BYTES > 0, \"boot hart needs both stacks\")\nASSERT(ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW) <= ORIGIN(PSRAM_RUNTIME), \"every IRQ stack must lie below every task stack\")",
            self.harts
        );
        text
    }

    /// Macros expanding inside the runtime's existing extern block and into
    /// an array of addresses. No additional unsafe declaration is emitted.
    pub fn stack_symbols(&self) -> String {
        let mut text = String::from(
            "// Generated from the chip hart geometry.\nmacro_rules! stack_markers { () => {\n",
        );
        for hart in 0..self.harts {
            for suffix in [
                "irq_stack_bottom",
                "irq_stack_top",
                "irq_stack_guard_bytes",
                "task_stack_bottom",
                "task_stack_top",
            ] {
                let _ = writeln!(text, "safe static __runtime_cpu{hart}_{suffix}: [u8; 0];");
            }
        }
        text.push_str("}; }\nmacro_rules! stack_addresses { () => { [\n");
        for hart in 0..self.harts {
            let _ = writeln!(
                text,
                "HartStacks {{ irq: range(addr_of!(__runtime_cpu{hart}_irq_stack_bottom), addr_of!(__runtime_cpu{hart}_irq_stack_top)), task: range(addr_of!(__runtime_cpu{hart}_task_stack_bottom), addr_of!(__runtime_cpu{hart}_task_stack_top)), guard: addr_of!(__runtime_cpu{hart}_irq_stack_guard_bytes) as usize }},"
            );
        }
        text.push_str("] }; }\n");
        text
    }

    /// Exact coverage inventory plus the reviewed, handwritten ABI obligations.
    pub fn stack_coverage(&self) -> Result<String> {
        let template = include_str!("../../../staged-layout/stack-coverage-reviews.toml");
        if !template.contains("symbols = [") {
            return Err("missing assembly coverage symbols".into());
        }
        let entries = entry_symbols(self)
            .iter()
            .map(|entry| format!("{entry:?}, "))
            .collect::<String>();
        Ok(format!(
            "# Generated by oer-espressif-facts; edit stack-coverage-reviews.toml and chip facts.\n{}",
            template.replacen("symbols = [", &format!("symbols = [{entries}"), 1)
        ))
    }
}
