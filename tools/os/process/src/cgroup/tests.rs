//! The group bookkeeping against a directory that stands for the
//! hierarchy; the kernel's side (moving, killing) is checked on the host.

use super::*;

const OWN: &str = "0::/user.slice/app.slice/oer-lab.service\n";

fn events(group: &Path, populated: bool) {
    fs::write(
        group.join("cgroup.events"),
        format!("populated {}\nfrozen 0\n", u8::from(populated)),
    )
    .unwrap();
}

#[test]
fn taking_the_group_moves_this_process_into_its_leaf() {
    let mount = tempfile::tempdir().unwrap();
    let delegated = Delegated::take_at(mount.path(), OWN, 42).unwrap();
    let unit = mount.path().join("user.slice/app.slice/oer-lab.service");
    assert_eq!(
        fs::read_to_string(unit.join("service/cgroup.procs")).unwrap(),
        "42"
    );
    assert_eq!(delegated.commands, unit.join("commands"));
    // A process that took its group already sits in its leaf.
    let again = Delegated::take_at(
        mount.path(),
        "0::/user.slice/app.slice/oer-lab.service/service\n",
        43,
    )
    .unwrap();
    assert_eq!(again.commands, unit.join("commands"));
}

#[test]
fn a_group_left_by_a_previous_owner_is_killed_first() {
    let mount = tempfile::tempdir().unwrap();
    let left = mount
        .path()
        .join("user.slice/app.slice/oer-lab.service/commands/7-1");
    fs::create_dir_all(&left).unwrap();
    events(&left, false);
    Delegated::take_at(mount.path(), OWN, 42).unwrap();
    assert_eq!(fs::read_to_string(left.join("cgroup.kill")).unwrap(), "1");
}

#[test]
fn a_stopped_group_admits_no_process_and_is_killed_whole() {
    let mount = tempfile::tempdir().unwrap();
    let delegated = Delegated::take_at(mount.path(), OWN, 42).unwrap();
    assert!(delegated.group("../escape").is_err());
    let group = delegated.group("7-1").unwrap();
    events(&group.path, true);
    assert!(group.populated().unwrap());
    group.kill().unwrap();
    assert_eq!(
        fs::read_to_string(group.path.join("cgroup.kill")).unwrap(),
        "1"
    );
    let refused = group
        .spawn(&mut Command::new("true"), None)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("closed"), "{refused}");
    assert!(!group.wait_empty(Duration::ZERO).unwrap());
    events(&group.path, false);
    assert!(group.wait_empty(Duration::ZERO).unwrap());
}

#[test]
fn populated_reads_the_events_line() {
    assert!(populated("populated 1\nfrozen 0\n").unwrap());
    assert!(!populated("populated 0\n").unwrap());
    assert!(populated("frozen 0\n").is_err());
}
