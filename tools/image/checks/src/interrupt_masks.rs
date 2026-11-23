//! Who masks interrupts: every instruction of the runtime image that clears
//! `mstatus.MIE` or writes the CLIC's threshold, with the function that holds
//! it. A report, never a gate: no list of allowed functions, no count.
//!
//! The CSR numbers are the architecture's, from `oer-portable-riscv` (its
//! `SOURCE:` lines): `mstatus` and the CLIC's `mintthresh`.

use std::fmt::Write as _;

use oer_tools_analysis_riscv_decode::{CsrOp, Extension, Extensions, Instruction, Operand};

use crate::{Result, stack::Image};

pub const REPORT: &str = "interrupt-masks.txt";

/// `mstatus.MIE`, bit 3.
///
/// SOURCE: RISC-V Privileged Architecture, "Machine Status Register
/// (mstatus)", `MIE` (bit 3).
const MIE: u32 = 1 << 3;

/// What one instruction does to the hart's interrupts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mask {
    /// Clears `mstatus.MIE` by an immediate with bit 3.
    ClearsMie,
    /// Clears bits of `mstatus` named by a register: `MIE` among them or not.
    ClearsMstatusBits,
    /// Writes `mstatus` whole.
    WritesMstatus,
    /// Writes, sets or clears the CLIC's threshold.
    WritesThreshold,
}

impl Mask {
    const fn name(self) -> &'static str {
        match self {
            Self::ClearsMie => "clears mstatus.MIE",
            Self::ClearsMstatusBits => "clears mstatus bits by a register",
            Self::WritesMstatus => "writes mstatus",
            Self::WritesThreshold => "writes the threshold",
        }
    }
}

/// What `instruction` does to the interrupts, if anything.
pub(crate) fn mask_of(instruction: &Instruction) -> Option<Mask> {
    let Instruction::Extension(Extension::Csr {
        op, source, csr, ..
    }) = instruction
    else {
        return None;
    };
    // A set or a clear by `x0` or by the immediate 0 reads only.
    let writes = !matches!(
        (op, source),
        (
            CsrOp::Set | CsrOp::Clear,
            Operand::Register(0) | Operand::Immediate(0)
        )
    );
    match (*csr, op, source) {
        (oer_portable_riscv::machine::MSTATUS, CsrOp::Clear, Operand::Immediate(bits))
            if bits & MIE != 0 =>
        {
            Some(Mask::ClearsMie)
        }
        (oer_portable_riscv::machine::MSTATUS, CsrOp::Clear, Operand::Register(register))
            if *register != 0 =>
        {
            Some(Mask::ClearsMstatusBits)
        }
        (oer_portable_riscv::machine::MSTATUS, CsrOp::Write, _) => Some(Mask::WritesMstatus),
        (oer_portable_riscv::clic::MINTTHRESH, _, _) if writes => Some(Mask::WritesThreshold),
        _ => None,
    }
}

/// One masking instruction: its address, its text, its kind.
pub(crate) struct Site {
    pub(crate) address: u32,
    pub(crate) instruction: String,
    pub(crate) mask: Mask,
}

/// Every masking instruction of the function whose code is `code`, at
/// `address`, decoded from its start.
pub(crate) fn scan(address: u32, code: &[u8]) -> Vec<Site> {
    let mut sites = Vec::new();
    let mut at = 0_usize;
    while at < code.len() {
        let Some((instruction, length)) = code
            .get(at..)
            .and_then(|bytes| oer_tools_analysis_riscv_decode::decode(bytes, Extensions::ALL))
        else {
            // Not an instruction the decoder knows: the next halfword.
            at = at.saturating_add(2);
            continue;
        };
        if let Some(mask) = mask_of(&instruction) {
            let offset = u32::try_from(at).unwrap_or(u32::MAX);
            sites.push(Site {
                address: address.saturating_add(offset),
                instruction: instruction.to_string(),
                mask,
            });
        }
        at = at.saturating_add(length.max(2));
    }
    sites
}

/// The report of the runtime image: one row a masking instruction, by
/// address, with the function that holds it; or that there is none.
pub fn report(image: &Image) -> Result<String> {
    let elf = oer_tools_analysis_elf::Elf::parse(&image.elf)?;
    let mut rows = Vec::new();
    for function in &image.functions {
        let section = elf.section(function.section)?;
        let start = u64::from(function.address)
            .checked_sub(section.address)
            .ok_or("a function before its section")?;
        let start = usize::try_from(start)?;
        let end = start.saturating_add(usize::try_from(function.size)?);
        let Some(code) = section.data.get(start..end) else {
            continue;
        };
        let name = image
            .names
            .get(&function.address)
            .cloned()
            .unwrap_or_else(|| format!("{:#010x}", function.address));
        rows.extend(
            scan(function.address, code)
                .into_iter()
                .map(|site| (site, name.clone())),
        );
    }
    rows.sort_by_key(|(site, _)| site.address);
    Ok(render(&rows))
}

pub(crate) fn render(rows: &[(Site, String)]) -> String {
    let mut report = String::from(
        "Instructions that clear mstatus.MIE or write the CLIC threshold (a report, not a check)\naddress\tinstruction\tdoes\tfunction\n",
    );
    if rows.is_empty() {
        report.push_str("none\n");
    }
    for (site, function) in rows {
        // Writing into a String cannot fail.
        let _ = writeln!(
            report,
            "{:#010x}\t{}\t{}\t{function}",
            site.address,
            site.instruction,
            site.mask.name()
        );
    }
    report
}

#[cfg(test)]
mod tests;
