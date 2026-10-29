//! Actual linked allocations; the contract's `irq-stack-bytes` is a ceiling.

use super::{InterruptStacks, Result};
use oer_analysis_elf::{Elf, StorageBounds};
use oer_espressif_facts::{InterruptContract, Memory};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct LinkedStack {
    pub bottom: u64,
    pub bytes: u32,
    pub guard_bytes: u32,
    pub from_bound: bool,
}

pub(crate) fn read(
    elf: &Elf<'_>,
    contract: &InterruptContract,
    memory: &Memory,
) -> Result<BTreeMap<u32, LinkedStack>> {
    contract.harts.iter().map(|&hart| {
        let bottom = format!("__runtime_cpu{hart}_irq_stack_bottom");
        let top = format!("__runtime_cpu{hart}_irq_stack_top");
        let bounds = StorageBounds::range(elf, &bottom, &top)?
            .ok_or_else(|| format!("the runtime lacks `{bottom}`..`{top}`"))?;
        let guard = elf.address(&format!("__runtime_cpu{hart}_irq_stack_guard_bytes"))
            .ok_or_else(|| format!("hart {hart}'s IRQ stack lacks its guard size"))?;
        let requested = contract.irq_stack_guard_bytes.get(&hart)
            .ok_or_else(|| format!("hart {hart}'s IRQ stack lacks its guard contract"))?;
        if (guard != 0 && (guard != u64::from(*requested)
                || bounds.capacity <= guard || bounds.start % guard != 0))
            || bounds.capacity % 16 != 0
            || !memory.sram.contains_range(bounds.start, bounds.end) {
            return Err(format!("hart {hart}'s IRQ stack {:#010x}..{:#010x} ({} bytes) must be in SRAM, aligned to 16 bytes and its {guard}-byte guard", bounds.start, bounds.end, bounds.capacity).into());
        }
        let bytes = u32::try_from(bounds.capacity)?;
        let from_bound = elf.address(&format!("__runtime_cpu{hart}_irq_stack_sized_from_bound")).unwrap_or(0) == 1;
        Ok((hart, LinkedStack { bottom: bounds.start, bytes, guard_bytes: u32::try_from(guard)?, from_bound }))
    }).collect()
}

impl InterruptStacks {
    pub fn allocated_bytes(&self, hart: u32) -> u32 {
        self.allocations
            .get(&hart)
            .map_or(self.contract.interrupts.irq_stack_bytes, |stack| {
                stack.bytes
            })
    }

    pub fn usable_bytes(&self, hart: u32) -> Result<u32> {
        self.allocated_bytes(hart)
            .checked_sub(self.guard_bytes(hart)?)
            .ok_or_else(|| format!("hart {hart}'s IRQ guard exceeds its allocation").into())
    }

    pub(crate) fn guard_bytes(&self, hart: u32) -> Result<u32> {
        if let Some(stack) = self.allocations.get(&hart) {
            return Ok(stack.guard_bytes);
        }
        self.contract
            .interrupts
            .irq_stack_guard_bytes
            .get(&hart)
            .copied()
            .ok_or_else(|| format!("hart {hart}'s IRQ stack lacks its guard contract").into())
    }

    pub(super) fn render_allocations(&self) -> String {
        let mut out = String::from(
            "Interrupt stack allocations (bytes)\nhart\tbound\tmargin\tguard\tallocated\tsize source\n",
        );
        for hart in &self.harts {
            let guard = match self.guard_bytes(hart.core) {
                Ok(guard) => guard,
                Err(error) => {
                    let _ = writeln!(out, "hart {}: {error}", hart.core);
                    continue;
                }
            };
            let bytes = hart.bytes.unwrap_or(hart.partial);
            let partial = if hart.bytes.is_some() { "" } else { " + ?" };
            let margin = self
                .contract
                .interrupts
                .irq_stack_with_margin(bytes)
                .map_or_else(
                    |_| "overflow".to_owned(),
                    |total| (total - bytes).to_string(),
                );
            let source = if self
                .allocations
                .get(&hart.core)
                .is_some_and(|stack| stack.from_bound)
            {
                match crate::stack::conditions(&hart.assumptions(), !hart.level_drops().is_empty())
                {
                    None => "proven bound".to_owned(),
                    Some(conditions) => format!("conditional bound ({conditions})"),
                }
            } else {
                "contract maximum".to_owned()
            };
            let _ = writeln!(
                out,
                "{}\t{bytes}{partial}\t{margin} ({}%)\t{}\t{}\t{source}",
                hart.core,
                self.contract.interrupts.irq_stack_margin_percent,
                guard,
                self.allocated_bytes(hart.core)
            );
            if guard == 0 {
                let requested = self
                    .contract
                    .interrupts
                    .irq_stack_guard_bytes
                    .get(&hart.core)
                    .map_or_else(|| "missing".to_owned(), ToString::to_string);
                let _ = writeln!(
                    out,
                    "  hart {} guard omitted (reported only): requested {} bytes do not fit beside the bound plus margin with NAPOT alignment within the contract maximum {} bytes",
                    hart.core, requested, self.contract.interrupts.irq_stack_bytes
                );
            }
        }
        out
    }
}
