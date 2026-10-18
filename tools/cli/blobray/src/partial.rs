//! A readable view of why analyzed functions are partial.
use blobray_domain::PartialCauses;

/// ` (by cause: 2 control flow, 5 opaque calls)` with every nonzero cause,
/// or nothing when no function is partial.
pub fn human(causes: &PartialCauses) -> String {
    let named = [
        (causes.decoding, "decoding"),
        (causes.control_flow, "control flow"),
        (causes.references, "references"),
        (causes.opaque_calls, "opaque calls"),
        (causes.value_limits, "value limits"),
        (causes.other, "other"),
    ];
    let listed: Vec<String> = named
        .iter()
        .filter(|(count, _)| *count != 0)
        .map(|(count, name)| format!("{count} {name}"))
        .collect();
    if listed.is_empty() {
        String::new()
    } else {
        format!(" (by cause: {})", listed.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::human;
    use blobray_domain::PartialCauses;

    #[test]
    fn only_nonzero_causes_are_listed() {
        assert_eq!(human(&PartialCauses::default()), "");
        let causes = PartialCauses {
            control_flow: 2,
            opaque_calls: 5,
            ..PartialCauses::default()
        };
        assert_eq!(
            human(&causes),
            " (by cause: 2 control flow, 5 opaque calls)"
        );
    }
}
