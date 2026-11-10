//! The network core's switchable diagnostics within a window
//! (`protocols/network/runtime/README.md`, "Diagnostics"): with
//! `OER_WIFI_LOAD_DIAGNOSTICS=on` each level switches them on before its
//! run and off after its collection, and records what they counted: the
//! collections and the places their readiness walks visited, the ends told,
//! the transmit plans' continuations and stops by reason, the returns and
//! the credits they restored, the loans out at each refusal for credit, the
//! frames built, and the network hart's cycles by stage. Off (the default),
//! the image's diagnostics stay off and nothing is recorded.

use oer_hil_services_load::{DIAGNOSTIC_STAGE_NAMES, DiagnosticsCollected, NetworkDiagnostics};
use serde_json::{Value, json};

/// Whether the levels switch the diagnostics on.
pub const ENV: &str = "OER_WIFI_LOAD_DIAGNOSTICS";

/// Whether `OER_WIFI_LOAD_DIAGNOSTICS` asks for them: `on`, or `off` (the
/// default).
pub fn requested() -> Result<bool, String> {
    match std::env::var(ENV) {
        Err(_) => Ok(false),
        Ok(text) => parse(&text),
    }
}

fn parse(text: &str) -> Result<bool, String> {
    match text.trim() {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("{ENV}=`{other}` is neither `on` nor `off`")),
    }
}

/// `part` over `whole`, none without a whole.
fn ratio(part: u32, whole: u32) -> Option<f64> {
    (whole != 0).then(|| f64::from(part) / f64::from(whole))
}

/// The window's record: the image's answer, with the ratios a reader of the
/// hot path's work wants.
pub fn window(collected: &DiagnosticsCollected) -> Value {
    let diagnostics = match collected {
        DiagnosticsCollected::Report(diagnostics) => diagnostics,
        DiagnosticsCollected::NoWindow => return json!({ "whole": false }),
        other => return json!({ "whole": false, "answer": format!("{other:?}") }),
    };
    let NetworkDiagnostics {
        turns,
        collections,
        walked,
        told,
        continued,
        stopped_spent,
        stopped_changed,
        stopped_grant,
        stopped_account,
        return_batches,
        blocks_returned,
        credits_restored,
        no_credit,
        no_credit_loans,
        no_credit_unreturned,
        materialised,
        stage_cycles,
    } = *diagnostics;
    let stages: serde_json::Map<_, _> = DIAGNOSTIC_STAGE_NAMES
        .iter()
        .zip(stage_cycles)
        .map(|(name, cycles)| ((*name).to_owned(), cycles.into()))
        .collect();
    let turn_cycles: u64 = stage_cycles.iter().take(6).sum();
    json!({
        "whole": true,
        "turns": turns,
        "collections": collections,
        "walked": walked,
        "walked_per_collection": ratio(walked, collections),
        "told": told,
        "materialised": materialised,
        "continued": continued,
        "collections_per_frame": ratio(collections, materialised),
        "stopped": {
            "spent": stopped_spent,
            "changed": stopped_changed,
            "grant": stopped_grant,
            "account": stopped_account,
        },
        "returns": {
            "batches": return_batches,
            "blocks": blocks_returned,
            "credits": credits_restored,
        },
        "no_credit": {
            "refusals": no_credit,
            "loans_sum": no_credit_loans,
            "unreturned_sum": no_credit_unreturned,
            "loans_per_refusal": ratio(no_credit_loans, no_credit),
            "unreturned_per_refusal": ratio(no_credit_unreturned, no_credit),
        },
        "stage_cycles": stages,
        "cycles_per_turn": (turns != 0).then(|| turn_cycles as f64 / f64::from(turns)),
    })
}

/// One line of the window.
pub fn summary(collected: &DiagnosticsCollected) -> String {
    let DiagnosticsCollected::Report(d) = collected else {
        return format!("no whole window ({collected:?})");
    };
    let turn_cycles: u64 = d.stage_cycles.iter().take(6).sum();
    let shares = DIAGNOSTIC_STAGE_NAMES
        .iter()
        .zip(d.stage_cycles)
        .take(6)
        .map(|(name, cycles)| {
            let share = if turn_cycles == 0 {
                0.0
            } else {
                100.0 * cycles as f64 / turn_cycles as f64
            };
            format!("{name}:{share:.0}%")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "turns {} collections {} walked {} frames {} continued {} stops s/c/g/a {}/{}/{}/{} \
         told {} returns {} credits {} no_credit {} (loans {} unreturned {}) {shares}",
        d.turns,
        d.collections,
        d.walked,
        d.materialised,
        d.continued,
        d.stopped_spent,
        d.stopped_changed,
        d.stopped_grant,
        d.stopped_account,
        d.told,
        d.return_batches,
        d.credits_restored,
        d.no_credit,
        d.no_credit_loans,
        d.no_credit_unreturned,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_on_or_off() {
        assert_eq!(parse("on"), Ok(true));
        assert_eq!(parse(" off "), Ok(false));
        assert!(parse("yes").is_err());
    }

    #[test]
    fn a_window_records_its_counts_and_ratios() {
        let diagnostics = NetworkDiagnostics {
            turns: 10,
            collections: 4,
            walked: 40,
            materialised: 16,
            continued: 12,
            no_credit: 2,
            no_credit_loans: 8,
            stage_cycles: [10, 0, 20, 0, 70, 0, 5],
            ..NetworkDiagnostics::default()
        };
        let value = window(&DiagnosticsCollected::Report(diagnostics));
        assert_eq!(value["whole"], true);
        assert_eq!(value["walked_per_collection"], 10.0);
        assert_eq!(value["collections_per_frame"], 0.25);
        assert_eq!(value["no_credit"]["loans_per_refusal"], 4.0);
        assert_eq!(value["stage_cycles"]["waiting"], 5);
        assert_eq!(value["cycles_per_turn"], 10.0);
        assert!(summary(&DiagnosticsCollected::Report(diagnostics)).contains("publish:70%"));
        assert_eq!(window(&DiagnosticsCollected::NoWindow)["whole"], false);
    }
}
