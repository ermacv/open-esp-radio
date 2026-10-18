//! Fail-closed comparison of the vendor and production traces.

use std::fmt;

use crate::record::Record;

/// Outcome of one scenario comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Both traces agree record for record.
    Match,
    /// The first differing position; `None` marks a shorter trace.
    Diff {
        index: usize,
        vendor: Option<Record>,
        port: Option<Record>,
    },
    /// The scenario cannot be compared, with the reason.
    Incomplete(String),
}

impl fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |record: &Option<Record>| {
            record
                .as_ref()
                .map_or_else(|| "<end>".to_owned(), ToString::to_string)
        };
        match self {
            Self::Match => write!(formatter, "MATCH"),
            Self::Diff {
                index,
                vendor,
                port,
            } => write!(
                formatter,
                "DIFF at {index}\n  vendor: {}\n  port:   {}",
                show(vendor),
                show(port)
            ),
            Self::Incomplete(reason) => write!(formatter, "INCOMPLETE: {reason}"),
        }
    }
}

/// Records the port does not own: calls leaving the driver (clocks, PHY,
/// BTBB, interrupt allocation, time, critical sections) and public-call
/// return values.
fn compared(record: &Record) -> bool {
    !matches!(record, Record::External { .. } | Record::Return { .. })
}

fn vendor_asserted(record: &Record) -> bool {
    matches!(record, Record::Event { name, .. } if name == "assert_failed")
}

/// Compare the vendor trace with the port's outcome.
///
/// A vendor assertion, a port panic after one, or a scenario the port
/// cannot run is `Incomplete`; a port panic without a vendor assertion is a
/// `Diff` at the end of the port trace.
pub fn compare(vendor: &[Record], port: Result<Vec<Record>, String>) -> Verdict {
    if vendor.iter().any(vendor_asserted) {
        return Verdict::Incomplete("the vendor driver asserted".to_owned());
    }
    let port = match port {
        Ok(port) => port,
        Err(reason) => return Verdict::Incomplete(reason),
    };
    let vendor: Vec<&Record> = vendor.iter().filter(|record| compared(record)).collect();
    let length = vendor.len().max(port.len());
    for index in 0..length {
        let (left, right) = (vendor.get(index).copied(), port.get(index));
        if left != right {
            return Verdict::Diff {
                index,
                vendor: left.cloned(),
                port: right.cloned(),
            };
        }
    }
    Verdict::Match
}

#[cfg(test)]
mod tests {
    use super::{Verdict, compare};
    use crate::record::{Argument, Record};

    fn ll(value: u64) -> Record {
        Record::Ll {
            name: "ieee802154_ll_set_cmd".to_owned(),
            arguments: vec![Argument::Value(value)],
        }
    }

    fn external() -> Record {
        Record::External {
            name: "esp_timer_get_time".to_owned(),
            arguments: Vec::new(),
        }
    }

    #[test]
    fn calls_leaving_the_driver_are_not_compared() {
        assert_eq!(
            compare(&[external(), ll(0x45)], Ok(vec![ll(0x45)])),
            Verdict::Match
        );
    }

    #[test]
    fn a_differing_or_missing_record_is_a_diff() {
        assert_eq!(
            compare(&[ll(0x45)], Ok(vec![ll(0x42)])),
            Verdict::Diff {
                index: 0,
                vendor: Some(ll(0x45)),
                port: Some(ll(0x42)),
            }
        );
        assert_eq!(
            compare(&[ll(0x45), ll(0x42)], Ok(vec![ll(0x45)])),
            Verdict::Diff {
                index: 1,
                vendor: Some(ll(0x42)),
                port: None,
            }
        );
    }

    #[test]
    fn an_assertion_or_an_unported_step_is_incomplete() {
        let assertion = Record::Event {
            name: "assert_failed".to_owned(),
            arguments: vec![1],
            first: Vec::new(),
            second: Vec::new(),
        };
        assert!(matches!(
            compare(&[assertion], Ok(Vec::new())),
            Verdict::Incomplete(_)
        ));
        assert!(matches!(
            compare(&[ll(0x45)], Err("unported".to_owned())),
            Verdict::Incomplete(_)
        ));
    }
}
