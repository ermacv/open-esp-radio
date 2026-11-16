A station takes its two places with the channel arbiter and the number of
group keys its port holds at once, `GROUPS`. Every constructor takes a
`GroupCapacity` proof that this number is nonzero, since any later profile
may protect its link.

One group key:

```
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _) -> _ = construct::<1>;
core::hint::black_box(constructor);
}
```

No group key:

```compile_fail,E0080
# include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources.rs"));
fn main() {
let constructor: fn(_, _, _) -> _ = construct::<0>;
core::hint::black_box(constructor);
}
```
