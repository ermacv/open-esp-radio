//! What a caller may know of a callee's results and argument-bit flow.
//!
//! [`CallSummaries`] supplies one shared contract for value and bit-flow users.
//! A missing summary means unknown results and May consumption of every
//! argument bit. Result constants require a proof at every return; argument
//! dependencies describe consumed bits and physical result positions.
/// The function a call reaches, as its caller names it: an address in a
/// linked image, or the global symbol a relocatable object's call names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Callee<'a> {
    Address(u32),
    Symbol(&'a [u8]),
}

/// One argument's source-bit consumption. May remains a use.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArgumentConsumption {
    pub exact: u32,
    pub may: u32,
}

/// Returned dependencies in physical result positions. Exact singleton maps
/// are bit wiring; May maps retain conservative dependencies. Inversion is
/// explicit so an inverted bit can never prove unchanged preservation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReturnedArgument {
    pub argument: u8,
    pub register: u8,
    pub exact: [u32; 32],
    pub may: [u32; 32],
    pub inverted: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArgumentSummary {
    /// a0 through a7, indexed by ABI argument word.
    pub consumed: [ArgumentConsumption; 8],
    pub returned: Vec<ReturnedArgument>,
}

/// ABI shape supplied by a signature through the same callee port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallContract {
    pub arguments: u8,
    pub results: u8,
}

/// One producer's complete callee contract, shared by value and bit-flow users.
/// Dynamic storage is admitted and retained by the producer, never this port.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CallSummary {
    pub contract: Option<CallContract>,
    /// True only for a signature fallback; computed consumption stays authoritative.
    pub contract_only: bool,
    /// Sorted distinct constants at every return, for a0 and a1; a missing
    /// slot means the result is not a proven constant set.
    pub results: [Option<Vec<u32>>; 2],
    pub arguments: ArgumentSummary,
}
impl CallSummary {
    pub fn returns(&self, register: u8) -> bool {
        register
            .checked_sub(10)
            .is_some_and(|word| word < self.contract.map_or(2, |c| c.results))
    }
    pub fn result(&self, register: u8) -> Option<&[u32]> {
        if !self.returns(register) {
            return None;
        }
        self.results
            .get(usize::from(register.checked_sub(10)?))?
            .as_deref()
    }
}

/// Facts of the function a call reaches. Absence means unknown results and
/// May consumption of every argument bit. Consumers never invent signatures.
pub trait CallSummaries {
    /// Contract of the currently analyzed function, when scoped by its producer.
    fn current(&self) -> Option<&CallSummary> {
        None
    }
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary>;
}

/// No callee summary is known. This sentinel supplies neither result values
/// nor argument narrowing, and does not prove that a function is noreturn.
pub struct NoSummaries;
impl CallSummaries for NoSummaries {
    fn summary(&self, _: Callee<'_>) -> Option<&CallSummary> {
        None
    }
}

/// Proof of a callee's control-flow behavior, independent of the values
/// it might return. Missing summaries ([`NoSummaries`]) do not prove noreturn.
pub trait CallBehavior {
    /// True only when `callee` is proven never to return. Missing or
    /// ambiguous facts must answer false and retain the continuation.
    fn noreturn(&self, callee: Callee<'_>) -> bool;
}

/// No return-behavior proofs: every call may continue.
pub struct UnknownCallBehavior;

impl CallBehavior for UnknownCallBehavior {
    fn noreturn(&self, _: Callee<'_>) -> bool {
        false
    }
}
