//! The network core's turn profile within a window: the sums of a
//! least-squares fit of each turn's active cycles to the frames it took in,
//! the frames its port accepted, the notices it handed the policy and the
//! requests it took; the turns of one kind of work by its count; the turns
//! by cycles, notices and frames; the publications in turns that settled;
//! and the station's return bursts; the steps inside the turns (each
//! publication attempt's choice, transmit and answer, the flush, the notice
//! phases and their debits), what the policy's choices pulled and weighed,
//! what came home, the loans at each frame left unlent for want of credit,
//! and the datapath actor's intakes. Only a trace image counts them;
//! `available` says whether it did. The record keeps the sums, from which a
//! reader fits; the summary prints the means.

use oer_hil_services_load::{
    PROFILE_STEPS, PROFILE_VARIABLES, ReturnBursts, StepCost, TurnCell, TurnCells, TurnChoices,
    TurnLoans, TurnMoments, TurnSpread, TurnStepCosts,
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
/// The names of the steps, the choices' ends, the reasons work was withheld
/// and the kinds of the port's refusals of chosen work, in their order.
const STEPS: [&str; PROFILE_STEPS] = ["choice", "transmit", "answer", "flush", "notices", "debit"];
const CHOICE_ENDS: [&str; 4] = ["work", "empty", "withheld", "fault"];
const WITHHELD: [&str; 3] = ["no_grant", "capacity", "no_work"];
const UNSENT: [&str; 7] = [
    "stale",
    "port",
    "rejected",
    "slot_too_short",
    "declined",
    "fill_misreported",
    "port_contract",
];
const DOUBLING_BINS: [&str; 7] = ["0", "1", "2", "3-4", "5-8", "9-16", "17+"];
const FLUSH_BINS: [&str; 9] = ["0", "1", "2", "3", "4", "5", "6", "7", "8+"];
const CHOICE_BINS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9+"];
const LOAN_BINS: [&str; 6] = ["1", "2", "3-4", "5-8", "9-16", "17+"];
const RETURNED_BINS: [&str; 6] = ["0", "1", "2", "3-4", "5-8", "9+"];

/// The six pages of one window's profile.
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile {
    pub moments: TurnMoments,
    pub cells: TurnCells,
    pub spread: TurnSpread,
    pub steps: TurnStepCosts,
    pub choices: TurnChoices,
    pub loans: TurnLoans,
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

fn step(cost: &StepCost) -> Value {
    json!({
        "count": cost.count,
        "cycles": cost.cycles,
        "cycles_squared": cost.squared,
        "mean_cycles": (cost.count != 0).then(|| cost.cycles as f64 / f64::from(cost.count)),
    })
}

fn steps(steps: &[StepCost; PROFILE_STEPS]) -> Value {
    Value::Object(
        STEPS
            .iter()
            .zip(steps)
            .map(|(name, cost)| ((*name).to_owned(), step(cost)))
            .collect(),
    )
}

fn choices(choices: &TurnChoices) -> Value {
    let [examined, walks, candidates, accounts, widened] = choices.weighed;
    let [least, ceiling] = choices.air_costs;
    json!({
        "examined": examined,
        "walks": walks,
        "candidates": candidates,
        "accounts": accounts,
        "widened": widened,
        "chosen": named(&CHOICE_ENDS, &choices.chosen),
        "withheld": named(&WITHHELD, &choices.withheld),
        "unsent": named(&UNSENT, &choices.unsent),
        "examined_by_choice": named(&DOUBLING_BINS, &choices.examined_by_choice),
        "accepted_by_flush": named(&FLUSH_BINS, &choices.accepted_by_flush),
        "choices_by_turn": named(&CHOICE_BINS, &choices.choices_by_turn),
        "air_costs_computed": { "least": least, "ceiling": ceiling },
    })
}

fn loans(loans: &TurnLoans) -> Value {
    let [loans_back, loan_turns, blocks_home, block_turns] = loans.home;
    let [drops, frames, outstanding, returned, with_returned] = loans.no_credit;
    let home_only = &loans.home_only;
    let intakes = &loans.intakes;
    json!({
        "loans_back": loans_back,
        "loan_turns": loan_turns,
        "blocks_home": blocks_home,
        "block_turns": block_turns,
        "home_only": {
            "turns": home_only.turns,
            "cycles": home_only.cycles,
            "cycles_squared": home_only.squared,
            "loans": home_only.loans,
            "mean_cycles": (home_only.turns != 0)
                .then(|| home_only.cycles as f64 / f64::from(home_only.turns)),
        },
        "loans_by_turn": named(&LOAN_BINS, &loans.loans_by_turn),
        "no_credit": {
            "takes": drops,
            "frames": frames,
            "outstanding": outstanding,
            "returned": returned,
            "takes_with_returned": with_returned,
            "returned_at_drop": named(&RETURNED_BINS, &loans.returned_at_drop),
        },
        "actor_intakes": {
            "intakes": intakes.intakes,
            "taking": intakes.taking,
            "msdus": intakes.msdus,
            "msdus_by_intake": named(&DOUBLING_BINS, &intakes.msdus_by_intake),
        },
    })
}

/// The window's record: every sum as the image counted it.
pub fn window(profile: &Profile) -> Value {
    let Profile {
        moments,
        cells: cell,
        spread,
        steps: step_costs,
        choices: choice,
        loans: loan,
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
        "steps": steps(&step_costs.steps),
        "turn_steps": steps(&step_costs.turn_steps),
        "choices": choices(choice),
        "loans": loans(loan),
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
    let per = |cost: &StepCost| {
        if cost.count == 0 {
            0.0
        } else {
            cost.cycles as f64 / f64::from(cost.count)
        }
    };
    let [choice, transmit, answer, _, _, debit] = &profile.steps.steps;
    let [examined, ..] = profile.choices.weighed;
    let [loans_back, ..] = profile.loans.home;
    let [drops, ..] = profile.loans.no_credit;
    format!(
        "{turns} turns, {:.0} cycles each, frames {:.2} accepted {:.2} notices {:.2} \
         requests {:.2}; {} empty, {} idle; {} return bursts of {:.2}; {} choices of {:.0} \
         cycles pulling {:.2}, transmit {:.0}, answer {:.0}, debit {:.0}; {} loans back, \
         {} home-only turns; {} no-credit takes",
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
        choice.count,
        per(choice),
        if choice.count == 0 {
            0.0
        } else {
            f64::from(examined) / f64::from(choice.count)
        },
        per(transmit),
        per(answer),
        per(debit),
        loans_back,
        profile.loans.home_only.turns,
        drops,
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
        profile.steps.steps[0] = StepCost {
            count: 2,
            cycles: 900,
            squared: 450_000,
        };
        profile.choices.weighed = [6, 2, 4, 2, 0];
        profile.loans.home = [5, 2, 0, 0];
        profile.loans.no_credit = [1, 1, 12, 3, 1];
        profile.loans.returned_at_drop[3] = 1;
        let record = window(&profile);
        assert_eq!(record["steps"]["choice"]["mean_cycles"], 450.0);
        assert_eq!(record["steps"]["debit"]["mean_cycles"], Value::Null);
        assert_eq!(record["choices"]["examined"], 6);
        assert_eq!(record["loans"]["loans_back"], 5);
        assert_eq!(record["loans"]["no_credit"]["returned"], 3);
        assert_eq!(record["loans"]["no_credit"]["returned_at_drop"]["3-4"], 1);
        let line = summary(&profile);
        assert!(line.starts_with("4 turns, 1000 cycles each"), "{line}");
        assert!(
            line.contains("2 choices of 450 cycles pulling 3.00"),
            "{line}"
        );
        assert!(line.contains("5 loans back"), "{line}");
        profile.moments.available = false;
        assert_eq!(summary(&profile), "not counted (no trace image)");
    }
}
