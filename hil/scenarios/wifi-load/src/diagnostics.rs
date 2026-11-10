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
use oer_tools_hil_harness::Failure;
use serde_json::{Value, json};

/// A level's diagnostics switch.
pub trait Switch {
    /// Ask the image's diagnostics on or off: `Ok` once it answered.
    fn switch(&mut self, on: bool) -> Result<(), Failure>;
    /// Record what the switch off came to.
    fn record(&mut self, off: &Result<(), Failure>);
}

/// Run `measure` once with the diagnostics on: switch them on, measure if
/// that was answered, then ask them off whatever came before, record that
/// result, and return the first failure, the level's before the switch
/// off's.
pub fn around<S: Switch, T>(
    switch: &mut S,
    measure: impl FnOnce(&mut S) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let measured = switch.switch(true).and_then(|()| measure(switch));
    let off = switch.switch(false);
    switch.record(&off);
    let value = measured?;
    off?;
    Ok(value)
}

/// The record of a switch off.
pub fn switched_off(off: &Result<(), Failure>) -> Value {
    match off {
        Ok(()) => json!({ "switched_off": true }),
        Err(Failure::Fail(reason)) => json!({ "switched_off": false, "failed": reason }),
        Err(Failure::Incomplete(reason)) => {
            json!({ "switched_off": false, "incomplete": reason })
        }
    }
}

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
        DiagnosticsCollected::Exhausted => {
            return json!({ "whole": false, "exhausted": true });
        }
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

    /// A switch whose answers the test sets, which writes down every step:
    /// `None` answers as after a request of unknown outcome, every later
    /// request refused unsent, as the case's context does.
    #[derive(Default)]
    struct Script {
        on: Option<Result<(), Failure>>,
        off: Option<Result<(), Failure>>,
        unconfirmed: bool,
        steps: Vec<String>,
    }

    impl Switch for Script {
        fn switch(&mut self, on: bool) -> Result<(), Failure> {
            if self.unconfirmed {
                self.steps.push(format!("refused unsent {on}"));
                return Err(Failure::Incomplete("unconfirmed".to_owned()));
            }
            self.steps.push(format!("switch {on}"));
            let answer = if on { self.on.take() } else { self.off.take() };
            answer.unwrap_or(Ok(()))
        }

        fn record(&mut self, off: &Result<(), Failure>) {
            self.steps.push(format!("record {}", switched_off(off)));
        }
    }

    fn incomplete(reason: &str) -> Failure {
        Failure::Incomplete(reason.to_owned())
    }

    fn reason(result: &Result<u32, Failure>) -> String {
        format!("{result:?}")
    }

    #[test]
    fn a_measured_level_switches_off_and_records_it() {
        let mut script = Script::default();
        let mut runs = 0;
        let result = around(&mut script, |_| {
            runs += 1;
            Ok(7)
        });
        assert_eq!(result.ok(), Some(7));
        assert_eq!(runs, 1);
        assert_eq!(
            script.steps,
            [
                "switch true",
                "switch false",
                r#"record {"switched_off":true}"#
            ]
        );
    }

    #[test]
    fn a_refused_start_still_switches_off_and_its_failure_comes_first() {
        let mut script = Script::default();
        let mut runs = 0;
        let result = around(&mut script, |_| -> Result<u32, Failure> {
            runs += 1;
            Err(Failure::Fail("the image's load: Refused(Busy)".to_owned()))
        });
        assert!(reason(&result).contains("Refused(Busy)"));
        assert_eq!(runs, 1, "the level is not run again");
        assert_eq!(script.steps[1], "switch false");
    }

    #[test]
    fn a_collection_error_still_switches_off_and_a_failed_off_is_recorded_apart() {
        let mut script = Script {
            off: Some(Err(incomplete("no response"))),
            ..Script::default()
        };
        let result = around(&mut script, |_| -> Result<u32, Failure> {
            Err(incomplete("the image's diagnostics: no response"))
        });
        assert!(
            reason(&result).contains("the image's diagnostics"),
            "the level's failure first"
        );
        assert_eq!(
            script.steps[2],
            r#"record {"incomplete":"no response","switched_off":false}"#
        );
    }

    #[test]
    fn a_failed_off_after_a_good_level_fails_the_level_with_its_measurement_kept() {
        let mut script = Script {
            off: Some(Err(Failure::Fail("payload".to_owned()))),
            ..Script::default()
        };
        let result = around(&mut script, |_| Ok(3));
        assert!(reason(&result).contains("payload"));
        assert_eq!(
            script.steps[2],
            r#"record {"failed":"payload","switched_off":false}"#
        );
    }

    #[test]
    fn after_an_unconfirmed_request_nothing_more_is_sent_or_measured_again() {
        let mut script = Script::default();
        let mut runs = 0;
        let result = around(&mut script, |script| -> Result<u32, Failure> {
            runs += 1;
            // The level's collection got no response: the context holds
            // every later request back.
            script.unconfirmed = true;
            Err(incomplete("unconfirmed collection"))
        });
        assert!(reason(&result).contains("unconfirmed collection"));
        assert_eq!(runs, 1, "the measurement is not repeated");
        assert_eq!(script.steps[1], "refused unsent false");
        assert_eq!(
            script.steps[2],
            r#"record {"incomplete":"unconfirmed","switched_off":false}"#
        );
    }

    #[test]
    fn a_switch_on_that_failed_measures_nothing_and_still_asks_off() {
        let mut script = Script {
            on: Some(Err(incomplete("no response"))),
            ..Script::default()
        };
        let mut runs = 0;
        let result = around(&mut script, |_| {
            runs += 1;
            Ok(1)
        });
        assert!(result.is_err());
        assert_eq!(runs, 0);
        assert_eq!(script.steps[1], "switch false");
    }

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
