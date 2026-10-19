//! Concrete RV32 execution over `oer-analysis-riscv-lift`'s decoding and lifting; no
//! I/O authority.

#![forbid(unsafe_code)]
mod execution;
pub use execution::{RiscvExecutor, Rv32imacExecutor};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_decode::{Extensions, Inst, Instruction};
use oer_analysis_riscv_lift::{RiscvDecoder, decode_instruction};
use oer_analysis_riscv_model::*;
