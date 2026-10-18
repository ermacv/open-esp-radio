//! Concrete RV32 execution over `oer-riscv-lift`'s decoding and lifting; no
//! I/O authority.
mod execution;
use blobray_domain::*;
pub use execution::{RiscvExecutor, Rv32imacExecutor};
use oer_riscv_decode::{Extensions, Inst, Instruction};
use oer_riscv_lift::{RiscvDecoder, decode_instruction};
use oer_riscv_model::*;
