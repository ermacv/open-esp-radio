//! What the network core's turns did within a window: the requests they
//! took, how often a socket's budget ran out with requests left, the frames
//! the port accepted per turn, how publishing ended, what had run out when a
//! turn ended, and how each pass after a turn went on. Only a trace image on
//! the network core counts them; `available` says whether it did.

use oer_hil_services_load::NetworkTurns;
use serde_json::{Value, json};

/// The names of the accepted-per-turn bins.
const BINS: [&str; 7] = ["0", "1", "2", "3", "4", "5-8", "9+"];

/// `part` over `whole`, none without a whole.
fn ratio(part: u32, whole: u32) -> Option<f64> {
    (whole != 0).then(|| f64::from(part) / f64::from(whole))
}

/// The window's record, with the ratios the expectation reads.
pub fn window(turns: &NetworkTurns) -> Value {
    let bins: serde_json::Map<_, _> = BINS
        .iter()
        .zip(turns.accepted_by_turn)
        .map(|(name, count)| ((*name).to_owned(), count.into()))
        .collect();
    json!({
        "available": turns.available,
        "turns": turns.turns,
        "requests": {
            "reserve": turns.reserves,
            "send": turns.sends,
            "release": turns.releases,
            "close": turns.closes,
        },
        "reserve_per_send": ratio(turns.reserves, turns.sends),
        "budget_spent": turns.budget_spent,
        "budget_turns": turns.budget_turns,
        "budget_turn_share": ratio(turns.budget_turns, turns.turns),
        "intake_full": turns.intake_full,
        "accepted": turns.accepted,
        "accepted_per_turn": ratio(turns.accepted, turns.turns),
        "accepted_by_turn": bins,
        "publish_end": {
            "chose": turns.publish_chose,
            "refused": turns.publish_refused,
            "budget": turns.publish_budget,
            "deferred": turns.publish_deferred,
            "fault": turns.publish_fault,
        },
        "out_at_turn_end": {
            "blocks": turns.blocks_out,
            "credits": turns.credits_out,
        },
        "yields": turns.yields,
        "yields_with_work": turns.yields_with_work,
        "waits_pending": turns.waits_pending,
        "waits_ready": turns.waits_ready,
    })
}

/// The window's one line.
pub fn summary(turns: &NetworkTurns) -> String {
    if !turns.available {
        return "not counted (a trace image on the network core counts the turns)".to_owned();
    }
    let fixed = |value: Option<f64>| value.map_or("-".to_owned(), |value| format!("{value:.2}"));
    let bins = BINS
        .iter()
        .zip(turns.accepted_by_turn)
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} turns; reserve {} send {} release {} close {} (reserve/send {}); \
         budget spent in {} turns ({} sockets, share {}); intake full {}; \
         accepted {} ({} per turn; {bins}); publish chose {} refused {} budget {} \
         deferred {} fault {}; out at end: blocks {} credits {}; yields {} \
         ({} with work), waits pending {} ready {}",
        turns.turns,
        turns.reserves,
        turns.sends,
        turns.releases,
        turns.closes,
        fixed(ratio(turns.reserves, turns.sends)),
        turns.budget_turns,
        turns.budget_spent,
        fixed(ratio(turns.budget_turns, turns.turns)),
        turns.intake_full,
        turns.accepted,
        fixed(ratio(turns.accepted, turns.turns)),
        turns.publish_chose,
        turns.publish_refused,
        turns.publish_budget,
        turns.publish_deferred,
        turns.publish_fault,
        turns.blocks_out,
        turns.credits_out,
        turns.yields,
        turns.yields_with_work,
        turns.waits_pending,
        turns.waits_ready,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_records_every_count_and_its_line_names_them() {
        let turns = NetworkTurns {
            available: true,
            turns: 8,
            reserves: 16,
            sends: 16,
            budget_spent: 7,
            budget_turns: 7,
            accepted: 16,
            accepted_by_turn: [4, 0, 0, 0, 4, 0, 0],
            publish_chose: 8,
            yields: 8,
            yields_with_work: 7,
            ..NetworkTurns::default()
        };
        let value = window(&turns);
        assert_eq!(value["requests"]["reserve"], 16);
        assert_eq!(value["reserve_per_send"], 1.0);
        assert_eq!(value["budget_turn_share"], 0.875);
        assert_eq!(value["accepted_per_turn"], 2.0);
        assert_eq!(value["accepted_by_turn"]["4"], 4);
        assert_eq!(value["accepted_by_turn"]["5-8"], 0);
        assert_eq!(value["publish_end"]["chose"], 8);
        assert_eq!(value["out_at_turn_end"]["blocks"], 0);
        assert_eq!(
            summary(&turns),
            "8 turns; reserve 16 send 16 release 0 close 0 (reserve/send 1.00); \
             budget spent in 7 turns (7 sockets, share 0.88); intake full 0; \
             accepted 16 (2.00 per turn; 0:4 1:0 2:0 3:0 4:4 5-8:0 9+:0); publish chose 8 \
             refused 0 budget 0 deferred 0 fault 0; out at end: blocks 0 credits 0; \
             yields 8 (7 with work), waits pending 0 ready 0"
        );
    }

    #[test]
    fn an_image_that_does_not_count_says_so() {
        let turns = NetworkTurns::default();
        assert_eq!(window(&turns)["reserve_per_send"], Value::Null);
        assert!(summary(&turns).starts_with("not counted"));
    }
}
