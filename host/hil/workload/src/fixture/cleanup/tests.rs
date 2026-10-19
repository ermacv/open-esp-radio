use super::*;

#[test]
fn cleanup_records_failures_during_unwinding() {
    let path = std::env::temp_dir().join(format!("oer-cleanup-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    struct Owner;
    impl Drop for Owner {
        fn drop(&mut self) {
            record("restore test fixture", || {
                Err("injected restore failure".into())
            });
        }
    }
    let result = std::panic::catch_unwind(|| {
        let _scope = Scope::new(&path);
        let _owner = Owner;
        panic!("injected operation failure");
    });
    assert!(result.is_err());
    let records: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("cleanup.json")).unwrap()).unwrap();
    assert_eq!(records[0]["failure"], "injected restore failure");
    std::fs::remove_dir_all(path).unwrap();
}
