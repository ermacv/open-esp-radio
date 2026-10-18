//! The run store's pins as `cargo hil` keeps them: a hand note (`cargo hil
//! runs pin`) and the evidence archive's holders side by side in one note.
//!
//! A note's reason is the hand reason, the archive's holders, or both:
//! `<hand reason>; held for the evidence archive by: <holder>, ...`. A
//! holder is a version packed and not yet published, or the selection
//! `cargo hil archive ref` made (`selection-<millis>-<process>-<n>`). Each
//! side changes only itself: `runs unpin` drops the hand reason and keeps
//! the holders, publishing a version drops only that version, and a run
//! stays pinned while either side remains (rule 5).

use oer_hil_run_bundle::{
    RunStore,
    store::{Note, Notes, Sidecar},
};

/// Where the archive's holders start in a note's reason.
pub(crate) const ARCHIVE: &str = "held for the evidence archive by: ";

/// Between the hand reason and the archive's holders.
const BOTH: &str = "; ";

/// The prefix of a holder `cargo hil archive ref` makes.
pub(crate) const SELECTION: &str = "selection-";

/// A new selection's holder, unique to this process and moment.
pub(crate) fn new_selection(now: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MADE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{SELECTION}{now}-{}-{}",
        std::process::id(),
        MADE.fetch_add(1, Ordering::Relaxed)
    )
}

/// A note's two sides.
#[derive(Debug, Eq, PartialEq)]
struct Sides<'a> {
    hand: Option<&'a str>,
    holders: Vec<&'a str>,
}

fn sides(reason: &str) -> Sides<'_> {
    match reason.find(ARCHIVE) {
        Some(at) => Sides {
            hand: (at > 0).then(|| reason[..at].strip_suffix(BOTH).unwrap_or(&reason[..at])),
            holders: reason[at + ARCHIVE.len()..].split(", ").collect(),
        },
        None => Sides {
            hand: Some(reason),
            holders: Vec::new(),
        },
    }
}

/// The reason of `sides`, or `None` when neither side remains.
fn reason(hand: Option<&str>, holders: &[&str]) -> Option<String> {
    let archive = (!holders.is_empty()).then(|| format!("{ARCHIVE}{}", holders.join(", ")));
    match (hand, archive) {
        (None, None) => None,
        (Some(hand), None) => Some(hand.to_owned()),
        (None, Some(archive)) => Some(archive),
        (Some(hand), Some(archive)) => Some(format!("{hand}{BOTH}{archive}")),
    }
}

/// The archive holders of a note.
pub(crate) fn holders(note: &Note) -> Vec<&str> {
    sides(&note.reason).holders
}

/// Add `holder` to the pin of `run`; a hand reason stays.
pub(crate) fn hold(pins: &mut Notes, run: &str, holder: &str, by: &str, now: u64) {
    let next = match pins.get(run) {
        None => Note {
            by: by.to_owned(),
            reason: reason(None, &[holder]).expect("one holder"),
            unix_millis: now,
        },
        Some(note) => {
            let mut sides = sides(&note.reason);
            if sides.holders.contains(&holder) {
                return;
            }
            sides.holders.push(holder);
            Note {
                reason: reason(sides.hand, &sides.holders).expect("a holder"),
                ..note.clone()
            }
        }
    };
    pins.insert(run.to_owned(), next);
}

/// Drop `holder` from the pin of `run`; whether it held it. The pin goes
/// when no hand reason and no other holder remains.
pub(crate) fn release(pins: &mut Notes, run: &str, holder: &str) -> bool {
    let Some(note) = pins.get(run) else {
        return false;
    };
    let sides = sides(&note.reason);
    if !sides.holders.contains(&holder) {
        return false;
    }
    let rest = sides
        .holders
        .iter()
        .copied()
        .filter(|held| *held != holder)
        .collect::<Vec<_>>();
    match reason(sides.hand, &rest) {
        Some(reason) => {
            let note = Note {
                reason,
                ..note.clone()
            };
            pins.insert(run.to_owned(), note);
        }
        None => {
            pins.remove(run);
        }
    }
    true
}

/// Each archive holder of `pins` and the runs it holds, in order.
pub(crate) fn holders_of(pins: &Notes) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut holders = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (run, note) in pins {
        for holder in self::holders(note) {
            holders
                .entry(holder.to_owned())
                .or_default()
                .push(run.clone());
        }
    }
    holders
}

/// Drop `holder` from every pin; the runs it held.
pub(crate) fn release_everywhere(pins: &mut Notes, holder: &str) -> usize {
    let runs = pins.keys().cloned().collect::<Vec<_>>();
    runs.into_iter()
        .filter(|run| release(pins, run, holder))
        .count()
}

