use super::*;

#[derive(Default)]
struct RecordingHardware {
    starts: usize,
    stops: usize,
    tsf: u64,
    snapshots: usize,
}

impl ApTsfSnapshotHardware for RecordingHardware {
    fn access_point_tsf(&mut self) -> u64 {
        self.snapshots += 1;
        self.tsf
    }
}

impl ApTsfHardware for RecordingHardware {
    fn reset_and_start_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.starts += 1;
    }

    fn stop_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.stops += 1;
    }
}

#[test]
fn the_owner_publishes_exactly_one_start_and_stop_edge() {
    let mut hardware = RecordingHardware::default();
    let mut owner = AccessPointTsf::new(1);

    owner.restart(&mut hardware);
    owner.stop(&mut hardware);

    assert_eq!(hardware.starts, 1);
    assert_eq!(hardware.stops, 1);
}

#[test]
fn every_restart_and_stop_starts_a_new_generation() {
    let mut hardware = RecordingHardware::default();
    let mut owner = AccessPointTsf::new(7);
    let created = owner.generation();
    owner.restart(&mut hardware);
    let started = owner.generation();
    assert_ne!(started, created);
    owner.stop(&mut hardware);
    assert_ne!(owner.generation(), started);
    owner.restart(&mut hardware);
    assert_ne!(
        owner.generation(),
        started,
        "a restart never reuses a generation"
    );
}

#[test]
fn owners_of_different_epochs_never_share_a_generation() {
    let mut hardware = RecordingHardware::default();
    let mut first = AccessPointTsf::new(1);
    let mut second = AccessPointTsf::new(2);
    first.restart(&mut hardware);
    second.restart(&mut hardware);
    assert_ne!(first.generation(), second.generation());
}

#[test]
fn reading_snapshots_once_and_preserves_the_relation_across_low_word_carry() {
    let mut hardware = RecordingHardware::default();
    let owner = AccessPointTsf::new(3);
    let generation = owner.generation();
    for value in [u64::from(u32::MAX), u64::from(u32::MAX) + 1, u64::MAX] {
        hardware.tsf = value;
        assert_eq!(owner.read(&mut hardware).as_micros(), value);
        assert_eq!(owner.generation(), generation);
    }
    assert_eq!(hardware.snapshots, 3);
    assert_eq!(hardware.starts, 0);
    assert_eq!(hardware.stops, 0);
}
