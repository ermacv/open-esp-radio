//! Streaming structural inventory over captured byte sources. No path access.
//! Archive framing and ELF inspection have one implementation each; consumers
//! choose storage or presentation through synchronous borrowed-record ports.
use blobray_domain::*;
use oer_riscv_model::*;
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
pub use function::{DataView, FunctionView, PreparedObject, with_prepared_object};
