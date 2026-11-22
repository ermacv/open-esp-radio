//! `radio-pair-compare A B`: the words two runs of the radio pair differ
//! in, from their results files (`target/hil/results/radio-pair-*.json`):
//! a deaf boot's against a hearing boot's of the same image. Each
//! measurement is compared by name; a snapshot's readings by item and index,
//! every other value leaf by leaf. Exit 0 when the two agree, 1 when they
//! differ, 2 when a file cannot be read.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

use std::{collections::BTreeMap, process::ExitCode};

use serde_json::Value;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [first, second] = arguments.as_slice() else {
        eprintln!("usage: radio-pair-compare <results A> <results B>");
        return ExitCode::from(2);
    };
    let (first, second) = match (read(first), read(second)) {
        (Ok(first), Ok(second)) => (first, second),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let differences = compare(&first, &second);
    for difference in &differences {
        println!("{difference}");
    }
    println!("{} differences", differences.len());
    if differences.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn read(path: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{path}: {error}"))
}

/// Each measurement of a results file by its name.
fn measurements(results: &Value) -> BTreeMap<String, &Value> {
    results
        .get("measurements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|measurement| {
            Some((
                measurement.get("name")?.as_str()?.to_owned(),
                measurement.get("value")?,
            ))
        })
        .collect()
}

/// The lines naming every word `first` and `second` differ in.
fn compare(first: &Value, second: &Value) -> Vec<String> {
    let first = measurements(first);
    let second = measurements(second);
    let mut differences = Vec::new();
    let names = first
        .keys()
        .chain(second.keys())
        .collect::<std::collections::BTreeSet<_>>();
    for name in names {
        match (first.get(name), second.get(name)) {
            (Some(a), Some(b)) => match (readings(a), readings(b)) {
                (Some(a), Some(b)) => {
                    let keys = a
                        .keys()
                        .chain(b.keys())
                        .collect::<std::collections::BTreeSet<_>>();
                    for key in keys {
                        let (left, right) = (a.get(key), b.get(key));
                        if left != right {
                            differences.push(format!(
                                "{name} {key}: {} | {}",
                                shown(left.copied()),
                                shown(right.copied())
                            ));
                        }
                    }
                }
                _ => leaves(name, a, b, &mut differences),
            },
            (a, b) => differences.push(format!(
                "{name}: {} | {}",
                shown(a.copied()),
                shown(b.copied())
            )),
        }
    }
    differences
}

/// A snapshot's readings by `item[index]`, when `value` is one.
fn readings(value: &Value) -> Option<BTreeMap<String, &Value>> {
    let array = value.as_array()?;
    array
        .iter()
        .map(|reading| {
            let item = reading.get("item")?.as_str()?;
            let index = reading.get("index")?.as_u64()?;
            Some((format!("{item}[{index}]"), reading.get("value")?))
        })
        .collect()
}

/// Every leaf `a` and `b` differ in, under `path`.
fn leaves(path: &str, a: &Value, b: &Value, differences: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let keys = a
                .keys()
                .chain(b.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => leaves(&format!("{path}.{key}"), a, b, differences),
                    (a, b) => {
                        differences.push(format!("{path}.{key}: {} | {}", shown(a), shown(b)))
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                leaves(&format!("{path}[{index}]"), a, b, differences);
            }
        }
        (a, b) if a != b => differences.push(format!("{path}: {a} | {b}")),
        _ => {}
    }
}

fn shown(value: Option<&Value>) -> String {
    value.map_or_else(|| "absent".to_owned(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(measurements: Value) -> Value {
        serde_json::json!({ "measurements": measurements })
    }

    #[test]
    fn two_runs_differ_in_the_words_and_leaves_that_changed_and_in_nothing_else() {
        let reading = |item: &str, index: u64, word: u64| serde_json::json!({ "item": item, "index": index, "value": { "Word": word } });
        let deaf = run(serde_json::json!([
            { "case": "c", "name": "receiver/snapshot-up", "value": [
                reading("PmuRfPwc", 0, 7), reading("TemperatureCode", 0, 40),
            ]},
            { "case": "c", "name": "receiver/counters-up", "value": { "Counted": { "mac": { "mpdu": 0, "end": 2 } } } },
            { "case": "c", "name": "sender/rounds", "value": 1 },
        ]));
        let hearing = run(serde_json::json!([
            { "case": "c", "name": "receiver/snapshot-up", "value": [
                reading("PmuRfPwc", 0, 7), reading("TemperatureCode", 0, 41),
            ]},
            { "case": "c", "name": "receiver/counters-up", "value": { "Counted": { "mac": { "mpdu": 9, "end": 2 } } } },
        ]));
        assert_eq!(
            compare(&deaf, &hearing),
            [
                "receiver/counters-up.Counted.mac.mpdu: 0 | 9",
                "receiver/snapshot-up TemperatureCode[0]: {\"Word\":40} | {\"Word\":41}",
                "sender/rounds: 1 | absent",
            ]
        );
        assert!(compare(&deaf, &deaf).is_empty());
    }
}
