//! The run store's pins as `cargo hil` keeps them: one hand note per run
//! (`cargo hil runs pin`), which `cargo hil runs unpin` drops.

use oer_tool_service_hil_run_bundle::{
    RunStore,
    store::{Note, Notes, Sidecar},
};

/// Pin `run` of `store` by hand, in one locked change: the run must still
/// exist under the lock, since a prune that read the pins before may have
/// deleted it since.
pub(crate) fn pin_existing_by_hand(store: &RunStore, run: &str, note: Note) -> crate::Result<()> {
    store.update_notes(Sidecar::Pins, |pins| {
        if !store.run(run).is_dir() {
            return Err(format!("run {run} was deleted before it could be pinned"));
        }
        pins.insert(run.to_owned(), note);
        Ok(())
    })??;
    Ok(())
}

/// Drop `run`'s pin; whether it had one.
pub(crate) fn unpin_by_hand(pins: &mut Notes, run: &str) -> bool {
    pins.remove(run).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(reason: &str) -> Note {
        Note {
            by: String::from("phy"),
            reason: reason.to_owned(),
            unix_millis: 1,
        }
    }

    #[test]
    fn a_hand_pin_refuses_a_run_pruned_meanwhile_and_unpins() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        assert!(pin_existing_by_hand(&store, "1-a", note("investigation")).is_err());
        assert!(store.read::<Notes>(Sidecar::Pins).unwrap().is_empty());
        std::fs::create_dir_all(store.run("1-a")).unwrap();
        pin_existing_by_hand(&store, "1-a", note("investigation")).unwrap();
        assert_eq!(
            store.read::<Notes>(Sidecar::Pins).unwrap()["1-a"].reason,
            "investigation"
        );
        let mut pins = store.read::<Notes>(Sidecar::Pins).unwrap();
        assert!(unpin_by_hand(&mut pins, "1-a"));
        assert!(!unpin_by_hand(&mut pins, "1-a"));
    }
}
