//! Streaming structural inventory over captured byte sources. No path access.
//! Archive framing and ELF inspection have one implementation each; consumers
//! choose storage or presentation through synchronous borrowed-record ports.
#![expect(
    clippy::cast_possible_truncation,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::unwrap_used,
    reason = "host tool: the ban covers the code an image reaches"
)]
use oer_tools_analysis_blobray_domain::*;
use oer_tools_analysis_riscv_model::*;
mod definition;
mod image;
pub use definition::{inspect_link_definition, undefined_names};
pub use image::{LinkRootFacts, ValidatedImage, inspect_link_input, validate_image};
mod cursor;
mod meter;
mod stream;
pub use cursor::{MemberCursor, MemberIndex, MemberRange};
use object::endian::Endian;
use object::read::elf::{FileHeader, Rel, Rela, SectionHeader, Sym};
use object::{Endianness, FileKind, elf};
pub use stream::{inspect_payload, inspect_source};

fn diagnostic(
    code: DiagnosticCode,
    context: impl Into<String>,
    message: impl ToString,
) -> Diagnostic {
    Diagnostic {
        code,
        context: context.into(),
        message: message.to_string(),
    }
}

mod function;
pub use function::{
    DataView, FunctionView, PreparedObject, prepare_captured, with_prepared_object,
};
