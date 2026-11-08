//! The ADDBA Requests the station took since boot, at a run's window's
//! end: the last of each TID, with the Buffer Size the access point asked
//! for (zero asks the station to choose) and what became of it, and the
//! requests by outcome. Only a trace image counts them.

use oer_hil_services_load::{Addba, AddbaOutcome, AddbaRequest};
use serde_json::{Value, json};

fn outcome(outcome: AddbaOutcome) -> Value {
    match outcome {
        AddbaOutcome::Accepted { window } => json!({ "accepted": { "window": window } }),
        AddbaOutcome::Repeated { window } => json!({ "repeated": { "window": window } }),
        AddbaOutcome::Refused { status } => json!({ "refused": { "status": status } }),
        AddbaOutcome::Unanswered => json!("unanswered"),
    }
}

/// Each TID's last request, with the TID.
fn requests(addba: &Addba) -> impl Iterator<Item = (usize, &AddbaRequest)> {
    addba
        .last
        .iter()
        .enumerate()
        .filter_map(|(tid, request)| Some((tid, request.as_ref()?)))
}

/// The run's record.
pub fn window(addba: &Addba) -> Value {
    let last: serde_json::Map<_, _> = requests(addba)
        .map(|(tid, request)| {
            (
                tid.to_string(),
                json!({ "requested": request.requested, "outcome": outcome(request.outcome) }),
            )
        })
        .collect();
    json!({
        "last": last,
        "accepted": addba.accepted,
        "repeated": addba.repeated,
        "refused": addba.refused,
        "unanswered": addba.unanswered,
    })
}

/// The run's one line: each TID's last request and the counts.
pub fn summary(addba: &Addba) -> String {
    let last: Vec<String> = requests(addba)
        .map(|(tid, request)| {
            let outcome = match request.outcome {
                AddbaOutcome::Accepted { window } => format!("window {window}"),
                AddbaOutcome::Repeated { window } => format!("repeated, window {window}"),
                AddbaOutcome::Refused { status } => format!("refused, status {status}"),
                AddbaOutcome::Unanswered => "unanswered".to_owned(),
            };
            format!("TID {tid} asked {}: {outcome}", request.requested)
        })
        .collect();
    let last = if last.is_empty() {
        "no request".to_owned()
    } else {
        last.join("; ")
    };
    format!(
        "{last}; {} accepted, {} repeated, {} refused, {} unanswered",
        addba.accepted, addba.repeated, addba.refused, addba.unanswered
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_and_its_line_name_each_tid_s_request_and_the_counts() {
        let mut addba = Addba {
            accepted: 2,
            refused: 1,
            ..Addba::default()
        };
        addba.last[0] = Some(AddbaRequest {
            requested: 0,
            outcome: AddbaOutcome::Accepted { window: 16 },
        });
        addba.last[6] = Some(AddbaRequest {
            requested: 64,
            outcome: AddbaOutcome::Accepted { window: 1 },
        });
        addba.last[9] = Some(AddbaRequest {
            requested: 8,
            outcome: AddbaOutcome::Refused { status: 37 },
        });
        let value = window(&addba);
        assert_eq!(value["last"]["0"]["requested"], 0);
        assert_eq!(value["last"]["0"]["outcome"]["accepted"]["window"], 16);
        assert_eq!(value["last"]["6"]["outcome"]["accepted"]["window"], 1);
        assert_eq!(value["last"]["9"]["outcome"]["refused"]["status"], 37);
        assert!(value["last"].get("1").is_none());
        assert_eq!(
            (value["accepted"].clone(), value["refused"].clone()),
            (2.into(), 1.into())
        );
        assert_eq!(
            summary(&addba),
            "TID 0 asked 0: window 16; TID 6 asked 64: window 1; TID 9 asked 8: refused, \
             status 37; 2 accepted, 0 repeated, 1 refused, 0 unanswered"
        );
        assert_eq!(
            summary(&Addba::default()),
            "no request; 0 accepted, 0 repeated, 0 refused, 0 unanswered"
        );
    }
}
