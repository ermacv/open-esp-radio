//! Physical call sites resolved to keys of the shared callee-summary port.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum CallKey {
    Address(u32),
    Symbol(Vec<u8>),
}
impl CallKey {
    pub fn borrowed(&self) -> Callee<'_> {
        match self {
            Self::Address(a) => Callee::Address(*a),
            Self::Symbol(n) => Callee::Symbol(n),
        }
    }
    pub fn target(target: CallTarget) -> Option<Self> {
        target.address.map(Self::Address).or_else(|| {
            target
                .function
                .and_then(|f| f.name)
                .or_else(|| target.name.map(String::into_bytes))
                .map(Self::Symbol)
        })
    }
}

pub(crate) struct CallLinks<'m> {
    links: AdmittedVec<'m, (u64, CallKey)>,
    _names: MemoryReservation<'m>,
}
impl<'m> CallLinks<'m> {
    pub fn new(
        facts: &Facts<'_, 'm>,
        caller: &LibraryFunction,
        catalog: &Callees<'m>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut sites = callees::CallSites::new(facts, memory);
        let mut links = AdmittedVec::new(memory);
        let mut names = Ledger::new(memory, c)?;
        for record in facts.records() {
            c.checkpoint(1)?;
            if let FunctionRecord::CallInputs { offset, .. } = record
                && let Some(key) = sites
                    .at(*offset, caller, catalog, c)?
                    .and_then(CallKey::target)
            {
                if let CallKey::Symbol(name) = &key {
                    names.admit(name.capacity(), c)?;
                }
                links.push((*offset, key), c.position())?;
            }
        }
        c.checkpoint(links.len() as u64 * u64::from(links.len().max(1).ilog2() + 1))?;
        links.sort_unstable_by_key(|link| link.0);
        Ok(Self {
            links,
            _names: names.into_reservation(),
        })
    }
    pub fn callee(&self, site: u64) -> Option<Callee<'_>> {
        self.links
            .binary_search_by_key(&site, |l| l.0)
            .ok()
            .and_then(|id| self.links.get(id))
            .map(|l| l.1.borrowed())
    }
    pub fn keys(&self) -> impl Iterator<Item = &CallKey> {
        self.links.iter().map(|l| &l.1)
    }
}
