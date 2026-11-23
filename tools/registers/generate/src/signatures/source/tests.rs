use super::{pinned_bytes, pinned_gitlink, pinned_revision};

const PINNED: &[u8] = b"void original(void);\n";

#[test]
fn a_header_is_pinned_only_when_its_committed_bytes_and_its_file_match_the_pin() {
    let pin = oer_tools_os_durable::sha256_bytes(PINNED);
    assert!(pinned_bytes("api.h", PINNED, &pin, &pin).is_ok());
    // A commit of other bytes under the old pin.
    let changed = b"void changed(int a);\n";
    let error = pinned_bytes("api.h", changed, &pin, &pin).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("api.h differs from its pinned committed bytes")
    );
    // A dirty file beside the pinned commit.
    let dirty = oer_tools_os_durable::sha256_bytes(changed);
    assert!(pinned_bytes("api.h", PINNED, &dirty, &pin).is_err());
}

#[test]
fn a_submodule_is_pinned_only_by_its_own_commit_s_gitlink() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let entry = format!("160000 commit {commit}\tcomponents/sub");
    assert!(pinned_gitlink("components/sub", &entry, commit).is_ok());
    let other = "fedcba9876543210fedcba9876543210fedcba98";
    assert!(pinned_gitlink("components/sub", &entry, other).is_err());
    // A blob at the path is no gitlink.
    let blob = format!("100644 blob {commit}\tcomponents/sub");
    assert!(pinned_gitlink("components/sub", &blob, commit).is_err());
}

#[test]
fn the_checkout_is_pinned_only_at_its_revision() {
    assert!(pinned_revision("abc", "abc").is_ok());
    assert!(
        pinned_revision("abd", "abc")
            .unwrap_err()
            .to_string()
            .contains("revision changed")
    );
}
