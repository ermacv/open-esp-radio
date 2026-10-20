//! A readable view of a jump table the analysis followed.
use oer_analysis_riscv_model::JumpDispatch;
use std::collections::BTreeMap;

/// `jump table +176: +1bc <- cases 0, 3; +2de <- case 1`, targets ascending,
/// each with the case values (`first_case` plus the entry index) selecting it,
/// or with the entry indexes (`+1bc <- entries 0, 3`) when the case values are
/// unknown. A target set lists addresses without entry or case numbers.
pub fn human(dispatch: &JumpDispatch) -> String {
    match dispatch {
        JumpDispatch::Table {
            site,
            first_case,
            entries,
        } => ordered(*site, *first_case, entries),
        JumpDispatch::TargetSet { site, targets } => {
            let targets: Vec<_> = targets
                .iter()
                .map(|target| format!("+{target:x}"))
                .collect();
            format!("jump targets +{site:x}: {}", targets.join(", "))
        }
    }
}

fn ordered(site: u64, first_case: Option<i64>, entries: &[u64]) -> String {
    let mut cases: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    for (index, target) in entries.iter().enumerate() {
        cases
            .entry(*target)
            .or_default()
            .push(first_case.unwrap_or(0) + index as i64);
    }
    let (one, many) = match first_case {
        Some(_) => ("case", "cases"),
        None => ("entry", "entries"),
    };
    let targets: Vec<String> = cases
        .iter()
        .map(|(target, cases)| {
            let list: Vec<String> = cases.iter().map(i64::to_string).collect();
            let word = if cases.len() == 1 { one } else { many };
            format!("+{target:x} <- {word} {}", list.join(", "))
        })
        .collect();
    format!("jump table +{:x}: {}", site, targets.join("; "))
}

#[cfg(test)]
mod tests {
    use super::human;
    use oer_analysis_riscv_model::JumpDispatch;

    #[test]
    fn targets_list_the_cases_that_select_them() {
        let table = JumpDispatch::Table {
            site: 0x176,
            first_case: Some(0),
            entries: vec![0x41a, 0x2de, 0x41a, 0x1bc],
        };
        assert_eq!(
            human(&table),
            "jump table +176: +1bc <- case 3; +2de <- case 1; +41a <- cases 0, 2"
        );
        let mut rebased = table.clone();
        if let JumpDispatch::Table { first_case, .. } = &mut rebased {
            *first_case = Some(8);
        }
        assert_eq!(
            human(&rebased),
            "jump table +176: +1bc <- case 11; +2de <- case 9; +41a <- cases 8, 10"
        );
        let mut unknown = table;
        if let JumpDispatch::Table { first_case, .. } = &mut unknown {
            *first_case = None;
        }
        assert_eq!(
            human(&unknown),
            "jump table +176: +1bc <- entry 3; +2de <- entry 1; +41a <- entries 0, 2",
            "unknown case values are never claimed"
        );
        let json = serde_json::to_value(&unknown).expect("serialize ordered table");
        assert_eq!(json["kind"], "table");
        assert_eq!(json["first_case"], serde_json::Value::Null);
        assert_eq!(
            json["entries"],
            serde_json::json!([0x41a, 0x2de, 0x41a, 0x1bc])
        );
    }
}
