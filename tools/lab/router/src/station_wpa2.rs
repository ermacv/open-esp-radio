//! Router actions for WPA2-Personal station runs. Credentials are write-only
//! through [`crate::router::Profile::station_wpa2`]; command evidence uses SSH redaction.

use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result, token},
};
use zeroize::Zeroizing;

/// A short GTK lifetime for a run's 100-echo traffic window. The lab profile
/// is committed without restoration, just like the other station settings.
pub const GROUP_REKEY_SECONDS: u32 = 30;

/// Read-only admission check. Native neighbor operations trigger the kernel's
/// ARP request on the next echo in the ongoing measured router ping series.
pub fn verify_observation_tools(remote: &mut dyn Remote) -> Result<()> {
    let _hidden = Zeroizing::new(
        remote
            .run("ip -4 neigh show >/dev/null && command -v tcpdump >/dev/null")
            .map_err(
                |_| "WPA2 station requires native ip neigh and tcpdump on the managed router",
            )?,
    );
    Ok(())
}

/// Check the running BSS, filtering every value on the router before it
/// reaches stdout. Configuration keys and passphrases are never read back.
pub fn verify_runtime(remote: &mut dyn Remote, radio: &str, interface: &str) -> Result<()> {
    if !token(radio) || !token(interface) {
        return Err("invalid WPA2 station runtime profile selection".to_owned());
    }
    let script = format!(
        "set +x\nset -e\nphy=$(ubus call iwinfo phyname {} | jsonfilter -e '@.phyname')\n\
        case \"$phy\" in phy[0-9]*) ;; *) exit 1;; esac\n\
        awk -F= -v iface={} '\n\
        $1==\"interface\" || $1==\"bss\" {{selected=($2==iface); if(selected) found++}}\n\
        $1==\"ieee80211n\" {{n=$2}}\n$1==\"ieee80211ax\" {{ax=$2}}\n\
        $1==\"ht_capab\" {{wide=($2 ~ /HT40/)}}\n\
        selected && $1==\"wpa\" {{wpa=$2}}\n\
        selected && $1==\"wpa_key_mgmt\" {{psk=($2==\"WPA-PSK\")}}\n\
        selected && $1==\"wpa_pairwise\" {{ccmp=($2==\"CCMP\")}}\n\
        selected && $1==\"ieee80211w\" {{pmf=$2}}\n\
        END {{printf \"%d %d %d %d %d %d %d %d\\n\",found,n,ax,wide,wpa,psk,ccmp,pmf}}' \"/var/run/hostapd-$phy.conf\"",
        quote(&format!("{{\"section\":\"{radio}\"}}"))?,
        quote(interface)?
    );
    let output = remote
        .run(&script)
        .map_err(|_| "WPA2 station runtime profile verification failed")?;
    if output.trim() != "1 1 0 0 2 1 1 0" {
        return Err("running BSS is not WPA2-PSK/CCMP HT20 with HE and PMF disabled".to_owned());
    }
    Ok(())
}

/// Read only the selected BSS's running GTK interval, never its credentials.
pub fn group_rekey_seconds(remote: &mut dyn Remote, radio: &str, interface: &str) -> Result<u32> {
    if !token(radio) || !token(interface) {
        return Err("invalid GTK profile selection".into());
    }
    let script = format!(
        "set +x\nphy=$(ubus call iwinfo phyname {} | jsonfilter -e '@.phyname')\ncase \"$phy\" in phy[0-9]*) ;; *) exit 1;; esac\nawk -F= -v iface={} '$1==\"interface\" || $1==\"bss\" {{selected=($2==iface)}} selected && $1==\"wpa_group_rekey\" {{print $2}}' \"/var/run/hostapd-$phy.conf\"",
        quote(&format!("{{\"section\":\"{radio}\"}}"))?,
        quote(interface)?
    );
    let output = Zeroizing::new(
        remote
            .run(&script)
            .map_err(|_| "cannot read running GTK interval")?,
    );
    output
        .trim()
        .parse()
        .map_err(|_| "missing or invalid running GTK interval".into())
}

