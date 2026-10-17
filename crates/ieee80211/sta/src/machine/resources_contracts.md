A station needs at least one ticket and small buffer, checked at compile
time; eight remains the maximum per pool. Open mode accepts zero group-key
slots. The resources must match `StaTokens::GROUP_SLOTS`. The personal constructor
also checks at compile time that this capacity is nonzero.

No ticket:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _, _) -> _ = construct::<0, 1, 1>;
core::hint::black_box(constructor);
}
```

No buffer:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _, _) -> _ = construct::<1, 0, 1>;
core::hint::black_box(constructor);
}
```

Too many tickets:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _, _) -> _ = construct::<9, 1, 1>;
core::hint::black_box(constructor);
}
```

The compiling twin: one owner of each kind:

```
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _, _) -> _ = construct::<1, 1, 1>;
core::hint::black_box(constructor);
}
```

Open-mode composition may supply no group slots:

```
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _, _, _) -> _ = construct::<1, 1, 0>;
core::hint::black_box(constructor);
}
```
