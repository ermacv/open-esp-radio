//! A readable view of a jump table the analysis followed.
use blobray_domain::JumpTable;
use std::collections::BTreeMap;

/// `jump table +176: +1bc <- cases 0, 3; +2de <- case 1`, targets ascending,
/// each with the case values (`first_case` plus the entry index) selecting it,
/// or with the entry indexes (`+1bc <- entries 0, 3`) when the case values are
/// unknown.
pub fn human(table: &JumpTable) -> String {
    let mut cases: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    for (index, target) in table.entries.iter().enumerate() {
        cases
            .entry(*target)
            .or_default()
            .push(table.first_case.unwrap_or(0) + index as i64);
    }
    let (one, many) = match table.first_case {
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
    format!("jump table +{:x}: {}", table.site, targets.join("; "))
}

#[cfg(test)]
mod tests {
    use super::human;
    use blobray_domain::JumpTable;

    #[test]
    fn targets_list_the_cases_that_select_them() {
        let table = JumpTable {
            site: 0x176,
            first_case: Some(0),
            entries: vec![0x41a, 0x2de, 0x41a, 0x1bc],
        };
        assert_eq!(
            human(&table),
            "jump table +176: +1bc <- case 3; +2de <- case 1; +41a <- cases 0, 2"
        );
        let rebased = JumpTable {
            first_case: Some(8),
            ..table.clone()
        };
        assert_eq!(
            human(&rebased),
            "jump table +176: +1bc <- case 11; +2de <- case 9; +41a <- cases 8, 10"
        );
        let unknown = JumpTable {
            first_case: None,
            ..table
        };
        assert_eq!(
            human(&unknown),
            "jump table +176: +1bc <- entry 3; +2de <- entry 1; +41a <- entries 0, 2",
            "unknown case values are never claimed"
        );
    }
}
