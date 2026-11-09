//! The network core's turn profile within a window: the sums of a
//! least-squares fit of each turn's active cycles to the frames it took in,
//! the frames its port accepted, the notices it handed the policy and the
//! requests it took; the turns of one kind of work by its count; the turns
//! by cycles, notices and frames; the publications in turns that settled;
//! and the station's return bursts. Only a trace image counts them;
//! `available` says whether it did. The record keeps the sums, from which a
//! reader fits; the summary prints the means.

use oer_hil_services_load::{
    PROFILE_VARIABLES, ReturnBursts, TurnCell, TurnCells, TurnMoments, TurnSpread,
};
use serde_json::{Value, json};

/// The names of the variables, in the moments' order.
pub const VARIABLES: [&str; PROFILE_VARIABLES] =
    ["one", "frames", "accepted", "notices", "requests", "cycles"];
/// The names of the gaps, in their order.
const GAPS: [&str; 3] = ["after_yield", "after_ready_wait", "after_pending_wait"];
/// The names of the notice cells and bins.
const NOTICE_CELLS: [&str; 7] = ["1", "2", "3-4", "5-8", "9-16", "17-32", "33+"];
const NOTICE_BINS: [&str; 8] = ["0", "1", "2", "3-4", "5-8", "9-16", "17-32", "33+"];
const FRAME_BINS: [&str; 9] = ["0", "1", "2", "3", "4", "5", "6", "7", "8+"];
const ITEM_CELLS: [&str; 8] = ["1", "2", "3", "4", "5", "6", "7", "8+"];
const BURST_BINS: [&str; 7] = ["1", "2", "3-4", "5-8", "9-12", "13-16", "17+"];
const OCCUPANCY_BINS: [&str; 6] = ["0", "1-4", "5-8", "9-12", "13-15", "16+"];

/// The three pages of one window's profile.
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile {
    pub moments: TurnMoments,
    pub cells: TurnCells,
    pub spread: TurnSpread,
}

/// The moment `Σ v[i] v[j]` of `moments`, in either order.
pub fn moment(moments: &TurnMoments, i: usize, j: usize) -> u64 {
    let (i, j) = (i.min(j), i.max(j));
    let index = (0..i).map(|row| PROFILE_VARIABLES - row).sum::<usize>() + (j - i);
    moments.moments.get(index).copied().unwrap_or(0)
}

/// `names` paired with `counts`, as a JSON object.
fn named<T: Copy + Into<Value>>(names: &[&str], counts: &[T]) -> Value {
    Value::Object(
        names
            .iter()
            .zip(counts)
            .map(|(name, count)| ((*name).to_owned(), (*count).into()))
            .collect(),
    )
}

fn cells(names: &[&str], cells: &[TurnCell], items: &str) -> Value {
    Value::Object(
        names
            .iter()
            .zip(cells)
            .filter(|(_, cell)| cell.turns != 0)
            .map(|(name, cell)| {
                (
                    (*name).to_owned(),
                    json!({
                        "turns": cell.turns,
                        "cycles": cell.cycles,
                        items: cell.items,
                        "mean_cycles": cell.cycles as f64 / f64::from(cell.turns),
                    }),
                )
            })
            .collect(),
    )
}

fn returns(returns: &ReturnBursts) -> Value {
    json!({
        "bursts": returns.bursts,
        "queued": returns.queued,
        "mean_burst": (returns.bursts != 0)
            .then(|| f64::from(returns.queued) / f64::from(returns.bursts)),
        "queued_by_burst": named(&BURST_BINS, &returns.queued_by_burst),
        "before_by_burst": named(&OCCUPANCY_BINS, &returns.before_by_burst),
        "after_by_burst": named(&OCCUPANCY_BINS, &returns.after_by_burst),
        "held_bursts": returns.held_bursts,
        "held": returns.held,
    })
}