/// Pin `run` by hand with `note`; the archive's holders stay. A reason that
/// names the archive's holders is refused: it would read as theirs.
pub(crate) fn pin_by_hand(pins: &mut Notes, run: &str, note: Note) -> Result<(), String> {
    if note.reason.contains(ARCHIVE) {
        return Err(format!("a hand pin's reason may not contain `{ARCHIVE}`"));
    }
    let holders = pins
        .get(run)
        .map(|existing| {
            holders(existing)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let holders = holders.iter().map(String::as_str).collect::<Vec<_>>();
    pins.insert(
        run.to_owned(),
        Note {
            reason: reason(Some(&note.reason), &holders).expect("a hand reason"),
            ..note
        },
    );
    Ok(())
}

/// Pin `run` of `store` by hand, in one locked change: the run must still
/// exist under the lock, since a prune that read the pins before may have
/// deleted it since; the archive's holders of it stay.
pub(crate) fn pin_existing_by_hand(store: &RunStore, run: &str, note: Note) -> crate::Result<()> {
    store.update_notes(Sidecar::Pins, |pins| {
        if !store.run(run).is_dir() {
            return Err(format!("run {run} was deleted before it could be pinned"));
        }
        pin_by_hand(pins, run, note)
    })??;
    Ok(())
}

/// Drop the hand reason of `run`'s pin; the archive holders that still pin
/// it, which only `cargo hil archive unpin` drops.
pub(crate) fn unpin_by_hand(pins: &mut Notes, run: &str) -> Vec<String> {
    let Some(note) = pins.get(run) else {
        return Vec::new();
    };
    let holders = holders(note)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let kept = holders.iter().map(String::as_str).collect::<Vec<_>>();
    match reason(None, &kept) {
        Some(reason) => {
            let note = Note {
                reason,
                ..note.clone()
            };
            pins.insert(run.to_owned(), note);
        }
        None => {
            pins.remove(run);
        }
    }
    holders
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
    fn hand_reasons_and_archive_holders_stay_side_by_side() {
        let mut pins = Notes::new();
        // A hand pin first, then the archive: both are kept.
        pin_by_hand(&mut pins, "1-a", note("investigation")).unwrap();
        hold(&mut pins, "1-a", "selection-1-1", "hil", 2);
        assert_eq!(
            pins["1-a"].reason,
            format!("investigation{BOTH}{ARCHIVE}selection-1-1")
        );
        // `runs unpin` drops only the hand reason: the archive still pins it.
        assert_eq!(unpin_by_hand(&mut pins, "1-a"), ["selection-1-1"]);
        assert_eq!(pins["1-a"].reason, format!("{ARCHIVE}selection-1-1"));
        // `runs pin` on an archive pin keeps its holders.
        pin_by_hand(&mut pins, "1-a", note("again")).unwrap();
        assert_eq!(holders(&pins["1-a"]), ["selection-1-1"]);
        // Releasing the last holder leaves the hand pin; then it can go.
        assert!(release(&mut pins, "1-a", "selection-1-1"));
        assert_eq!(pins["1-a"].reason, "again");
        assert!(unpin_by_hand(&mut pins, "1-a").is_empty());
        assert!(pins.is_empty());
        assert!(pin_by_hand(&mut pins, "1-a", note(ARCHIVE)).is_err());
    }

    #[test]
    fn a_hand_pin_refuses_a_run_pruned_meanwhile() {
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
    }

    #[test]
    fn each_selection_holds_and_releases_only_its_own_runs() {
        let mut pins = Notes::new();
        let (one, two) = ("selection-1-1", "selection-1-2");
        hold(&mut pins, "1-a", one, "a", 1);
        hold(&mut pins, "1-a", two, "b", 2);
        hold(&mut pins, "1-b", two, "b", 2);
        hold(&mut pins, "1-a", one, "a", 3);
        assert_eq!(holders(&pins["1-a"]), [one, two]);
        // Packing selection one into a version moves only one's pins.
        hold(&mut pins, "1-a", "evidence-v1", "a", 4);
        assert!(release(&mut pins, "1-a", one));
        assert_eq!(holders(&pins["1-a"]), [two, "evidence-v1"]);
        // Dropping selection two leaves one's version.
        assert_eq!(release_everywhere(&mut pins, two), 2);
        assert_eq!(holders(&pins["1-a"]), ["evidence-v1"]);
        assert!(!pins.contains_key("1-b"));
        assert!(!release(&mut pins, "1-a", two));
        assert!(new_selection(5).starts_with("selection-5-"));
    }
}
