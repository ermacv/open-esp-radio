//! What the network core's turns did within a window: the requests they
//! took, how often a socket's budget ran out with requests left, the frames
//! the port accepted per turn, how publishing ended, what had run out when a
//! turn ended, how each pass after a turn went on, the core's drops by
//! reason, and the receive loans out at each turn's end. Only a trace image
//! on the network core counts them; `available` says whether it did.

use oer_hil_services_load::{DROP_NAMES, NetworkTurns};
use serde_json::{Value, json};

/// The names of the accepted-per-turn bins.
const BINS: [&str; 7] = ["0", "1", "2", "3", "4", "5-8", "9+"];
/// The names of the reservation-size bins, in blocks.
const SIZES: [&str; 5] = ["1", "2", "3-4", "5-8", "9-16"];
/// The names of the loans-at-turn-end bins.
const LOANS: [&str; 6] = ["0", "1", "2", "3", "4", "5+"];

/// `names` paired with `counts`, as `name:count` words.
fn binned(names: &[&str], counts: &[u32]) -> String {
    names
        .iter()
        .zip(counts)
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(" ")
}

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
    let sizes: serde_json::Map<_, _> = SIZES
        .iter()
        .zip(turns.reserve_by_size)
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
        "reserve_blocks": turns.reserve_blocks,
        "blocks_per_reserve": ratio(turns.reserve_blocks, turns.reserves),
        "reserve_by_size": sizes,
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
            "fault": turns.publish_fault,
        },
        "notices_full": turns.notices_full,
        "out_at_turn_end": {
            "blocks": turns.blocks_out,
            "credits": turns.credits_out,
        },
        "yields": turns.yields,
        "yields_with_work": turns.yields_with_work,
        "waits_pending": turns.waits_pending,
        "waits_ready": turns.waits_ready,
        "producer": {
            "offered": turns.local_offered,
            "refused": turns.local_refused,
            "held": turns.local_held,
            "declined": turns.local_declined,
            "withdrawn": turns.local_withdrawn,
            "waits_offers": turns.local_waits_offers,
            "waits_changes": turns.local_waits_changes,
            "wakes": turns.producer_wakes,
            "unknown": turns.producer_unknown,
        },
        "producer_wakes_per_offer": ratio(turns.producer_wakes, turns.local_offered),
        "drops": DROP_NAMES
            .iter()
            .zip(turns.drops)
            .map(|(name, count)| ((*name).to_owned(), count.into()))
            .collect::<serde_json::Map<_, _>>(),
        "loans_at_turn_end": LOANS
            .iter()
            .zip(turns.loans_at_turn_end)
            .map(|(name, count)| ((*name).to_owned(), count.into()))
            .collect::<serde_json::Map<_, _>>(),
    })
}

