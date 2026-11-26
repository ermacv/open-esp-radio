use super::*;

fn function(name: &str, code: &str, named: &str, tokens: &[u64]) -> Function {
    Function {
        member: "a.o".into(),
        name: name.into(),
        code: code.into(),
        named: named.into(),
        size: 2 * tokens.len(),
        tokens: tokens.to_vec(),
        calls: vec![],
    }
}

#[test]
fn functions_are_classified_by_name_then_code() {
    let old = [
        function("same", "c1", "n1", &[1, 2]),
        function("callees", "c2", "n2", &[3]),
        function("old_name", "c3", "n3", &[4, 5]),
        function("edited", "c4", "n4", &[6, 7, 8, 9]),
        function("gone", "peer", "n5", &[10, 11, 12, 13]),
    ];
    let new = [
        function("same", "c1", "n1", &[1, 2]),
        function("callees", "c2", "n2b", &[3]),
        function("new_name", "c3", "n3b", &[4, 5]),
        function("edited", "c4b", "n4b", &[6, 7, 8, 0]),
        function("successor", "c6", "n6", &[10, 11, 12, 14]),
    ];
    let entries = compare(&old, &new);
    let status = |name: &str| &entries.iter().find(|e| e.name == name).unwrap().status;
    assert_eq!(status("same"), &Status::Unchanged);
    assert_eq!(status("callees"), &Status::ReferencesRenamed);
    assert_eq!(
        status("old_name"),
        &Status::Renamed {
            new: "new_name".into()
        }
    );
    assert_eq!(status("edited"), &Status::Changed { similarity: 0.75 });
    assert_eq!(
        status("gone"),
        &Status::Removed {
            closest: Some(("successor".into(), 0.75))
        }
    );
    assert_eq!(status("successor"), &Status::Added);
}

#[test]
fn the_overlap_of_two_sorted_multisets_is_held() {
    for (a, b, shared) in [
        (&[1, 2, 3][..], &[2, 3, 4][..], 2),
        // A token counts as often as both hold it.
        (&[1, 1, 2], &[1, 2, 2], 2),
        (&[1, 1, 1], &[1, 1], 2),
        (&[], &[1], 0),
        (&[], &[], 0),
        // One list wholly below the other, and the two interleaved.
        (&[5], &[1, 2, 3], 0),
        (&[1, 3, 5], &[2, 4, 6], 0),
        (&[1, 2, 3], &[1, 2, 3], 3),
        // The shared token is the last of one list and the first of the other.
        (&[1, 2, 7], &[7, 8, 9], 1),
    ] {
        assert_eq!(overlap(a, b), shared, "{a:?} {b:?}");
        assert_eq!(overlap(b, a), shared, "{b:?} {a:?}");
    }
}

/// Every sorted list of up to `length` tokens out of three.
fn multisets(length: usize) -> Vec<Vec<u64>> {
    let mut all = vec![vec![]];
    let mut last = vec![vec![]];
    for _ in 0..length {
        last = last
            .iter()
            .flat_map(|list: &Vec<u64>| {
                (list.last().copied().unwrap_or(0)..3).map(move |token| {
                    let mut longer = list.clone();
                    longer.push(token);
                    longer
                })
            })
            .collect();
        all.extend(last.iter().cloned());
    }
    all
}

#[test]
fn the_overlap_is_the_definition_s_for_every_short_pair() {
    let multisets = multisets(4);
    assert_eq!(multisets.len(), 35);
    let count = |list: &[u64], token: u64| list.iter().filter(|other| **other == token).count();
    for a in &multisets {
        for b in &multisets {
            let expected: usize = (0..3)
                .map(|token| count(a, token).min(count(b, token)))
                .sum();
            assert_eq!(overlap(a, b), expected, "{a:?} {b:?}");
        }
    }
}

fn member(member: &str, name: &str, code: &str, tokens: &[u64]) -> Function {
    Function {
        member: member.into(),
        ..function(name, code, code, tokens)
    }
}

fn statuses(entries: &[Entry]) -> Vec<(&str, &str, &Status)> {
    entries
        .iter()
        .map(|entry| (entry.member.as_str(), entry.name.as_str(), &entry.status))
        .collect()
}

#[test]
fn a_changed_function_is_compared_with_the_most_similar_of_its_name() {
    let old = [member("a.o", "f", "c0", &[1, 2, 3, 4])];
    let new = [
        member("a.o", "f", "c1", &[1, 2, 9, 9]),
        member("b.o", "f", "c2", &[1, 2, 3, 9]),
        member("c.o", "f", "c3", &[9, 9, 9, 9]),
    ];
    assert_eq!(
        statuses(&compare(&old, &new)),
        [("a.o", "f", &Status::Changed { similarity: 0.75 })]
    );
    // Two of the name equally similar: the same similarity, and neither is
    // reported as added.
    let new = [
        member("a.o", "f", "c1", &[1, 2, 3, 9]),
        member("b.o", "f", "c2", &[1, 2, 3, 8]),
    ];
    assert_eq!(
        statuses(&compare(&old, &new)),
        [("a.o", "f", &Status::Changed { similarity: 0.75 })]
    );
}

