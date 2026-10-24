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

#[derive(Debug, serde::Serialize)]
pub struct NeighborRefresh {
    pub before_present: bool,
    pub delete_attempted: bool,
    pub after_present: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct NeighborState {
    pub after_deletion: bool,
    pub present: bool,
}

/// Establish an absent station neighbor, accepting an already absent entry
/// and a racing deletion only after a successful confirming read. The next
/// measured router ping triggers ARP; only air/target evidence passes gate c.
pub fn refresh_neighbor(
    mut execute: impl FnMut(&str) -> Result<std::process::Output>,
    interface: &str,
    address: std::net::Ipv4Addr,
    mut observe: impl FnMut(NeighborState),
) -> Result<NeighborRefresh> {
    if !token(interface) || !crate::w3::usable_address(address) {
        return Err("invalid W4 ARP selection".to_owned());
    }
    let available = execute("command -v ip")?;
    if !available.status.success() {
        return Err("W4 native ip command is unavailable; see router command evidence".into());
    }
    let show = format!("ip -4 neigh show {address} dev {}", quote(interface)?);
    let before = execute(&show)?;
    if !before.status.success() {
        return Err("W4 neighbor read failed; see router command evidence".into());
    }
    let present =
        |output: &std::process::Output| !output.stdout.iter().all(u8::is_ascii_whitespace);
    observe(NeighborState {
        after_deletion: false,
        present: present(&before),
    });
    if !present(&before) {
        return Ok(NeighborRefresh {
            before_present: false,
            delete_attempted: false,
            after_present: false,
        });
    }
    let delete = execute(&format!(
        "ip -4 neigh del {address} dev {}",
        quote(interface)?
    ))?;
    if matches!(delete.status.code(), Some(126 | 127)) {
        return Err("W4 native ip command became unavailable; see router command evidence".into());
    }
    let after = execute(&show)?;
    if !after.status.success() {
        return Err("W4 neighbor confirmation failed; see router command evidence".into());
    }
    observe(NeighborState {
        after_deletion: true,
        present: present(&after),
    });
    if present(&after) {
        return Err("W4 neighbor remains after deletion; see router command evidence".into());
    }
    Ok(NeighborRefresh {
        before_present: true,
        delete_attempted: true,
        after_present: false,
    })
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
    fn neighbor_state_is_observed_before_and_after_even_when_delete_fails() {
        use std::{
            collections::VecDeque,
            os::unix::process::ExitStatusExt,
            process::{ExitStatus, Output},
        };
        let ip = "10.231.0.100".parse().expect("IP");
        // Present/deleted, absent, deletion racing with another remover,
        // deletion failure retaining the entry, and missing native command.
        for (answers, success, deleted) in [
            (
                vec![(0, "ip"), (0, "neighbor"), (0, ""), (0, "")],
                true,
                true,
            ),
            (vec![(0, "ip"), (0, "")], true, false),
            (
                vec![(0, "ip"), (0, "neighbor"), (2, ""), (0, "")],
                true,
                true,
            ),
            (
                vec![(0, "ip"), (0, "neighbor"), (2, ""), (0, "neighbor")],
                false,
                true,
            ),
            (vec![(127, "")], false, false),
            (vec![(1, "")], false, false),
            (vec![(0, "ip"), (127, "")], false, false),
            (vec![(0, "ip"), (0, "neighbor"), (127, "")], false, true),
        ] {
            let mut answers = VecDeque::from(answers);
            let mut scripts = Vec::new();
            let mut states = Vec::new();
            let result = refresh_neighbor(
                |script| {
                    scripts.push(script.to_owned());
                    let (code, stdout) = answers.pop_front().expect("scripted command");
                    Ok(Output {
                        status: ExitStatus::from_raw(code << 8),
                        stdout: stdout.as_bytes().to_vec(),
                        stderr: b"RTNETLINK diagnostic\n".to_vec(),
                    })
                },
                "br-oer-w3",
                ip,
                |state| states.push(state),
            );
            assert_eq!(result.is_ok(), success);
            assert!(answers.is_empty());
            assert_eq!(
                scripts.iter().any(|script| script.contains(" neigh del ")),
                deleted
            );
            assert!(scripts.iter().all(|script| !script.contains("/dev/null")));
            if let Ok(observation) = result {
                assert_eq!(observation.before_present, deleted);
                assert_eq!(observation.delete_attempted, deleted);
                assert!(!observation.after_present);
                assert!(!states.last().expect("observed state").present);
            } else if states.len() == 2 {
                assert!(states[1].after_deletion && states[1].present);
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
