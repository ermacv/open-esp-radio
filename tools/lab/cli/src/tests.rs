use super::*;
use clap::{CommandFactory as _, Parser as _};

#[test]
fn the_complete_tree_parses_existing_forms_and_an_opaque_lease_command() {
    Cli::command().debug_assert();
    for words in [
        vec![],
        vec!["queue", "--json"],
        vec!["--owner=phy", "queue"],
        vec!["wait", "--service"],
        vec!["wait", "--service", "dut", "peer"],
        vec!["owner"],
        vec!["owner", "set", "phy"],
        vec!["owner", "merge", "old", "new"],
        vec!["owner", "forget", "old"],
        vec!["preempt", "#12", "--reason", "test"],
        vec!["preempt", "--reason", "test", "12"],
        vec![
            "lease",
            "--board",
            "dut",
            "--owner",
            "phy",
            "--",
            "sh",
            "-c",
            "echo --owner=untouched",
        ],
        vec!["board", "reset", "dut", "--via", "jtag"],
        vec!["devices", "--json"],
        vec!["devices", "maintenance", "dut", "--reason", "test"],
        vec!["devices", "release", "--stand"],
        vec!["discover"],
        vec!["doctor"],
        vec!["init", "--force"],
        vec!["install"],
        vec!["fixtures"],
        vec!["fixture", "install", "--provider", "linux-net", "--dry-run"],
        vec!["fixture", "build-hostapd"],
        vec!["fixture", "build-udhcpc"],
        vec!["--root=/c", "queue"],
        vec!["queue", "--root", "/c"],
    ] {
        assert!(Cli::try_parse_from(&words).is_ok(), "{words:?}");
    }
    let cli = Cli::try_parse_from([
        "--owner=outer",
        "lease",
        "--owner=inner",
        "--stand",
        "--",
        "cmd",
        "--root=/other",
        "--owner=child",
        "--",
        "-x",
    ])
    .unwrap();
    assert_eq!(cli.options.owner.as_deref(), Some("inner"));
    assert_eq!(cli.checkout.root, None);
    let Some(StandCommand::Lease(cli)) = cli.command else {
        panic!("lease command")
    };
    assert_eq!(
        cli.command,
        ["cmd", "--root=/other", "--owner=child", "--", "-x"]
    );
    for words in [
        vec!["wait"],
        vec!["wait", "dut"],
        vec!["preempt", "12"],
        vec!["queue", "--bad"],
        vec!["owner", "merge", "old"],
        vec!["unknown"],
        vec!["lease", "cmd"],
    ] {
        assert!(Cli::try_parse_from(&words).is_err(), "{words:?}");
    }
}

#[test]
fn command_tree_keeps_the_json_contract_with_explicit_corrections() {
    let mut expected: Vec<oer_os_command_tree::CommandNode> =
        serde_json::from_str(include_str!("../tests/fixtures/command-tree.json")).unwrap();
    // --owner is inherited from the root, rather than declared again by lease.
    expected
        .iter_mut()
        .find(|node| node.path == ["stand", "lease"])
        .unwrap()
        .flags
        .retain(|flag| flag != "--owner");
    let mut actual = command_tree();
    // Clap emits command nodes in depth-first declaration order.
    expected.sort_by(|a, b| a.path.cmp(&b.path));
    actual.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(actual, expected);
}

#[test]
fn parser_failures_keep_the_exit_codes_of_existing_commands() {
    for (words, status) in [
        (vec!["wait"], 1),
        (vec!["preempt", "12"], 1),
        (vec!["queue", "--bad"], 1),
        (vec!["owner", "merge", "old"], 1),
        (vec!["unknown"], 1),
        (vec!["lease", "--stand"], 2),
        (vec!["board", "reset"], 2),
        (vec!["devices", "release"], 2),
        (vec!["--owner"], 1),
        (vec!["queue", "--help"], 0),
        (vec!["wait", "--help"], 1),
        (vec!["owner", "--help"], 1),
        (vec!["--help"], 0),
    ] {
        let error = Cli::try_parse_from(&words)
            .err()
            .expect("parser error or help");
        let args = words
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(usage_status(&error, &args), status, "{words:?}");
    }
}
