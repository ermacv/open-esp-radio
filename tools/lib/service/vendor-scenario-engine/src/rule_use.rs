//! How often each reviewed effect rule selects an effect.
//!
//! Every executed request counts, per contract its cases select, the
//! effects each rule selects on either side, through the comparison's own
//! classification. The scenario writes the counts beside its run, and a
//! review that selects nothing in any contract and case of its scenario
//! fails it: the review is stale, or no case exercises the path it reviews,
//! so the contract claims more than the evidence shows.
use crate::harness::{Result, invalid};
use blobray_domain::{
    EffectContract, EffectSelection, ExecutionEvent, ExecutionEvidence, ExecutionRequest,
};
use oer_riscv_model::{ArtifactId, Result as DomainResult, RunControl};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One contract selected by content, and its rules' selections so far.
pub struct Reviewed {
    pub name: String,
    pub reference: ArtifactId,
    pub contract: EffectContract,
    /// Whether a case selected the contract; per rule, its selections.
    pub used: bool,
    pub counts: Vec<u64>,
}

impl Reviewed {
    pub fn new(name: &str, reference: ArtifactId, contract: EffectContract) -> Self {
        Self {
            name: name.into(),
            reference,
            counts: vec![0; contract.rules.len()],
            contract,
            used: false,
        }
    }
}

/// Classification after the run: it spends no resource budget.
struct Unmetered;

impl RunControl for Unmetered {
    fn checkpoint(&mut self, _units: u64) -> DomainResult<()> {
        Ok(())
    }
}

/// The rule `selection` names.
fn rule(selection: EffectSelection) -> Option<usize> {
    match selection {
        EffectSelection::Required(i)
        | EffectSelection::Ignored(i)
        | EffectSelection::Omitted(i)
        | EffectSelection::Replaced(i)
        | EffectSelection::Added(i) => Some(usize::from(i)),
        EffectSelection::Unlisted
        | EffectSelection::Unclassified
        | EffectSelection::Violation(_) => None,
    }
}

/// Count the selections of `request`'s cases, whose run gave `records`.
pub fn count(
    reviewed: &mut [Reviewed],
    request: &ExecutionRequest,
    records: &[ExecutionEvidence],
) -> Result<()> {
    let mut events: BTreeMap<(u32, bool), Vec<ExecutionEvent>> = BTreeMap::new();
    for record in records {
        if let ExecutionEvidence::Event {
            case,
            replacement,
            event,
        } = record
        {
            events
                .entry((*case, *replacement))
                .or_default()
                .push(event.clone());
        }
    }
    for (index, case) in request.cases.iter().enumerate() {
        let Some(selected) = case.relation.as_ref().and_then(|r| r.effects.as_ref()) else {
            continue;
        };
        let Some(contract) = reviewed.iter_mut().find(|c| &c.reference == selected) else {
            continue;
        };
        contract.used = true;
        for side in [false, true] {
            let Some(events) = events.get(&(index as u32, side)) else {
                continue;
            };
            let selections = blobray_verification::classify_effects(
                &contract.contract,
                events,
                side,
                &mut Unmetered,
            )?;
            for (_, selection) in selections {
                if let Some(count) = rule(selection).and_then(|i| contract.counts.get_mut(i)) {
                    *count += 1;
                }
            }
        }
    }
    Ok(())
}

/// The counts of every used contract, one rule per line.
pub fn render(reviewed: &[Reviewed]) -> String {
    let mut text = String::new();
    for contract in reviewed.iter().filter(|c| c.used) {
        text.push_str(&format!("\n== {}\n", contract.name));
        for (rule, count) in contract.contract.rules.iter().zip(&contract.counts) {
            text.push_str(&format!(
                "  {count:8}  {:?} {}\n",
                rule.disposition, rule.name
            ));
        }
    }
    text
}

/// The reviews that selected nothing in any used contract, each as its
/// `contract: rule` instances. A review is a disposition with its reason:
/// one review may be instantiated per register, such as the transport
/// polling of each port, and shared by the contracts of one scenario that
/// each cover some of a root's paths. It is dead only when none of its
/// instances selects an effect anywhere in the scenario.
pub fn dead(reviewed: &[Reviewed]) -> Vec<String> {
    let mut reviews: BTreeMap<(String, &str), (u64, Vec<String>)> = BTreeMap::new();
    for contract in reviewed.iter().filter(|c| c.used) {
        for (rule, count) in contract.contract.rules.iter().zip(&contract.counts) {
            let review = reviews
                .entry((format!("{:?}", rule.disposition), rule.reason.as_str()))
                .or_default();
            review.0 += count;
            review.1.push(format!("{}: {}", contract.name, rule.name));
        }
    }
    reviews
        .into_values()
        .filter(|(count, _)| *count == 0)
        .map(|(_, instances)| instances.join("+"))
        .collect()
}

/// Write the counts below `run`, then fail on a rule that selected nothing.
pub fn check(run: &Path, suite: &str, reviewed: &[Reviewed]) -> Result<PathBuf> {
    let path = run.join(format!("rules-{suite}.txt"));
    std::fs::write(&path, render(reviewed))?;
    let dead = dead(reviewed);
    if dead.is_empty() {
        Ok(path)
    } else {
        Err(invalid(format!(
            "{suite}: effect reviews that select no effect in any case: {}; counts in {}",
            dead.join(", "),
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_without_a_selection_is_dead_only_in_a_used_contract() {
        let contract = crate::failure::tests::contract();
        let reference = blobray_application::in_process::effect_contract_id(&contract).unwrap();
        let mut reviewed = [Reviewed::new("fixture", reference, contract)];
        assert!(dead(&reviewed).is_empty());
        reviewed[0].used = true;
        assert_eq!(dead(&reviewed), ["fixture: settled-check"]);
        reviewed[0].counts[0] = 3;
        assert!(dead(&reviewed).is_empty());
        assert!(render(&reviewed).contains("       3  Added settled-check"));
    }

    #[test]
    fn one_review_per_register_is_dead_only_when_no_register_selects() {
        let mut contract = crate::failure::tests::contract();
        let mut port = contract.rules[0].clone();
        port.name = "settled-check-other-port".into();
        contract.rules.push(port);
        let reference = blobray_application::in_process::effect_contract_id(&contract).unwrap();
        let mut reviewed = [Reviewed::new("fixture", reference, contract)];
        reviewed[0].used = true;
        assert_eq!(
            dead(&reviewed),
            ["fixture: settled-check+fixture: settled-check-other-port"]
        );
        reviewed[0].counts[1] = 1;
        assert!(dead(&reviewed).is_empty());
    }
}
