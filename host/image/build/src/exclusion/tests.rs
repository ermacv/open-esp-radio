use super::*;

#[test]
fn the_compile_cache_is_used_by_one_build_at_a_time() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("nested");
    let first = crate::compiler::lock_compile_cache(&cache).unwrap();
    let path = cache.join("build.lock");
    assert!(
        FileLock::try_acquire(&path, Mode::Shared)
            .unwrap()
            .is_none()
    );
    drop(first);
    let _second = crate::compiler::lock_compile_cache(&cache).unwrap();
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_build_slot_is_one_of_the_host_s_slots() {
    let directory = tempfile::tempdir().unwrap();
    let mut held = Vec::new();
    for _ in 0..slots() {
        held.push(slot(directory.path()).unwrap());
    }
    // Every slot is taken: none is free without waiting.
    for index in 0..slots() {
        let path = directory.path().join(format!("{index}.lock"));
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
    }
    held.pop();
    let _freed = slot(directory.path()).unwrap();
}
