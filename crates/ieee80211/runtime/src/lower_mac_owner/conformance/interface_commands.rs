//! Configure ownership observations shared by backend fixtures. Exercise both
//! interfaces in FIFO order, recover their permissions, and submit both again.
//! Capture each opaque context's identity before admission and from the actual
//! terminal, including terminals drained during disable. A fixture must also
//! poll the event stream after both returns to detect an extra terminal.

use alloc::vec::Vec;
use oer_ieee80211_lower_mac::VifRole;

/// A stable allocation owned solely by the opaque configuration context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    pub role: VifRole,
    pub context: usize,
}

pub struct Trace {
    /// Both original owners, in FIFO admission order.
    pub admitted: [Identity; 2],
    /// Actual terminals over the original and the reuse round, in FIFO order.
    pub returned: Vec<Identity>,
    /// Context destructors observed before the last terminal was consumed.
    pub premature_drops: usize,
    /// A terminal observed after all four admitted commands returned.
    pub extra_terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    Interfaces,
    IdentityOrOrder,
    PrematureDrop,
    ExtraTerminal,
}

pub fn verify(trace: Trace) -> Result<(), Violation> {
    let [first, second] = trace.admitted;
    if first.role == second.role || first.context == second.context {
        return Err(Violation::Interfaces);
    }
    if trace.returned != [first, second, first, second] {
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
    fn trace() -> Trace {
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
