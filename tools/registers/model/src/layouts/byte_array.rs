//! Owned-byte arrays use borrowed generated element views, never raw byte offsets.

use std::fmt::Write as _;

use super::{LayoutUnit, MemoryLayout, camel, push_doc, sources_line};

pub(super) fn render(output: &mut String, layout: &MemoryLayout, element: &MemoryLayout) {
    let Some(array) = &layout.array else { return };
    let ty = camel(&layout.name);
    let entry = camel(&element.name);
    let size = layout.size;
    let count = array.count;
    push_doc(output, &layout.description, "");
    if let Some(line) = sources_line(&layout.review, &layout.name) {
        let _ = writeln!(output, "/// {line}");
    }
    let _ = writeln!(
        output,
        "\
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
        pub struct {ty}<'a> {{ bytes: &'a [u8; {size}] }}\n\
        impl<'a> {ty}<'a> {{\n\
        pub const SIZE: usize = {size};\n\
        pub const COUNT: usize = {count};\n\
        /// Decode the complete array after the ownership handover.\n\
        pub const fn new(bytes: &'a [u8; {size}]) -> Self {{ Self {{ bytes }} }}\n\
        /// The array's complete byte image.\n\
        pub const fn bytes(self) -> &'a [u8; {size}] {{ self.bytes }}\n\
        /// Decode one element, or no element when the index is outside the array.\n\
        pub fn entry(self, index: usize) -> Option<{entry}<'a>> {{\n\
            let bytes = self.bytes.chunks_exact({entry}::SIZE).nth(index)?;\n\
            Some({entry}::new(bytes.try_into().ok()?))\n\
        }}\n\
        }}\n"
    );
    if element.units.iter().any(LayoutUnit::is_cpu_written) {
        let _ = writeln!(
            output,
            "\
            /// A writer over the array's CPU-owned elements.\n\
            #[derive(Debug, Eq, PartialEq)]\n\
            pub struct {ty}Mut<'a> {{ bytes: &'a mut [u8; {size}] }}\n\
            impl<'a> {ty}Mut<'a> {{\n\
            pub const fn new(bytes: &'a mut [u8; {size}]) -> Self {{ Self {{ bytes }} }}\n\
            /// Read the array as last written.\n\
            pub const fn view(&self) -> {ty}<'_> {{ {ty}::new(self.bytes) }}\n\
            /// Borrow a typed element writer, or no writer for an outside index.\n\
            pub fn entry(&mut self, index: usize) -> Option<{entry}Mut<'_>> {{\n\
                let bytes = self.bytes.chunks_exact_mut({entry}::SIZE).nth(index)?;\n\
                Some({entry}Mut::new(bytes.try_into().ok()?))\n\
            }}\n\
            }}\n"
        );
    }
}