/// The window's record: every sum as the image counted it.
pub fn window(profile: &Profile) -> Value {
    let Profile {
        moments,
        cells: cell,
        spread,
    } = profile;
    let sums: serde_json::Map<_, _> = (0..PROFILE_VARIABLES)
        .flat_map(|i| (i..PROFILE_VARIABLES).map(move |j| (i, j)))
        .map(|(i, j)| {
            let name = format!(
                "{}*{}",
                VARIABLES.get(i).unwrap_or(&"?"),
                VARIABLES.get(j).unwrap_or(&"?")
            );
            (name, moment(moments, i, j).into())
        })
        .collect();
    let gaps: serde_json::Map<_, _> = GAPS
        .iter()
        .zip(moments.gaps)
        .map(|(name, [count, cycles])| {
            (
                (*name).to_owned(),
                json!({ "count": count, "cycles": cycles }),
            )
        })
        .collect();
    json!({
        "available": moments.available,
        "moments": sums,
        "empty": {
            "turns": moments.empty[0],
            "cycles": moments.empty[1],
            "cycles_squared": moments.empty[2],
        },
        "idle": { "turns": moments.idle[0], "cycles": moments.idle[1] },
        "gaps": gaps,
        "receive_only": cells(&ITEM_CELLS, &cell.receive_only, "requests"),
        "transmit_only": cells(&ITEM_CELLS, &cell.transmit_only, "requests"),
        "notice_only": cells(&NOTICE_CELLS, &cell.notice_only, "notices"),
        "cycles_by_log2": spread.cycles_by_log2,
        "notices_by_turn": named(&NOTICE_BINS, &spread.notices_by_turn),
        "frames_by_turn": named(&FRAME_BINS, &spread.frames_by_turn),
        "settling": {
            "turns": spread.settling[0],
            "publishing": spread.settling[1],
            "accepted": spread.settling[2],
        },
        "returns": returns(&spread.returns),
    })
}

/// One line: the turns, their mean cycles and items, and the return bursts.
pub fn summary(profile: &Profile) -> String {
    let moments = &profile.moments;
    if !moments.available {
        return "not counted (no trace image)".to_owned();
    }
    let turns = moment(moments, 0, 0);
    if turns == 0 {
        return "no turn".to_owned();
    }
    let mean = |variable: usize| moment(moments, 0, variable) as f64 / turns as f64;
    let returns = &profile.spread.returns;
    format!(
        "{turns} turns, {:.0} cycles each, frames {:.2} accepted {:.2} notices {:.2} \
         requests {:.2}; {} empty, {} idle; {} return bursts of {:.2}",
        mean(5),
        mean(1),
        mean(2),
        mean(3),
        mean(4),
        moments.empty[0],
        moments.idle[0],
        returns.bursts,
        if returns.bursts == 0 {
            0.0
        } else {
            f64::from(returns.queued) / f64::from(returns.bursts)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_found_in_either_order() {
        let mut moments = TurnMoments::default();
        for (index, place) in moments.moments.iter_mut().enumerate() {
            *place = index as u64;
        }
        assert_eq!(moment(&moments, 0, 0), 0);
        assert_eq!(moment(&moments, 0, 5), 5);
        assert_eq!(moment(&moments, 1, 1), 6);
        assert_eq!(moment(&moments, 5, 1), 10);
        assert_eq!(moment(&moments, 5, 5), 20);
    }

    #[test]
    fn the_record_names_every_moment_and_only_filled_cells() {
        let mut profile = Profile::default();
        profile.moments.available = true;
        profile.moments.moments[0] = 4;
        profile.moments.moments[5] = 4_000;
        profile.cells.receive_only[2] = TurnCell {
            turns: 2,
            cycles: 3_000,
            items: 1,
        };
        profile.spread.returns.bursts = 2;
        profile.spread.returns.queued = 6;
        let record = window(&profile);
        assert_eq!(record["moments"]["one*one"], 4);
        assert_eq!(record["moments"]["one*cycles"], 4_000);
        assert_eq!(record["moments"].as_object().unwrap().len(), 21);
        let cells = record["receive_only"].as_object().unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells["3"]["mean_cycles"], 1_500.0);
        assert_eq!(record["returns"]["mean_burst"], 3.0);
        assert!(summary(&profile).starts_with("4 turns, 1000 cycles each"));
        profile.moments.available = false;
        assert_eq!(summary(&profile), "not counted (no trace image)");
    }
}