/// The window's one line.
pub fn summary(turns: &NetworkTurns) -> String {
    if !turns.available {
        return "not counted (a trace image on the network core counts the turns)".to_owned();
    }
    let fixed = |value: Option<f64>| value.map_or("-".to_owned(), |value| format!("{value:.2}"));
    let bins = binned(&BINS, &turns.accepted_by_turn);
    let sizes = binned(&SIZES, &turns.reserve_by_size);
    let loans = binned(&LOANS, &turns.loans_at_turn_end);
    let drops = DROP_NAMES
        .iter()
        .zip(turns.drops)
        .filter(|(_, count)| *count != 0)
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>();
    let drops = if drops.is_empty() {
        "none".to_owned()
    } else {
        drops.join(" ")
    };
    format!(
        "{} turns; reserve {} send {} release {} close {} (reserve/send {}); \
         reserved blocks {} ({} per reserve; {sizes}); \
         budget spent in {} turns ({} sockets, share {}); intake full {}; \
         accepted {} ({} per turn; {bins}); publish chose {} refused {} budget {} \
         fault {}; notices full {}; out at end: blocks {} credits {}; yields {} \
         ({} with possible work), waits pending {} ready {}; producer offered {} \
         refused {} held {} declined {} withdrawn {} unknown {}, waits for offers {} \
         changes {}, wakes {}; core drops {drops}; loans at end {loans}",
        turns.turns,
        turns.reserves,
        turns.sends,
        turns.releases,
        turns.closes,
        fixed(ratio(turns.reserves, turns.sends)),
        turns.reserve_blocks,
        fixed(ratio(turns.reserve_blocks, turns.reserves)),
        turns.budget_turns,
        turns.budget_spent,
        fixed(ratio(turns.budget_turns, turns.turns)),
        turns.intake_full,
        turns.accepted,
        fixed(ratio(turns.accepted, turns.turns)),
        turns.publish_chose,
        turns.publish_refused,
        turns.publish_budget,
        turns.publish_fault,
        turns.notices_full,
        turns.blocks_out,
        turns.credits_out,
        turns.yields,
        turns.yields_with_work,
        turns.waits_pending,
        turns.waits_ready,
        turns.local_offered,
        turns.local_refused,
        turns.local_held,
        turns.local_declined,
        turns.local_withdrawn,
        turns.producer_unknown,
        turns.local_waits_offers,
        turns.local_waits_changes,
        turns.producer_wakes,
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
            reserves: 4,
            sends: 16,
            reserve_blocks: 50,
            reserve_by_size: [1, 0, 0, 0, 3],
            budget_spent: 7,
            budget_turns: 7,
            accepted: 16,
            accepted_by_turn: [4, 0, 0, 0, 4, 0, 0],
            publish_chose: 8,
            yields: 8,
            yields_with_work: 7,
            local_offered: 6,
            local_held: 1,
            local_waits_changes: 2,
            producer_wakes: 3,
            drops: core::array::from_fn(|kind| match DROP_NAMES[kind] {
                "no_credit" => 5,
                "pressure_copy" => 1,
                _ => 0,
            }),
            loans_at_turn_end: [3, 0, 0, 0, 5, 0],
            ..NetworkTurns::default()
        };
        let value = window(&turns);
        assert_eq!(value["requests"]["reserve"], 4);
        assert_eq!(value["reserve_per_send"], 0.25);
        assert_eq!(value["reserve_blocks"], 50);
        assert_eq!(value["blocks_per_reserve"], 12.5);
        assert_eq!(value["reserve_by_size"]["9-16"], 3);
        assert_eq!(value["budget_turn_share"], 0.875);
        assert_eq!(value["accepted_per_turn"], 2.0);
        assert_eq!(value["accepted_by_turn"]["4"], 4);
        assert_eq!(value["accepted_by_turn"]["5-8"], 0);
        assert_eq!(value["publish_end"]["chose"], 8);
        assert_eq!(value["out_at_turn_end"]["blocks"], 0);
        assert_eq!(value["producer"]["offered"], 6);
        assert_eq!(value["producer"]["wakes"], 3);
        assert_eq!(value["producer_wakes_per_offer"], 0.5);
        assert_eq!(
            summary(&turns),
            "8 turns; reserve 4 send 16 release 0 close 0 (reserve/send 0.25); \
             reserved blocks 50 (12.50 per reserve; 1:1 2:0 3-4:0 5-8:0 9-16:3); \
             budget spent in 7 turns (7 sockets, share 0.88); intake full 0; \
             accepted 16 (2.00 per turn; 0:4 1:0 2:0 3:0 4:4 5-8:0 9+:0); publish chose 8 \
             refused 0 budget 0 fault 0; notices full 0; out at end: blocks 0 credits 0; \
             yields 8 (7 with possible work), waits pending 0 ready 0; producer offered 6 \
             refused 0 held 1 declined 0 withdrawn 0 unknown 0, waits for offers 0 \
             changes 2, wakes 3; core drops no_credit:5 pressure_copy:1; \
             loans at end 0:3 1:0 2:0 3:0 4:5 5+:0"
        );
        assert_eq!(value["drops"]["no_credit"], 5);
        assert_eq!(value["drops"]["lent_lane_full"], 0);
        assert_eq!(value["loans_at_turn_end"]["4"], 5);
    }

    #[test]
    fn an_image_that_does_not_count_says_so() {
        let turns = NetworkTurns::default();
        assert_eq!(window(&turns)["reserve_per_send"], Value::Null);
        assert!(summary(&turns).starts_with("not counted"));
    }
}