/// One recorded read of the station neighbor entry. `state` is the kernel NUD
/// state (`REACHABLE`, `STALE`, `INCOMPLETE`, `FAILED`, ...) or `None` when the
/// entry is absent or the read failed.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct NeighborState {
    pub after_flush: bool,
    pub read_succeeded: bool,
    pub state: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct NeighborRefresh {
    pub before: NeighborState,
    pub after: NeighborState,
}

fn neighbor_state(after_flush: bool, output: &std::process::Output) -> NeighborState {
    let text = String::from_utf8_lossy(&output.stdout);
    NeighborState {
        after_flush,
        read_succeeded: output.status.success(),
        state: text
            .lines()
            .find_map(|line| line.split_whitespace().last())
            .map(str::to_owned),
    }
}

/// Flush the station neighbor (BusyBox `ip` has `flush`, no `del`). Both reads
/// around the flush are only recorded: router traffic recreates the entry at
/// once, so its presence proves nothing. Only a missing `ip` or a failing
/// `flush` (with its stderr) is an error. The group key change gate carries the proof
/// from air and target evidence.
pub fn refresh_neighbor(
    mut execute: impl FnMut(&str) -> Result<std::process::Output>,
    interface: &str,
    address: std::net::Ipv4Addr,
    mut observe: impl FnMut(&NeighborState),
) -> Result<NeighborRefresh> {
    if !token(interface) || !crate::station::usable_address(address) {
        return Err("invalid WPA2 station ARP selection".to_owned());
    }
    let available = execute("command -v ip")?;
    if !available.status.success() {
        return Err(
            "WPA2 station native ip command is unavailable; see router command evidence".into(),
        );
    }
    let show = format!("ip -4 neigh show {address} dev {}", quote(interface)?);
    let before = neighbor_state(false, &execute(&show)?);
    observe(&before);
    let flush = execute(&format!(
        "ip -4 neigh flush to {address} dev {}",
        quote(interface)?
    ))?;
    if !flush.status.success() {
        return Err(format!(
            "WPA2 station neighbor flush failed ({}): {}",
            flush.status,
            String::from_utf8_lossy(&flush.stderr).trim()
        ));
    }
    let after = neighbor_state(true, &execute(&show)?);
    observe(&after);
    Ok(NeighborRefresh { before, after })
}

/// IEEE 802.11 reason 2, previous authentication no longer valid: the reason
/// a hostapd control-interface deauthentication carries.
const DEAUTH_REASON: u16 = 2;

