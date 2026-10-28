# oer-wire

Allocation-free fixed-width bitfield declarations and bounded byte cursors for
radio formats. `bitfield!` declares field names, half-open bit ranges, value types
and byte order; numeric and enum setters refuse values that do not fit and retain
all other bits. Wire enums preserve unknown values as `Reserved(raw)`. Overlap
is a compile-time error unless at least one field in that pair carries the
explicit `, view` suffix. A view never permits overlaps between other ordinary
fields.

Parenthesized integer fields (for example `(u16)`) return a `U16<WIDTH>` proof
and take that same proof in an infallible setter; raw integer fields admit input with
`Result`. `constant::<VALUE>()` checks a fixed value at compile time. Numeric
proofs and `PrefixLength<N>` retain admission instead of asserting it during
encoding or owned-buffer access. Prefix borrowing trims the unused array tail
through a slice iterator without a panicking split.

`Reader` borrows complete fields; `Writer` admits each write before modifying its
destination. Both return `Result` on short buffers without advancing. A frame
builder admits the whole frame before writing its optional fields and elements.
The crate has no dependencies, allocation, unsafe code or protocol knowledge.
See the crate documentation for declaration syntax.

The crate denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and disallowed-macro
Clippy lints. Its local Clippy configuration includes the common bans and the
complete sans-IO catalogue. Fixed array geometry is checked with const-only
panics; dynamic cursor lengths use checked arithmetic. Field access traverses
the admitted array with non-panicking splits; writing consumes an input-first
iterator pair and retains the exact unwritten suffix.
