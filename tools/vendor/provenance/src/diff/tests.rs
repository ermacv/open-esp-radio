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
