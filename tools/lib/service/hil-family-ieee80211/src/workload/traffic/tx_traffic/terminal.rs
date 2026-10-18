//! Retain terminal MAC receipts separately from UDP and unfinished TX work.
use crate::Result;
use oer_hil_protocol::network::StationTxTerminalEvidence;

pub(super) fn retain(
    results: &oer_hil_workload::results::Results,
    value: StationTxTerminalEvidence,
) -> Result<()> {
    results.observe("station-tx-terminal", &value);
    validate(value)
}

fn validate(value: StationTxTerminalEvidence) -> Result<()> {
    if value.invalid_statuses != 0
        || value.mpdus != value.acknowledged.wrapping_add(value.unacknowledged)
    {
        return Err("inconsistent station terminal TX receipt counters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unacknowledged_is_valid_evidence_but_broken_accounting_is_not() {
        let mut value = StationTxTerminalEvidence {
            exchanges: 1,
            mpdus: 3,
            acknowledged: 2,
            unacknowledged: 1,
            ..Default::default()
        };
        assert!(validate(value).is_ok());
        value.invalid_statuses = 1;
        assert!(validate(value).is_err());
        value.invalid_statuses = 0;
        value.acknowledged = 3;
        assert!(validate(value).is_err());
    }
}