/// Ask the selected AP to deauthenticate one station for the reconnect gate,
/// through the router's own hostapd ubus object (`hostapd.<interface>`,
/// `del_client` with `deauth` and no ban). The caller holds router and air
/// ownership for the complete run. The full MAC is used only in the stdin
/// script and redacted in command evidence. A missing `ubus` or a nonzero exit
/// is an error carrying the exit status and stderr.
pub fn deauthenticate(
    mut execute: impl FnMut(&str) -> Result<std::process::Output>,
    interface: &str,
    station: Mac,
) -> Result<()> {
    if !token(interface) {
        return Err("invalid router AP interface".to_owned());
    }
    let address = station.octets().map(|byte| format!("{byte:02x}")).join(":");
    let object = format!("hostapd.{interface}");
    let arguments = format!(
        "{{\"addr\":\"{address}\",\"reason\":{DEAUTH_REASON},\"deauth\":true,\"ban_time\":0}}"
    );
    let output = execute(&format!(
        "set +x\nubus call {} del_client {}",
        quote(&object)?,
        quote(&arguments)?,
    ))?;
    if !output.status.success() {
        return Err(format!(
            "router station deauthentication failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted {
        answer: Result<String>,
        scripts: Vec<String>,
    }

    impl Remote for Scripted {
        fn run(&mut self, script: &str) -> Result<String> {
            self.scripts.push(script.to_owned());
            self.answer.clone()
        }
    }

    #[test]
    fn running_interval_is_numeric_and_rejects_missing_or_secret_output() {
        let mut remote = Scripted {
            answer: Ok("0\n".into()),
            scripts: Vec::new(),
        };
        assert_eq!(
            group_rekey_seconds(&mut remote, "radio1", "phy1-ap0").expect("disabled"),
            0
        );
        remote.answer = Ok("30\n".into());
        assert_eq!(
            group_rekey_seconds(&mut remote, "radio1", "phy1-ap0").expect("WPA2 station"),
            30
        );
        remote.answer = Ok("secret-passphrase".into());
        assert!(
            !group_rekey_seconds(&mut remote, "radio1", "phy1-ap0")
                .expect_err("invalid")
                .contains("secret-passphrase")
        );
        assert!(!remote.scripts[0].contains("wpa_passphrase"));
    }

    #[test]
    fn runtime_profile_is_numeric_only_and_never_returns_keys_or_diagnostics() {
        let mut remote = Scripted {
            answer: Ok("1 1 0 0 2 1 1 0\n".to_owned()),
            scripts: Vec::new(),
        };
        verify_runtime(&mut remote, "radio1", "phy1-ap0").expect("WPA2 runtime");
        assert!(remote.scripts[0].contains("wpa_pairwise"));
        assert!(!remote.scripts[0].contains("wpa_passphrase"));
        remote.answer = Ok("1 1 0 0 2 1 0 0".to_owned());
        assert!(verify_runtime(&mut remote, "radio1", "phy1-ap0").is_err());
        remote.answer = Err("secret-passphrase PMK PTK GTK".to_owned());
        assert!(
            !verify_runtime(&mut remote, "radio1", "phy1-ap0")
                .expect_err("remote failure")
                .contains("secret-passphrase")
        );
    }

    #[test]
    fn missing_fixture_tools_stop_admission_and_errors_reveal_no_remote_output() {
        let mut remote = Scripted {
            answer: Err("private-router private-passphrase".to_owned()),
            scripts: Vec::new(),
        };
        let error = verify_observation_tools(&mut remote).expect_err("fixture tools required");
        assert!(error.contains("ip neigh"));
        assert!(!error.contains("private-"));
        assert_eq!(
            remote.scripts,
            ["ip -4 neigh show >/dev/null && command -v tcpdump >/dev/null"]
        );
        remote.answer = Ok(String::new());
        verify_observation_tools(&mut remote).expect("read-only admission");
    }

    #[test]
    fn neighbor_flush_records_both_reads_and_fails_only_on_flush_or_missing_command() {
        use std::{
            collections::VecDeque,
            os::unix::process::ExitStatusExt,
            process::{ExitStatus, Output},
        };
        let ip = "10.231.0.100".parse().expect("IP");
        let entry = |state: &str| {
            format!("10.231.0.100 dev br-oer-station lladdr 02:00:00:00:00:01 {state}\n")
        };
        // (answers as (exit, stdout, stderr), success, flush attempted,
        // recorded states)
        let cases: Vec<(Vec<(i32, String, &str)>, bool, bool, Vec<Option<&str>>)> = vec![
            // Entry recreated by traffic right after the flush.
            (
                vec![
                    (0, "ip".into(), ""),
                    (0, entry("STALE"), ""),
                    (0, String::new(), ""),
                    (0, entry("INCOMPLETE"), ""),
                ],
                true,
                true,
                vec![Some("STALE"), Some("INCOMPLETE")],
            ),
            (
                vec![
                    (0, "ip".into(), ""),
                    (0, entry("REACHABLE"), ""),
                    (0, String::new(), ""),
                    (0, entry("REACHABLE"), ""),
                ],
                true,
                true,
                vec![Some("REACHABLE"), Some("REACHABLE")],
            ),
            // Absent before and after; a failed read does not fail the step.
            (
                vec![
                    (0, "ip".into(), ""),
                    (0, String::new(), ""),
                    (0, String::new(), ""),
                    (1, String::new(), "read"),
                ],
                true,
                true,
                vec![None, None],
            ),
            // Flush failure carries stderr.
            (
                vec![
                    (0, "ip".into(), ""),
                    (0, entry("STALE"), ""),
                    (2, String::new(), "RTNETLINK diagnostic"),
                ],
                false,
                true,
                vec![Some("STALE")],
            ),
            // Missing command.
            (
                vec![(127, String::new(), "not found")],
                false,
                false,
                vec![],
            ),
        ];
        for (answers, success, flushed, expected) in cases {
            let mut answers = VecDeque::from(answers);
            let mut scripts = Vec::new();
            let mut states = Vec::new();
            let result = refresh_neighbor(
                |script| {
                    scripts.push(script.to_owned());
                    let (code, stdout, stderr) = answers.pop_front().expect("scripted command");
                    Ok(Output {
                        status: ExitStatus::from_raw(code << 8),
                        stdout: stdout.into_bytes(),
                        stderr: stderr.as_bytes().to_vec(),
                    })
                },
                "br-oer-station",
                ip,
                |state| states.push(state.state.clone()),
            );
            assert_eq!(result.is_ok(), success);
            assert!(answers.is_empty());
            assert_eq!(
                scripts.iter().any(
                    |script| script.contains(" neigh flush to 10.231.0.100 dev br-oer-station")
                ),
                flushed
            );
            assert!(scripts.iter().all(|script| !script.contains("/dev/null")));
            let expected: Vec<Option<String>> =
                expected.into_iter().map(|s| s.map(str::to_owned)).collect();
            assert_eq!(states, expected);
            if let Err(error) = &result
                && flushed
            {
                assert!(error.contains("RTNETLINK diagnostic"));
            }
        }
        for interface in ["", "br-oer-station;opkg", "-i"] {
            assert!(
                refresh_neighbor(
                    |_| panic!("invalid input never executes"),
                    interface,
                    ip,
                    |_| panic!("invalid input never observes")
                )
                .is_err()
            );
        }
    }

    fn output(code: i32, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn deauthentication_calls_the_ap_ubus_object_for_the_exact_station() {
        let mut scripts = Vec::new();
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        deauthenticate(
            |script| {
                scripts.push(script.to_owned());
                Ok(output(0, ""))
            },
            "phy7-ap3",
            station,
        )
        .unwrap();
        assert_eq!(scripts.len(), 1);
        let script = &scripts[0];
        assert!(script.starts_with("set +x\n"));
        assert!(script.contains("ubus call hostapd.phy7-ap3 del_client"));
        assert!(script.contains("\"addr\":\"02:ab:cd:ef:f6:d0\""));
        assert!(script.contains("\"deauth\":true"));
        assert!(script.contains("\"ban_time\":0"));
        assert!(!script.contains("hostapd_cli"));
    }

    #[test]
    fn deauthentication_failure_carries_exit_status_and_stderr() {
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        let error = deauthenticate(
            |_| Ok(output(4, "Command failed: Not found\n")),
            "phy7-ap3",
            station,
        )
        .unwrap_err();
        assert!(error.contains("Command failed: Not found"), "{error}");
        assert!(error.contains("exit status: 4"), "{error}");
        // A missing command is a nonzero exit with the shell's diagnostic.
        let error = deauthenticate(
            |_| Ok(output(127, "sh: ubus: not found\n")),
            "phy7-ap3",
            station,
        )
        .unwrap_err();
        assert!(error.contains("ubus: not found"), "{error}");
        // A transport error passes through.
        assert_eq!(
            deauthenticate(|_| Err("no answer".to_owned()), "phy7-ap3", station).unwrap_err(),
            "no answer"
        );
    }

    #[test]
    fn deauthentication_rejects_bad_interfaces_without_running() {
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        for interface in ["phy7-ap3; secret-passphrase", "-i", ""] {
            assert_eq!(
                deauthenticate(
                    |_| panic!("invalid input never executes"),
                    interface,
                    station
                )
                .unwrap_err(),
                "invalid router AP interface"
            );
        }
    }
}