#[test]
fn a_removed_function_names_its_closest_candidate_or_none() {
    let gone = member("a.o", "gone", "c0", &[1, 2, 3, 4, 5, 6]);
    let removed = |closest: Option<(&str, f64)>| Status::Removed {
        closest: closest.map(|(name, score)| (name.to_owned(), score)),
    };
    // The most similar of the unmatched new functions.
    let new = [
        member("a.o", "far", "c1", &[1, 2, 9, 9, 9, 9]),
        member("a.o", "near", "c2", &[1, 2, 3, 4, 5, 9]),
        member("a.o", "other", "c3", &[7, 7, 7, 7, 7, 7]),
    ];
    let entries = compare(std::slice::from_ref(&gone), &new);
    assert_eq!(
        statuses(&entries),
        [
            ("a.o", "gone", &removed(Some(("near", 0.833_333)))),
            ("a.o", "far", &Status::Added),
            ("a.o", "near", &Status::Added),
            ("a.o", "other", &Status::Added),
        ]
    );
    // Two equally similar: the later of the two in the new revision.
    let new = [
        member("a.o", "first", "c1", &[1, 2, 3, 4, 5, 9]),
        member("a.o", "second", "c2", &[1, 2, 3, 4, 5, 8]),
    ];
    let entries = compare(std::slice::from_ref(&gone), &new);
    assert!(
        matches!(
            &entries[0].status,
            Status::Removed { closest: Some((name, _)) } if name == "second"
        ),
        "{entries:?}"
    );
    // Nothing within twice or half its length, and nothing new at all.
    let new = [
        member("a.o", "long", "c1", &[1; 13]),
        member("a.o", "short", "c2", &[1, 2]),
    ];
    assert_eq!(
        compare(std::slice::from_ref(&gone), &new)[0].status,
        removed(None)
    );
    assert_eq!(
        statuses(&compare(std::slice::from_ref(&gone), &[])),
        [("a.o", "gone", &removed(None))]
    );
    // A new function matched by an old one, or named as an old one is, is no
    // candidate.
    let old = [
        gone.clone(),
        member("a.o", "kept", "c9", &[1, 2, 3, 4, 5, 7]),
    ];
    let new = [member("b.o", "kept", "c9", &[1, 2, 3, 4, 5, 7])];
    assert_eq!(
        statuses(&compare(&old, &new)),
        [
            ("a.o", "gone", &removed(None)),
            ("a.o", "kept", &Status::Unchanged)
        ]
    );
}

#[test]
fn two_removed_functions_of_one_name_and_member_each_name_their_own_closest_candidate() {
    // Two static functions of one name that a partial link put in one member.
    let old = [
        member("a.o", "init", "c0", &[1, 2, 3, 4, 5, 6]),
        member("a.o", "init", "c1", &[11, 12, 13, 14, 15, 16]),
    ];
    let new = [
        member("a.o", "near_first", "c2", &[1, 2, 3, 4, 5, 9]),
        member("a.o", "near_second", "c3", &[11, 12, 13, 14, 15, 19]),
    ];
    let closest: Vec<_> = compare(&old, &new)
        .into_iter()
        .filter_map(|entry| match entry.status {
            Status::Removed { closest } => closest.map(|(name, _)| name),
            _ => None,
        })
        .collect();
    assert_eq!(closest, ["near_first", "near_second"]);
}

#[test]
fn a_renamed_function_is_one_whose_code_stands_under_a_name_the_old_revision_lacks() {
    let old = [
        member("a.o", "one", "same", &[1, 2]),
        member("a.o", "two", "same", &[1, 2]),
    ];
    // The code stands under `two` too, which is an old name: `one` is renamed
    // to the name that is new, and `two` is unchanged.
    let new = [
        member("a.o", "two", "same", &[1, 2]),
        member("a.o", "three", "same", &[1, 2]),
    ];
    assert_eq!(
        statuses(&compare(&old, &new)),
        [
            (
                "a.o",
                "one",
                &Status::Renamed {
                    new: "three".into()
                }
            ),
            ("a.o", "two", &Status::Unchanged),
        ]
    );
}

#[test]
fn the_summary_counts_each_status_that_occurs_in_the_order_of_its_name() {
    let old = [
        function("same", "c1", "n1", &[1, 2]),
        function("callees", "c2", "n2", &[3]),
        function("old_name", "c3", "n3", &[4, 5]),
        function("edited", "c4", "n4", &[6, 7, 8, 9]),
        function("gone", "peer", "n5", &[10, 11, 12, 13]),
        function("also_same", "c7", "n7", &[1]),
    ];
    let new = [
        function("same", "c1", "n1", &[1, 2]),
        function("callees", "c2", "n2b", &[3]),
        function("new_name", "c3", "n3b", &[4, 5]),
        function("edited", "c4b", "n4b", &[6, 7, 8, 0]),
        function("successor", "c6", "n6", &[10, 11, 12, 14]),
        function("also_same", "c7", "n7", &[1]),
    ];
    assert_eq!(
        summary(&compare(&old, &new)),
        "added=1 changed=1 references-renamed=1 removed=1 renamed=1 unchanged=2"
    );
    assert_eq!(summary(&compare(&old[..1], &new[..1])), "unchanged=1");
    assert_eq!(summary(&compare(&[], &[])), "");
}
