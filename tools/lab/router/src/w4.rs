//! Router actions for WPA2-Personal station runs. Credentials are write-only
//! through [`crate::router::Profile::w4`]; command evidence uses SSH redaction.

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
            .map_err(|_| "W4 requires native ip neigh and tcpdump on the managed router")?,
    );
    Ok(())
}

/// Check the running BSS, filtering every value on the router before it
/// reaches stdout. Configuration keys and passphrases are never read back.
pub fn verify_runtime(remote: &mut dyn Remote, radio: &str, interface: &str) -> Result<()> {
    if !token(radio) || !token(interface) {
        return Err("invalid W4 runtime profile selection".to_owned());
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
        .map_err(|_| "W4 runtime profile verification failed")?;
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
/// `flush` (with its stderr) is an error. Gate c carries the proof from air and
/// target evidence.
pub fn refresh_neighbor(
    mut execute: impl FnMut(&str) -> Result<std::process::Output>,
    interface: &str,
    address: std::net::Ipv4Addr,
    mut observe: impl FnMut(&NeighborState),
) -> Result<NeighborRefresh> {
    if !token(interface) || !crate::w3::usable_address(address) {
        return Err("invalid W4 ARP selection".to_owned());
    }
    let available = execute("command -v ip")?;
    if !available.status.success() {
        return Err("W4 native ip command is unavailable; see router command evidence".into());
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
            "W4 neighbor flush failed ({}): {}",
            flush.status,
            String::from_utf8_lossy(&flush.stderr).trim()
        ));
    }
    let after = neighbor_state(true, &execute(&show)?);
    observe(&after);
    Ok(NeighborRefresh { before, after })
}

/// Ask the selected AP to deauthenticate one station for the reconnect gate.
/// The caller holds router and air ownership for the complete run. The full
/// MAC is used only in the stdin script and redacted in command evidence.
pub fn deauthenticate(remote: &mut dyn Remote, interface: &str, station: Mac) -> Result<()> {
    if !token(interface) {
        return Err("invalid router AP interface".to_owned());
    }
    let address = station.octets().map(|byte| format!("{byte:02x}")).join(":");
    let script = format!(
        "set +x\nreply=$(hostapd_cli -i {} deauthenticate {}) && test \"$reply\" = OK",
        quote(interface)?,
        quote(&address)?,
    );
    match remote.run(&script) {
        Ok(output) => {
            let _hidden = Zeroizing::new(output);
            Ok(())
        }
        Err(error) => {
            let _hidden = Zeroizing::new(error);
            Err("router station deauthentication failed".to_owned())
        }
    }
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
            group_rekey_seconds(&mut remote, "radio1", "phy1-ap0").expect("W4"),
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
        let entry =
            |state: &str| format!("10.231.0.100 dev br-oer-w3 lladdr 02:00:00:00:00:01 {state}\n");
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
                "br-oer-w3",
                ip,
                |state| states.push(state.state.clone()),
            );
            assert_eq!(result.is_ok(), success);
            assert!(answers.is_empty());
            assert_eq!(
                scripts
                    .iter()
                    .any(|script| script.contains(" neigh flush to 10.231.0.100 dev br-oer-w3")),
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
        for interface in ["", "br-oer-w3;opkg", "-i"] {
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

    #[test]
    fn deauthentication_selects_the_ap_and_the_exact_station_and_requires_ok() {
        let mut remote = Scripted {
            answer: Ok(String::new()),
            scripts: Vec::new(),
        };
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        deauthenticate(&mut remote, "phy7-ap3", station).unwrap();
        assert_eq!(remote.scripts.len(), 1);
        let script = &remote.scripts[0];
        assert!(script.contains("hostapd_cli -i phy7-ap3 deauthenticate 02:ab:cd:ef:f6:d0"));
        assert!(script.contains("&& test \"$reply\" = OK"));
        assert!(script.starts_with("set +x\n"));
        assert!(!script.contains("/dev/null"));
        assert!(!script.contains("disassociate"));
    }

    #[test]
    fn remote_errors_and_bad_interfaces_never_repeat_identities_or_credentials() {
        let mut remote = Scripted {
            answer: Err("secret-passphrase phy7-ap3 02:ab:cd:ef:f6:d0".to_owned()),
            scripts: Vec::new(),
        };
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        assert_eq!(
            deauthenticate(&mut remote, "phy7-ap3", station).unwrap_err(),
            "router station deauthentication failed"
        );
        for interface in ["phy7-ap3; secret-passphrase", "-i", ""] {
            assert_eq!(
                deauthenticate(&mut remote, interface, station).unwrap_err(),
                "invalid router AP interface"
            );
        }
        assert_eq!(remote.scripts.len(), 1);
    }
}
