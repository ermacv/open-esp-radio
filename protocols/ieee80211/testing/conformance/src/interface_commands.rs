//! Configure ownership observations shared by backend fixtures. Exercise both
//! interfaces in FIFO order, recover their permissions, and submit both again.
//! Capture each opaque context's identity before admission and from the actual
//! terminal, including terminals drained during disable. A fixture must also
//! poll the event stream after both returns to detect an extra terminal.

use oer_ieee80211_contracts_port::VifRole;

use super::{Environment, Execution, Report};

/// When disable intersects the two admitted configuration commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Drain {
    /// Both rounds return through the running event stream.
    Running,
    /// Disable drains the first round while its commands are queued.
    Queued,
    /// Disable drains the first round after progress posted terminals.
    Posted,
}
/// One FIFO/context-reuse configuration case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Case {
    /// Point where disable intersects the first round.
    pub drain: Drain,
    /// Whether the AP command is admitted before the station command.
    pub ap_first: bool,
}
impl Case {
    /// Every disable point, with each interface admitted first.
    pub fn all() -> impl Iterator<Item = Self> {
        [Drain::Running, Drain::Queued, Drain::Posted]
            .into_iter()
            .flat_map(|drain| {
                [
                    Self {
                        drain,
                        ap_first: false,
                    },
                    Self {
                        drain,
                        ap_first: true,
                    },
                ]
            })
    }
    /// Configuration and disable require no injected hardware result.
    pub const fn execution(self) -> Execution {
        Execution::Hardware
    }
}
/// Backend fixture returning actual command contexts and permissions.
pub trait Harness {
    /// Storage for captured terminals, including any duplicate or extra return.
    type Returns: AsRef<[Identity]>;
    /// Execute both FIFO/reuse rounds through the real backend API.
    fn exercise(&mut self, case: Case) -> impl core::future::Future<Output = Trace<Self::Returns>>;
}
/// Run the shared configuration cases, accounting for every selected case.
pub async fn check(
    harness: &mut impl Harness,
    environment: Environment,
) -> Result<Report, (Case, Violation)> {
    let mut report = Report::default();
    for case in Case::all() {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        verify(harness.exercise(case).await).map_err(|why| (case, why))?;
        report.passed();
    }
    Ok(report)
}

/// A stable allocation owned solely by the opaque configuration context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    /// Virtual interface owning the command.
    pub role: VifRole,
    /// Identity captured from the opaque command context.
    pub context: usize,
}

/// Actual configuration context returns over two reuse rounds.
pub struct Trace<R> {
    /// Both original owners, in FIFO admission order.
    pub admitted: [Identity; 2],
    /// Actual terminals over the original and the reuse round, in FIFO order.
    pub returned: R,
    /// Context destructors observed before the last terminal was consumed.
    pub premature_drops: usize,
    /// A terminal observed after all four admitted commands returned.
    pub extra_terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A configuration context or terminal ownership violation.
pub enum Violation {
    /// The two admitted commands did not carry distinct interface/context identities.
    Interfaces,
    /// Terminals changed context identities, order or return count.
    IdentityOrOrder,
    /// A context was destroyed before its terminal was consumed.
    PrematureDrop,
    /// A terminal was observed after all commands had returned.
    ExtraTerminal,
}

/// Check FIFO return, context lifetime and absence of extra terminals.
pub fn verify<R: AsRef<[Identity]>>(trace: Trace<R>) -> Result<(), Violation> {
    let [first, second] = trace.admitted;
    if first.role == second.role || first.context == second.context {
        return Err(Violation::Interfaces);
    }
    if trace.returned.as_ref() != [first, second, first, second] {
        return Err(Violation::IdentityOrOrder);
    }
    if trace.premature_drops != 0 {
        return Err(Violation::PrematureDrop);
    }
    if trace.extra_terminal {
        return Err(Violation::ExtraTerminal);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trace() -> Trace<alloc::vec::Vec<Identity>> {
        let first = Identity {
            role: VifRole::Station,
            context: 1,
        };
        let second = Identity {
            role: VifRole::AccessPoint,
            context: 2,
        };
        Trace {
            admitted: [first, second],
            returned: alloc::vec![first, second, first, second],
            premature_drops: 0,
            extra_terminal: false,
        }
    }
    #[test]
    fn rejects_wrong_order_lost_context_and_extra_terminal() {
        assert_eq!(verify(trace()), Ok(()));
        let mut reordered = trace();
        reordered.returned.swap(0, 1);
        assert_eq!(verify(reordered), Err(Violation::IdentityOrOrder));
        let mut lost = trace();
        lost.returned.pop();
        assert_eq!(verify(lost), Err(Violation::IdentityOrOrder));
        let mut dropped = trace();
        dropped.premature_drops = 1;
        assert_eq!(verify(dropped), Err(Violation::PrematureDrop));
        let mut extra = trace();
        extra.extra_terminal = true;
        assert_eq!(verify(extra), Err(Violation::ExtraTerminal));
    }
}
