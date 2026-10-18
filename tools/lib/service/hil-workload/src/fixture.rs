//! The fixtures of one repetition: the set the registry's fixture providers
//! prepared, and the cleanup scope every restoration records into.

use std::any::Any;

use crate::Result;

pub mod cleanup;

/// The fixtures prepared for one repetition, one of each type, dropped in
/// the reverse order of their preparation: a fixture prepared later may
/// lean on an earlier one.
#[derive(Default)]
pub struct Fixtures {
    prepared: Vec<Box<dyn Any>>,
}

impl Fixtures {
    /// Add `fixture`; a fixture of the same type prepared earlier is an error.
    pub fn insert<T: 'static>(&mut self, fixture: T) -> Result<()> {
        if self.get::<T>().is_some() {
            return Err(format!(
                "the fixture {} was prepared twice",
                std::any::type_name::<T>()
            )
            .into());
        }
        self.prepared.push(Box::new(fixture));
        Ok(())
    }

    /// The prepared fixture of type `T`, if a provider prepared one.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.prepared
            .iter()
            .find_map(|fixture| fixture.downcast_ref::<T>())
    }

    /// The prepared fixture of type `T`; an error naming it otherwise.
    pub fn require<T: 'static>(&self) -> Result<&T> {
        self.get::<T>().ok_or_else(|| {
            format!(
                "the scenario's plan prepared no {} fixture",
                std::any::type_name::<T>()
            )
            .into()
        })
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        while let Some(fixture) = self.prepared.pop() {
            drop(fixture);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    struct Logged(&'static str, Rc<RefCell<Vec<&'static str>>>);

    impl Drop for Logged {
        fn drop(&mut self) {
            self.1.borrow_mut().push(self.0);
        }
    }

    struct Ap(Logged);
    struct Peer(Logged);

    #[test]
    fn fixtures_are_found_by_type_and_dropped_last_prepared_first() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut fixtures = Fixtures::default();
        assert!(fixtures.require::<Ap>().is_err());
        fixtures.insert(Ap(Logged("ap", Rc::clone(&log)))).unwrap();
        fixtures
            .insert(Peer(Logged("peer", Rc::clone(&log))))
            .unwrap();
        assert!(
            fixtures
                .insert(Ap(Logged("second ap", Rc::clone(&log))))
                .is_err()
        );
        assert_eq!(fixtures.require::<Ap>().unwrap().0.0, "ap");
        assert_eq!(fixtures.require::<Peer>().unwrap().0.0, "peer");
        log.borrow_mut().clear();
        drop(fixtures);
        assert_eq!(*log.borrow(), ["peer", "ap"]);
    }
}
