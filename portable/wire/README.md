# oer-wire

Allocation-free fixed-width bitfield declarations and bounded byte cursors for
radio formats. `bitfield!` declares field names, half-open bit ranges, value types
and byte order; numeric and enum setters refuse values that do not fit and retain
all other bits. Wire enums preserve unknown values as `Reserved(raw)`. Overlap
is a compile-time error unless at least one field in that pair carries the
explicit `, view` suffix. A view never permits overlaps between other ordinary
fields.

`Reader` borrows complete fields; `Writer` admits each write before modifying its
destination. Both return `Result` on short buffers without advancing. A frame
builder admits the whole frame before writing its optional fields and elements.
The crate has no dependencies, allocation, unsafe code or protocol knowledge.
See the crate documentation for declaration syntax.
