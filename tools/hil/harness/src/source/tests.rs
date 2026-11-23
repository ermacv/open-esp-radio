use super::*;

#[test]
fn status_entries_give_their_paths_and_a_rename_its_origin() {
    assert!(dirty_paths(b"").is_empty());
    assert_eq!(
        dirty_paths(b" M a b.txt\0?? new\0R  to.txt\0ab c.txt\0C  copy\0of\0"),
        ["a b.txt", "new", "to.txt", "ab c.txt", "copy", "of"]
    );
}
