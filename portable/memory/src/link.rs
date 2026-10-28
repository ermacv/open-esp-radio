//! Typed ownership of symbols used by foreign declarations and image tools.
#![allow(
    unsafe_code,
    reason = "slot owners attest symbol names, kinds and exact ABI types"
)]

/// One component of an owned symbol name, including indexed symbol families.
pub enum NamePart {
    Text(&'static str),
    Decimal(u8),
}

/// A fixed name or indexed name family owned by a slot declaration.
pub struct SlotName(pub &'static [NamePart]);

impl SlotName {
    /// Exact matching, without allocating or interpreting arbitrary names.
    pub const fn matches(&self, name: &str) -> bool {
        let mut remaining = name.as_bytes();
        let mut parts = self.0;
        while let [part, tail @ ..] = parts {
            parts = tail;
            match part {
                NamePart::Text(text) => {
                    let mut bytes = text.as_bytes();
                    while let [expected, rest @ ..] = bytes {
                        bytes = rest;
                        let [actual, rest @ ..] = remaining else {
                            return false;
                        };
                        if *actual != *expected {
                            return false;
                        }
                        remaining = rest;
                    }
                }
                NamePart::Decimal(value) => {
                    let digits = [
                        b'0'.wrapping_add(*value / 100),
                        b'0'.wrapping_add(*value / 10 % 10),
                        b'0'.wrapping_add(*value % 10),
                    ];
                    let [_, tens, units] = digits;
                    let two = [tens, units];
                    let one = [units];
                    let mut digits: &[u8] = if *value >= 100 {
                        &digits
                    } else if *value >= 10 {
                        &two
                    } else {
                        &one
                    };
                    while let [expected, rest @ ..] = digits {
                        digits = rest;
                        let [actual, rest @ ..] = remaining else {
                            return false;
                        };
                        if *actual != *expected {
                            return false;
                        }
                        remaining = rest;
                    }
                }
            }
        }
        remaining.is_empty()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SlotKind {
    Function,
    Static,
}

/// Ownership of a foreign symbol's name, kind and complete Rust/ABI type.
///
/// # Safety
/// The owner reserves exactly `NAME`, with `KIND` and `Value` as its only
/// permitted definition. Function pointers include the ABI and complete
/// signature. No other slot or foreign contract may assign another kind or
/// type to any name in this slot's family. Definitions have no extra caller
/// preconditions. The name belongs to this owner, including any weak default
/// of that ABI; it cannot replace compiler or language-runtime symbols.
/// Every foreign reader must be sound for any safe
/// definition of `Value`; additional memory-safety obligations must be enforced
/// by its typed API. The linker rejects duplicate strong definitions.
// SAFETY: implementations must attest the unique external name/kind/type
// contract; every safe declaration and definition checks that attestation.
pub unsafe trait LinkSlot {
    type Value: Sync;
    const NAME: SlotName;
    const KIND: SlotKind;
}

/// The trace clock's fixed Rust ABI slot; trace itself stays entirely safe.
pub enum TraceClockSlot {}
// SAFETY: the trace clock's sole name denotes a safe Rust `fn() -> u64`.
// Its declaration and every accepted definition below check this same slot.
unsafe impl LinkSlot for TraceClockSlot {
    type Value = fn() -> u64;
    const NAME: SlotName = SlotName(&[NamePart::Text("__oer_trace_now_micros")]);
    const KIND: SlotKind = SlotKind::Function;
}

/// Declare or define an unsafe-owned typed symbol slot. Ordinary function
/// names are identifiers; interrupt names come from fixed families. Internal
/// emission arms also const-check the complete name against the slot owner,
/// so calling an emission arm directly cannot choose another symbol.
#[macro_export]
macro_rules! link_fn {
    (declare $slot:ty => $(#[$attribute:meta])* fn $symbol:ident($($argument:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?;) => {
        const _: () = {
            if !<$slot as $crate::LinkSlot>::NAME.matches(stringify!($symbol)) {
                panic!("foreign declaration names another slot");
            }
            if !matches!(<$slot as $crate::LinkSlot>::KIND, $crate::SlotKind::Function) {
                panic!("a static slot cannot declare a function");
            }
            let _: Option<<$slot as $crate::LinkSlot>::Value> = None::<fn($($ty),*) $(-> $ret)?>;
        };
        // SAFETY: the unsafe slot owner reserves this exact name for this
        // ABI/signature with no caller precondition; both halves check it.
        unsafe extern "Rust" {
            $(#[$attribute])*
            safe fn $symbol($($argument: $ty),*) $(-> $ret)?;
        }
    };
    (define $slot:ty => fn $symbol:ident($($argument:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? = $definition:path;) => {
        const _: () = {
            $crate::link_fn! { @function $slot, stringify!($symbol) =>
                extern "Rust" fn definition($($argument: $ty),*) $(-> $ret)? {
                    $definition($($argument),*)
                }
            }
        };
    };
    (interrupt $slot:ty, hart $hart:literal, line $line:literal => $($definition:tt)*) => {
        $crate::link_fn! { @function $slot, concat!("__oer_interrupt_h", $hart, "_l", $line) => $($definition)* }
    };
    (table $slot:ty => $($definition:tt)*) => {
        $crate::link_fn! { @static $slot, "__OER_INTERRUPT_TABLE" => $($definition)* }
    };
    (exemption $slot:ty, hart $hart:literal, line $line:literal => $($definition:tt)*) => {
        $crate::link_fn! { @static $slot, concat!("__OER_INTERRUPT_EXEMPT___oer_interrupt_h", $hart, "_l", $line) => $($definition)* }
    };
    (@function $slot:ty, $symbol:expr => $(#[$attribute:meta])* $vis:vis extern $abi:literal fn $name:ident($($argument:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? $body:block) => {
        $(#[$attribute])*
        // SAFETY: the unsafe slot owner reserves the exact name/kind/type;
        // inline const checks reject any name, kind or signature mismatch.
        #[unsafe(export_name = $symbol)]
        $vis extern $abi fn $name($($argument: $ty),*) $(-> $ret)? {
            const {
                if !<$slot as $crate::LinkSlot>::NAME.matches($symbol) {
                    panic!("definition names another slot");
                }
                if !matches!(<$slot as $crate::LinkSlot>::KIND, $crate::SlotKind::Function) {
                    panic!("a static slot cannot define a function");
                }
                let _: Option<<$slot as $crate::LinkSlot>::Value> = None::<extern $abi fn($($ty),*) $(-> $ret)?>;
            }
            $body
        }
    };
    (@static $slot:ty, $symbol:expr => $(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty = $value:expr;) => {
        const _: () = {
            if !<$slot as $crate::LinkSlot>::NAME.matches($symbol) {
                panic!("definition names another slot");
            }
            if !matches!(<$slot as $crate::LinkSlot>::KIND, $crate::SlotKind::Static) {
                panic!("a function slot cannot define a static");
            }
            let _: Option<<$slot as $crate::LinkSlot>::Value> = None::<$ty>;
        };
        $(#[$attribute])*
        // SAFETY: the unsafe slot owner reserves this exact name and static
        // type; the const checks above reject another name, kind or type.
        #[unsafe(export_name = $symbol)]
        $vis static $name: $ty = $value;
    };
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::{NamePart, SlotName};

    #[test]
    fn indexed_names_accept_only_the_exact_decimal_components() {
        for first in [0, 9, 10, 99, 100, 255] {
            for second in [0, 9, 10, 99, 100, 255] {
                // Runtime values use an owned local list; the matcher itself
                // also handles the promoted static lists in slot declarations.
                let parts = std::boxed::Box::leak(std::boxed::Box::new([
                    NamePart::Text("handler_h"),
                    NamePart::Decimal(first),
                    NamePart::Text("_l"),
                    NamePart::Decimal(second),
                ]));
                let name = SlotName(parts);
                assert!(name.matches(&std::format!("handler_h{first}_l{second}")));
                assert!(!name.matches(&std::format!("handler_h0{first}_l{second}")));
                assert!(!name.matches(&std::format!("handler_h{first}_l{second}extra")));
            }
        }
    }
}
