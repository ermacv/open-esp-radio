use std::path::Path;

use super::*;

fn git(repository: &Path, arguments: &[&str]) {
    let mut all = vec![
        "-c",
        "user.name=Synthetic",
        "-c",
        "user.email=synthetic@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ];
    all.extend_from_slice(arguments);
    oer_tools_os_process::git::run(repository, all).expect("git runs in the test repository");
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("a temporary directory");
    git(directory.path(), &["init", "--quiet"]);
    std::fs::write(directory.path().join("kept.txt"), "one\n").expect("a file");
    std::fs::write(directory.path().join("ab c.txt"), "two\n").expect("a file");
    git(directory.path(), &["add", "."]);
    git(directory.path(), &["commit", "--quiet", "-m", "fixture"]);
    directory
}

#[test]
fn a_clean_checkout_names_its_tree_and_no_path() {
    let directory = repository();
    let source = Source::read(directory.path()).expect("the source reads");
    let tree = oer_tools_os_process::git::text(directory.path(), ["rev-parse", "HEAD^{tree}"])
        .expect("the tree");
    assert_eq!(
        source,
        Source {
            tree,
            dirty: Vec::new()
        }
    );
}

#[test]
fn a_changed_checkout_names_each_changed_path_and_keeps_the_head_s_tree() {
    let directory = repository();
    let clean = Source::read(directory.path()).expect("the source reads");
    std::fs::write(directory.path().join("kept.txt"), "changed\n").expect("a change");
    std::fs::write(directory.path().join("new.txt"), "new\n").expect("an untracked file");
    git(directory.path(), &["mv", "ab c.txt", "renamed.txt"]);
    let source = Source::read(directory.path()).expect("the source reads");
    // The tree is the head's: what is dirty is named beside it.
    assert_eq!(source.tree, clean.tree);
    let mut dirty = source.dirty.clone();
    dirty.sort();
    assert_eq!(dirty, ["ab c.txt", "kept.txt", "new.txt", "renamed.txt"]);
}

#[test]
fn status_entries_give_their_paths_and_a_rename_its_origin() {
    assert!(dirty_paths(b"").is_empty());
    assert_eq!(
        dirty_paths(b" M a b.txt\0?? new\0R  to.txt\0ab c.txt\0C  copy\0of\0"),
        ["a b.txt", "new", "to.txt", "ab c.txt", "copy", "of"]
    );
}
