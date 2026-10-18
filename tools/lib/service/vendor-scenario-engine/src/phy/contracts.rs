//! Reviewed effect contracts of PHY comparisons: the analog I2C transport's
//! polling is ignored plumbing, and every other MMIO, fence and delay effect
//! compares exactly, in order and value.
use super::layout::layout;
use blobray_domain::{
    CallEndpoint, EffectClaimCeiling, EffectContract, EffectDisposition, EffectPattern, EffectRule,
    EffectSelector, EffectValue, UnclassifiedEffects,
};

/// Transport registers whose reads are command polling.
fn transport_reads() -> [u32; 4] {
    let layout = layout();
    [
        layout.i2c_ports[0],
        layout.i2c_ports[1],
        layout.i2c_read_mask,
        layout.i2c_host_map,
    ]
}

/// Transport configuration words written around each command.
fn transport_writes() -> [u32; 2] {
    [layout().i2c_read_mask, layout().i2c_host_map]
}
/// Requested wait between two polls of a status register.
const POLL_DELAY_MICROS: u32 = 1;

fn word(selector: fn(u32) -> EffectSelector, address: u32) -> EffectPattern {
    EffectPattern {
        selector: selector(address),
        value: EffectValue::Any,
        preceded_by: None,
        occurrence: None,
        followed_by: None,
    }
}

fn read(address: u32) -> EffectSelector {
    EffectSelector::MmioRead { address, width: 4 }
}

fn write(address: u32) -> EffectSelector {
    EffectSelector::MmioWrite { address, width: 4 }
}

/// An effect either side may perform any number of times up to `maximum`.
pub fn ignored(name: String, pattern: EffectPattern, maximum: u32, reason: &str) -> EffectRule {
    EffectRule {
        name,
        vendor: Some(pattern),
        replacement: Some(pattern),
        disposition: EffectDisposition::Ignored,
        min_occurrences: 0,
        max_occurrences: maximum,
        reason: reason.into(),
    }
}

/// Vendor reads of `address` production may omit, at most `maximum` times
/// per case. Retained production occurrences still compare exactly.
pub fn omitted_read(name: String, address: u32, maximum: u32, reason: &str) -> EffectRule {
    EffectRule {
        disposition: EffectDisposition::Omitted,
        ..ignored(name, word(read, address), maximum, reason)
    }
}

/// Vendor writes of `address` production may omit, at most `maximum` times
/// per case. Retained production occurrences still compare exactly.
pub fn omitted_write(name: String, address: u32, maximum: u32, reason: &str) -> EffectRule {
    EffectRule {
        disposition: EffectDisposition::Omitted,
        ..ignored(name, word(write, address), maximum, reason)
    }
}

/// A vendor read of `address` immediately followed by a read of `successor`
/// that production may omit, at most `maximum` times per case. Retained
/// production occurrences still compare exactly.
pub fn omitted_read_before(
    name: String,
    address: u32,
    successor: u32,
    maximum: u32,
    reason: &str,
) -> EffectRule {
    let pattern = EffectPattern {
        selector: read(address),
        value: EffectValue::Any,
        preceded_by: None,
        occurrence: None,
        followed_by: Some(read(successor)),
    };
    EffectRule {
        name,
        vendor: Some(pattern),
        replacement: Some(pattern),
        disposition: EffectDisposition::Omitted,
        min_occurrences: 0,
        max_occurrences: maximum,
        reason: reason.into(),
    }
}

/// Analog I2C port reads, which poll command completion and return read
/// data. Waits and transport configuration accesses stay compared.
pub fn port_polling(maximum: u32) -> Vec<EffectRule> {
    layout()
        .i2c_ports
        .map(|address| {
            ignored(
                format!("port-read-{address:08x}"),
                word(read, address),
                maximum,
                "analog I2C completion polling; read data reaches compared state",
            )
        })
        .to_vec()
}

/// Transport reads and read-mask and host-map writes, for roots whose analog
/// I2C commands never wait.
pub fn transport(maximum: u32) -> Vec<EffectRule> {
    let mut rules = vec![];
    for address in transport_reads() {
        rules.push(ignored(
            format!("transport-read-{address:08x}"),
            word(read, address),
            maximum,
            "analog I2C command polling; the command and its result compare separately",
        ));
    }
    for address in transport_writes() {
        rules.push(ignored(
            format!("transport-write-{address:08x}"),
            word(write, address),
            maximum,
            "analog I2C transport configuration around each command",
        ));
    }
    rules
}

/// [`transport`], and the single-microsecond wait immediately before a
/// transport read or a read of one of `wait_status`. Every other delay,
/// including readiness waits, stays compared.
pub fn plumbing(wait_status: &[u32], maximum: u32) -> Vec<EffectRule> {
    let mut rules = transport(maximum);
    let reads = transport_reads();
    let mut polled: Vec<u32> = reads.to_vec();
    polled.extend(wait_status.iter().filter(|a| !reads.contains(a)));
    for address in polled {
        rules.push(ignored(
            format!("poll-wait-{address:08x}"),
            EffectPattern {
                selector: EffectSelector::Delay {
                    micros: Some(POLL_DELAY_MICROS),
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: Some(read(address)),
            },
            maximum,
            "polling interval before a status read; the poll count is implementation timing",
        ));
    }
    rules
}

/// Contract between exact vendor and production entries: `rules`, and exact
/// comparison of every effect no rule selects.
pub fn phy_contract(
    vendor: CallEndpoint,
    replacement: CallEndpoint,
    rules: Vec<EffectRule>,
    applicability: &str,
) -> EffectContract {
    EffectContract {
        vendor,
        replacement,
        rules,
        unclassified: UnclassifiedEffects::Required,
        claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
        applicability: applicability.into(),
        reason: "PHY register effects compare exactly except reviewed transport plumbing".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plumbing_rules_do_not_overlap_and_wait_status_is_deduplicated() {
        crate::install(&crate::chip::TEST);
        let status = layout().radio_mmio + 0x890;
        let rules = plumbing(
            &[layout().i2c_ports[0], status],
            super::super::layout::MAX_EVENTS,
        );
        let waits = rules
            .iter()
            .filter(|r| r.name.starts_with("poll-wait"))
            .count();
        assert_eq!(waits, transport_reads().len() + 1);
        for (i, a) in rules.iter().enumerate() {
            for b in &rules[i + 1..] {
                assert!(!a.vendor.unwrap().overlaps(b.vendor.unwrap()), "{}", a.name);
            }
        }
    }
}
